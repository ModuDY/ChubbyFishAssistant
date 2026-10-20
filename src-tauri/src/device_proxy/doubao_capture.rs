//! 豆包 `a_bogus` 模板抓取（MITM 旁路采集）。
//!
//! 背景：`a_bogus` 是豆包请求里的自签名参数，由前端 JS 生成、服务端反向校验。
//! 网关要自己生成签名，就需要一份「模板/指纹来源」——把真实请求里的**非凭据**结构
//! 摘出来落盘（[`crate::tasks::doubao_abogus::extract_template`] 反推的模板 + 一组
//! 静态指纹参数），后续由网关复用。
//!
//! 采集时机：代理转发到**已签名的豆包请求**（query 带 `a_bogus=`）时顺手落盘
//!（见 `handler::forward_upstream`）。
//!
//! **端点范围（实测修正）**：`aid` / `fp` 等指纹键**随端点族变化**——账号 SDK 端点
//!（`accounts.doubao.com/passport/*`、`www.doubao.com/samantha/user/*`、`alice/profile/self`）
//! 用 `aid=497858` + `fp=verify_<js-fp>`，而网关要复刻的 `/chat/completion` 用
//! `aid=582478` + `fp=verify_<device_id>`。旧的「端点无关 + 谁新用谁」合并会把前者的指纹
//! 覆盖进后者，导致网关拿别的端点族的 aid/fp 去签 completion。故**指纹与 msToken 仅采自
//! [`FINGERPRINT_ENDPOINT`]**；`a_bogus` 模板为端点无关的结构化常量，仍从任意已签名端点采集。
//!
//! ── 硬约束（脱敏红线）────────────────────────────────────
//! 只处理 query 参数与 `a_bogus` 模板；`sessionid` / `sid_guard` / `ttwid` / `cookie`
//! / `passport_csrf_token` 等**凭据一律不采集、不落盘、不打印**（白名单机制保证）。
//! 任何失败都在内部吞掉（顶多写一行 warning 到 proxy 日志），**绝不影响转发链路**。
//!
//! 落盘：`<数据根目录>/doubao_abogus_capture.json`，幂等——同一字段「谁新用谁」覆盖。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// 采集文件名（数据根目录下）
const CAPTURE_FILE_NAME: &str = "doubao_abogus_capture.json";

/// 指纹类参数白名单（query 键名 = JSON 字段名）。命中才采集，值原样保留（已百分号编码）。
///
/// 刻意不含 `a_bogus` / `msToken`（单独处理）与任何凭据键；`web_tab_id` 等每次会话变化
/// 的易变参数不入表，避免污染静态指纹。
const FINGERPRINT_KEYS: &[&str] = &[
    "aid",
    "real_aid",
    "device_id",
    "web_id",
    "tea_uuid",
    "fp",
    "pc_version",
    "doubao_pc_version",
    "version_code",
    "runtime_version",
    "chromium_version",
    "channel",
    "client_platform",
    "doubao_device_platform",
    "region",
    "sys_region",
    "language",
    "tz_name",
    "pkg_type",
    "web_platform",
    "samantha_web",
    "use-olympus-account",
    "device_platform",
    "runtime",
];

/// 指纹 / msToken 的采集端点（prefix 匹配）：只有网关真正复刻的端点才允许写入指纹，
/// 避免跨端点族污染（见模块头注释）。
const FINGERPRINT_ENDPOINT: &str = "/chat/completion";

/// 从一次**已签名的豆包请求**目标（path，含 query）中提取非凭据信息并落盘。
/// 幂等：同一字段用「谁新用谁」覆盖。
///
/// **端点敏感**：指纹 / msToken 只从 [`FINGERPRINT_ENDPOINT`]（即 `/chat/completion`）采集——
/// 实测账号 SDK 端点（`passport/*`、`samantha/user/*`、`alice/profile/self`）同带 `a_bogus`
/// 但用的是另一套 `aid` / `fp`，混采会把它们覆盖进 completion 指纹。`a_bogus` 模板为端点无关
/// 的结构化常量，仍从任意已签名端点采集。
pub fn capture_from_signed_request(path: &str) {
    let Some(dir) = data_dir() else {
        return;
    };
    if let Err(e) = capture_into(&dir, path) {
        warn(&dir, &format!("[abogus-capture] 采集失败（已忽略，不影响转发）: {e}"));
    }
}

/// 数据根目录（与 AppState 同源：`state::data_root()`，Windows 默认 D:\work\app-data）
fn data_dir() -> Option<PathBuf> {
    Some(crate::state::data_root())
}

/// 一次请求目标里摘出的待落盘信息（纯解析结果，便于单测）。
#[derive(Debug, Default, PartialEq, Eq)]
struct Captured {
    /// 指纹参数（键名 → 原样百分号编码值）
    fingerprint: BTreeMap<String, String>,
    /// `msToken`（百分号解码后的原值）
    ms_token: Option<String>,
    /// `a_bogus`（百分号解码后，待 `extract_template`）
    a_bogus_raw: Option<String>,
}

/// 解析请求目标（path 含 query）；无 query 返回 `None`。
fn parse(path: &str) -> Option<Captured> {
    let query = path.split_once('?').map(|(_, q)| q)?;
    let mut cap = Captured::default();
    for kv in query.split('&') {
        let Some((k, v)) = kv.split_once('=') else {
            continue;
        };
        if FINGERPRINT_KEYS.contains(&k) {
            cap.fingerprint.insert(k.to_string(), v.to_string());
        } else if k == "msToken" {
            cap.ms_token = Some(percent_decode(v));
        } else if k == "a_bogus" {
            cap.a_bogus_raw = Some(percent_decode(v));
        }
        // 其余键（含凭据类）一律忽略：白名单之外不采集
    }
    Some(cap)
}

/// 采集主体（`dir` 可注入，便于单测指向临时目录）。
fn capture_into(dir: &Path, path: &str) -> Result<(), String> {
    let cap = parse(path).ok_or_else(|| "请求目标无 query".to_string())?;
    // 指纹 / msToken 仅采自网关复刻的端点；其余端点只允许贡献 a_bogus 模板（见模块头注释）。
    // 这是本次实测修正的核心：跨端点族合并曾把 aid=497858 / fp=verify_<js-fp> 覆盖进 completion 指纹。
    let fp_endpoint = path.contains(FINGERPRINT_ENDPOINT);
    let fingerprint = if fp_endpoint { cap.fingerprint } else { BTreeMap::new() };
    let ms_token = if fp_endpoint { cap.ms_token } else { None };
    let has_fp = !fingerprint.is_empty();
    let has_ms = ms_token.is_some();
    // 无可采内容（非目标端点且无 a_bogus 模板）→ 不落盘，避免噪声文件
    if !has_fp && !has_ms && cap.a_bogus_raw.is_none() {
        return Ok(());
    }

    let file = dir.join(CAPTURE_FILE_NAME);
    // 读旧值做幂等合并（缺失/解析失败/非对象一律按空对象起算）
    let mut root: serde_json::Value = std::fs::read(&file)
        .ok()
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .filter(|v| v.is_object())
        .unwrap_or_else(|| serde_json::json!({}));

    let mut changed = false;
    if has_fp || has_ms {
        root["captured_at"] = serde_json::json!(now_ms());
        changed = true;
    }
    if has_fp {
        if !root.get("fingerprint").map(serde_json::Value::is_object).unwrap_or(false) {
            root["fingerprint"] = serde_json::json!({});
        }
        if let Some(obj) = root["fingerprint"].as_object_mut() {
            for (k, v) in &fingerprint {
                // 谁新用谁：本次出现的键覆盖旧值，未出现的旧键保留
                obj.insert(k.clone(), serde_json::Value::String(v.clone()));
            }
        }
    }
    if let Some(mt) = ms_token.as_ref() {
        root["ms_token"] = serde_json::Value::String(mt.clone());
    }
    if let Some(raw) = &cap.a_bogus_raw {
        // 解码失败保持历史模板（不覆盖为残值）
        if let Ok(tpl) = crate::tasks::doubao_abogus::extract_template(raw) {
            root["abogus_template"] = template_json(&tpl);
            changed = true;
        }
    }
    // 实际什么都没更新（例如非目标端点只带了非法 a_bogus）→ 静默返回，不写空文件
    if !changed {
        return Ok(());
    }

    let text = serde_json::to_string_pretty(&root).map_err(|e| e.to_string())?;
    std::fs::write(&file, text).map_err(|e| format!("写盘失败: {e}"))
}

/// 模板 → JSON（三个字节数组用十六进制字符串）
fn template_json(t: &crate::tasks::doubao_abogus::ABogusTemplate) -> serde_json::Value {
    serde_json::json!({
        "prefix": to_hex(&t.prefix),
        "version": to_hex(&t.version),
        "trailer": to_hex(&t.trailer),
        "prefix50": to_hex(&t.prefix50),
        "device": t.device,
    })
}

/// 字节 → 小写十六进制
fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// 当前毫秒时间戳
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 百分号解码（只用于 msToken / a_bogus 取值；非法转义原样保留）
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

/// 采集失败兜底：best-effort 直写 proxy 日志一行（旁路逻辑，不接入 ProxyLog 投递线程，
/// 避免为一次罕见失败引入额外耦合；写失败静默）。
fn warn(dir: &Path, msg: &str) {
    let logs = dir.join("logs");
    let _ = std::fs::create_dir_all(&logs);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(logs.join("proxy.log")) {
        use std::io::Write;
        let _ = writeln!(f, "[{}] {msg}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::doubao_abogus::{encode, ABogusParts};

    /// 临时目录（测试隔离：不触碰真实 %APPDATA% 数据）
    fn temp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("aiwork_abogus_cap_{}_{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// 合成 query：自造 aid/device_id 等假值（不含任何真机真实值 / 凭据）
    fn synthetic_path() -> String {
        "/samantha/chat/completion?aid=1111&real_aid=2222&device_id=8888888888888888\
         &web_id=7000000000000000000&tea_uuid=abcdef00-0000-0000-0000-000000000000\
         &fp=verify_synthetic&pc_version=3.15.6&doubao_pc_version=3.15.6&version_code=31506\
         &runtime_version=123.0.0&chromium_version=123.0.0&channel=synthetic_channel\
         &client_platform=pc&doubao_device_platform=pc&region=CN&sys_region=CN&language=zh\
         &tz_name=Asia%2FShanghai&pkg_type=1&web_platform=web&samantha_web=1\
         &use-olympus-account=1&device_platform=web&runtime=web\
         &msToken=FAKE%2Ftoken%2Bvalue%3D&a_bogus=PLACEHOLDER"
            .to_string()
    }

    /// 指纹抽取 + msToken 百分号解码：自造假值，且凭据类键不得进入结果
    #[test]
    fn parses_fingerprint_and_decodes_ms_token() {
        let cap = parse(&synthetic_path()).expect("含 query 应可解析");
        assert_eq!(cap.fingerprint.get("aid").map(String::as_str), Some("1111"));
        assert_eq!(cap.fingerprint.get("real_aid").map(String::as_str), Some("2222"));
        assert_eq!(cap.fingerprint.get("device_id").map(String::as_str), Some("8888888888888888"));
        assert_eq!(cap.fingerprint.get("fp").map(String::as_str), Some("verify_synthetic"));
        // 值原样保留（已百分号编码，不二次解码）
        assert_eq!(cap.fingerprint.get("tz_name").map(String::as_str), Some("Asia%2FShanghai"));
        // msToken 百分号解码后的原值
        assert_eq!(cap.ms_token.as_deref(), Some("FAKE/token+value="));
        // a_bogus 取值（此处为占位符，仅验证取值/解码路径）
        assert_eq!(cap.a_bogus_raw.as_deref(), Some("PLACEHOLDER"));
    }

    /// 凭据红线：sessionid/sid_guard/ttwid/cookie/passport_csrf_token 一律不采集不落盘
    #[test]
    fn never_captures_credentials() {
        let path = "/chat/completion?aid=1111&sessionid=SECRET_SID&sid_guard=SECRET_GUARD\
                    &ttwid=SECRET_TTWID&cookie=SECRET_COOKIE&passport_csrf_token=SECRET_CSRF\
                    &msToken=FAKE&a_bogus=PLACEHOLDER";
        let cap = parse(path).expect("应可解析");
        assert_eq!(cap.fingerprint.len(), 1, "只有白名单内 aid 被采集");
        assert!(cap.fingerprint.contains_key("aid"));
        for secret in ["SECRET_SID", "SECRET_GUARD", "SECRET_TTWID", "SECRET_COOKIE", "SECRET_CSRF"] {
            assert!(
                !cap.fingerprint.values().any(|v| v.contains(secret)),
                "凭据 {secret} 不得进入指纹"
            );
        }
        // 落盘后文件里同样不得出现任何凭据明文
        let dir = temp_dir("noperm");
        capture_into(&dir, path).unwrap();
        let text = std::fs::read_to_string(dir.join(CAPTURE_FILE_NAME)).unwrap();
        for secret in ["SECRET_SID", "SECRET_GUARD", "SECRET_TTWID", "SECRET_COOKIE", "SECRET_CSRF"] {
            assert!(!text.contains(secret), "落盘文件含凭据: {secret}");
        }
    }

    /// 幂等「谁新用谁」：新值覆盖同键、未出现的旧键保留、msToken 同规则
    #[test]
    fn merge_is_idempotent_latest_wins() {
        let dir = temp_dir("merge");
        // 第一次：aid=A、device_id=D1、msToken=T1
        capture_into(&dir, "/chat/completion?aid=A&device_id=D1&msToken=T1&a_bogus=PLACEHOLDER").unwrap();
        let v1: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join(CAPTURE_FILE_NAME)).unwrap()).unwrap();
        assert_eq!(v1["fingerprint"]["aid"], "A");
        assert_eq!(v1["fingerprint"]["device_id"], "D1");
        assert_eq!(v1["ms_token"], "T1");

        // 第二次：aid=B 覆盖、device_id 缺席保留、msToken=T2 覆盖
        capture_into(&dir, "/chat/completion?aid=B&msToken=T2&a_bogus=PLACEHOLDER").unwrap();
        let v2: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join(CAPTURE_FILE_NAME)).unwrap()).unwrap();
        assert_eq!(v2["fingerprint"]["aid"], "B", "同键新值覆盖");
        assert_eq!(v2["fingerprint"]["device_id"], "D1", "缺席键保留旧值");
        assert_eq!(v2["ms_token"], "T2");
    }

    /// 端点范围（真机实测回归）：非目标端点（账号 SDK 族，同带 a_bogus）的指纹 / msToken
    /// 不得写入，更不得覆盖已写入的 completion 指纹——此前曾把 aid=497858 / fp=verify_<js-fp>
    /// 覆盖进 completion 指纹，导致网关拿别的端点族的 aid/fp 去签名。
    #[test]
    fn fingerprint_scoped_to_completion_endpoint() {
        let dir = temp_dir("scope");
        // 1) completion 端点写入正确指纹（aid=582478、fp=verify_<device_id>）
        capture_into(
            &dir,
            "/chat/completion?aid=582478&fp=verify_1127431750825948&msToken=GOOD&a_bogus=PLACEHOLDER",
        )
        .unwrap();
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join(CAPTURE_FILE_NAME)).unwrap()).unwrap();
        assert_eq!(v["fingerprint"]["aid"], "582478");
        assert_eq!(v["fingerprint"]["fp"], "verify_1127431750825948");
        assert_eq!(v["ms_token"], "GOOD");

        // 2) 账号 SDK 端点（同带 a_bogus）不得污染
        capture_into(
            &dir,
            "/alice/profile/self?aid=497858&fp=verify_jsfp&msToken=BAD&a_bogus=PLACEHOLDER",
        )
        .unwrap();
        let v2: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join(CAPTURE_FILE_NAME)).unwrap()).unwrap();
        assert_eq!(v2["fingerprint"]["aid"], "582478", "非目标端点不得覆盖 aid");
        assert_eq!(v2["fingerprint"]["fp"], "verify_1127431750825948", "非目标端点不得覆盖 fp");
        assert_eq!(v2["ms_token"], "GOOD", "非目标端点不得覆盖 msToken");

        // 3) 非目标端点即使带 a_bogus 且无任何可采内容，也不落盘（避免噪声文件）
        let dir2 = temp_dir("scope_noise");
        capture_into(&dir2, "/alice/profile/self?aid=497858&fp=verify_jsfp&a_bogus=PLACEHOLDER").unwrap();
        assert!(!dir2.join(CAPTURE_FILE_NAME).exists(), "非目标端点不应落盘");
    }

    /// a_bogus 模板提取：由合成 parts 编码出合法 a_bogus（含 `/`、`-` 需百分号编码往返），
    /// 落盘后 `abogus_template` 三字节十六进制与 device 串均正确
    #[test]
    fn extracts_abogus_template_from_synthetic() {
        // 合成 parts：payload = 定长域 50 + 设备串(长度≡1 mod 3) + 尾部 3 ⇒ 63（3 的倍数）
        let device = "1234567890"; // 10 字符，10 % 3 == 1
        let mut payload = vec![0x41u8; 50];
        payload.extend_from_slice(device.as_bytes());
        payload.extend_from_slice(b"123");
        assert_eq!(payload.len(), 63);
        let parts = ABogusParts {
            prefix: [0x01, 0x02, 0x03, 0x04],
            version: [0x10, 0x20, 0x30, 0x40, 0x50, 0x60, 0x70, 0x80],
            payload,
            trailer: vec![],
        };
        let a_bogus = encode(&parts);
        // 真机 a_bogus 走 query 时是百分号编码形态；此处按 URL 安全集编码还原
        let encoded_a_bogus = percent_encode(&a_bogus);
        let path = format!("/chat/completion?aid=1111&msToken=FAKE&a_bogus={encoded_a_bogus}");

        let dir = temp_dir("template");
        capture_into(&dir, &path).unwrap();
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join(CAPTURE_FILE_NAME)).unwrap()).unwrap();
        let tpl = &v["abogus_template"];
        assert_eq!(tpl["prefix"], "01020304");
        assert_eq!(tpl["version"], "1020304050607080");
        assert_eq!(tpl["trailer"], "");
        assert_eq!(tpl["prefix50"].as_str().unwrap().len(), 100, "50 字节 → 100 hex 字符");
        assert_eq!(tpl["device"], device);
    }

    /// 降级：a_bogus 非法/不可解码时**不得 panic**，跳过模板（保留历史值/不写入）
    #[test]
    fn skips_template_when_undecodable() {
        let bad_path = "/chat/completion?aid=1111&msToken=FAKE&a_bogus=%21%21%21%21";
        let dir = temp_dir("bad_abogus");
        capture_into(&dir, bad_path).unwrap();
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join(CAPTURE_FILE_NAME)).unwrap()).unwrap();
        assert!(v.get("abogus_template").is_none(), "非法 a_bogus 不得写入模板");
        assert_eq!(v["fingerprint"]["aid"], "1111", "其余非凭据字段仍正常采集");
    }

    /// path 无 query：解析返回 None，采集返回 Err（公开入口会吞掉）且不落盘、不 panic
    #[test]
    fn tolerates_path_without_query() {
        assert!(parse("/chat/completion").is_none());
        let dir = temp_dir("noquery");
        // 内部函数对无 query 返回 Err（公开入口 catch 后写 warning）；此处验证不落盘
        assert!(capture_into(&dir, "/chat/completion").is_err());
        assert!(!dir.join(CAPTURE_FILE_NAME).exists(), "无 query 不应落盘");
    }

    /// 与 handler 同款的百分号编码（仅供测试构造 URL 形态的 a_bogus）
    fn percent_encode(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        for b in s.bytes() {
            match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
                _ => out.push_str(&format!("%{b:02X}")),
            }
        }
        out
    }
}
