//! 豆包上游 HTTP 请求构造与风控识别（E-01，真机口径）。
//!
//! ── 请求四要素（批次 0 真机抓包固化，见设计文档附录 A.2 / B.4）──────────
//! 1. **端点** `POST https://www.doubao.com/chat/completion?<query>`（PC 客户端口径）。
//! 2. **query** = 安装级真实指纹（24 键，字母序）+ `web_tab_id`（每次新 uuid）
//!    + `msToken` + `a_bogus`；其中 `a_bogus` 由 [`crate::tasks::doubao_abogus::generate`]
//!    按「去掉 `a_bogus` 后的整串 params」生成（服务端按同一 params 反算校验，故顺序
//!    必须与实发一致）。
//! 3. **请求头** = 真机集（`Agw-Js-Conv: str, str`、`Accept: */*`、
//!    `Referer: https://www.doubao.com/`、桌面客户端 UA、`sec-ch-ua` 三件等）。
//! 4. **响应头 `x-ms-token`** 为 `msToken` 续期源（按安装级缓存回填）。
//!
//! ── 指纹与模板来源 ─────────────────────────────────────────
//! MITM 旁路采集文件 `<数据根目录>/doubao_abogus_capture.json`
//!（由 `device_proxy::doubao_capture` 落盘，**只含非凭据信息**：指纹白名单 + msToken +
//! a_bogus 模板）。缺失时无法自签名 → 明确报错、**不发起请求**。
//!
//! ── 硬约束 ─────────────────────────────────────────────────
//! `DoubaoCreds` / [`DoubaoSinger`] **禁 Debug/Serialize/日志**，凭证与 token 绝不落盘或回显；
//! 错误文案只输出结构化原因，不含任何取值。

use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde_json::Value;

use super::wb_upstream::UpstreamErr;
use crate::tasks::doubao_abogus::{self, ABogusTemplate};
use crate::tasks::doubao_chats::build_cookie;

/// 真机端点（PC 客户端口径，与 `/samantha/chat/completion` 同族不同宿主）
pub const CHAT_PATH: &str = "/chat/completion";
/// 真机桌面客户端 UA（批次 0 抓包采样值）
pub const PC_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
(KHTML, like Gecko) Chrome/147.0.0.0 Safari/537.36 SamanthaDoubao/2.32.5";
/// 采集文件名（与 `device_proxy::doubao_capture::CAPTURE_FILE_NAME` 同名）
const CAPTURE_FILE: &str = "doubao_abogus_capture.json";

/// 豆包凭证（仅内存；禁日志/序列化——不派生 Debug/Serialize）
pub struct DoubaoCreds {
    pub session_id: String,
    pub sid_guard: String,
    pub ttwid: String,
}

/// 组装 Cookie 串（复用 `doubao_chats::build_cookie` 语义：固定顺序 + `%` 防二次编码）
pub fn cookie_of(c: &DoubaoCreds) -> String {
    // build_cookie 需要 user_id/session_id/sid_guard/ttwid 字段；uid 仅用于报错文案
    let acc = serde_json::json!({
        "user_id": "",
        "session_id": c.session_id,
        "sid_guard": c.sid_guard,
        "ttwid": c.ttwid,
    });
    build_cookie(&acc).unwrap_or_default()
}

/// 豆包 SSE Agent：连接 10s / 空闲读 300s（对齐 `wb_upstream::wb_agent`）
pub fn doubao_agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_read(Duration::from_secs(300))
        .timeout_write(Duration::from_secs(30))
        .timeout_connect(Duration::from_secs(10))
        .max_idle_connections(20)
        .max_idle_connections_per_host(20)
        .build()
}

// ==================== 安装级签名器（指纹 + msToken + a_bogus 模板） ====================

/// 安装级签名材料：真实指纹（字母序）+ 可续期 `msToken` + `a_bogus` 结构模板。
///
/// 不派生 `Debug`——内含 `msToken`，禁止任何形式的回显/落盘。
pub struct DoubaoSinger {
    /// 指纹键值：值保持采集时的**原样百分号编码**（不再二次编码）
    fingerprint: BTreeMap<String, String>,
    /// `msToken`（原值；发 query 时百分号编码）
    ms_token: Option<String>,
    /// `a_bogus` 模板（payload 定长域 + 设备串 + 三段头）
    template: Option<ABogusTemplate>,
    /// 采集时间（毫秒，仅供诊断）
    captured_at: Option<i64>,
}

impl DoubaoSinger {
    /// 由采集文件 JSON 构造；无指纹即报错（签名无意义）。
    pub fn from_capture_json(v: &Value) -> Result<Self, String> {
        let fp_obj = v
            .get("fingerprint")
            .and_then(Value::as_object)
            .ok_or_else(|| "采集文件缺 fingerprint 对象".to_string())?;
        if fp_obj.is_empty() {
            return Err("采集文件 fingerprint 为空".to_string());
        }
        let mut fingerprint = BTreeMap::new();
        for (k, val) in fp_obj {
            if let Some(s) = val.as_str() {
                fingerprint.insert(k.clone(), s.to_string());
            }
        }
        let ms_token = v
            .get("ms_token")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        let template = v
            .get("abogus_template")
            .map(|t| template_from_json(t))
            .transpose()?;
        let captured_at = v.get("captured_at").and_then(Value::as_i64);
        Ok(Self {
            fingerprint,
            ms_token,
            template,
            captured_at,
        })
    }

    /// 指纹键数（诊断用，不回显取值）
    pub fn fingerprint_len(&self) -> usize {
        self.fingerprint.len()
    }

    /// 采集时间（毫秒；0 = 未知）
    pub fn captured_at(&self) -> i64 {
        self.captured_at.unwrap_or(0)
    }

    /// 是否已具备 `a_bogus` 模板（无模板则无法生成签名）
    pub fn has_template(&self) -> bool {
        self.template.is_some()
    }

    /// 是否已有 `msToken`
    pub fn has_ms_token(&self) -> bool {
        self.ms_token.as_deref().map(|s| !s.is_empty()).unwrap_or(false)
    }

    /// `x-ms-token` 续期回填（空值忽略）
    pub fn set_ms_token(&mut self, token: &str) {
        if !token.is_empty() {
            self.ms_token = Some(token.to_string());
        }
    }

    /// 组装 query **去掉 `a_bogus` 的整串 params**：
    /// 指纹（字母序）→ `web_tab_id`（本次新 uuid）→ `msToken`（有则带，百分号编码）。
    fn params_without_abogus(&self) -> String {
        let mut parts: Vec<String> = Vec::with_capacity(self.fingerprint.len() + 2);
        for (k, v) in &self.fingerprint {
            parts.push(format!("{k}={v}"));
        }
        // web_tab_id 为易变参数（采集时被刻意排除），每次请求新掷一个 uuid v4
        parts.push(format!("web_tab_id={}", uuid::Uuid::new_v4()));
        if let Some(mt) = self.ms_token.as_deref().filter(|s| !s.is_empty()) {
            parts.push(format!("msToken={}", percent_encode(mt)));
        }
        parts.join("&")
    }

    /// 生成完整签名 query（params + `a_bogus`）；缺模板返回 `None`。
    pub fn signed_query(&self, now_ms: i64) -> Option<String> {
        let tpl = self.template.as_ref()?;
        let params = self.params_without_abogus();
        let a_bogus = doubao_abogus::generate(&params, tpl, now_ms);
        Some(format!("{params}&a_bogus={}", percent_encode(&a_bogus)))
    }
}

/// 模板 JSON（十六进制分段）→ [`ABogusTemplate`]
fn template_from_json(v: &Value) -> Result<ABogusTemplate, String> {
    let get_hex = |key: &str| -> Result<Vec<u8>, String> {
        let s = v
            .get(key)
            .and_then(Value::as_str)
            .ok_or_else(|| format!("模板缺字段 {key}"))?;
        hex_decode(s)
    };
    let prefix_v = get_hex("prefix")?;
    let version_v = get_hex("version")?;
    let trailer = get_hex("trailer")?;
    let prefix50_v = get_hex("prefix50")?;
    let device = v
        .get("device")
        .and_then(Value::as_str)
        .ok_or_else(|| "模板缺字段 device".to_string())?
        .to_string();

    let mut prefix = [0u8; doubao_abogus::PREFIX_LEN];
    if prefix_v.len() != prefix.len() {
        return Err(format!("模板 prefix 长度应为 4 字节，实为 {}", prefix_v.len()));
    }
    prefix.copy_from_slice(&prefix_v);
    let mut version = [0u8; doubao_abogus::VERSION_LEN];
    if version_v.len() != version.len() {
        return Err(format!("模板 version 长度应为 8 字节，实为 {}", version_v.len()));
    }
    version.copy_from_slice(&version_v);
    let mut prefix50 = [0u8; 50];
    if prefix50_v.len() != prefix50.len() {
        return Err(format!("模板 prefix50 长度应为 50 字节，实为 {}", prefix50_v.len()));
    }
    prefix50.copy_from_slice(&prefix50_v);

    Ok(ABogusTemplate {
        prefix,
        version,
        trailer,
        prefix50,
        device,
    })
}

/// 十六进制 → 字节（奇数长度/非法字符报错）
fn hex_decode(s: &str) -> Result<Vec<u8>, String> {
    if s.len() % 2 != 0 {
        return Err(format!("十六进制长度应为偶数，实为 {}", s.len()));
    }
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len() / 2);
    for chunk in b.chunks_exact(2) {
        let hi = (chunk[0] as char)
            .to_digit(16)
            .ok_or_else(|| "十六进制含非法字符".to_string())?;
        let lo = (chunk[1] as char)
            .to_digit(16)
            .ok_or_else(|| "十六进制含非法字符".to_string())?;
        out.push((hi * 16 + lo) as u8);
    }
    Ok(out)
}

/// 百分号编码（unreserved 之外一律 %XX；真机 query 口径）
pub fn percent_encode(s: &str) -> String {
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

/// 百分号解码（非法转义原样保留；当前仅单测使用）
#[allow(dead_code)]
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// 当前毫秒时间戳
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 链路追踪头 `x-flow-trace`（真机形态 `04-<32 hex>-<16 hex>-01`；每请求新掷）
fn flow_trace() -> String {
    let b = uuid::Uuid::new_v4().into_bytes();
    let hex = |bytes: &[u8]| -> String { bytes.iter().map(|x| format!("{x:02x}")).collect() };
    format!("04-{}-{}-01", hex(&b[..16]), hex(&b[..8]))
}

/// 进程内签名器缓存（一次加载，跨请求复用；`msToken` 续期就地回填）
fn signer_store() -> &'static Mutex<Option<DoubaoSinger>> {
    static STORE: OnceLock<Mutex<Option<DoubaoSinger>>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(None))
}

fn safe_lock<'a, T>(m: &'a Mutex<T>) -> std::sync::MutexGuard<'a, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// 确保签名器已加载（首次从采集文件读入；已加载则直接返回）
pub fn ensure_signer(data_dir: &Path) -> Result<(), String> {
    if safe_lock(signer_store()).is_some() {
        return Ok(());
    }
    let path = data_dir.join(CAPTURE_FILE);
    let bytes = std::fs::read(&path)
        .map_err(|e| format!("读取采集文件失败（{}）: {e}", path.display()))?;
    let v: Value = serde_json::from_slice(&bytes).map_err(|e| format!("采集文件解析失败: {e}"))?;
    let signer = DoubaoSinger::from_capture_json(&v)?;
    *safe_lock(signer_store()) = Some(signer);
    Ok(())
}

/// `x-ms-token` 续期回填（未加载/空值忽略）
pub fn note_ms_token(token: &str) {
    if let Some(s) = safe_lock(signer_store()).as_mut() {
        s.set_ms_token(token);
    }
}

/// 签名器诊断快照（`/status` 用；只输出计数与布尔，不回显取值）
#[derive(Clone, serde::Serialize)]
pub struct SignerInfo {
    pub loaded: bool,
    pub fingerprint_keys: usize,
    pub has_template: bool,
    pub has_ms_token: bool,
    pub captured_at: i64,
}

/// 当前签名器概况
pub fn signer_info() -> SignerInfo {
    let guard = safe_lock(signer_store());
    match guard.as_ref() {
        Some(s) => SignerInfo {
            loaded: true,
            fingerprint_keys: s.fingerprint_len(),
            has_template: s.has_template(),
            has_ms_token: s.has_ms_token(),
            captured_at: s.captured_at(),
        },
        None => SignerInfo {
            loaded: false,
            fingerprint_keys: 0,
            has_template: false,
            has_ms_token: false,
            captured_at: 0,
        },
    }
}

/// 一次豆包响应的读取端与续期信息
pub struct DoubaoResponse {
    /// SSE 流 reader
    pub reader: Box<dyn Read + Send>,
    /// 响应头 `x-ms-token`（已就地回填签名器）
    pub ms_token: Option<String>,
}

/// 发起豆包 completion 请求（真机端点 + 自签名 query + 真机请求头）。
///
/// 签名材料缺失（无采集文件/无模板）→ `Err`（500，不发起请求，提示先走一次代理抓包）。
pub fn make_doubao_request(
    cookie: &str,
    body: &Value,
    data_dir: &Path,
) -> Result<DoubaoResponse, UpstreamErr> {
    ensure_signer(data_dir).map_err(|e| (500u16, format!("豆包签名不可用: {e}"), None))?;
    let query = {
        let guard = safe_lock(signer_store());
        guard
            .as_ref()
            .and_then(|s| s.signed_query(now_ms()))
            .ok_or_else(|| (500u16, "豆包签名生成失败（缺 a_bogus 模板）".to_string(), None))?
    };
    let url = format!("https://www.doubao.com{CHAT_PATH}?{query}");

    let out = doubao_agent()
        .post(&url)
        .set("content-type", "application/json")
        .set("cookie", cookie)
        .set("agw-js-conv", "str, str")
        .set("last-event-id", "undefined")
        .set("user-agent", PC_UA)
        .set("accept", "*/*")
        .set("accept-language", "zh-CN,zh;q=0.9")
        .set("referer", "https://www.doubao.com/")
        // 真机头集**没有** `Origin`，且带一个形如 `04-<32hex>-<16hex>-01` 的链路追踪头；
        // 此前我们多发 Origin、缺 x-flow-trace，是与真机最明显的两处头部差异
        .set("x-flow-trace", &flow_trace())
        .set("sec-fetch-site", "cross-site")
        .set("sec-fetch-mode", "cors")
        .set("sec-fetch-dest", "empty")
        .set("sec-ch-ua", "\"Chromium\";v=\"147\", \"Not.A/Brand\";v=\"8\"")
        .set("sec-ch-ua-mobile", "?0")
        .set("sec-ch-ua-platform", "\"Windows\"")
        .send_string(&body.to_string());

    match out {
        Ok(r) => {
            // 先取续期头（借用期），再转 reader
            let token = r.header("x-ms-token").map(str::to_string);
            if let Some(t) = token.as_deref() {
                note_ms_token(t);
            }
            Ok(DoubaoResponse {
                reader: Box::new(r.into_reader()),
                ms_token: token,
            })
        }
        Err(ureq::Error::Status(code, resp)) => {
            // 失败响应同样可能带续期头（风控页也会回吐）
            if let Some(t) = resp.header("x-ms-token") {
                note_ms_token(t);
            }
            let body_text = resp
                .into_string()
                .unwrap_or_else(|e| format!("<响应体读取失败: {e}>"));
            Err((code, body_text, None))
        }
        Err(e) => Err((502, format!("豆包传输错误: {e}"), None)),
    }
}

/// 豆包风控码（账号级；命中即冷却/禁用）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RiskCode {
    /// 710012001：sessionid 被吊销 → 禁用账号（需重新登录）
    Revoked,
    /// 710022004：验证码墙 → 长冷却 + needs_captcha（需人工过验证码）
    Captcha,
    /// 712010702：Cookie 或指纹错误 → 换号（v1 简化）
    Fingerprint,
}

impl RiskCode {
    pub fn code(self) -> &'static str {
        match self {
            RiskCode::Revoked => "710012001",
            RiskCode::Captcha => "710022004",
            RiskCode::Fingerprint => "712010702",
        }
    }
}

/// 从响应体（或流内任意行文本）识别已知风控码
pub fn detect_risk(_status: u16, body: &str) -> Option<RiskCode> {
    for c in [RiskCode::Revoked, RiskCode::Captcha, RiskCode::Fingerprint] {
        if body.contains(c.code()) {
            return Some(c);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_known_risk_codes() {
        assert_eq!(detect_risk(200, "xx 710012001 yy"), Some(RiskCode::Revoked));
        assert_eq!(detect_risk(200, "{\"code\":710022004}"), Some(RiskCode::Captcha));
        assert_eq!(detect_risk(200, "err 712010702"), Some(RiskCode::Fingerprint));
        assert_eq!(detect_risk(200, "normal body"), None);
    }

    #[test]
    fn cookie_of_builds_expected_order() {
        let c = DoubaoCreds {
            session_id: "SID".into(),
            sid_guard: "G".into(),
            ttwid: "T".into(),
        };
        let cookie = cookie_of(&c);
        assert_eq!(cookie, "sessionid=SID; sessionid_ss=SID; sid_tt=SID; sid_guard=G; ttwid=T");
    }

    #[test]
    fn percent_encode_roundtrip_and_reserved() {
        assert_eq!(percent_encode("abc-D_1.~"), "abc-D_1.~");
        assert_eq!(percent_encode("a/b+c="), "a%2Fb%2Bc%3D");
        for s in ["", "a", "x/y+z=1", "%2F already"] {
            assert_eq!(percent_decode(&percent_encode(s)), s);
        }
    }

    #[test]
    fn hex_decode_rules() {
        assert_eq!(hex_decode("").unwrap(), Vec::<u8>::new());
        assert_eq!(hex_decode("0aFF").unwrap(), vec![0x0a, 0xFF]);
        assert!(hex_decode("abc").is_err(), "奇数长度应报错");
        assert!(hex_decode("zz").is_err(), "非法字符应报错");
    }

    /// 合成采集 JSON → 签名器：query 键序为「指纹字母序 + web_tab_id + msToken + a_bogus」，
    /// 且 `a_bogus` 可解码、payload 的 9/12/15 三字节等于对同一 params 的 URL 哈希三字节。
    #[test]
    fn signed_query_shape_and_hash_consistency() {
        // 合成模板（不含任何真实取值）
        let mut prefix50 = String::new();
        for i in 0..50u8 {
            prefix50.push_str(&format!("{:02x}", i.wrapping_mul(7).wrapping_add(1) | 1));
        }
        let capture = serde_json::json!({
            "captured_at": 1_700_000_000_000i64,
            "fingerprint": {
                "aid": "1111",
                "web_id": "9999",
                "device_id": "88888888",
                "tz_name": "Asia%2FShanghai",
                "channel": "win",
            },
            "ms_token": "TOK/val+ue=",
            "abogus_template": {
                "prefix": "01020304",
                "version": "1020304050607080",
                "trailer": "0a0b",
                "prefix50": prefix50,
                "device": "1234567890",
            }
        });

        let signer = DoubaoSinger::from_capture_json(&capture).expect("合成采集应可解析");
        assert!(signer.has_template());
        assert!(signer.has_ms_token());
        assert_eq!(signer.fingerprint_len(), 5);

        let query = signer.signed_query(1_600_000_000_000).expect("应能生成签名");
        // 字母序：aid < channel < device_id < tz_name < web_id，随后 web_tab_id / msToken / a_bogus
        let keys: Vec<&str> = query
            .split('&')
            .map(|kv| kv.split('=').next().unwrap_or(""))
            .collect();
        assert_eq!(
            keys,
            vec!["aid", "channel", "device_id", "tz_name", "web_id", "web_tab_id", "msToken", "a_bogus"]
        );
        assert!(query.contains("tz_name=Asia%2FShanghai"), "指纹值保持原样编码");
        assert!(query.contains("msToken=TOK%2Fval%2Bue%3D"), "msToken 应百分号编码");
        assert!(query.contains("web_tab_id="), "web_tab_id 每次新掷");

        // 取下 a_bogus 原样编码值 → 百分号解码 → 解码；params = query 去掉 &a_bogus=
        let ab_enc = query.split("&a_bogus=").nth(1).expect("应带 a_bogus");
        let ab = percent_decode(ab_enc);
        let parts = doubao_abogus::decode(&ab).expect("自产 a_bogus 应可解码");
        let params = query.split("&a_bogus=").next().unwrap();
        let t = doubao_abogus::url_hash_triplet(params);
        assert_eq!(parts.payload[9], t[0]);
        assert_eq!(parts.payload[12], t[1]);
        assert_eq!(parts.payload[15], t[2]);
        // 设备串与尾部结构保持（50 + 10 + 3）
        assert_eq!(parts.payload.len(), 63);
    }

    /// 无模板 / 无指纹：签名不可用
    #[test]
    fn signed_query_requires_template() {
        let capture = serde_json::json!({
            "fingerprint": { "aid": "1" },
            "ms_token": "T"
        });
        let signer = DoubaoSinger::from_capture_json(&capture).expect("有指纹即可构造");
        assert!(!signer.has_template());
        assert!(signer.signed_query(0).is_none(), "无模板不得产出签名");

        let empty_fp = serde_json::json!({ "fingerprint": {} });
        assert!(DoubaoSinger::from_capture_json(&empty_fp).is_err(), "空指纹应报错");
    }

    /// 模板字段缺失/长度不符 → 明确报错
    #[test]
    fn template_json_validation() {
        let bad_len = serde_json::json!({
            "prefix": "0102",
            "version": "1020304050607080",
            "trailer": "",
            "prefix50": "00",
            "device": "d"
        });
        assert!(template_from_json(&bad_len).is_err(), "prefix 长度不为 4 应报错");

        let missing = serde_json::json!({ "prefix": "01020304" });
        assert!(template_from_json(&missing).is_err(), "缺字段应报错");
    }

    /// 续期回填：空串忽略、非空覆盖
    #[test]
    fn set_ms_token_ignores_empty() {
        let capture = serde_json::json!({ "fingerprint": { "aid": "1" }, "ms_token": "T1" });
        let mut signer = DoubaoSinger::from_capture_json(&capture).unwrap();
        signer.set_ms_token("");
        assert!(signer.has_ms_token());
        let q1 = signer.params_without_abogus();
        assert!(q1.contains("msToken=T1"));
        signer.set_ms_token("T2/x");
        let q2 = signer.params_without_abogus();
        assert!(q2.contains("msToken=T2%2Fx"));
        assert!(!q2.contains("T1"));
    }

    // ---------------- 真机闭环诊断（`#[ignore]`，仅本机跑） ----------------

    /// 从 vault 解析一份可用豆包凭证（**不回显**）。
    ///
    /// 与生产 `doubao_identity` 回调同口径：先按池内账号 uid 取，再兜底全局抓包槽 `captured`。
    fn resolve_probe_creds(dd: &std::path::Path) -> Option<DoubaoCreds> {
        let pick = |v: &Value| DoubaoCreds {
            session_id: v.get("session_id").and_then(Value::as_str).unwrap_or("").to_string(),
            sid_guard: v.get("sid_guard").and_then(Value::as_str).unwrap_or("").to_string(),
            ttwid: v.get("ttwid").and_then(Value::as_str).unwrap_or("").to_string(),
        };
        let conn = crate::store::db(dd);
        let pool = crate::store::docs::doubao_pool_load(&conn);
        for a in &pool.accounts {
            if let Some(sec) = crate::vault::ns_get(dd, "doubao", &a.user_id) {
                let c = pick(&sec);
                if !c.session_id.is_empty() {
                    return Some(c);
                }
            }
        }
        crate::vault::ns_get(dd, "doubao", "captured")
            .map(|sec| pick(&sec))
            .filter(|c| !c.session_id.is_empty())
    }

    /// 真机闭环诊断：用 vault 凭证 + 采集文件模板走一次真实 `/chat/completion`，
    /// 并把 SSE 聚合成 OpenAI 完成体。**只打印非敏感元数据**（HTTP 结果 / 签名器概况 /
    /// 事件计数 / 正文与思考的字符数 / 风控码），**绝不打印 cookie、msToken、a_bogus 或正文全文**。
    ///
    /// 运行：`cargo test --no-default-features doubao_upstream::tests::real_machine_probe -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn real_machine_probe() {
        let dir = std::path::Path::new(&std::env::var("APPDATA").expect("APPDATA 缺失"))
            .join(crate::state::DATA_DIR_NAME);

        // ① 签名材料（采集文件 → 进程内签名器）
        if let Err(e) = ensure_signer(&dir) {
            println!("[probe] 签名材料不可用: {e}");
            return;
        }
        let si = signer_info();
        println!(
            "[probe] signer loaded={} fp_keys={} has_template={} has_ms_token={}",
            si.loaded, si.fingerprint_keys, si.has_template, si.has_ms_token
        );
        // 签名 query 长度（只报长度，不报内容）
        let qlen = safe_lock(signer_store())
            .as_ref()
            .and_then(|s| s.signed_query(now_ms()))
            .map(|q| q.len());
        println!("[probe] signed_query_len={:?}", qlen);

        // ② 凭证（vault 解析；仅内存）
        let Some(creds) = resolve_probe_creds(&dir) else {
            println!("[probe] 未找到可用豆包凭证（vault 空）");
            return;
        };
        let cookie = cookie_of(&creds);
        println!("[probe] cookie_len={} (值不回显)", cookie.len());

        // ③ 请求体（auto 档）
        let m = crate::api_server::doubao_payload::lookup("doubao-auto").expect("目录应含 doubao-auto");
        let peek = serde_json::json!({
            "messages": [{"role": "user", "content": "用一句话介绍你自己"}]
        });
        let body = match (
            std::env::var("DOUBAO_PROBE_CID"),
            std::env::var("DOUBAO_PROBE_SECTION"),
        ) {
            (Ok(cid), Ok(sec)) => {
                // 目标会话由环境变量注入（**不落代码**）：用于验证「续聊体」是否避开新建会话频控
                let idx = std::env::var("DOUBAO_PROBE_INDEX")
                    .ok()
                    .and_then(|s| s.parse::<i64>().ok())
                    .unwrap_or(0);
                println!(
                    "[probe] 续聊模式 cid_len={} section_len={} index={}",
                    cid.len(),
                    sec.len(),
                    idx
                );
                crate::api_server::doubao_payload::build_body_continue(&peek, m, &cid, &sec, idx)
            }
            _ => crate::api_server::doubao_payload::build_body(&peek, m),
        };
        println!("[probe] body_bytes={}", body.to_string().len());

        // ④ 真机请求
        let resp = match make_doubao_request(&cookie, &body, &dir) {
            Ok(r) => r,
            Err((status, msg, _)) => {
                println!(
                    "[probe] HTTP 层错误: status={status} body_len={} risk={:?}",
                    msg.len(),
                    detect_risk(status, &msg).map(|c| c.code())
                );
                return;
            }
        };
        println!("[probe] HTTP 200；响应头 x-ms-token 存在={}", resp.ms_token.is_some());

        // ⑤ 先收全原始行（诊断用）：落盘 + 打印帧结构（**只打键名/长度，不打正文与任何参数值**）
        let lines =
            match crate::api_server::wb_upstream::lines_with_first_byte_timeout_interruptible(resp.reader) {
                Ok(l) => l,
                Err(()) => {
                    println!("[probe] 首字超时或首行前出错");
                    return;
                }
            };
        let raw: Vec<String> = lines.collect();
        let total: usize = raw.iter().map(|l| l.len() + 1).sum();
        let data_lines = raw.iter().filter(|l| l.trim_start().starts_with("data:")).count();
        println!(
            "[probe] raw_lines={} data_lines={} raw_bytes={}",
            raw.len(),
            data_lines,
            total
        );
        let mut shown = 0usize;
        for l in &raw {
            let t = l.trim().strip_prefix("data:").unwrap_or(l).trim();
            if t.is_empty() || t == "[DONE]" {
                continue;
            }
            if let Ok(v) = serde_json::from_str::<Value>(t) {
                let keys: Vec<&str> = v
                    .as_object()
                    .map(|o| o.keys().map(String::as_str).collect())
                    .unwrap_or_default();
                println!("[probe] frame#{shown} keys={keys:?} line_len={}", t.len());
                shown += 1;
                if shown >= 8 {
                    break;
                }
            }
        }
        if !raw.is_empty() {
            let dump = std::env::temp_dir().join("doubao_probe_body.txt");
            let _ = std::fs::write(&dump, raw.join("\n"));
            println!("[probe] raw dumped: {}", dump.display());
        }

        // ⑥ SSE 聚合（复用生产同款投影）
        let risk_slot: crate::api_server::doubao_sse::RiskSlot = std::sync::Arc::new(Mutex::new(None));
        let (completion, err, risk) =
            crate::api_server::doubao_sse::aggregate(raw.into_iter(), "probe", risk_slot);
        match &completion {
            Some(v) => {
                let msg = &v["choices"][0]["message"];
                let text = msg["content"].as_str().unwrap_or("");
                let reasoning = msg["reasoning_content"].as_str().unwrap_or("");
                println!(
                    "[probe] 聚合成功: content_chars={} reasoning_chars={} usage={}",
                    text.chars().count(),
                    reasoning.chars().count(),
                    v.get("usage").cloned().unwrap_or(Value::Null)
                );
                let preview: String = text.chars().take(40).collect();
                println!("[probe] content_preview_40={preview}");
            }
            None => println!(
                "[probe] 聚合失败: err_status={:?} risk={:?}",
                err.as_ref().map(|(c, _)| *c),
                risk.map(|c| c.code())
            ),
        }
    }
}
