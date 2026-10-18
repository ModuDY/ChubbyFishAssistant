//! WorkBuddy 公共请求层（原 src-python/wb_common.py 的 Rust 移植）。
//! 依据 docs/workbuddy-product-design.md §5.1~§5.4：
//! - 凭证双源化（F-10）：工具侧 token_store 与桌面 auth 文件「谁新用谁」（expiresAtMs 晚者胜出）
//! - 统一请求头（§5.3）+ 宽容解析 dig（复用 fs_utils::dig 信封下钻语义，对齐 wb.dig）
//! - 红线：chat 请求绝不携带 X-Refresh-Token；仅 refresh 端点携带
//! - 网络：ureq 默认直连（不读系统/环境代理），对齐 python OPENER 绕代理约定
//!
//! P1 先行落地（wb_credits 依赖）；P2 wb_checkin / trae_checkin 直接复用本层。

use std::path::PathBuf;

use serde::Serialize;
use serde_json::Value;

use crate::fs_utils;
use crate::state::AppState;

// ── 路径 ────────────────────────────────────────────────────────────────────
// （SQLite 化 P3：pool/token store/checkin results 均改走 store，文件路径函数已删除）

/// 桌面 auth 文件读取路径：settings.wb_auth_file_path 人工指定优先（与
/// commands/workbuddy/common.rs auth_file_path_of 同语义），否则默认布局。
fn auth_file_path(state: &AppState) -> PathBuf {
    if let Some(p) = state.settings().wb_auth_file_path.as_deref() {
        let p = p.trim();
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    let local = std::env::var("LOCALAPPDATA").unwrap_or_default();
    PathBuf::from(local)
        .join("CodeBuddyExtension")
        .join("Data")
        .join("Public")
        .join("auth")
        .join("workbuddy-desktop.info")
}

// ── 凭证结构（对齐 python creds_of 返回 dict）───────────────────────────────

#[derive(Serialize, Clone, Default, Debug)]
pub struct Creds {
    #[serde(default)]
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: String,
    #[serde(default)]
    pub expires_at_ms: Option<i64>,
    #[serde(default)]
    pub refresh_expires_at_ms: Option<i64>,
    #[serde(default)]
    pub uid: String,
    #[serde(default)]
    pub domain: String,
    #[serde(default)]
    pub nickname: String,
    #[serde(default)]
    pub edition: String,
    /// 企业标识（auth 文件 `account.enterpriseId`；个人号为空）。
    /// 非空即企业号 —— 企业号无个人成长体系，签到/成长任务等须整体跳过
    #[serde(default)]
    pub enterprise_id: String,
    /// 客户端 account 段的企业标志（不同版本命名不一，任一为真即企业号）：
    /// `isCurrentOneIdEnterprise` / `isCurrentOneIdPersonal`(取反) / `accountType`
    #[serde(default)]
    pub enterprise_flag: bool,
}

/// 字符串字段提取：兼容 WorkBuddy 5.8.2+ 静态加密包装
/// （auth 文件中 accessToken/refreshToken/nickname 为 `{$wbEncrypted:1,envelope}`）；
/// token store 记录由本工具写入、恒为明文字符串，走 as_str 原样返回。
/// 解密失败回退空串（与历史「字段缺失」同语义，上层按 no_credential 处理，fail-secure）。
fn s_of(v: Option<&Value>) -> String {
    let Some(v) = v else {
        return String::new();
    };
    if let Ok(Some(plain)) = crate::commands::workbuddy::atrest::decrypt_if_wrapped(v) {
        return plain;
    }
    v.as_str().unwrap_or("").to_string()
}

/// 宽容整数：数字或数字字符串（对齐 python int(v) 兜底）
fn i_of(v: Option<&Value>) -> Option<i64> {
    let v = v?;
    v.as_i64().or_else(|| v.as_str()?.trim().parse::<i64>().ok())
}

/// 从 auth 文件 / token store 记录中提取凭证字段（兼容多种嵌套形态，F-04）。
pub fn creds_of(source: &Value) -> Creds {
    let auth = source.get("auth").filter(|v| v.is_object()).unwrap_or(source);
    let account = source
        .get("account")
        .filter(|v| v.is_object())
        .unwrap_or(source);
    Creds {
        access_token: s_of(fs_utils::dig(auth, &["access_token", "accessToken", "token"])),
        refresh_token: s_of(fs_utils::dig(auth, &["refresh_token", "refreshToken"])),
        expires_at_ms: i_of(fs_utils::dig(
            auth,
            &[
                "expires_at_ms",
                "expiresAtMs",
                "expiresAt",
                "expires_in_ms",
                "accessTokenExpiresAtMs",
            ],
        )),
        refresh_expires_at_ms: i_of(fs_utils::dig(
            auth,
            &["refresh_expires_at_ms", "refreshExpiresAtMs", "refreshExpiresAt"],
        )),
        uid: s_of(fs_utils::dig(account, &["uid", "userId", "user_id", "id"])),
        domain: s_of(fs_utils::dig(source, &["domain"])),
        nickname: s_of(fs_utils::dig(account, &["nickname", "name", "displayName"])),
        edition: s_of(fs_utils::dig(account, &["editionType", "edition_type", "edition"])),
        enterprise_id: s_of(fs_utils::dig(
            account,
            &["enterpriseId", "enterprise_id", "entId"],
        )),
        enterprise_flag: enterprise_flag_of(account),
    }
}

/// 客户端 account 段的企业标志识别（各版本字段命名不一，逐项判定）：
/// - `isCurrentOneIdEnterprise == true` → 企业
/// - `accountType` 含 enterprise/corp/company → 企业
///
/// **不能**用「`isCurrentOneIdPersonal == false` → 企业」：实测个人账号的
/// auth 文件里 enterprise 与 personal 两个标志**同时为 false**（均未置位），
/// 按此判据会把所有个人号误判成企业号、整体跳过签到与成长任务。
///
/// 这些字段来自客户端本地文件（非凭据），只作「是否企业号」判定，不外传。
fn enterprise_flag_of(account: &Value) -> bool {
    let truthy = |keys: &[&str]| -> bool {
        fs_utils::dig(account, keys).is_some_and(|v| match v {
            Value::Bool(b) => *b,
            Value::String(s) => !matches!(
                s.to_ascii_lowercase().as_str(),
                "" | "false" | "0" | "no"
            ),
            Value::Number(n) => n.as_i64().unwrap_or(0) != 0,
            _ => false,
        })
    };
    if truthy(&["enterprise_flag", "isCurrentOneIdEnterprise", "isEnterprise"]) {
        return true;
    }
    fs_utils::dig(account, &["accountType", "account_type"])
        .map(|v| s_of(Some(v)).to_ascii_lowercase())
        .is_some_and(|t| t.contains("enterprise") || t.contains("corp") || t.contains("company"))
}

/// 是否企业号：auth 文件带非空 `enterpriseId`，或客户端 account 段带企业标志。
///
/// 企业号**没有个人成长体系**：签到、成长任务、连登兑换、旅行、抽奖在上游一律
/// 被拒（400 code 10001「企业账号不支持该操作」/ 403「growth system is only
/// available for personal users」）。调用方应在发起这类请求前整体跳过，
/// 而不是每天空发一轮注定失败的请求；额度改走企业专用接口。
pub fn is_enterprise_account(creds: &Creds) -> bool {
    !creds.enterprise_id.trim().is_empty() || creds.enterprise_flag
}

fn read_auth_file(state: &AppState) -> Creds {
    creds_of(&fs_utils::read_json::<Value>(&auth_file_path(state)))
}

// ── token store 凭证 vault 收敛（审查 P0-1）────────────────────────────────
//
// wb_tokens 表中敏感字段（access_token / refresh_token）一律占位（空串）存储，
// 明文经 vault::ns_set 走 Stronghold + DPAPI 加密（与 Trae 家族同一 vault）。
// 读取统一回填（仅内存）；写入统一占位。vault 写失败时禁止明文落库（对齐 Trae 红线）。

/// 敏感字段键名集合：snake_case 为主（save_token_store / oauth 写入形态），
/// 兼容 auth 文件导入的 camelCase 形态（accessToken / refreshToken）。
const TOKEN_SENSITIVE_KEYS: [&str; 4] = [
    "access_token",
    "accessToken",
    "refresh_token",
    "refreshToken",
];

/// DB 读取 + vault 回填（仅内存，不落明文）：敏感字段为占位空串时从 vault 回填。
/// vault 不可用 / 无记录 → 保持空串（上层按 no_credential 处理，fail-secure）。
pub fn token_store_load_secure(data_dir: &std::path::Path) -> Value {
    let mut store = crate::store::docs::wb_token_store_load(&crate::store::db(data_dir));
    let Some(tokens) = store.get_mut("tokens").and_then(Value::as_object_mut) else {
        return store;
    };
    for (id, rec) in tokens.iter_mut() {
        let Some(rm) = rec.as_object_mut() else { continue };
        let Some(sec) = crate::vault::ns_get(data_dir, "wb", id) else { continue };
        let acc = sec.get("access_token").and_then(Value::as_str).unwrap_or("");
        let rt = sec.get("refresh_token").and_then(Value::as_str).unwrap_or("");
        let mut has_access_key = false;
        let mut has_refresh_key = false;
        for k in TOKEN_SENSITIVE_KEYS {
            let is_access = matches!(k, "access_token" | "accessToken");
            let val = if is_access { acc } else { rt };
            if !rm.contains_key(k) {
                continue;
            }
            if is_access {
                has_access_key = true;
            } else {
                has_refresh_key = true;
            }
            // 仅填空值：DB 明文优先（更新鲜，如迁移残留，待下次写入收敛）
            if !val.is_empty()
                && rm.get(k).and_then(Value::as_str).map_or(true, |s| s.is_empty())
            {
                rm.insert(k.to_string(), serde_json::json!(val));
            }
        }
        // rec 无任何敏感键但 vault 有值 → 补 snake_case 两键（与 save_token_store 写入形态一致）
        if !acc.is_empty() && !has_access_key {
            rm.insert("access_token".into(), serde_json::json!(acc));
        }
        if !rt.is_empty() && !has_refresh_key {
            rm.insert("refresh_token".into(), serde_json::json!(rt));
        }
    }
    store
}

/// 单账号 UPSERT + 敏感字段 vault 收敛：非空敏感值字段级合并写入 vault（不丢旧值），
/// DB 记录一律占位（空串）。vault 写失败时仍落占位行并返回 Err（禁止明文落库）。
pub fn token_store_upsert_secure(
    data_dir: &std::path::Path,
    id: &str,
    rec: &Value,
) -> Result<(), String> {
    let mut rec = rec.clone();
    let Some(rm) = rec.as_object_mut() else {
        return crate::store::docs::wb_token_store_upsert(&crate::store::db(data_dir), id, &rec);
    };
    // 提取非空敏感值（两组键名各取其一）
    let mut acc = String::new();
    let mut rt = String::new();
    for k in TOKEN_SENSITIVE_KEYS {
        let v = rm.get(k).and_then(Value::as_str).unwrap_or("");
        if v.is_empty() {
            continue;
        }
        if matches!(k, "access_token" | "accessToken") {
            if acc.is_empty() { acc = v.to_string(); }
        } else if rt.is_empty() {
            rt = v.to_string();
        }
    }
    let mut changed = false;
    let vault_result = if acc.is_empty() && rt.is_empty() {
        Ok(())
    } else {
        let mut entry = crate::vault::ns_get(data_dir, "wb", id)
            .unwrap_or_else(|| serde_json::json!({}));
        changed = (!acc.is_empty() && entry.get("access_token").and_then(Value::as_str) != Some(acc.as_str()))
            || (!rt.is_empty() && entry.get("refresh_token").and_then(Value::as_str) != Some(rt.as_str()));
        if !acc.is_empty() {
            entry["access_token"] = serde_json::json!(acc);
        }
        if !rt.is_empty() {
            entry["refresh_token"] = serde_json::json!(rt);
        }
        crate::vault::ns_set(data_dir, "wb", id, &entry)
    };
    // 无论 vault 成败，DB 一律占位
    rm.remove("accessToken");
    rm.remove("refreshToken");
    rm.insert("access_token".into(), serde_json::json!(""));
    rm.insert("refresh_token".into(), serde_json::json!(""));
    crate::store::docs::wb_token_store_upsert(&crate::store::db(data_dir), id, &rec)?;
    if vault_result.is_ok() && changed { super::wb_session::remove_health(data_dir, id)?; }
    vault_result.map_err(|e| {
        format!("WB 凭据加密存储失败（已仅保存占位信息，重新登录可恢复）: {e}")
    })
}

/// 生效凭证 = token store 与 auth 文件中 expiresAtMs 更晚者（F-10 谁新用谁）。
/// auth 文件仅当其 uid 与账号匹配时参与双源比较（桌面当前登录态）。
/// 读 token store（SQLite 化 P3：wb_tokens 表；结构 {version, tokens:{id:rec}}）。
/// 全部 token store 读点统一入口。
pub fn load_token_store(state: &AppState) -> Value {
    token_store_load_secure(&state.data_dir)
}

/// 删除 token store 记录（账号移除时同步清理 vault 凭证，防悬空凭证残留）：
/// 池外孤儿凭证的唯一清理出口（对齐 qoder_common::remove_token）。
/// 顺序（审查 L）：DB 行摘除落库成功后才清 vault——save 失败时保留 vault 凭证并
/// 返 Err，避免「库里还在、密钥已删」的悬挂态。
/// 凭证红线（审查 P0）：此处只需摘行，必须读 DB 原始占位表回写——不可走
/// load_token_store（vault 回填后的明文整表写回，会把其余账号明文凭证重新落库）。
pub fn remove_token(state: &AppState, id: &str) -> Result<(), String> {
    let _guard = WB_TOKEN_STORE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut store = crate::store::docs::wb_token_store_load(&crate::store::db(&state.data_dir));
    if let Some(tokens) = store.get_mut("tokens").and_then(Value::as_object_mut) {
        tokens.remove(id);
    }
    crate::store::docs::wb_token_store_save(&crate::store::db(&state.data_dir), &store)?;
    crate::vault::ns_remove_strict(&state.data_dir, "wb", id)?;
    super::wb_session::remove_health(&state.data_dir, id)
}

pub fn effective_creds(state: &AppState, acct_id: &str, acct_uid: &str) -> Creds {
    let pool_account = crate::store::db(&state.data_dir).row_get("wb_accounts", acct_id).ok().flatten();
    let Some(pool_account) = pool_account else { return Creds::default() };
    let pool_uid = pool_account.get("uid").and_then(Value::as_str).filter(|uid| !uid.is_empty()).unwrap_or(acct_uid);
    let stored = select_creds(creds_of(&token_record_secure(&state.data_dir, acct_id)), Creds::default(), pool_uid);
    let candidate = select_creds(stored.clone(), read_auth_file(state), pool_uid);
    super::wb_session::select_current(stored, candidate, &super::wb_session::health(&state.data_dir, acct_id))
}

pub(crate) fn stored_creds(data_dir: &std::path::Path, acct_id: &str) -> Creds {
    let Some(account) = crate::store::db(data_dir).row_get("wb_accounts", acct_id).ok().flatten() else {
        return Creds::default();
    };
    let uid = account.get("uid").and_then(Value::as_str).unwrap_or_default();
    select_creds(creds_of(&token_record_secure(data_dir, acct_id)), Creds::default(), uid)
}

pub fn token_record_secure(data_dir: &std::path::Path, id: &str) -> Value {
    let mut rec = crate::store::db(data_dir).row_get("wb_tokens", id).ok().flatten().unwrap_or(Value::Null);
    if let Some(fields) = rec.as_object_mut() {
        if let Some(secret) = crate::vault::ns_get(data_dir, "wb", id) {
            for (canonical, alias) in [("access_token", "accessToken"), ("refresh_token", "refreshToken")] {
                let value = [canonical, alias].iter().find_map(|key| fields.get(*key).and_then(Value::as_str).filter(|value| !value.is_empty()))
                    .or_else(|| secret.get(canonical).and_then(Value::as_str)).unwrap_or("").to_string();
                fields.remove(alias);
                fields.insert(canonical.into(), serde_json::json!(value));
            }
        }
    }
    rec
}

fn select_creds(mut store_creds: Creds, mut file_creds: Creds, acct_uid: &str) -> Creds {
    if !acct_uid.is_empty() {
        if store_creds.uid.is_empty() { store_creds.uid = acct_uid.to_string(); }
        if store_creds.uid != acct_uid { store_creds = Creds::default(); }
    }
    let expected_uid = if acct_uid.is_empty() { &store_creds.uid } else { acct_uid };
    if expected_uid.is_empty() || file_creds.uid != expected_uid {
        file_creds = Creds::default();
    }
    let a = store_creds.expires_at_ms;
    let b = file_creds.expires_at_ms;
    // python: file 有 token 且 (store 无到期时间 或 file 到期 >= store 到期)
    let file_newer = a.is_none() || b.is_some_and(|bv| a.map_or(true, |av| bv >= av));
    if !file_creds.access_token.is_empty() && file_newer {
        return file_creds;
    }
    store_creds
}

// ── 并发刷新防护（审查 H-1，对齐 qoder_common 同款双锁）──────────────────────

/// token store 表级读改写互斥：load→merge→upsert 非原子，签到/积分/续期/导入/
/// OAuth 多通道并发写会互相覆盖丢更新（last-writer-wins 抹掉彼此的新 token）。
/// tasks 侧 save_token_store 与 commands 侧 upsert_token_store 共用本锁串行化
/// 表级读改写；仅护本地 IO，不覆盖网络请求路径（无死锁面）。
/// 锁序约定：账号刷新锁 → 本表锁（单向获取，无环）。
pub(crate) static WB_TOKEN_STORE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 每账号刷新互斥（审查 H-1/P0）：签到/积分 401 自愈/续期/调度器与 schtasks CLI
/// 双进程多通道并发触发同一账号刷新时，两个线程可能同时拿旧 refresh_token 换新
/// token——服务端一次性轮换下后落库者覆盖先落库者，被覆盖方刚拿到的令牌即刻失效
///（严重时 refresh_token 一并丢，被迫重登）。以账号 id 为键的进程内锁串行化
/// 「读凭证→网络刷新→落库」全程；持锁后重读 token store 天然构成二次检查。
static WB_REFRESH_LOCKS: std::sync::Mutex<
    Option<std::collections::HashMap<String, std::sync::Weak<std::sync::Mutex<()>>>>,
> = std::sync::Mutex::new(None);

pub(crate) fn refresh_lock_for(acct_id: &str) -> std::sync::Arc<std::sync::Mutex<()>> {
    let mut g = WB_REFRESH_LOCKS.lock().unwrap_or_else(|e| e.into_inner());
    let map = g.get_or_insert_with(std::collections::HashMap::new);
    map.retain(|_, lock| lock.strong_count() > 0);
    if let Some(lock) = map.get(acct_id).and_then(std::sync::Weak::upgrade) { return lock; }
    let lock = std::sync::Arc::new(std::sync::Mutex::new(()));
    map.insert(acct_id.to_string(), std::sync::Arc::downgrade(&lock));
    lock
}


/// 跨进程刷新互斥（H-1 ②）：GUI 调度器与 schtasks CLI（--task-run，绕开单实例
/// 插件）双进程并存，进程内锁无法约束跨进程并发。命名互斥体按账号隔离
///（ns="wb"，scope=账号 id，锁名含 data_dir 短哈希）；抢锁失败幂等跳过本次刷新
///（沿用现有凭证，仅少一次自愈机会，无损失）。
/// C1（审查）：改按 data_dir 传参——网关侧 wb_upstream::refresh_access_token
/// 无 AppState，与 tasks 侧复用同一把锁；pub(crate) 供 api_server 模块调用。
pub(crate) fn cross_refresh_lock(
    data_dir: &std::path::Path,
    acct_id: &str,
) -> Option<crate::tasks::qoder_common::CrossProcLock> {
    let (guard, fail) =
        crate::tasks::qoder_common::CrossProcLock::try_acquire_ns(data_dir, "wb", acct_id, 5000);
    if guard.is_none() {
        let reason = fail
            .as_ref()
            .map(|f| f.describe())
            .unwrap_or_default();
        fs_utils::app_log(
            data_dir,
            &format!("[wb-fresh] 跨进程刷新锁未获取(id={acct_id}, {reason})，本轮沿用现有凭证"),
        );
    }
    guard
}

/// 写工具侧凭证副本（F-10 谁新用谁）。version≠1 拒绝写入（版本闸门）；
/// 非空字段合并 + updated_at（与 commands/workbuddy/common.rs upsert_token_store 同语义）。
/// 凭证收敛（P0-1）：落库走 token_store_upsert_secure——敏感字段进 vault、DB 占位；
/// vault 写失败时仅落占位并返回 Err（禁止明文落库）。
pub fn save_token_store(state: &AppState, id: &str, creds: &Creds) -> Result<(), String> {
    save_token_store_at(&state.data_dir, id, creds)
}

pub(crate) fn save_token_store_at(data_dir: &std::path::Path, id: &str, creds: &Creds) -> Result<(), String> {
    // H-1：表级读改写互斥（与 commands 侧 upsert_token_store 共锁）
    let _table = WB_TOKEN_STORE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if crate::store::db(data_dir).row_get("wb_accounts", id)?.is_none() {
        return Err("账号已删除，拒绝写回凭证".into());
    }
    let db = crate::store::db(data_dir);
    match db.kv_get_raw("wb_tokens_meta") {
        Some(version) if version.parse::<i64>().ok() != Some(1) => {
            return Err("token_store 版本不识别，拒绝写入".to_string());
        }
        _ => {}
    }
    let mut rec = token_record_secure(data_dir, id);
    if !rec.is_object() { rec = serde_json::json!({}); }
    if let Some(rm) = rec.as_object_mut() {
        let val = serde_json::to_value(creds).map_err(|e| e.to_string())?;
        for (k, v) in val.as_object().into_iter().flatten() {
            if !v.is_null() && !v.as_str().is_some_and(str::is_empty) {
                rm.insert(k.clone(), v.clone());
            }
        }
        rm.insert("expires_at_ms".into(), serde_json::json!(creds.expires_at_ms));
        rm.insert("refresh_expires_at_ms".into(), serde_json::json!(creds.refresh_expires_at_ms));
        rm.insert("updated_at".into(), serde_json::json!(fs_utils::now_iso()));
    }
    token_store_upsert_secure(data_dir, id, &rec)?;
    // version 闸门语义不变（整库字段，单行 upsert 不携带 version——写入 kv 元数据）
    crate::store::docs::wb_token_store_save_version(&crate::store::db(data_dir), 1)
}

// ── 客户端指纹伪装（issue #48）─────────────────────────────────────────────

/// billing/签到/刷新链路桌面端 UA 伪装：必须带版本号——裸 "WorkBuddy" 在
/// 个人中心「请求明细」的客户端列识别为 "-"（issue #48 反馈的可检测特征）。
pub const WB_DESKTOP_UA: &str = "WorkBuddy/5.5.6";

/// 账号级稳定设备指纹：sha256("wb-fingerprint:{kind}:{uid}") 前 16 字节 →
/// 32 位小写十六进制。同账号恒定（虚拟设备稳定）、跨账号隔离（防关联）；
/// uid 为空返回 None（缺失即不带，不伪造）。
pub fn derive_device_fingerprint(uid: &str, kind: &str) -> Option<String> {
    if uid.trim().is_empty() {
        return None;
    }
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(format!("wb-fingerprint:{kind}:{uid}").as_bytes());
    let d = h.finalize();
    Some(d[..16].iter().map(|b| format!("{b:02x}")).collect())
}

// ── 统一请求头（§5.3）───────────────────────────────────────────────────────

/// Bearer + X-User-Id（缺省 X-No-* 占位）+ 客户端指纹（issue #48：UA 带版本、
/// X-Machine-ID/X-Session-ID 账号级稳定派生、X-Domain 域标识）；web_platform=true
/// 附加 X-Client-Platform: web（积分三件套必需）。
pub fn build_auth_headers(creds: &Creds, web_platform: bool) -> Vec<(String, String)> {
    let mut h = vec![
        (
            "Authorization".to_string(),
            format!("Bearer {}", creds.access_token),
        ),
        ("User-Agent".to_string(), WB_DESKTOP_UA.to_string()),
        ("Content-Type".to_string(), "application/json".to_string()),
    ];
    if creds.uid.is_empty() {
        h.push(("X-No-User-Id".to_string(), "1".to_string()));
    } else {
        h.push(("X-User-Id".to_string(), creds.uid.clone()));
        // 客户端指纹（issue #48）：个人中心请求明细按设备/会话标识识别客户端
        if let Some(mid) = derive_device_fingerprint(&creds.uid, "machine") {
            h.push(("X-Machine-ID".to_string(), mid));
        }
        if let Some(sid) = derive_device_fingerprint(&creds.uid, "session") {
            h.push(("X-Session-ID".to_string(), sid));
        }
    }
    if !creds.domain.is_empty() {
        h.push(("X-Domain".to_string(), creds.domain.clone()));
    }
    if web_platform {
        h.push(("X-Client-Platform".to_string(), "web".to_string()));
    }
    h
}

/// POST JSON → (http_status, parsed, raw_text)；status=0 表示网络不可达
///（对齐 python post_json 三元组；HTTPError 同样返回状态码与响应体原文）。
pub fn post_json_raw(
    agent: &ureq::Agent,
    url: &str,
    headers: &[(String, String)],
    body: &Value,
) -> (u16, Option<Value>, String) {
    let mut req = agent.post(url);
    for (k, v) in headers {
        req = req.set(k, v);
    }
    match req.send_string(&body.to_string()) {
        Ok(resp) => {
            // 审查 P2：读取失败不再吞成空串（调用方诊断日志需区分「空响应」与「读取失败」）
            let raw = resp.into_string().unwrap_or_else(|e| format!("<响应体读取失败: {e}>"));
            let parsed = serde_json::from_str(&raw).ok();
            (200, parsed, raw)
        }
        Err(ureq::Error::Status(code, resp)) => {
            let raw = resp.into_string().unwrap_or_else(|e| format!("<响应体读取失败: {e}>"));
            let parsed = serde_json::from_str(&raw).ok();
            (code, parsed, raw)
        }
        Err(e) => (0, None, e.to_string()),
    }
}

/// POST JSON → (http_status, parsed)；status=0 表示网络不可达
///（对齐 python post_json 三元组的可消费部分；HTTPError 同样返回状态码）。
pub fn post_json(
    agent: &ureq::Agent,
    url: &str,
    headers: &[(String, String)],
    body: &Value,
) -> (u16, Option<Value>) {
    let (status, parsed, _) = post_json_raw(agent, url, headers, body);
    (status, parsed)
}

/// GET 请求 → (http_status, parsed, raw_text)；status=0 网络不可达
///（对齐 python get_json，F-17 成长中心等 GET 端点；HTTPError 同样返回状态码）。
pub fn get_json(
    agent: &ureq::Agent,
    url: &str,
    headers: &[(String, String)],
) -> (u16, Option<Value>, String) {
    let mut req = agent.get(url);
    for (k, v) in headers {
        req = req.set(k, v);
    }
    match req.call() {
        Ok(resp) => {
            // 审查 P2：读取失败不再吞成空串（调用方诊断日志需区分「空响应」与「读取失败」）
            let raw = resp.into_string().unwrap_or_else(|e| format!("<响应体读取失败: {e}>"));
            let parsed = serde_json::from_str(&raw).ok();
            (200, parsed, raw)
        }
        Err(ureq::Error::Status(code, resp)) => {
            let raw = resp.into_string().unwrap_or_else(|e| format!("<响应体读取失败: {e}>"));
            let parsed = serde_json::from_str(&raw).ok();
            (code, parsed, raw)
        }
        Err(e) => (0, None, e.to_string()),
    }
}

// ── 区域路由（T4.5/F-36，§5.2）──────────────────────────────────────────────
// CN：billing/积分 + 活动接口走 www.codebuddy.cn；Global（domain 含 .workbuddy.ai）：
// 全走 www.workbuddy.ai。plugin 网关（token refresh）固定 codebuddy.cn 不随区域。

pub const BILLING_BASE_CN: &str = "https://www.codebuddy.cn";
pub const BILLING_BASE_GLOBAL: &str = "https://www.workbuddy.ai";
pub const REFRESH_URL: &str = "https://www.codebuddy.cn/v2/plugin/auth/token/refresh";

pub fn is_global_region(domain: &str) -> bool {
    domain.contains(".workbuddy.ai")
}

pub fn region_billing_base(domain: &str) -> &'static str {
    if is_global_region(domain) {
        BILLING_BASE_GLOBAL
    } else {
        BILLING_BASE_CN
    }
}

/// Web 端（成长中心页面）域名：**领奖必须走这里**。
/// CN 是 `www.workbuddy.cn`（不是拿 token 的 `www.codebuddy.cn`，后者领奖返回
/// 400 "task not completed"）；Global 沿用 `www.workbuddy.ai`。
pub const WEB_BASE_CN: &str = "https://www.workbuddy.cn";
pub const WEB_BASE_GLOBAL: &str = "https://www.workbuddy.ai";

/// Web 端域名（CN `www.workbuddy.cn` / Global `www.workbuddy.ai`）
pub fn region_web_base(domain: &str) -> &'static str {
    if is_global_region(domain) {
        WEB_BASE_GLOBAL
    } else {
        WEB_BASE_CN
    }
}

/// 对话域基址（`/v2/report` 桌面事件族与 `/v2/chat/completions` 同域）：
/// CN `copilot.tencent.com` / Global `www.workbuddy.ai`
pub const CHAT_BASE_CN: &str = "https://copilot.tencent.com";
pub const CHAT_BASE_GLOBAL: &str = "https://www.workbuddy.ai";

pub fn region_chat_base(domain: &str) -> &'static str {
    if is_global_region(domain) {
        CHAT_BASE_GLOBAL
    } else {
        CHAT_BASE_CN
    }
}

/// 在既有账号头上补齐 Web 端标记（领奖用）：`x-client-platform: web` 与成长中心
/// Origin/Referer（浏览器实请求形状，上游按此判定来源端）。
/// 独立成函数是为了让调用方能把「刷新后的新凭证头」再套一层，避免拿旧 token 领奖。
pub fn web_flavor(base: &[(String, String)], web_base: &str) -> Vec<(String, String)> {
    let mut h = base.to_vec();
    h.push(("Accept".to_string(), "application/json, text/plain, */*".to_string()));
    h.push(("Origin".to_string(), web_base.to_string()));
    h.push(("Referer".to_string(), format!("{web_base}/profile/growth-center")));
    h.push(("X-Client-Platform".to_string(), "web".to_string()));
    h
}

/// 域名双探测（§2.2 接口稳定性）：主域名在前、备用域名在后。
pub fn billing_bases(domain: &str) -> [&'static str; 2] {
    let main = region_billing_base(domain);
    let alt = if main == BILLING_BASE_CN {
        BILLING_BASE_GLOBAL
    } else {
        BILLING_BASE_CN
    };
    [main, alt]
}

// ── token 刷新（F-09）──────────────────────────────────────────────────────

/// refresh 失败原因（审查 P1-4：区分「凭证失效」与「网络故障」，
/// 供调用方决定是否标记 needs_relogin——网络失败不应误标）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshFail {
    /// 无 refreshToken（不可刷新）
    NoRefreshToken,
    /// 网络不可达 / 传输层失败（不标记重登录，下次重试）
    Network,
    /// 明确的认证拒绝。
    Auth,
    RateLimited,
    Rejected,
    Cooldown,
    /// 200 但响应无 accessToken（响应结构异常，不标记重登录）
    BadResponse,
    /// H-1：跨进程锁未获取（他方进程正在刷新同账号，本侧幂等跳过；不标记重登录）
    Busy,
    Persistence,
}

pub(crate) fn refresh_request(agent: &ureq::Agent, creds: &Creds) -> Result<Creds, super::wb_session::RefreshError> {
    refresh_request_at(agent, creds, REFRESH_URL)
}

fn classify_refresh(status: u16, body: Option<&Value>) -> RefreshFail {
    if status == 0 || status == 408 || status >= 500 { return RefreshFail::Network; }
    if status == 429 { return RefreshFail::RateLimited; }
    let code = body.and_then(|body| body.get("error").and_then(Value::as_str)
        .or_else(|| body.pointer("/error/code").and_then(Value::as_str))
        .or_else(|| fs_utils::dig(body, &["code", "errorCode"]).and_then(Value::as_str))).unwrap_or_default();
    if status == 401 || matches!(code, "invalid_grant" | "invalid_token" | "refresh_token_expired" | "refresh_token_revoked") {
        RefreshFail::Auth
    } else if (400..500).contains(&status) { RefreshFail::Rejected }
    else { RefreshFail::BadResponse }
}

fn refresh_request_at(agent: &ureq::Agent, creds: &Creds, url: &str) -> Result<Creds, super::wb_session::RefreshError> {
    use super::wb_session::RefreshError;
    if creds.refresh_token.is_empty() {
        return Err(RefreshError::new(RefreshFail::NoRefreshToken));
    }
    let mut h = build_auth_headers(creds, false);
    h.push(("X-Refresh-Token".to_string(), creds.refresh_token.clone()));
    h.push((
        "X-Auth-Refresh-Source".to_string(),
        "workbuddy".to_string(),
    ));
    let mut request = agent.post(url);
    for (key, value) in &h { request = request.set(key, value); }
    let response = match request.send_json(serde_json::json!({})) {
        Ok(response) | Err(ureq::Error::Status(_, response)) => response,
        Err(_) => return Err(RefreshError::new(RefreshFail::Network)),
    };
    let status = response.status();
    let retry_after_ms = response.header("Retry-After").and_then(|value| {
        value.parse::<i64>().ok().filter(|seconds| *seconds >= 0).map(|seconds| seconds.saturating_mul(1000))
            .or_else(|| chrono::DateTime::parse_from_rfc2822(value).ok()
                .map(|date| date.timestamp_millis().saturating_sub(chrono::Utc::now().timestamp_millis()).max(0)))
    });
    let body: Option<Value> = response.into_json().ok();
    if status != 200 {
        return Err(RefreshError { kind: classify_refresh(status, body.as_ref()), http_status: Some(status), retry_after_ms });
    }
    let body = body.ok_or_else(|| RefreshError::new(RefreshFail::BadResponse))?;
    let acc = s_of(fs_utils::dig(&body, &["accessToken", "access_token"]));
    if acc.is_empty() {
        return Err(RefreshError { kind: classify_refresh(status, Some(&body)), http_status: Some(status), retry_after_ms });
    }
    let now_ms = chrono::Utc::now().timestamp_millis();
    let mut out = creds.clone();
    out.access_token = acc;
    let ref_tok = s_of(fs_utils::dig(&body, &["refreshToken", "refresh_token"]));
    let rotated = !ref_tok.is_empty() && ref_tok != creds.refresh_token;
    if !ref_tok.is_empty() {
        out.refresh_token = ref_tok;
    }
    out.expires_at_ms = i_of(fs_utils::dig(&body, &["expiresIn"])).filter(|seconds| *seconds > 0)
        .and_then(|seconds| seconds.checked_mul(1000)?.checked_add(now_ms));
    if out.expires_at_ms.is_none() { return Err(RefreshError::new(RefreshFail::BadResponse)); }
    out.refresh_expires_at_ms = i_of(fs_utils::dig(&body, &["refreshExpiresIn"]))
        .filter(|seconds| *seconds > 0).and_then(|seconds| seconds.checked_mul(1000)?.checked_add(now_ms))
        .or(if rotated { None } else { creds.refresh_expires_at_ms });
    Ok(out)
}

/// 标记账号需重新登录（审查 P1-4：刷新凭证失效时回写账号池，
/// 网关池同步（sync_from_wb disabled=needs_relogin）与调度跳过随之生效）
pub fn mark_needs_relogin(state: &AppState, acct_id: &str, reason: &str) {
    let store = crate::store::db(&state.data_dir);
    // 读-改-写互斥：罩住 load→改→save 段，防命令/后台/调度线程旧副本整文档覆盖
    let _guard = state.wb_pool_lock.lock().unwrap_or_else(|e| e.into_inner());
    let mut pool = crate::store::docs::wb_pool_load(&store);
    let mut changed = false;
    if let Some(arr) = pool.get_mut("accounts").and_then(Value::as_array_mut) {
        for a in arr {
            if a.get("id").and_then(Value::as_str) == Some(acct_id) {
                if a.get("needs_relogin").and_then(Value::as_bool) == Some(true)
                    && a.get("relogin_reason").and_then(Value::as_str) == Some(reason) { continue; }
                a["needs_relogin"] = serde_json::json!(true);
                a["relogin_reason"] = serde_json::json!(reason);
                changed = true;
            }
        }
    }
    if changed {
        let _ = crate::store::docs::wb_pool_save(&store, &pool);
        fs_utils::app_log(
            &state.data_dir,
            &format!("wb: 账号 {acct_id} 已标记需重新登录（{reason}）"),
        );
    }
}


// ── 惰性刷新（F-09/F-55，原 wb_common.ensure_fresh）────────────────────────

/// 惰性刷新：距过期 < lazy_hours 才刷；一次调用最多一次刷新。
/// 返回 (creds, refreshed, note)，note ∈ no_credential/fresh/expired_needs_relogin/refreshed/refresh_failed。
///
/// H-1（P0 修复）：「读凭证→网络刷新→落库」全程持每账号刷新锁（进程内）+ 跨进程
/// 命名互斥体，双进程/多通道并发触发同账号刷新时串行化；持锁后重读凭证构成二次
/// 检查——他人已刷新落库则直接命中新凭证（fresh），不再重复发网络请求。新鲜快路径
/// 无锁直返（调度/签到热路径不承担锁开销）。
pub fn ensure_fresh(
    state: &AppState,
    agent: &ureq::Agent,
    acct: &Value,
    lazy_hours: i64,
) -> (Creds, bool, &'static str) {
    let acct_id = acct.get("id").and_then(Value::as_str).unwrap_or("");
    let acct_uid = acct.get("uid").and_then(Value::as_str).unwrap_or("");
    let creds = effective_creds(state, acct_id, acct_uid);
    if creds.access_token.is_empty() {
        return (creds, false, "no_credential");
    }
    match super::wb_session::refresh(&state.data_dir, acct_id, &creds, super::wb_session::RefreshMode::Due(lazy_hours), agent) {
        Ok((current, refreshed)) => (current, refreshed, if refreshed { "refreshed" } else { "fresh" }),
        Err(error) => {
            if error.permanent() { mark_needs_relogin(state, acct_id, &error.message()); }
            (creds, false, error.note())
        }
    }
}

/// 401 自愈专用加锁刷新（H-1）：串行化「网络刷新→落库」并做二次检查——他人已刷新
/// 落库（token store 中该账号 access_token 与调用方持有凭证不同）则直接复用新凭证，
/// 不再重复发网络请求；否则以调用方凭证刷新并落库（调用方无须再自行 save）。
/// 跨进程抢锁失败 → (None, Some(RefreshFail::Busy))：调用方按非 Auth 处理
///（不误标 needs_relogin，下次自然重试）。
pub fn refresh_token_once_locked(
    state: &AppState,
    agent: &ureq::Agent,
    acct_id: &str,
    stale: &Creds,
) -> (Option<Creds>, Option<RefreshFail>) {
    match super::wb_session::refresh(&state.data_dir, acct_id, stale, super::wb_session::RefreshMode::Recover, agent) {
        Ok((creds, _)) => (Some(creds), None),
        Err(error) => {
            if error.permanent() { mark_needs_relogin(state, acct_id, &error.message()); }
            (None, Some(error.kind))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_refresh_classification_requires_explicit_auth_evidence() {
        for status in [0, 408, 500, 502, 503] { assert_eq!(classify_refresh(status, None), RefreshFail::Network); }
        assert_eq!(classify_refresh(429, None), RefreshFail::RateLimited);
        assert_eq!(classify_refresh(401, None), RefreshFail::Auth);
        for status in [400, 403, 404, 422] { assert_eq!(classify_refresh(status, None), RefreshFail::Rejected); }
        assert_eq!(classify_refresh(400, Some(&serde_json::json!({"error":"invalid_grant"}))), RefreshFail::Auth);
        assert_eq!(classify_refresh(403, Some(&serde_json::json!({"error":{"code":"refresh_token_revoked"}}))), RefreshFail::Auth);
        assert_eq!(classify_refresh(429, Some(&serde_json::json!({"error":"invalid_grant"}))), RefreshFail::RateLimited);
    }

    fn mock_session_response(status: u16, extra: &str, body: &str, creds: &Creds) -> Result<Creds, crate::tasks::wb_session::RefreshError> {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/refresh", listener.local_addr().unwrap());
        let response = format!("HTTP/1.1 {status} Mock\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n{extra}\r\n{body}", body.len());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 2048];
            loop {
                let size = stream.read(&mut buffer).unwrap();
                request.extend_from_slice(&buffer[..size]);
                if size == 0 { break; }
                if let Some(end) = request.windows(4).position(|chunk| chunk == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..end]);
                    let length = headers.lines().find_map(|line| line.split_once(':')
                        .filter(|(name, _)| name.eq_ignore_ascii_case("Content-Length"))
                        .and_then(|(_, value)| value.trim().parse::<usize>().ok())).unwrap_or(0);
                    if request.len() >= end + 4 + length { break; }
                }
            }
            stream.write_all(response.as_bytes()).unwrap();
        });
        let result = refresh_request_at(&crate::tasks::http_agent(5), creds, &url);
        server.join().unwrap();
        result
    }

    #[test]
    fn session_refresh_http_429_keeps_actual_status_and_retry_after_without_secrets() {
        let creds = Creds { access_token: "synthetic-access".into(), refresh_token: "synthetic-refresh".into(), ..Default::default() };
        let error = mock_session_response(429, "Retry-After: 120\r\n", r#"{"message":"secret upstream body"}"#, &creds).err().unwrap();
        assert_eq!(error.kind, RefreshFail::RateLimited);
        assert_eq!(error.http_status, Some(429));
        assert_eq!(error.retry_after_ms, Some(120_000));
        assert!(!error.message().contains("需重新登录"));
        assert!(!error.message().contains("401"));
        assert!(!error.message().contains("secret upstream body"));
        assert!(!format!("{error:?}").contains("synthetic-refresh"));
    }

    #[test]
    fn session_refresh_rotated_rt_drops_old_expiry_and_supports_envelopes() {
        let creds = Creds { access_token: "synthetic-access".into(), refresh_token: "synthetic-refresh".into(), refresh_expires_at_ms: Some(123_000), ..Default::default() };
        let result = mock_session_response(200, "", r#"{"data":{"accessToken":"synthetic-new-access","refreshToken":"synthetic-new-refresh","expiresIn":3600}}"#, &creds).unwrap();
        assert_eq!(result.access_token, "synthetic-new-access");
        assert_eq!(result.refresh_token, "synthetic-new-refresh");
        assert_eq!(result.refresh_expires_at_ms, None);
        let retained = mock_session_response(200, "", r#"{"accessToken":"synthetic-new-access","expiresIn":3600}"#, &creds).unwrap();
        assert_eq!(retained.refresh_token, creds.refresh_token);
        assert_eq!(retained.refresh_expires_at_ms, creds.refresh_expires_at_ms);
    }

    #[test]
    fn session_refresh_rejects_missing_or_overflowing_lifetime() {
        let creds = Creds { refresh_token: "synthetic-refresh".into(), ..Default::default() };
        for body in [r#"{"accessToken":"synthetic-new"}"#, r#"{"accessToken":"synthetic-new","expiresIn":9223372036854775807}"#] {
            assert_eq!(mock_session_response(200, "", body, &creds).err().unwrap().kind, RefreshFail::BadResponse);
        }
    }

    #[test]
    fn credential_selection_requires_positive_identity_match() {
        let stored = || Creds { access_token: "stored-access".into(), ..Default::default() };
        let local = |uid: &str| Creds { uid: uid.into(), access_token: "local-access".into(), expires_at_ms: Some(99999), ..Default::default() };
        assert_eq!(select_creds(stored(), local("other"), "target").access_token, "stored-access");
        assert_eq!(select_creds(stored(), local(""), "target").access_token, "stored-access");
        assert_eq!(select_creds(Creds::default(), local("other"), "").access_token, "");
        assert_eq!(select_creds(stored(), local("target"), "target").access_token, "local-access");
        let recovered = select_creds(stored(), Creds::default(), "target");
        assert_eq!(recovered.uid, "target");
        assert!(build_auth_headers(&recovered, false).contains(&("X-User-Id".into(), "target".into())));
        let wrong_store = Creds { uid: "other".into(), ..stored() };
        assert!(select_creds(wrong_store, Creds::default(), "target").access_token.is_empty());
    }

    #[test]
    fn canonical_fields_override_legacy_aliases() {
        let creds = creds_of(&serde_json::json!({"access_token": "new-access", "accessToken": "old-access", "refresh_token": "new-refresh", "refreshToken": "old-refresh", "expires_at_ms": 200, "expiresAtMs": 100}));
        assert_eq!(creds.access_token, "new-access");
        assert_eq!(creds.refresh_token, "new-refresh");
        assert_eq!(creds.expires_at_ms, Some(200));
    }

    #[cfg(windows)]
    #[test]
    fn camelcase_secure_roundtrip_preserves_separate_tokens() {
        let dir = std::env::temp_dir().join(format!("wb-secure-roundtrip-{}-{}", std::process::id(), chrono::Utc::now().timestamp_nanos_opt().unwrap()));
        std::fs::create_dir_all(dir.join("data")).unwrap();
        token_store_upsert_secure(&dir, "test", &serde_json::json!({"accessToken": "synthetic-access", "refreshToken": "synthetic-refresh", "uid": "test-uid"})).unwrap();
        let creds = creds_of(&token_record_secure(&dir, "test"));
        assert_eq!(creds.access_token, "synthetic-access");
        assert_eq!(creds.refresh_token, "synthetic-refresh");
        token_store_upsert_secure(&dir, "test", &serde_json::json!({"access_token": "rotated-access", "accessToken": "stale-access"})).unwrap();
        let creds = creds_of(&token_record_secure(&dir, "test"));
        assert_eq!(creds.access_token, "rotated-access");
        assert_eq!(creds.refresh_token, "synthetic-refresh");
        let raw = crate::store::db(&dir).row_get("wb_tokens", "test").unwrap().unwrap();
        assert_eq!(raw["access_token"], "");
        assert_eq!(raw["refresh_token"], "");
        assert!(raw.get("accessToken").is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn serialized_enterprise_flag_survives_reload() {
        let original = Creds { enterprise_flag: true, uid: "synthetic-enterprise".into(), ..Default::default() };
        let saved = serde_json::to_value(&original).unwrap();
        assert!(is_enterprise_account(&creds_of(&saved)));
    }

    #[test]
    fn refresh_lock_stays_shared_while_waiters_exist() {
        let first = refresh_lock_for("synthetic-lifecycle");
        let waiter = refresh_lock_for("synthetic-lifecycle");
        assert!(std::sync::Arc::ptr_eq(&first, &waiter));
        drop(first);
        assert!(std::sync::Arc::ptr_eq(&waiter, &refresh_lock_for("synthetic-lifecycle")));
    }

    // ==================== 企业号判定 ====================

    #[test]
    fn enterprise_detected_from_enterprise_id() {
        // auth 文件带非空 enterpriseId → 企业号（参考实现的同一判据）
        let raw = serde_json::json!({
            "auth": {"accessToken": "at", "domain": "www.workbuddy.cn"},
            "account": {"uid": "u1", "enterpriseId": "ent-1", "nickname": "企业号"}
        });
        let c = creds_of(&raw);
        assert_eq!(c.enterprise_id, "ent-1");
        assert!(is_enterprise_account(&c));
    }

    #[test]
    fn personal_account_not_enterprise() {
        // 个人号：无 enterpriseId，且 isCurrentOneIdEnterprise=false
        let raw = serde_json::json!({
            "auth": {"accessToken": "at"},
            "account": {
                "uid": "u1", "isCurrentOneIdEnterprise": false,
                "isCurrentOneIdPersonal": false, "accountType": ""
            }
        });
        let c = creds_of(&raw);
        assert!(c.enterprise_id.is_empty());
        // personal 与 enterprise 同为 false 时不得判成企业（缺省噪声）
        assert!(!is_enterprise_account(&c));
    }

    #[test]
    fn enterprise_detected_from_client_flag() {
        // 客户端标志形态（无 enterpriseId 时兜底）：isCurrentOneIdEnterprise=true
        let raw = serde_json::json!({
            "auth": {"accessToken": "at"},
            "account": {"uid": "u2", "isCurrentOneIdEnterprise": true}
        });
        assert!(is_enterprise_account(&creds_of(&raw)));

        // accountType 含 enterprise
        let raw2 = serde_json::json!({
            "auth": {"accessToken": "at"},
            "account": {"uid": "u3", "accountType": "Enterprise"}
        });
        assert!(is_enterprise_account(&creds_of(&raw2)));
    }

    // ==================== 客户端指纹伪装（issue #48） ====================

    #[test]
    fn derive_device_fingerprint_deterministic_and_isolated() {
        // 同账号恒定、32 位小写 hex；不同 kind / 不同 uid 互异；uid 空缺失即不带
        let a1 = derive_device_fingerprint("u-1", "machine").unwrap();
        let a2 = derive_device_fingerprint("u-1", "machine").unwrap();
        assert_eq!(a1, a2, "同账号指纹必须恒定");
        assert_eq!(a1.len(), 32);
        assert!(a1.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(
            derive_device_fingerprint("u-1", "machine").unwrap(),
            derive_device_fingerprint("u-1", "session").unwrap()
        );
        assert_ne!(
            derive_device_fingerprint("u-1", "machine").unwrap(),
            derive_device_fingerprint("u-2", "machine").unwrap(),
            "跨账号必须隔离防关联"
        );
        assert!(derive_device_fingerprint("", "machine").is_none());
        assert!(derive_device_fingerprint("  ", "machine").is_none());
    }

    #[test]
    fn auth_headers_carry_client_fingerprint() {
        let creds = Creds {
            access_token: "tk".into(),
            uid: "u-1".into(),
            domain: "d1".into(),
            ..Default::default()
        };
        let h = build_auth_headers(&creds, false);
        let get = |hs: &[(String, String)], k: &str| {
            hs.iter()
                .find(|(hk, _)| hk.eq_ignore_ascii_case(k))
                .map(|(_, v)| v.clone())
        };
        // UA 带版本（issue #48：裸 "WorkBuddy" 在个人中心明细识别为 "-"）
        assert_eq!(get(&h, "User-Agent").as_deref(), Some(WB_DESKTOP_UA));
        assert_eq!(get(&h, "X-User-Id").as_deref(), Some("u-1"));
        assert_eq!(get(&h, "X-Domain").as_deref(), Some("d1"));
        // 设备/会话指纹：与派生函数一致、32 位 hex
        let mid = get(&h, "X-Machine-ID").expect("X-Machine-ID 必须存在");
        let sid = get(&h, "X-Session-ID").expect("X-Session-ID 必须存在");
        assert_eq!(mid, derive_device_fingerprint("u-1", "machine").unwrap());
        assert_eq!(sid, derive_device_fingerprint("u-1", "session").unwrap());
        assert_eq!(mid.len(), 32);
        // web 平台附加
        let hw = build_auth_headers(&creds, true);
        assert!(hw.iter().any(|(k, v)| k == "X-Client-Platform" && v == "web"));
        // uid/domain 缺失：X-No-* 占位、不带指纹与域标识
        let empty = build_auth_headers(&Creds::default(), false);
        assert_eq!(get(&empty, "X-No-User-Id").as_deref(), Some("1"));
        assert!(get(&empty, "X-User-Id").is_none());
        assert!(get(&empty, "X-Machine-ID").is_none());
        assert!(get(&empty, "X-Session-ID").is_none());
        assert!(get(&empty, "X-Domain").is_none());
    }
}
