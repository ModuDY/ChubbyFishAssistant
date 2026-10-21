//! 天气查询（可联网数据源，wttr.in）
//!
//! 为什么单独成模块：桌宠对话（`commands::pet`）与网关工具代执行
//! （`api_server::wb_toolexec`）都要用同一个数据源和同一套口径，
//! 两处各写一份必然漂移（字段名、中文映射、超时、失败文案）。
//!
//! 为什么选 wttr.in：国内直连可达（2026-10-21 本机实测 1~2s 返回 200），
//! 无需注册 key，支持按城市名 / 经纬度查询，JSON 字段稳定。
//! 备选源实测结论：DuckDuckGo 超时不可达；百度返回反爬空壳页；
//! Bing 可解析但需要同时抓标题+链接+摘要，适合「通用搜索」而非「精确天气」。
//!
//! 城市解析顺序（`resolve_city`）：
//! 1. 设置项 `pet_weather_city`（用户显式填的，最可信）；
//! 2. 调用方从问题里提取到的城市名（如「北京天气怎么样」）；
//! 3. 空串交给 wttr.in，由它按出口 IP 自动定位。
//!
//! 第 3 步有实测注意点：wttr.in 自己的 IP 定位库可能把结果落到邻近省份
//! （本机实测出口 IP 在东莞，wttr.in 却答郑州），故优先级必须放在最后，
//! 且结果里带上查询用的城市名，让用户能看出它答的是哪儿。

use serde_json::Value;
use std::time::Duration;

/// 单次请求总超时：天气是对话的一部分，用户等着看回复，超过这个时长就不值了
const TIMEOUT_SECS: u64 = 12;
/// 连接超时：DNS/握手阶段卡住时快速失败，好让上层回退
const TIMEOUT_CONNECT_SECS: u64 = 6;
/// 响应体读取上限：j1 JSON 约 40KB，留 1MB 足够且不会拖垮内存
const MAX_BODY_BYTES: usize = 1 << 20;

const UA: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0 Safari/537.36";

/// 查询结果：已整理成可以直接塞进 prompt 的中文短句
#[derive(Debug, Clone)]
pub struct WeatherReport {
    /// 实际查询用的地点（wttr.in 回报的 areaName，如 "Dongguan"）
    pub area: String,
    /// 一行摘要，例：「东莞：晴，34°C（体感 35°C），湿度 41%，西北风 12km/h，今日 27~36°C」
    pub summary: String,
}

impl WeatherReport {
    /// 给模型的文本：带上「数据来源与时间」，让模型知道这是刚查的实时值
    pub fn for_prompt(&self) -> String {
        format!(
            "【实时天气】{}\n（数据来源 wttr.in，查询于刚刚；若用户问的是别的城市，以用户说的为准）",
            self.summary
        )
    }
}

/// 天气描述英译中（wttr.in 的 `lang_zh` 字段实测恒等于英文 `weatherDesc`，
/// 2026-10-21 验证：请求 `lang=zh` 后 `weatherDesc[0].value` 仍为 "Sunny"，
/// `lang_zh[0].value` 也是 "Sunny"，故只能在客户端做映射）。
/// 未命中时原样返回英文，不致丢信息。
fn desc_zh(en: &str) -> &str {
    let key = en.trim().to_ascii_lowercase();
    match key.as_str() {
        "sunny" => "晴",
        "clear" => "晴",
        "partly cloudy" => "多云",
        "cloudy" => "阴",
        "overcast" => "阴",
        "mist" => "薄雾",
        "fog" => "雾",
        "haze" => "霾",
        "light rain" => "小雨",
        "light rain shower" => "阵雨",
        "moderate rain" => "中雨",
        "heavy rain" => "大雨",
        "torrential rain" => "暴雨",
        "patchy rain possible" | "patchy rain nearby" => "局部有雨",
        "light drizzle" => "毛毛雨",
        "rain" => "雨",
        "thundery outbreaks possible" => "可能有雷雨",
        "thunderstorm" => "雷雨",
        "light snow" => "小雪",
        "moderate snow" => "中雪",
        "heavy snow" => "大雪",
        "snow" => "雪",
        "sleet" => "雨夹雪",
        "blizzard" => "暴雪",
        "freezing fog" => "冻雾",
        _ => en.trim(),
    }
}

/// 风向角度转中文方位（wttr.in 的 `winddir16Point` 已是 16 方位英文缩写）
fn wind_zh(p: &str) -> &str {
    match p.trim().to_ascii_uppercase().as_str() {
        "N" => "北风",
        "NNE" | "NE" | "ENE" => "东北风",
        "E" => "东风",
        "ESE" | "SE" | "SSE" => "东南风",
        "S" => "南风",
        "SSW" | "SW" | "WSW" => "西南风",
        "W" => "西风",
        "WNW" | "NW" | "NNW" => "西北风",
        _ => "风向不定",
    }
}

/// 解析 wttr.in `format=j1` 的 JSON 为一行中文摘要。
///
/// 抽成纯函数便于单测：网络不可达时仍能验证字段口径（这是本模块唯一
/// 容易随上游改版而静默错位的地方）。
pub fn parse_j1(json: &str) -> Result<WeatherReport, String> {
    let v: Value = serde_json::from_str(json).map_err(|e| format!("天气数据解析失败: {e}"))?;
    let cur = v
        .pointer("/current_condition/0")
        .ok_or_else(|| "天气数据缺少当前实况".to_string())?;

    let field = |p: &str| -> String {
        cur.pointer(p)
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .trim()
            .to_string()
    };

    let area = v
        .pointer("/nearest_area/0/areaName/0/value")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let region = v
        .pointer("/nearest_area/0/region/0/value")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();

    // 各字段都可能是空串（上游偶发缺项），拼装时逐段判断，避免出现「，°C，」这种空值
    let desc_raw = cur
        .pointer("/weatherDesc/0/value")
        .and_then(|x| x.as_str())
        .unwrap_or("");
    let desc = desc_zh(desc_raw);
    let temp = field("/temp_C");
    let feels = field("/FeelsLikeC");
    let hum = field("/humidity");
    let wind = field("/windspeedKmph");
    let winddir_raw = field("/winddir16Point");
    let winddir = wind_zh(&winddir_raw);

    let today = v.pointer("/weather/0");
    let mint = today
        .and_then(|d| d.get("mintempC"))
        .and_then(|x| x.as_str())
        .unwrap_or("");
    let maxt = today
        .and_then(|d| d.get("maxtempC"))
        .and_then(|x| x.as_str())
        .unwrap_or("");

    let place = match (area.is_empty(), region.is_empty()) {
        (false, false) if area != region => format!("{area}（{region}）"),
        (false, _) => area.clone(),
        _ => region.clone(),
    };

    let mut parts: Vec<String> = Vec::new();
    if !desc.is_empty() {
        parts.push(desc.to_string());
    }
    if !temp.is_empty() {
        let mut t = format!("{temp}°C");
        if !feels.is_empty() && feels != temp {
            t.push_str(&format!("（体感 {feels}°C）"));
        }
        parts.push(t);
    }
    if !hum.is_empty() {
        parts.push(format!("湿度 {hum}%"));
    }
    if !wind.is_empty() {
        parts.push(format!("{winddir} {wind}km/h"));
    }
    if !mint.is_empty() && !maxt.is_empty() {
        parts.push(format!("今日 {mint}~{maxt}°C"));
    }
    if parts.is_empty() {
        return Err("天气数据为空".into());
    }

    let body = parts.join("，");
    Ok(WeatherReport {
        area: place.clone(),
        // 地名缺失时不能拼出「：5°C」这种悬空冒号
        summary: if place.is_empty() {
            body
        } else {
            format!("{place}：{body}")
        },
    })
}

/// 查询天气。`city` 为空 = 让 wttr.in 按出口 IP 自动定位。
///
/// 阻塞调用（ureq），调用方须放在 `spawn_blocking` / 独立线程里，
/// 不要直接阻塞 async 运行时。
pub fn query(city: &str) -> Result<WeatherReport, String> {
    // wttr.in 的城市名走 URL path：空格要编码，中文名它支持但编码后更稳
    let target = city.trim();
    let encoded = if target.is_empty() {
        String::new()
    } else {
        url_encode(target)
    };
    let url = if encoded.is_empty() {
        "https://wttr.in/?format=j1".to_string()
    } else {
        format!("https://wttr.in/{encoded}?format=j1")
    };

    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(TIMEOUT_CONNECT_SECS))
        .timeout(Duration::from_secs(TIMEOUT_SECS))
        .redirects(0)
        .build();

    let resp = agent
        .get(&url)
        .set("User-Agent", UA)
        .set("Accept-Language", "zh-CN,zh;q=0.9,en;q=0.8")
        .call()
        .map_err(|e| format!("天气服务请求失败: {e}"))?;

    let body = read_limited(resp)?;
    let report = parse_j1(&body)?;
    // 用户显式指定了城市但上游落到别处（拼写错误 / 同名的国外地）时，
    // 不能默默回一个不相关的天气：明确告知，让模型自行判断要不要追问
    if !target.is_empty() && !report.area.to_ascii_lowercase().contains(&target.to_ascii_lowercase())
    {
        // 中文城市名无法与上游英文 areaName 直接比对，只在纯 ASCII 查询时校验
        if target.is_ascii() {
            return Err(format!(
                "没查到「{target}」的天气（上游返回的是 {}），换个城市名再试",
                report.area
            ));
        }
    }
    Ok(report)
}

fn read_limited(resp: ureq::Response) -> Result<String, String> {
    use std::io::Read;
    let mut reader = resp.into_reader();
    let mut buf: Vec<u8> = Vec::with_capacity(64 * 1024);
    let mut chunk = [0u8; 16 * 1024];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                let remain = MAX_BODY_BYTES.saturating_sub(buf.len());
                buf.extend_from_slice(&chunk[..n.min(remain)]);
                if buf.len() >= MAX_BODY_BYTES {
                    break;
                }
            }
            Err(e) => return Err(format!("天气数据读取失败: {e}")),
        }
    }
    Ok(String::from_utf8_lossy(&buf).to_string())
}

/// 极简 percent-encode（保留字母数字与 `-_.~`），供 city 名进 URL path
fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// 从用户输入里提取城市名（纯函数，便于单测）。
///
/// 只在**明确出现**城市痕迹时返回 Some，宁可漏识别（回退到设置项 / IP 定位）
/// 也不乱猜——猜错的天气比不答更糟。
///
/// 识别两类形态：
/// 1. 「XX的天气 / XX天气 / XX今天天气」→ 取「天气」前的中文地名（≤6 字）；
/// 2. 「XX怎么样」在已出现「天气」语境下→ 同上。
pub fn extract_city(text: &str) -> Option<String> {
    let t = text.trim();
    if t.is_empty() {
        return None;
    }
    // 这些词紧邻「天气」时不是地名（「今天天气」「明天天气怎么样」）
    const NOT_CITY: &[&str] = &[
        "今天", "明天", "后天", "昨天", "今晚", "明晚", "现在", "当前", "最近", "这周", "周末",
        "天气", "这儿", "这里", "那边", "那边儿", "外面", "外面儿", "我这儿", "你这儿",
    ];

    let idx = t.find("天气")?;
    let before = &t[..idx];
    // 从左往右扫，取最靠右的连续中文串（最贴近「天气」的那个词才是地名）
    let chars: Vec<char> = before.chars().collect();
    let mut end = chars.len();
    while end > 0 && !is_cjk(chars[end - 1]) {
        end -= 1;
    }
    let mut start = end;
    while start > 0 && is_cjk(chars[start - 1]) {
        start -= 1;
    }
    if start == end {
        return None;
    }
    let candidate: String = chars[start..end].iter().collect();
    // 过长不是地名（「我想知道北京今天」这类整句）
    if candidate.chars().count() > 6 || candidate.chars().count() < 2 {
        return None;
    }
    // 「的」是常见连接词：「北京的天气」→ 去掉尾部的「的」
    let candidate = candidate.trim_end_matches('的').to_string();
    if candidate.is_empty() || NOT_CITY.contains(&candidate.as_str()) {
        return None;
    }
    // 「今天天气」跨词：候选若以上述词结尾，剥掉前缀再判
    for w in NOT_CITY {
        if let Some(rest) = candidate.strip_suffix(w) {
            if rest.chars().count() >= 2 {
                return Some(rest.to_string());
            }
            return None;
        }
    }
    Some(candidate)
}

fn is_cjk(c: char) -> bool {
    matches!(c, '\u{4e00}'..='\u{9fff}')
}

/// 用户这句话是不是在问天气（用于决定要不要主动查）。
/// 关键词表保守：只认明确的天气意图，不把「今天好热」当成要查天气。
pub fn looks_like_weather_query(text: &str) -> bool {
    const KEYS: &[&str] = &[
        "天气", "气温", "温度", "下雨", "下雪", "会不会热", "冷不冷", "热不热", "几度", "穿什么",
        "带伞", "要不要带伞", "有雨吗", "会下雨吗",
    ];
    let t = text.trim();
    KEYS.iter().any(|k| t.contains(k))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真实 wttr.in j1 响应裁剪版：验证字段口径与中文映射
    #[test]
    fn parse_real_j1_shape() {
        let json = r#"{
          "current_condition":[{
            "temp_C":"34","FeelsLikeC":"35","humidity":"41",
            "windspeedKmph":"12","winddir16Point":"NW",
            "weatherDesc":[{"value":"Sunny"}],"lang_zh":[{"value":"Sunny"}]
          }],
          "nearest_area":[{"areaName":[{"value":"Dongguan"}],"region":[{"value":"Guangdong"}]}],
          "weather":[{"maxtempC":"36","mintempC":"27"}]
        }"#;
        let r = parse_j1(json).unwrap();
        assert!(r.summary.contains("东莞") || r.summary.contains("Dongguan"));
        assert!(r.summary.contains("晴"), "Sunny 应映射为中文：{}", r.summary);
        assert!(r.summary.contains("34°C"));
        assert!(r.summary.contains("体感 35°C"));
        assert!(r.summary.contains("湿度 41%"));
        assert!(r.summary.contains("西北风 12km/h"));
        assert!(r.summary.contains("27~36°C"));
    }

    /// 体感与实测相同则不重复展示（否则「34°C（体感 34°C）」很啰嗦）
    #[test]
    fn parse_omits_redundant_feels_like() {
        let json = r#"{"current_condition":[{"temp_C":"20","FeelsLikeC":"20","humidity":"","windspeedKmph":"","winddir16Point":"","weatherDesc":[{"value":"Cloudy"}]}],"nearest_area":[{"areaName":[{"value":"X"}],"region":[{"value":"Y"}]}]}"#;
        let r = parse_j1(json).unwrap();
        assert!(!r.summary.contains("体感"), "{}", r.summary);
        assert!(r.summary.contains("阴"));
    }

    /// 缺字段不得 panic，也不能拼出空段
    #[test]
    fn parse_survives_missing_fields() {
        let json = r#"{"current_condition":[{"temp_C":"5"}],"nearest_area":[],"weather":[]}"#;
        let r = parse_j1(json).unwrap();
        assert_eq!(r.summary.trim(), "5°C");
    }

    #[test]
    fn parse_rejects_garbage() {
        assert!(parse_j1("not json").is_err());
        assert!(parse_j1("{}").is_err());
    }

    /// 城市提取：只认明确形态，不能把「今天」当地名
    #[test]
    fn extract_city_basic() {
        assert_eq!(extract_city("北京天气怎么样").as_deref(), Some("北京"));
        assert_eq!(extract_city("北京的天气如何").as_deref(), Some("北京"));
        assert_eq!(extract_city("上海今天天气").as_deref(), Some("上海"));
        assert_eq!(extract_city("深圳明天天气好嘛").as_deref(), Some("深圳"));
    }

    /// 回归红线：非地名的时间词绝不能被当成城市（否则查出一个不存在的城市）
    #[test]
    fn extract_city_rejects_time_words() {
        assert_eq!(extract_city("今天天气怎么样"), None);
        assert_eq!(extract_city("明天天气"), None);
        assert_eq!(extract_city("现在天气如何"), None);
        assert_eq!(extract_city("这周天气"), None);
    }

    #[test]
    fn extract_city_rejects_non_weather_text() {
        assert_eq!(extract_city("你好"), None);
        assert_eq!(extract_city(""), None);
        assert_eq!(extract_city("天气"), None);
    }

    #[test]
    fn weather_query_detection() {
        assert!(looks_like_weather_query("今天天气怎么样"));
        assert!(looks_like_weather_query("外面下雨吗"));
        assert!(looks_like_weather_query("要不要带伞"));
        assert!(!looks_like_weather_query("今天好累"));
        assert!(!looks_like_weather_query("帮我写段代码"));
    }

    // ---- 以下是**真实联网**用例（默认 ignore，手动跑：cargo test -- --ignored live_）----
    //
    // 单测只能验证解析，验不了「这台机器现在到底能不能连上」。
    // 而本次改动的全部动机正是旧搜索源在国内不可达，所以必须保留实测入口。

    #[test]
    #[ignore = "需要外网；手动执行验证真实可达性"]
    fn live_weather_by_city() {
        let r = query("Dongguan").expect("东莞天气查询应成功");
        println!("[live] {}", r.summary);
        assert!(r.summary.contains("°C"), "应含温度：{}", r.summary);
    }

    #[test]
    #[ignore = "需要外网；手动执行验证真实可达性"]
    fn live_weather_by_ip() {
        let r = query("").expect("IP 定位查询应成功");
        println!("[live] IP 定位 => {}", r.summary);
        assert!(!r.summary.is_empty());
    }

    /// 完整链路：用户原话 → 城市提取 → 真实查询
    #[test]
    #[ignore = "需要外网；手动执行验证真实可达性"]
    fn live_city_extraction_roundtrip() {
        let q = "东莞天气怎么样";
        let city = extract_city(q).expect("应能提取东莞");
        let r = query(&city).expect("提取出的城市应查得到");
        println!("[live] {q} => {city} => {}", r.summary);
        let lower = r.summary.to_ascii_lowercase();
        assert!(
            lower.contains("dongguan") || r.summary.contains("东莞"),
            "应命中东莞：{}",
            r.summary
        );
    }
}
