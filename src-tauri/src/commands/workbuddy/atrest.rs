//! WorkBuddy 5.8.2+ 凭证静态加密（at-rest）兼容解密。
//!
//! # 背景
//!
//! 5.8.2 起客户端对 auth 文件中的敏感字段（accessToken / refreshToken /
//! nickname / phoneNumber 等）启用静态加密，明文值变为包装对象：
//! `{"$wbEncrypted":1,"envelope":"<标准 base64(JSON 信封)>"}`，
//! 旧解析逻辑命中 object 即误报「未登录」。
//!
//! # 方案（与客户端 app.asar 内 at-rest 模块逐行对齐）
//!
//! - 算法：AES-256-GCM（nonce 12B，authTag 16B，tag 追加在密文尾部）。
//! - 内置 build key（随客户端分发的静态对称值，非用户凭证）：
//!   `protector = SHA256(atRestSecretKey 的 base64 字符串的 UTF-8 字节)`；
//!   `protectorKeyId = SHA256(protector)` 小写 hex 前 16 位。
//! - 主密钥 DEK：`<wb_data_dir>/keyblob`（version 1，slots 中 `static-v1` 槽位），
//!   `wrapped` 是用 protector 以 **file framing** 加密的信封；解出 32B DEK 后校验
//!   `SHA256(DEK)` 前 16 位 == keyblob.keyId（本机实测 `1d65767e3275b1a8`）。
//! - 字段信封：`{suite:1,keyId,nonce,authTag,ciphertext}`（后三者标准 base64），
//!   外层 envelope 字符串为该 JSON 的标准 base64。**实测 auth 字段信封的 keyId 为
//!   `9127dea1b44020a7`（= protector），即字段直接用 protector 以 field framing 加解密**；
//!   keyblob 的 DEK（`1d65...`）用于其它加密域。故解密统一「按信封 keyId 在本机候选
//!   密钥集合（protector / DEK）中解析」，而非写死某一把。
//! - sym-v1 的 AAD（54B）：
//!   `"WB-AAD\0"` || `0x01`
//!   || u32be(len) || framing magic（file=`WBEF1`，field=`WBEV1`）
//!   || u32be(6) || `"sym-v1"`
//!   || u32be(suite)
//!   || u32be(16) || keyId（16 位小写 hex）
//!   || framing 字节（file=1，field=2）
//!   || `0x00`（sequence 缺失）|| `0x00`（final 未定义）。
//!
//! # 安全红线
//!
//! 明文只返回内存，绝不写日志、绝不进入错误信息；错误信息只描述结构与失败类别。

use super::common::wb_data_dir;
use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::Path;
use std::sync::OnceLock;

/// 客户端内置 build key 的对称部分（native binding `loggerGet()` 返回的
/// atRestSecretKey：44 字符标准 base64 表示 32 字节）。随安装包分发的公开静态值，
/// 用于解包本机 keyblob；不是任何用户的登录凭证。
const AT_REST_BUILD_KEY_B64: &str = "Sik9U5aXhCdwTVEwsEySDOmDoB9r9ntFxHF1fst9LQI=";

const AAD_MAGIC: &[u8] = b"WB-AAD\0";
const FRAMING_MAGIC_FILE: &[u8] = b"WBEF1";
const FRAMING_MAGIC_FIELD: &[u8] = b"WBEV1";
const SCHEME_SYM_V1: &[u8] = b"sym-v1";
const FRAME_BYTE_FILE: u8 = 1;
const FRAME_BYTE_FIELD: u8 = 2;
const KEYBLOB_MAX: usize = 65_536;
const KEY_LEN: usize = 32;
const NONCE_LEN: usize = 12;
const TAG_LEN: usize = 16;
const SUITE_AES256GCM: u32 = 1;

/// 信封的认证上下文场景。
#[derive(Clone, Copy, PartialEq, Eq)]
enum Framing {
    /// 整体文件/密钥包装（keyblob 的 static-v1 槽位）。
    File,
    /// 单字段（auth 文件中的 $wbEncrypted 包装）。
    Field,
}

impl Framing {
    fn magic(self) -> &'static [u8] {
        match self {
            Framing::File => FRAMING_MAGIC_FILE,
            Framing::Field => FRAMING_MAGIC_FIELD,
        }
    }

    fn byte(self) -> u8 {
        match self {
            Framing::File => FRAME_BYTE_FILE,
            Framing::Field => FRAME_BYTE_FIELD,
        }
    }
}

fn sha256_bytes(data: &[u8]) -> [u8; KEY_LEN] {
    let mut h = Sha256::new();
    h.update(data);
    let digest = h.finalize();
    let mut out = [0u8; KEY_LEN];
    out.copy_from_slice(&digest);
    out
}

fn to_hex(data: &[u8]) -> String {
    let mut s = String::with_capacity(data.len() * 2);
    for b in data {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// 客户端 keyId 规则：SHA256(32B 密钥) 小写 hex 前 16 位。
fn key_id_of(key: &[u8; KEY_LEN]) -> String {
    to_hex(&sha256_bytes(key))[..16].to_string()
}

/// protector = SHA256(build key 的 base64 字符串本身的 UTF-8 字节)——
/// 注意是哈希「base64 字符串」，不是其解码后的 32 字节。
fn protector() -> [u8; KEY_LEN] {
    sha256_bytes(AT_REST_BUILD_KEY_B64.as_bytes())
}

/// keyId 形态校验：恰好 16 位小写十六进制。
fn is_key_id(s: &str) -> bool {
    s.len() == 16 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// 宽容 base64 解码：对齐 Node `Buffer.from(s, 'base64')` 的行为——
/// 同时接受标准/URL-safe 字符表、缺填充、夹杂空白/换行。
fn b64_decode_lenient(s: &str) -> Result<Vec<u8>, String> {
    let mut normalized = s.trim().replace('-', "+").replace('_', "/");
    normalized.retain(|c| matches!(c, 'A'..='Z' | 'a'..='z' | '0'..='9' | '+' | '/' | '='));
    while normalized.len() % 4 != 0 {
        normalized.push('=');
    }
    if normalized.is_empty() {
        return Err("base64 内容为空".into());
    }
    STANDARD
        .decode(normalized.as_bytes())
        .map_err(|e| format!("base64 解码失败: {e}"))
}

/// u32 大端长度前缀拼接（= Node encodeLengthPrefixed）。
fn push_length_prefixed(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    out.extend_from_slice(bytes);
}

/// 构造 sym-v1 认证 AAD（结构见模块文档）。
fn build_aad(key_id: &str, suite: u32, framing: Framing) -> Vec<u8> {
    let mut aad = Vec::with_capacity(64);
    aad.extend_from_slice(AAD_MAGIC);
    aad.push(0x01);
    push_length_prefixed(&mut aad, framing.magic());
    push_length_prefixed(&mut aad, SCHEME_SYM_V1);
    aad.extend_from_slice(&suite.to_be_bytes());
    push_length_prefixed(&mut aad, key_id.as_bytes());
    aad.push(framing.byte());
    // sym-v1 file/field 不允许携带 sequence/final：各仅一个「缺失」字节
    aad.push(0x00);
    aad.push(0x00);
    aad
}

/// 解析后的对称信封。
struct Envelope {
    suite: u32,
    key_id: String,
    nonce: Vec<u8>,
    auth_tag: Vec<u8>,
    ciphertext: Vec<u8>,
}

fn parse_envelope(b64: &str) -> Result<Envelope, String> {
    let json_bytes = b64_decode_lenient(b64)?;
    let v = serde_json::from_slice::<Value>(&json_bytes)
        .map_err(|e| format!("信封不是合法 JSON: {e}"))?;
    let suite = v
        .get("suite")
        .and_then(|x| x.as_u64())
        .ok_or_else(|| "信封缺少 suite 字段".to_string())? as u32;
    if suite != SUITE_AES256GCM {
        return Err(format!("不支持的加密套件: {suite}"));
    }
    let key_id = v
        .get("keyId")
        .and_then(|x| x.as_str())
        .filter(|s| is_key_id(s))
        .ok_or_else(|| "信封 keyId 缺失或形态非法（需 16 位小写 hex）".to_string())?
        .to_string();
    let nonce = b64_decode_lenient(v.get("nonce").and_then(|x| x.as_str()).unwrap_or(""))
        .map_err(|e| format!("信封 nonce {e}"))?;
    if nonce.len() != NONCE_LEN {
        return Err(format!("信封 nonce 长度必须为 {NONCE_LEN} 字节"));
    }
    let auth_tag = b64_decode_lenient(v.get("authTag").and_then(|x| x.as_str()).unwrap_or(""))
        .map_err(|e| format!("信封 authTag {e}"))?;
    if auth_tag.len() != TAG_LEN {
        return Err(format!("信封 authTag 长度必须为 {TAG_LEN} 字节"));
    }
    // ciphertext 允许为空串（空明文）
    let ciphertext = match v.get("ciphertext").and_then(|x| x.as_str()) {
        Some(s) if !s.is_empty() => b64_decode_lenient(s).map_err(|e| format!("信封 ciphertext {e}"))?,
        _ => Vec::new(),
    };
    Ok(Envelope { suite, key_id, nonce, auth_tag, ciphertext })
}

/// 打开对称信封：AES-256-GCM 验签解密（GCM tag 拼接在密文尾部，对齐 Node crypto）。
fn open_envelope(b64: &str, key: &[u8; KEY_LEN], framing: Framing) -> Result<Vec<u8>, String> {
    let env = parse_envelope(b64)?;
    if env.key_id != key_id_of(key) {
        return Err("信封密钥 ID 与当前密钥不匹配".into());
    }
    let cipher = Aes256Gcm::new_from_slice(Key::<Aes256Gcm>::from_slice(key))
        .map_err(|e| format!("初始化 AES-256-GCM 失败: {e}"))?;
    let mut sealed = env.ciphertext;
    sealed.extend_from_slice(&env.auth_tag);
    let aad = build_aad(&env.key_id, env.suite, framing);
    cipher
        .decrypt(
            Nonce::from_slice(&env.nonce),
            Payload { msg: &sealed, aad: &aad },
        )
        .map_err(|_| "信封认证失败（密钥不匹配或密文已损坏）".to_string())
}

/// 从指定目录的 `keyblob` 解出主密钥 DEK（不做进程级缓存，便于测试注入）。
fn load_dek_from_dir(dir: &Path) -> Result<[u8; KEY_LEN], String> {
    let path = dir.join("keyblob");
    let bytes = std::fs::read(&path)
        .map_err(|_| format!("未找到本地密钥包装文件（{}）", path.display()))?;
    if bytes.is_empty() || bytes.len() > KEYBLOB_MAX {
        return Err("本地密钥包装文件大小非法（空或超过 64KiB）".into());
    }
    let v = serde_json::from_slice::<Value>(&bytes)
        .map_err(|_| "本地密钥包装文件 JSON 解析失败".to_string())?;
    if v.get("version").and_then(|x| x.as_i64()) != Some(1) {
        return Err("本地密钥包装版本不受支持".into());
    }
    let blob_key_id = v
        .get("keyId")
        .and_then(|x| x.as_str())
        .filter(|s| is_key_id(s))
        .ok_or_else(|| "本地密钥包装缺少合法 keyId".to_string())?;
    let slots = v
        .get("slots")
        .and_then(|x| x.as_array())
        .ok_or_else(|| "本地密钥包装缺少 slots 数组".to_string())?;

    let protector = protector();
    let protector_id = key_id_of(&protector);
    let mut last_err = "没有可匹配当前 build key 的 static-v1 槽位".to_string();
    for slot in slots {
        if slot.get("type").and_then(|x| x.as_str()) != Some("static-v1") {
            continue;
        }
        // protectorKeyId 缺省放行（客户端 unwrapStaticV1Slot 同语义），存在则必须吻合
        if let Some(pid) = slot.get("protectorKeyId").and_then(|x| x.as_str()) {
            if pid != protector_id {
                continue;
            }
        }
        let Some(wrapped) = slot.get("wrapped").and_then(|x| x.as_str()) else {
            continue;
        };
        match open_envelope(wrapped, &protector, Framing::File) {
            Ok(plaintext) => {
                if plaintext.len() != KEY_LEN {
                    last_err = "解包主密钥长度不是 32 字节".into();
                    continue;
                }
                let mut dek = [0u8; KEY_LEN];
                dek.copy_from_slice(&plaintext);
                if key_id_of(&dek) != blob_key_id {
                    last_err = "解包主密钥 ID 与 keyblob 声明不一致".into();
                    continue;
                }
                return Ok(dek);
            }
            Err(e) => last_err = e,
        }
    }
    Err(format!("主密钥解包失败：{last_err}"))
}

/// 进程级 DEK 缓存（keyblob 在一次安装内不变；只缓存成功结果）。
static MASTER_DEK: OnceLock<[u8; KEY_LEN]> = OnceLock::new();

/// 取本机主密钥 DEK：首次调用时从 `<wb_data_dir>/keyblob` 解包。
pub(crate) fn master_dek() -> Result<&'static [u8; KEY_LEN], String> {
    if let Some(k) = MASTER_DEK.get() {
        return Ok(k);
    }
    let dek = load_dek_from_dir(&wb_data_dir())?;
    Ok(MASTER_DEK.get_or_init(|| dek))
}

/// 本机已知的对称密钥集合，按信封 `keyId` 解析：
/// - `protector`：由内置 build key 直接派生（keyId `9127...`）——**auth 字段信封即使用它**，
///   无磁盘依赖；
/// - `DEK`：本机 `keyblob` 中用 protector 解出的主密钥（keyId `1d65...`），供其它加密域
///   使用；keyblob 缺失/不可解时跳过，不影响字段解密。
fn candidate_keys() -> Vec<([u8; KEY_LEN], String)> {
    let prot = protector();
    let mut keys = vec![(prot, key_id_of(&prot))];
    if let Ok(dek) = master_dek() {
        let id = key_id_of(dek);
        if keys.iter().all(|(_, existing)| *existing != id) {
            keys.push((*dek, id));
        }
    }
    keys
}

/// 按信封自带 keyId 从候选密钥集合中选定密钥后打开信封。
/// keyId 不匹配任何候选 → 明确报错（客户端可能更新了内置 build key）。
fn open_envelope_auto(b64: &str, framing: Framing) -> Result<Vec<u8>, String> {
    let env = parse_envelope(b64)?;
    for (key, id) in candidate_keys() {
        if id == env.key_id {
            return open_envelope(b64, &key, framing);
        }
    }
    Err(format!(
        "信封密钥 ID（{}）不在本机已知密钥集合中（客户端可能已更换内置密钥，请升级大肥鱼助手）",
        env.key_id
    ))
}

/// 识别并解开 auth 文件字段包装：
///
/// - 值不是 `$wbEncrypted` 包装 → `Ok(None)`（调用方按普通字符串/类型处理）；
/// - 标准对称包装 `{$wbEncrypted:1,envelope}` → `Ok(Some(明文))`；
/// - asym-v1 高级加密 / 结构异常 / 本地解密失败 → `Err(明确原因)`。
///
/// 注意：调用方不应把这里的 Err 误报成「未登录」——包装存在本身就说明客户端
/// 处于加密登录态，失败多为 keyblob 缺失/客户端版本更新/文件损坏。
pub(crate) fn decrypt_if_wrapped(v: &Value) -> Result<Option<String>, String> {
    let Some(obj) = v.as_object() else {
        return Ok(None);
    };
    if obj.get("$wbEncrypted").and_then(|m| m.as_i64()) != Some(1) {
        return Ok(None);
    }
    // 高级（asym-v1）包装多一个 scheme 字段，当前版本不支持（本机数据未使用）。
    if let Some(scheme) = obj.get("scheme").and_then(|s| s.as_str()) {
        if scheme == "asym-v1" {
            return Err("检测到 WorkBuddy 高级加密（asym-v1）凭证，当前版本暂不支持解密".into());
        }
        return Err(format!("检测到使用不受支持加密方案（{scheme}）的 WorkBuddy 凭证"));
    }
    let envelope = obj
        .get("envelope")
        .and_then(|e| e.as_str())
        .ok_or_else(|| "加密包装缺少 envelope 字段".to_string())?;
    // 按信封 keyId 在本机候选密钥（protector/DEK）中解析后解密
    let plaintext = open_envelope_auto(envelope, Framing::Field)?;
    String::from_utf8(plaintext)
        .map_err(|_| "解密结果不是合法 UTF-8 文本".to_string())
        .map(Some)
}

// ── 单元测试（全部合成数据，禁止写入/断言任何真实凭证）────────────────────

/// 测试用密封：与客户端 seal() 同构造（AAD + ct||tag），输出外层标准 base64 信封。
#[cfg(test)]
fn seal_envelope_b64(plaintext: &[u8], key: &[u8; KEY_LEN], framing: Framing) -> String {
    use base64::Engine as _;
    let nonce = b"twa-nonce-12"; // 恰好 12B；测试专用，密钥为合成派生值
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let key_id = key_id_of(key);
    let aad = build_aad(&key_id, SUITE_AES256GCM, framing);
    let mut sealed = cipher
        .encrypt(Nonce::from_slice(nonce), Payload { msg: plaintext, aad: &aad })
        .expect("test seal");
    let tag = sealed.split_off(sealed.len() - TAG_LEN);
    let env = serde_json::json!({
        "suite": SUITE_AES256GCM,
        "keyId": key_id,
        "nonce": STANDARD.encode(nonce),
        "authTag": STANDARD.encode(tag),
        "ciphertext": STANDARD.encode(&sealed),
    });
    STANDARD.encode(serde_json::to_vec(&env).unwrap())
}

/// 测试专用：安装全 crate 共用的合成 DEK（MASTER_DEK 为全局 OnceLock，先装者胜，
/// 故所有用例必须使用同一把，见 tests::test_dek）。
#[cfg(test)]
fn install_test_dek() {
    let _ = MASTER_DEK.set(sha256_bytes(b"twa-atrest-test-dek-v1"));
}

/// 测试专用：以合成 DEK 为明文字段构造 `{$wbEncrypted:1,envelope}` 包装值。
/// 供 atrest 自身及 accounts 等兄弟模块的单测复用，保证跨模块 DEK 一致。
#[cfg(test)]
pub(crate) fn wrapped_field_value(plaintext: &str) -> Value {
    install_test_dek();
    let dek = sha256_bytes(b"twa-atrest-test-dek-v1");
    serde_json::json!({
        "$wbEncrypted": 1,
        "envelope": seal_envelope_b64(plaintext.as_bytes(), &dek, Framing::Field),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 全测试共用同一把合成 DEK（MASTER_DEK 全局 OnceLock，先装者胜，故值必须统一）。
    fn test_dek() -> [u8; KEY_LEN] {
        sha256_bytes(b"twa-atrest-test-dek-v1")
    }

    #[test]
    fn build_key_protector_id_matches_real_keyblob() {
        // 与真机 C:\Users\<u>\.workbuddy\keyblob 的 static-v1 protectorKeyId 一致；
        // 若客户端更换 build key，此用例最先暴露（值不是用户秘密，是两个 SHA256 前缀）。
        assert_eq!(key_id_of(&protector()), "9127dea1b44020a7");
    }

    #[test]
    fn aad_has_exact_sym_v1_layout() {
        let kid = "0123456789abcdef";
        let field = build_aad(kid, 1, Framing::Field);
        let file = build_aad(kid, 1, Framing::File);
        // 7 + 1 + (4+5) + (4+6) + 4 + (4+16) + 1 + 1 + 1 = 54
        assert_eq!(field.len(), 54);
        assert_eq!(file.len(), 54);
        assert!(field.starts_with(b"WB-AAD\0\x01"));
        // 偏移 8: u32be(5)；framing magic 位于偏移 12
        assert_eq!(&field[8..12], &5u32.to_be_bytes());
        assert_eq!(&field[12..17], b"WBEV1");
        assert_eq!(&file[12..17], b"WBEF1");
        // 偏移 17: u32be(6) + "sym-v1"
        assert_eq!(&field[17..21], &6u32.to_be_bytes());
        assert_eq!(&field[21..27], b"sym-v1");
        // 偏移 27: u32be(suite=1)；31: u32be(16) + keyId
        assert_eq!(&field[27..31], &1u32.to_be_bytes());
        assert_eq!(&field[31..35], &16u32.to_be_bytes());
        assert_eq!(&field[35..51], kid.as_bytes());
        // framing 字节（偏移 51）
        assert_eq!(field[51], FRAME_BYTE_FIELD);
        assert_eq!(file[51], FRAME_BYTE_FILE);
        // sequence 缺失 + final 未定义（末尾两字节）
        assert_eq!(&field[52..54], &[0, 0]);
        // 两种 framing 仅 magic 第 4 字节（F/V，偏移 15）与 framing 字节（51）不同
        let diff: Vec<usize> = (0..54).filter(|i| field[*i] != file[*i]).collect();
        assert_eq!(diff, vec![15, 51]);
    }

    #[test]
    fn field_envelope_seal_open_roundtrip() {
        let dek = test_dek();
        let msg = "access-token-synthetic-明文字段-!";
        let b64 = seal_envelope_b64(msg.as_bytes(), &dek, Framing::Field);
        let out = open_envelope(&b64, &dek, Framing::Field).unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), msg);
    }

    #[test]
    fn field_envelope_file_framing_must_not_open() {
        // file 与 field 的 AAD 不同：跨 framing 必须验签失败
        let dek = test_dek();
        let b64 = seal_envelope_b64(b"x", &dek, Framing::File);
        assert!(open_envelope(&b64, &dek, Framing::Field).is_err());
    }

    #[test]
    fn wrapper_plaintext_extraction() {
        install_test_dek();
        let w = wrapped_field_value("tok-synthetic-123");
        assert_eq!(decrypt_if_wrapped(&w).unwrap().as_deref(), Some("tok-synthetic-123"));
        // 普通字符串 / null / 数字 / 空对象都不是包装
        assert_eq!(decrypt_if_wrapped(&json!("plain")).unwrap(), None);
        assert_eq!(decrypt_if_wrapped(&Value::Null).unwrap(), None);
        assert_eq!(decrypt_if_wrapped(&json!(42)).unwrap(), None);
        assert_eq!(decrypt_if_wrapped(&json!({})).unwrap(), None);
    }

    #[test]
    fn wrapper_asym_scheme_rejected_with_clear_error() {
        let w = json!({ "$wbEncrypted": 1, "scheme": "asym-v1", "envelope": "AAAA" });
        let err = decrypt_if_wrapped(&w).unwrap_err();
        assert!(err.contains("asym-v1"), "{err}");
        let w2 = json!({ "$wbEncrypted": 1, "scheme": "future-v9", "envelope": "AAAA" });
        assert!(decrypt_if_wrapped(&w2).unwrap_err().contains("future-v9"));
    }

    #[test]
    fn wrapper_malformed_reports_error() {
        install_test_dek();
        let w = json!({ "$wbEncrypted": 1 });
        assert!(decrypt_if_wrapped(&w).unwrap_err().contains("envelope"));
    }

    #[test]
    fn tampered_envelope_fails_auth() {
        install_test_dek();
        let mut w = wrapped_field_value("tok-synthetic-secret");
        // 翻转 envelope 中密文区段的一个字符（仅改密文，不触碰包装结构）
        let env = w["envelope"].as_str().unwrap().to_string();
        let bytes = STANDARD.decode(&env).unwrap();
        let mut corrupted = bytes.clone();
        let idx = corrupted.len() - 3; // 落在 ciphertext 内
        corrupted[idx] ^= 0x01;
        w["envelope"] = json!(STANDARD.encode(corrupted));
        assert_ne!(env, w["envelope"].as_str().unwrap());
        assert!(decrypt_if_wrapped(&w).is_err());
    }

    #[test]
    fn wrong_key_fails_auth() {
        let dek = test_dek();
        let other = sha256_bytes(b"totally-different-dek");
        let b64 = seal_envelope_b64(b"plaintext", &dek, Framing::Field);
        // keyId 不匹配直接拒绝（在 GCM 之前）
        assert!(open_envelope(&b64, &other, Framing::Field).is_err());
    }

    #[test]
    fn urlsafe_envelope_also_decodes() {
        // 外层 envelope 客户端用标准 base64；解码保持宽容，URL-safe/无填充也接受
        let dek = test_dek();
        let b64std = seal_envelope_b64(b"urlsafe", &dek, Framing::Field);
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let raw = STANDARD.decode(&b64std).unwrap();
        let b64url = URL_SAFE_NO_PAD.encode(raw);
        assert!(b64url.contains('-') || !b64url.contains('='));
        let out = open_envelope(&b64url, &dek, Framing::Field).unwrap();
        assert_eq!(out, b"urlsafe");
    }

    #[test]
    fn keyblob_static_v1_slot_unwraps_synthetic() {
        let dir = std::env::temp_dir().join(format!(
            "twa_atrest_keyblob_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let dek = sha256_bytes(b"synthetic-master-dek");
        let prot = protector();
        let wrapped = seal_envelope_b64(&dek, &prot, Framing::File);
        let blob = json!({
            "version": 1,
            "keyId": key_id_of(&dek),
            "slots": [{
                "type": "static-v1",
                "protectorKeyId": key_id_of(&prot),
                "wrapped": wrapped,
            }],
        });
        std::fs::write(dir.join("keyblob"), serde_json::to_vec_pretty(&blob).unwrap()).unwrap();

        let got = load_dek_from_dir(&dir).unwrap();
        assert_eq!(got, dek);

        // 篡改密文后必须失败
        let mut bad = blob.clone();
        bad["slots"][0]["wrapped"] = json!(STANDARD.encode(b"not-an-envelope-json"));
        std::fs::write(dir.join("keyblob"), serde_json::to_vec(&bad).unwrap()).unwrap();
        assert!(load_dek_from_dir(&dir).is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn keyblob_missing_and_garbage_report_errors() {
        let dir = std::env::temp_dir().join(format!(
            "twa_atrest_missing_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let err = load_dek_from_dir(&dir).unwrap_err();
        assert!(err.contains("未找到本地密钥包装文件"), "{err}");
        std::fs::write(dir.join("keyblob"), b"{not json").unwrap();
        assert!(load_dek_from_dir(&dir).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 真机 keyblob 端到端校验（存在才跑；文件不可读/非本机环境自动跳过）。
    /// 这是对 file-framing AAD 与整条派生链唯一的真实数据验证：能解包即逐字节正确。
    #[test]
    fn real_machine_keyblob_unwraps_when_present() {
        let path = wb_data_dir().join("keyblob");
        let Ok(bytes) = std::fs::read(&path) else {
            eprintln!("skip: 真机 keyblob 不可读（{path:?}）");
            return;
        };
        if bytes.is_empty() {
            return;
        }
        let dir = path.parent().unwrap().to_path_buf();
        let dek = load_dek_from_dir(&dir).expect("真机 keyblob 必须能按内置 build key 解包");
        // 解出后 ID 必然吻合文件声明（load 内部已校验），此处再确认是 16 位 hex 形态
        assert!(is_key_id(&key_id_of(&dek)));
    }

    /// 字段信封由 **protector** 加密（keyId = protectorKeyId）：自动解析后必须解开。
    /// 与 wrapped_field_value（用 DEK 加密）互补，覆盖两把候选密钥。
    #[test]
    fn field_envelope_with_protector_key_resolves() {
        install_test_dek();
        let prot = protector();
        let b64 = seal_envelope_b64("tok-protector-合成".as_bytes(), &prot, Framing::Field);
        let w = json!({ "$wbEncrypted": 1, "envelope": b64 });
        assert_eq!(decrypt_if_wrapped(&w).unwrap().as_deref(), Some("tok-protector-合成"));
    }

    /// 信封 keyId 不属于任何候选密钥时，错误必须带上该 keyId（便于真机定位）。
    #[test]
    fn unknown_key_id_reports_id_in_error() {
        let orphan = sha256_bytes(b"orphan-key-not-in-store");
        let b64 = seal_envelope_b64(b"x", &orphan, Framing::Field);
        let w = json!({ "$wbEncrypted": 1, "envelope": b64 });
        let err = decrypt_if_wrapped(&w).unwrap_err();
        assert!(err.contains(&key_id_of(&orphan)), "{err}");
    }

    /// 真机 auth 文件端到端：存在加密包装时，accessToken 必须能被自动解析密钥解开
    /// （不存在/明文登录态自动跳过；只断言结果非空，绝不打印任何明文内容）。
    #[test]
    fn real_machine_auth_field_decrypts_when_present() {
        let Some(local) = std::env::var_os("LOCALAPPDATA") else { return };
        let path = std::path::PathBuf::from(local)
            .join("CodeBuddyExtension/Data/Public/auth/workbuddy-desktop.info");
        let Ok(bytes) = std::fs::read(&path) else { return };
        let Ok(v) = serde_json::from_slice::<Value>(&bytes) else { return };
        let Some(w) = v.get("auth").and_then(|a| a.get("accessToken")) else { return };
        if w.get("$wbEncrypted").is_none() {
            return; // 明文登录态，无需解密
        }
        let plain = decrypt_if_wrapped(w).expect("真机 auth.accessToken 字段信封必须可解密");
        assert!(plain.map(|s| !s.is_empty()).unwrap_or(false), "解密结果非空");
    }
}
