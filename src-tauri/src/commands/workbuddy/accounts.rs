//! WorkBuddy 账号域（原 workbuddy.rs 机械拆分）：M1 环境检测、账号池 CRUD（F-04）、
//! M3 凭证续期（F-09）、API 网关 WB 上游取号（T2.1）、账号库导入导出（F-46 扩展）。
//! 函数逻辑零改动，仅将跨子模块引用项提升为 `pub(super)`。

use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use crate::fs_utils;
use crate::state::AppState;

use super::cli::{load_cli_rotate_state, save_cli_rotate_state};
use super::common::{
    account_id_of, as_str, as_ts_seconds, auth_file_path_of, dig_plain_string,
    extract_access_token, is_running, load_pool, save_pool, snapshot_json_path, upsert_token_store,
    wb_data_dir, WorkBuddyAccount, WbPool,
};

// ── 数据结构 ────────────────────────────────────────────────────────────────

#[derive(Serialize, Clone)]
pub struct WorkBuddyAccountView {
    pub id: String,
    pub uid: String,
    pub nickname: String,
    pub phone_masked: String,
    pub edition_type: String,
    pub access_token_expires_at: Option<i64>,
    pub refresh_token_expires_at: Option<i64>,
    pub auth_saved_at: Option<i64>,
    pub needs_relogin: bool,
    pub relogin_reason: String,
    pub session_health: serde_json::Value,
    pub group_id: String,
    pub note: String,
    pub credits_balance: Option<f64>,
    pub credits_fetched_at: Option<String>,
    /// 最早积分包到期时间（Unix 秒；剩余>0 且未过期包取 min）；
    /// None = 无到期信息或全部包长期有效
    pub credits_expire_at: Option<i64>,
    /// 在线 = 本机 auth 文件当前生效账号（F-54 双态）
    pub is_current: bool,
    /// 有工具侧凭证副本或 auth 文件匹配
    pub has_credential: bool,
    /// 已录快照（PS 桥 profiles_workbuddy/<id>/）
    pub has_snapshot: bool,
    /// F2-2 已录 CodeBuddy 快照（profiles_codebuddy/<id>/）——双端登录态分端展示
    pub has_snapshot_codebuddy: bool,
    /// F2-2 WorkBuddy 端当前账号（桥 profiles_workbuddy/current_account.txt 标记）
    pub is_current_workbuddy: bool,
    /// F2-2 CodeBuddy 端当前账号（桥 profiles_codebuddy/current_account.txt 标记）
    pub is_current_codebuddy: bool,
    /// 共享标签 tag id 列表（account_tags 表联表填充，不改账号持久化结构，D3）
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

#[derive(Serialize, Clone)]
pub struct WorkBuddyEnvCheck {
    pub installed: bool,
    pub running: bool,
    pub version: Option<String>,
    pub exe: Option<String>,
    /// 实际读取的 auth 文件路径（人工覆盖优先，见 auth_file_path_of）
    pub auth_file_path: String,
    pub auth_file_exists: bool,
    pub data_dir_exists: bool,
    /// account-snapshot.json 当前登录 uid（可读时）
    pub snapshot_uid: Option<String>,
    pub snapshot_nickname: Option<String>,
    pub snapshot_edition: Option<String>,
}

/// auth 文件扫描结果（F-04 导入预览；凭证字段不回传前端）
#[derive(Serialize, Clone)]
pub struct WorkBuddyScanResult {
    pub id: String,
    pub uid: String,
    pub nickname: String,
    pub edition_type: String,
    pub has_access_token: bool,
    pub has_refresh_token: bool,
    pub access_token_expires_at: Option<i64>,
    /// 已在池中（同 token → 同 id）
    pub exists: bool,
    /// 同一账号（同 uid）已在池中：客户端换发 token 后 id 会变，但按账号身份（uid）判定已在池；
    /// 前端可后续据此展示，导入确认后走原位更新，不会重复入池
    pub already_in_pool: bool,
}

// ── M1 环境检测（workbuddy_env_check）──────────────────────────────────────

#[tauri::command(async)]
pub fn workbuddy_env_check(state: State<AppState>) -> WorkBuddyEnvCheck {
    // 复用 F-01 app_locate 四级探测（env.rs 内部实现走注册表/默认路径/进程反查）
    let locate = crate::commands::env::app_locate_inner(&state, "workbuddy");
    let auth_path = auth_file_path_of(&state);
    let snap = fs_utils::read_json::<serde_json::Value>(&snapshot_json_path());
    let has_snap = snap.is_object();
    WorkBuddyEnvCheck {
        installed: locate.exe.is_some(),
        running: is_running(),
        version: locate.version,
        exe: locate.exe,
        auth_file_path: auth_path.display().to_string(),
        auth_file_exists: auth_path.exists(),
        data_dir_exists: wb_data_dir().exists(),
        snapshot_uid: if has_snap { as_str(fs_utils::dig(&snap, &["uid", "accountId"])) } else { None },
        snapshot_nickname: if has_snap { as_str(fs_utils::dig(&snap, &["nickname", "displayName", "name"])) } else { None },
        snapshot_edition: if has_snap { as_str(fs_utils::dig(&snap, &["editionType", "edition"])) } else { None },
    }
}

/// 打开 auth 文件所在目录（资源管理器；人工覆盖路径优先）。目录不存在时报错提示。
#[tauri::command(async)]
pub fn workbuddy_open_auth_dir(state: State<AppState>) -> Result<(), String> {
    let path = auth_file_path_of(&state);
    let dir = path
        .parent()
        .map(|d| d.to_path_buf())
        .ok_or_else(|| format!("无法解析 auth 目录：{}", path.display()))?;
    if !dir.is_dir() {
        return Err(format!("目录不存在：{}", dir.display()));
    }
    std::process::Command::new("explorer")
        .arg(&dir)
        .spawn()
        .map_err(|e| format!("打开目录失败: {e}"))?;
    Ok(())
}

// ── 账号池（F-04）──────────────────────────────────────────────────────────

#[tauri::command]
pub fn workbuddy_accounts_list(state: State<AppState>) -> Result<Vec<WorkBuddyAccountView>, String> {
    accounts_list_inner(&state)
}

fn accounts_list_inner(state: &AppState) -> Result<Vec<WorkBuddyAccountView>, String> {
    let pool = load_pool(state);
    // 在线判定：auth 文件 uid 与账号一致（客户端当前生效登录）
    let auth_uid = {
        let raw = fs_utils::read_json::<serde_json::Value>(&auth_file_path_of(state));
        if raw.is_object() { as_str(fs_utils::dig(&raw, &["uid"])) } else { None }
    };
    let snap_path = state.data_dir.join("data").join("profiles_workbuddy");
    // F2-2：CodeBuddy 快照目录 + 双端"当前账号"标记（桥按端写入 <profiles>/current_account.txt）
    let snap_cb_path = state.data_dir.join("data").join("profiles_codebuddy");
    let cur_wb = read_current_account_marker(&snap_path);
    let cur_cb = read_current_account_marker(&snap_cb_path);
    // F2-5（实测 2026-09-15）：CodeBuddy 端「当前账号」以 live 登录信号为准——
    // genie.userId（客户端随登录/切换改写，池内按 uid 反查账号 id）。桥标记只反映
    // 上次切换目标：客户端内手动重登、切换后客户端未接受恢复（守卫跳过回写）时均失真，
    // 徽标会指向未登录的账号。genie 不可读（客户端未装/storage.json 缺失）回退桥标记。
    // WorkBuddy 端不受影响：is_current 用 auth 文件 uid（本就是 WorkBuddy 登录真源）。
    let cb_live_id = codebuddy_live_uid()
        .and_then(|uid| {
            pool.accounts.iter().find(|a| a.uid == uid).map(|a| a.id.clone())
        })
        .or(cur_cb.clone());
    let store = crate::store::docs::wb_token_store_load(&crate::store::db(&state.data_dir));
    let store_tokens = store.get("tokens").cloned().unwrap_or(serde_json::Value::Null);
    let tag_map = crate::store::docs::account_tags_map(&crate::store::db(&state.data_dir));

    let views = pool
        .accounts
        .iter()
        .map(|a| {
            let has_cred = store_tokens.get(&a.id).is_some()
                || auth_uid.as_deref() == Some(a.uid.as_str());
            let has_snapshot = snap_path.join(&a.id).is_dir();
            // 需重登（登录态已失效）账号不再展示历史余额/到期：该值已无法刷新，
            // 作为实时值展示会误导（用户实测「积分不对」即此陈旧值被冻结展示）。
            // 仅在视图层置空，池内存储值不动，重登成功后由积分刷新链路回填恢复。
            let session_health = crate::tasks::wb_session::health_view(&state.data_dir, &a.id, a.needs_relogin,
                a.access_token_expires_at, a.refresh_token_expires_at);
            let needs_relogin = a.needs_relogin || session_health["status"] == "auth_required";
            let (credits_balance, credits_expire_at) = if needs_relogin {
                (None, None)
            } else {
                (a.credits_balance, a.credits_expire_at)
            };
            WorkBuddyAccountView {
                id: a.id.clone(),
                uid: a.uid.clone(),
                nickname: a.nickname.clone(),
                phone_masked: a.phone_masked.clone(),
                edition_type: a.edition_type.clone(),
                access_token_expires_at: a.access_token_expires_at,
                refresh_token_expires_at: a.refresh_token_expires_at,
                auth_saved_at: a.auth_saved_at,
                needs_relogin,
                relogin_reason: if a.relogin_reason.is_empty() && needs_relogin {
                    session_health["message"].as_str().unwrap_or("需重新登录").into()
                } else { a.relogin_reason.clone() },
                session_health,
                group_id: a.group_id.clone(),
                note: a.note.clone(),
                credits_balance,
                credits_fetched_at: a.credits_fetched_at.clone(),
                credits_expire_at,
                is_current: auth_uid.is_some() && auth_uid == Some(a.uid.clone()),
                has_credential: has_cred,
                has_snapshot: has_snapshot,
                has_snapshot_codebuddy: snap_cb_path.join(&a.id).is_dir(),
                is_current_workbuddy: cur_wb.as_deref() == Some(a.id.as_str()),
                is_current_codebuddy: cb_live_id.as_deref() == Some(a.id.as_str()),
                tags: tag_map.get(&a.id).cloned().unwrap_or_default(),
            }
        })
        .collect();
    Ok(views)
}

#[tauri::command]
pub fn workbuddy_account_save(state: State<AppState>, user_id: String, name: Option<String>, note: Option<String>) -> Result<(), String> {
    // 池读-改-写互斥（wb_pool_lock）：防与其他写点并发时旧副本整文档覆盖丢改动
    let _guard = state.wb_pool_lock.lock().unwrap_or_else(|e| e.into_inner());
    let mut pool = load_pool(&state);
    let acct = pool
        .accounts
        .iter_mut()
        .find(|a| a.id == user_id)
        .ok_or_else(|| format!("账号不存在: {user_id}"))?;
    if let Some(n) = name {
        acct.nickname = n;
    }
    if let Some(n) = note {
        acct.note = n;
    }
    save_pool(&state, &pool)
}

#[tauri::command(async)]
pub fn workbuddy_account_remove(app: AppHandle, state: State<AppState>, user_id: String, delete_snapshot: Option<bool>) -> Result<(), String> {
    // 审查 P1：id 将作为 profiles_workbuddy/<id> 目录名参与 remove_dir_all，
    // 先过字符集白名单，杜绝 `..`/绝对路径注入导致的目录逃逸删除
    fs_utils::ensure_uid_safe(&user_id)?;
    let lock = crate::tasks::wb_common::refresh_lock_for(&user_id);
    let _refresh_guard = lock.lock().unwrap_or_else(|error| error.into_inner());
    let _cross = crate::tasks::wb_common::cross_refresh_lock(&state.data_dir, &user_id)
        .ok_or("账号正在其他进程续期，未执行删除，请稍后重试")?;
    let _pool_guard = state.wb_pool_lock.lock().unwrap_or_else(|error| error.into_inner());
    // 池外孤儿判定：token store 有记录即可清理（池删除接口是孤儿凭证的唯一出口）
    let old_creds = crate::tasks::wb_common::token_record_secure(&state.data_dir, &user_id);
    // 池读-改-写互斥（wb_pool_lock）：仅罩住池段即释放——后续凭证/快照/CLI 清理
    // 不涉池，避免池锁与其他写点锁嵌套成环
    let removed = {
        let _table = crate::tasks::wb_common::WB_TOKEN_STORE_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let mut pool = load_pool(&state);
        let before = pool.accounts.len();
        pool.accounts.retain(|a| a.id != user_id);
        if pool.accounts.len() == before {
            false
        } else {
            save_pool(&state, &pool)?;
            true
        }
    };
    let mut cleanup_errors = Vec::new();
    if let Err(error) = super::cli::clear_deleted_cli_token(&old_creds) { cleanup_errors.push(format!("CLI 凭证: {error}")); }
    // 共享标签绑定清理（审查修复，同 Qoder/Trae 删除路径；成员变化同时影响
    // org 展开索引——账号 group_id 与 account_tags 双维度）
    if let Err(e) = crate::store::docs::account_tags_remove_account(&crate::store::db(&state.data_dir), &user_id) {
        cleanup_errors.push(format!("标签绑定: {e}"));
        fs_utils::app_log(&state.data_dir, &format!("workbuddy 账号 {user_id} 标签绑定清理失败: {e}"));
    }
    crate::api_server::org_index::invalidate(&state.data_dir);
    // 凭证清理（对齐 qoder account_remove I14）：同步清理 wb_tokens 记录 + vault
    // 加密凭证，防悬空凭证残留；清理失败上抛（池移除已生效，仍需热重载剔除）
    {
        if let Err(e) = crate::tasks::wb_common::remove_token(&state, &user_id) {
            fs_utils::app_log(&state.data_dir, &format!("workbuddy 账号 {user_id} 凭证清理失败: {e}"));
            let msg = if removed {
                format!("账号已从池中移除，但凭证清理失败：{e}（再次删除可重试）")
            } else {
                format!("孤儿凭证清理失败：{e}")
            };
            cleanup_errors.push(msg);
        }
    }
    // 入池白名单摘除幽灵条目（账号已出池而 wb_enabled_uids 仍留其 id 会导致启动
    // 日志 whitelist 计数虚高、配置与账号池不一致；无条目时 no-op）
    if let Err(e) = crate::commands::api_server::wb_whitelist_remove(&state.data_dir, &user_id) {
        cleanup_errors.push(format!("网关白名单: {e}"));
        fs_utils::app_log(&state.data_dir, &format!("workbuddy 账号 {user_id} 入池白名单摘除失败: {e}"));
    }
    if delete_snapshot.unwrap_or(false) {
        // F2-2：双端快照槽对称清理——只清 WorkBuddy 会留下 CodeBuddy 孤儿槽，
        // 且账号列表徽标虽随池删除消失，孤儿目录持续占用磁盘。
        if let Err(error) = clean_managed_account_files(&state.data_dir, &user_id) {
            cleanup_errors.push(error);
        }
    }
    // 被删账号是 CodeBuddy CLI 当前号 → 清理轮换状态残留（active_account_id），
    // 避免轮换日志参考失真（下次轮换 current 判定按 settings.json 自愈）
    let mut st = load_cli_rotate_state(&state);
    if st.active_account_id.as_deref() == Some(user_id.as_str()) {
        st.active_account_id = None;
        if let Err(error) = save_cli_rotate_state(&state, &st) { cleanup_errors.push(format!("CLI 状态: {error}")); }
    }
    let db = crate::store::db(&state.data_dir);
    for key in ["workbuddy_credits_cache", "workbuddy_usage_official_cache", "workbuddy_usage_official_all_cache", "workbuddy_usage_sessions_cache", "workbuddy_activity_cache"] {
        if let Err(error) = db.kv_set(key, &serde_json::json!({})) { cleanup_errors.push(format!("缓存 {key}: {error}")); }
    }
    drop(_pool_guard);
    // 账号成员变更联动网关池热重载（删除的账号即时退出调度；服务未运行时 no-op）
    let rt = app.state::<std::sync::Mutex<Option<crate::commands::api_server::ApiServerRuntime>>>();
    crate::commands::api_server::reload_pools_if_running(&state, rt.inner());
    if cleanup_errors.is_empty() { Ok(()) } else { Err(format!("账号已移除，部分清理失败（可再次删除重试）：{}", cleanup_errors.join("；"))) }
}

fn clean_managed_account_files(data_dir: &std::path::Path, id: &str) -> Result<(), String> {
    fs_utils::ensure_uid_safe(id)?;
    if !data_dir.join("data").exists() { return Ok(()); }
    let data_root = data_dir.join("data").canonicalize().map_err(|error| error.to_string())?;
    let mut errors = Vec::new();
    for root in ["profiles_workbuddy", "profiles_codebuddy", "workbuddy_chats", "codebuddy_chats"] {
        let folder = data_root.join(root);
        if !folder.exists() { continue; }
        let resolved = match folder.canonicalize() {
            Ok(resolved) => resolved,
            Err(error) => { errors.push(format!("{root}: {error}")); continue; }
        };
        if !resolved.starts_with(&data_root) || resolved == data_root { errors.push(format!("{root} 路径超出助手数据目录，拒绝清理")); continue; }
        if read_current_account_marker(&resolved).as_deref() == Some(id) {
            if let Err(error) = std::fs::remove_file(resolved.join("current_account.txt")) { errors.push(format!("{root} 当前账号标记: {error}")); }
        }
        for suffix in ["", ".bak", ".bak2"] {
            let path = resolved.join(format!("{id}{suffix}"));
            if path.exists() {
                let target = match path.canonicalize() {
                    Ok(target) => target,
                    Err(error) => { errors.push(format!("{root}/{id}{suffix}: {error}")); continue; }
                };
                if !target.starts_with(&resolved) || target == resolved { errors.push(format!("{root}/{id}{suffix} 路径超出托管目录，拒绝清理")); continue; }
                if let Err(error) = std::fs::remove_dir_all(&path) { errors.push(format!("{root}/{id}{suffix}: {error}")); }
            }
        }
    }
    if errors.is_empty() { Ok(()) } else { Err(errors.join("；")) }
}

/// 扫描本机 auth 文件（F-04 导入预览；不写盘）
#[tauri::command(async)]
pub fn workbuddy_scan_auth_file(state: State<AppState>) -> Result<Option<WorkBuddyScanResult>, String> {
    let path = auth_file_path_of(&state);
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs_utils::read_json::<serde_json::Value>(&path);
    if !raw.is_object() {
        return Err("auth 文件格式无法识别（JSON 解析失败）".into());
    }
    // token 键对齐 tasks 侧 creds_of 超集（wb_common.rs）：含通用 `token`；
    // 失败分类报错（issue #58）：null/空串 → 未登录提示，键不存在 → 结构变更 + 键名诊断
    let access = extract_access_token(&raw)?;
    let id = account_id_of(&access);
    // 四键兼容（account.uid / uid / auth.account.uid / auth.uid）：与切换侧
    // auth_file_uid 同口径。此前只认顶层 uid，客户端把身份放在 account.uid
    // 下时导入即读空——池条目与 token store 双双落空串，签到侧拿不到 uid
    let uid = crate::switcher::authfile::auth_file_uid(&path).unwrap_or_default();
    let pool = load_pool(&state);
    let exists = pool.accounts.iter().any(|a| a.id == id);
    let already_in_pool = !uid.is_empty() && pool.accounts.iter().any(|a| a.uid == uid);
    Ok(Some(WorkBuddyScanResult {
        id,
        uid,
        // 5.8.2+ nickname 同样可能是 $wbEncrypted 包装，走透明解密
        nickname: dig_plain_string(&raw, &["nickname", "displayName", "name"]).unwrap_or_default(),
        edition_type: as_str(fs_utils::dig(&raw, &["editionType", "edition"])).unwrap_or_default(),
        has_access_token: true,
        has_refresh_token: dig_plain_string(&raw, &["refreshToken", "refresh_token"]).is_some(),
        // expires 键对齐 creds_of 超集：as_ts_seconds 自动识别毫秒级（>1e12 折算秒）
        access_token_expires_at: as_ts_seconds(fs_utils::dig(
            &raw,
            &["expiresAtMs", "expires_at_ms", "expiresAt", "expires_in_ms", "accessTokenExpiresAtMs"],
        )),
        exists,
        already_in_pool,
    }))
}

/// 池内账号幂等匹配（账号身份以 uid 为准）：uid 非空时按 uid 匹配，或按 id（token 派生）兜底匹配，
/// 保证「同 uid 换 token」与「同 token（auth 文件缺 uid / uid 漂移）」都不会产生重复条目。
/// pub(super)：OAuth 自动入池（oauth.rs）复用同一匹配语义，避免同 uid 重复入池。
pub(super) fn find_uid_or_id<'a>(pool: &'a mut WbPool, uid: &str, id: &str) -> Option<&'a mut WorkBuddyAccount> {
    pool.accounts
        .iter_mut()
        .find(|a| (!uid.is_empty() && a.uid == uid) || (!id.is_empty() && a.id == id))
}

/// F2-2：读桥按端写入的当前账号标记（<profiles_workbuddy|profiles_codebuddy>/current_account.txt）。
/// 文件不存在/空 → None（该端从未切换过）。
/// 注意剥 BOM：桥（Windows PowerShell 5.1）Set-Content -Encoding UTF8 写出带 BOM，
/// 而 U+FEFF 不属于 Rust trim() 的空白字符，不剥则标记永远匹配失败。
fn read_current_account_marker(dir: &std::path::Path) -> Option<String> {
    let s = std::fs::read_to_string(dir.join("current_account.txt")).ok()?;
    let t = s.trim().trim_start_matches('\u{feff}').trim().to_string();
    if t.is_empty() { None } else { Some(t) }
}

/// F-74：读指定端的当前账号标记（桥按端写入的 `profiles_<app>/current_account.txt`）。
/// 与 `is_current_workbuddy` / `is_current_codebuddy` 徽标同源；文件不存在 → None。
pub fn current_account_marker(state: &AppState, app: &str) -> Option<String> {
    let dir = match app {
        "CodeBuddy" => "profiles_codebuddy",
        _ => "profiles_workbuddy",
    };
    read_current_account_marker(&state.data_dir.join("data").join(dir))
}

/// F1-3（switch.rs 防误覆盖守卫用）：读共享 auth 文件当前 uid → 账号池中反查账号 id。
/// 返回的 id 与桥的 current_account.txt 同命名空间，供 -ExpectedCurrentUid 比对；
/// None = 未登录 / 池中无此账号（调用方 fail-open 传空串，不阻断切换）。
/// 注意：auth 文件是两端共同的"最近写入者"信号——WorkBuddy（auth 驱动）准确；
/// CodeBuddy（vscdb 驱动）被 WorkBuddy 覆盖时反查失败 → None，守卫按既定语义跳过回写。
pub fn pool_account_id_by_auth_uid(state: &AppState) -> Option<String> {
    let raw = fs_utils::read_json::<serde_json::Value>(&auth_file_path_of(state));
    if !raw.is_object() {
        return None;
    }
    let uid = as_str(fs_utils::dig(&raw, &["uid"]))?;
    if uid.is_empty() {
        return None;
    }
    let pool = load_pool(state);
    pool.accounts.iter().find(|a| a.uid == uid).map(|a| a.id.clone())
}

/// 池内按真实账号 uid（auth/genie 体系 uuid）反查账号 id（wb-<hash>）。
/// F2-5（switch.rs 保存/切换守卫用）：把客户端 live 登录 uid 映射到账号池 id。
pub fn pool_account_id_by_uid(state: &AppState, uid: &str) -> Option<String> {
    let uid = uid.trim();
    if uid.is_empty() {
        return None;
    }
    load_pool(state)
        .accounts
        .into_iter()
        .find(|a| a.uid == uid)
        .map(|a| a.id)
}

/// CodeBuddy 客户端 live 登录 uid：`%APPDATA%\CodeBuddy CN\User\globalStorage\storage.json`
/// 的 `genie.userId`。F2-5：这是 CodeBuddy 当前登录的**真源信号**——共享 auth 文件属
/// WorkBuddy（会被其覆盖），不能作为 CodeBuddy 的登录证据；genie.userId 由客户端随
/// 登录/切换改写（实测：恢复某账号快照启动后该值变为该账号 uid）。
/// None = 文件不存在/未登录/解析失败（调用方 fail-open）。
pub fn codebuddy_live_uid() -> Option<String> {
    let appdata = std::env::var("APPDATA").ok()?;
    let p = std::path::PathBuf::from(appdata)
        .join("CodeBuddy CN")
        .join("User")
        .join("globalStorage")
        .join("storage.json");
    let raw = fs_utils::read_json::<serde_json::Value>(&p);
    let uid = as_str(fs_utils::dig(&raw, &["genie.userId"]))?;
    (!uid.is_empty()).then_some(uid.to_string())
}

/// auth 文件导入的池合并输入（从 auth 文件解析出的可覆盖字段集合）
struct AuthMerge {
    /// 本次 token 派生 id（wb-<sha256 前 12 位>）；命中已有条目时不采用，保留旧 id
    id: String,
    uid: String,
    nickname: String,
    edition_type: String,
    access_token_expires_at: Option<i64>,
    refresh_token_expires_at: Option<i64>,
}

/// auth 文件入池合并（纯逻辑，便于单测）：按 uid（优先）/ id（兜底）匹配已有条目 →
/// 原位更新（uid/昵称/版本/过期时间/auth_saved_at 以新值覆盖）并**保留原 id**，返回该 id；
/// 未命中 → 新增（沿用现有 id 生成规则），返回新 id。
/// 保留原 id 的取舍：id 派生自 token，客户端换发 token 后重导入会派生新 id，若改用新 id
/// 会使已有快照（profiles_workbuddy/<id>/）、分组与外部引用悬空，故沿用旧 id 作为账号身份。
fn merge_auth_entry(pool: &mut WbPool, m: AuthMerge) -> String {
    if let Some(a) = find_uid_or_id(pool, &m.uid, &m.id) {
        // 重复导入（同 token，或同账号换发 token）= 原位更新，不产生重复条目
        if !m.uid.is_empty() {
            a.uid = m.uid;
        }
        a.nickname = m.nickname;
        if !m.edition_type.is_empty() {
            a.edition_type = m.edition_type;
        }
        a.access_token_expires_at = m.access_token_expires_at;
        a.refresh_token_expires_at = m.refresh_token_expires_at;
        a.auth_saved_at = Some(chrono::Utc::now().timestamp());
        a.needs_relogin = false;
        a.relogin_reason.clear();
        a.id.clone()
    } else {
        let id = m.id.clone();
        pool.accounts.push(WorkBuddyAccount {
            id: id.clone(),
            uid: m.uid,
            nickname: m.nickname,
            edition_type: m.edition_type,
            access_token_expires_at: m.access_token_expires_at,
            refresh_token_expires_at: m.refresh_token_expires_at,
            auth_saved_at: Some(chrono::Utc::now().timestamp()),
            ..Default::default()
        });
        id
    }
}

/// auth 文件导入入池（F-04）：写账号池 + 工具侧凭证副本（掩码入池、凭证不外泄）。
/// 按 uid 幂等：重复导入（同 token 或同账号换 token）原位更新并保留原 id，不产生重复条目。
#[tauri::command(async)]
pub fn workbuddy_account_import_auth(state: State<AppState>, name: Option<String>) -> Result<WorkBuddyAccountView, String> {
    let path = auth_file_path_of(&state);
    if !path.exists() {
        return Err("未找到 auth 文件，请先在 WorkBuddy 客户端登录".into());
    }
    let raw = fs_utils::read_json::<serde_json::Value>(&path);
    // token 键对齐 creds_of 超集 + 失败分类报错（issue #58，与 scan 同口径）
    let access = extract_access_token(&raw)?;
    // 5.8.2+ refreshToken 为 $wbEncrypted 包装时透明解密
    let refresh = dig_plain_string(&raw, &["refreshToken", "refresh_token"]);
    let id = account_id_of(&access);
    // 四键兼容（与 scan 同口径）：只认顶层 uid 会在身份存于 account.uid 时读空
    let uid = crate::switcher::authfile::auth_file_uid(&path).unwrap_or_default();
    let nickname = name
        .clone()
        .or_else(|| dig_plain_string(&raw, &["nickname", "displayName", "name"]))
        .unwrap_or_else(|| uid.chars().take(8).collect());

    // 池读-改-写互斥（wb_pool_lock）：仅罩住池段；凭证副本落库（token store 锁）在锁外
    let target_id = {
        let _guard = state.wb_pool_lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut pool = load_pool(&state);
        let target_id = merge_auth_entry(
            &mut pool,
            AuthMerge {
                id,
                uid: uid.clone(),
                nickname,
                edition_type: as_str(fs_utils::dig(&raw, &["editionType", "edition"])).unwrap_or_default(),
                // expires 键对齐 creds_of 超集（与 scan 同口径）
                access_token_expires_at: as_ts_seconds(fs_utils::dig(
                    &raw,
                    &["expiresAtMs", "expires_at_ms", "expiresAt", "expires_in_ms", "accessTokenExpiresAtMs"],
                )),
                refresh_token_expires_at: as_ts_seconds(fs_utils::dig(&raw, &["refreshExpiresAtMs", "refreshExpiresAt", "refresh_expires_at"])),
            },
        );
        save_pool(&state, &pool)?;
        target_id
    };

    // 工具侧凭证副本（F-10 双源化）：写回保留的原 id 名下（换 token 重导入时与池条目对齐，
    // 避免 pool 用旧 id / token store 用新 id 导致凭证与账号脱钩）
    let creds = serde_json::json!({
        "access_token": access,
        "refresh_token": refresh,
        // 审查 P2：到期时间兼容整数毫秒/秒与 RFC3339 字符串（as_ts_seconds 归一为秒，再折算毫秒存储）
        "expires_at_ms": as_ts_seconds(fs_utils::dig(
            &raw,
            &["expiresAtMs", "expires_at_ms", "expiresAt", "expires_in_ms", "accessTokenExpiresAtMs"],
        ))
        .map(|s| s * 1000),
        "refresh_expires_at_ms": as_ts_seconds(fs_utils::dig(
            &raw, &["refreshExpiresAtMs", "refreshExpiresAt", "refresh_expires_at"],
        )).map(|s| s * 1000),
        "uid": uid,
        "domain": as_str(fs_utils::dig(&raw, &["domain"])),
    });
    upsert_token_store(&state, &target_id, &creds)?;

    // 返回合并视图（简化：直接重查）
    let views = accounts_list_inner(&state)?;
    views
        .into_iter()
        .find(|v| v.id == target_id)
        .ok_or_else(|| "导入后回读失败".into())
}

/// 从快照槽读回 uid（两级真源，均离线且按账号槽隔离）。
///
/// ① `<槽>/meta.json` 的 `uid`——保存快照时由四键读取写入，直接可用；
/// ② `<槽>/auth/workbuddy-desktop.info` 走四键 `auth_file_uid`——meta 缺失
///    （合成快照写失败等）时回退到原始登录态文件。
///
/// 刻意不读共享 auth 文件：它只反映**当前登录**的那一个账号，拿它回填会把
/// A 的身份写到 B 名下。
fn uid_from_snapshot_slot(profiles_dir: &std::path::Path, slot: &str) -> Option<String> {
    let meta = fs_utils::read_json::<serde_json::Value>(&profiles_dir.join(slot).join("meta.json"));
    let from_meta = meta
        .get("uid")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    if from_meta.is_some() {
        return from_meta;
    }
    crate::switcher::authfile::auth_file_uid(&crate::switcher::authfile::slot_auth_file(
        profiles_dir,
        slot,
    ))
}

/// 补齐账号池里缺失的 uid（离线，无需逐个重新登录）。
///
/// uid 是签到侧「活跃上报/任务判据」的归属凭据（`creds.uid` 进事件
/// `userId`）；缺失时这些上报无法计分，Active/成长任务全部空转。补齐源是本机
/// 快照槽——每个槽存有该账号自己的登录态，逐个切号重登代价远高于此。
///
/// 只填空、不覆盖：已在池里的 uid 一律保留（避免把别名类字段写坏）。
#[tauri::command(async)]
pub fn workbuddy_uid_backfill(
    state: State<AppState>,
) -> Result<(usize, Vec<String>), String> {
    let profiles_dir = state.data_dir.join("data").join("profiles_workbuddy");
    // 池读-改-写互斥（wb_pool_lock）：覆盖整段读-改-写
    let _guard = state.wb_pool_lock.lock().unwrap_or_else(|e| e.into_inner());
    let mut pool = load_pool(&state);
    let mut filled = 0usize;
    let mut missing: Vec<String> = Vec::new();
    for a in pool.accounts.iter_mut() {
        if !a.uid.trim().is_empty() {
            continue;
        }
        // id 参与路径拼接，先过白名单（对齐 account_remove 的目录逃逸防护）
        if fs_utils::ensure_uid_safe(&a.id).is_err() {
            missing.push(a.id.clone());
            continue;
        }
        let Some(uid) = uid_from_snapshot_slot(&profiles_dir, &a.id) else {
            missing.push(a.id.clone());
            continue;
        };
        a.uid = uid.clone();
        // 同步进 token store：签到侧 creds.uid 取自这里，只补池条目不生效
        let _ = upsert_token_store(&state, &a.id, &serde_json::json!({ "uid": uid }));
        filled += 1;
    }
    save_pool(&state, &pool)?;
    drop(_guard);
    fs_utils::app_log(
        &state.data_dir,
        &format!(
            "workbuddy: uid 回填 补齐 {filled} / 仍缺 {}",
            missing.len()
        ),
    );
    Ok((filled, missing))
}

// ── M3 凭证续期（F-09，Rust 侧手动触发；schtasks 每周兜底走 python --renew-only）──

/// 刷新单账号凭证：读 token store（双源副本）→ POST plugin refresh → 回写。
/// 客户端运行中跳过 auth 文件写入（只更新工具侧副本，F-10 保证谁新用谁）。
/// force=false（默认）时惰性：accessToken 剩余有效期充足则跳过续期（减少无谓
/// refresh 调用——refresh 端点可能轮换 refreshToken，减少轮换面）；
/// force=true 强制续期（手动按钮使用）。
#[tauri::command(async)]
pub fn workbuddy_refresh_token(
    state: State<AppState>,
    user_id: String,
    force: Option<bool>,
) -> Result<String, String> {
    let acct = load_pool(&state).accounts.into_iter().find(|account| account.id == user_id)
        .ok_or_else(|| format!("账号不存在: {user_id}"))?;
    let creds = crate::tasks::wb_common::effective_creds(&state, &user_id, &acct.uid);
    let agent = crate::tasks::http_agent(30);
    let mode = if force.unwrap_or(false) { crate::tasks::wb_session::RefreshMode::Force }
        else { crate::tasks::wb_session::RefreshMode::Due(24) };
    let (new, refreshed) = crate::tasks::wb_session::refresh(&state.data_dir, &user_id, &creds, mode, &agent)
        .map_err(|error| error.message())?;
    if !refreshed { return Ok("凭证有效，无需续期（未联网验证）".into()); }
    {
        let _pool_guard = state.wb_pool_lock.lock().unwrap_or_else(|error| error.into_inner());
        let mut pool = load_pool(&state);
        if let Some(account) = pool.accounts.iter_mut().find(|account| account.id == user_id) {
            account.access_token_expires_at = new.expires_at_ms.map(|expiry| expiry / 1000);
            account.refresh_token_expires_at = new.refresh_expires_at_ms.map(|expiry| expiry / 1000);
            account.needs_relogin = false;
            account.relogin_reason.clear();
            save_pool(&state, &pool)?;
        }
    }
    Ok("凭证已续期".into())
}

/// 惰性续期判定（纯函数化便于单测）：true = 可跳过（剩余有效期充足）。
/// 跳过 = accessToken 剩余 > 24h——refresh 端点可能轮换 refreshToken，减少无谓轮换。
/// expires_at 缺失 → false（放行刷新，顺带拿到准确的 expiresIn）。
fn lazy_renew_skippable(access_token_expires_at: Option<i64>, now_ts: i64) -> bool {
    const WB_LAZY_RENEW_MIN_SECS: i64 = 24 * 3600;
    match access_token_expires_at {
        None => false,
        Some(exp) => exp - now_ts > WB_LAZY_RENEW_MIN_SECS,
    }
}

// ── API 网关 WB 上游取号（T2.1）───────────────────────────────────────────

/// 汇总 WB 上游账号：账号池启用账号 + token store 凭证 → WbSyncAccount。
/// 区域判定（§5.2）：domain 含 `.workbuddy.ai` → Global（chat 全走 www.workbuddy.ai）。
/// 仅纳入有工具侧凭证副本的账号（auth 文件为只读态，不在此兜底）。
pub(crate) fn wb_upstream_accounts(state: &AppState) -> Vec<crate::api_server::pool::WbSyncAccount> {
    let pool = load_pool(state);
    let mut out = Vec::new();
    for a in &pool.accounts {
        let creds = crate::tasks::wb_common::stored_creds(&state.data_dir, &a.id);
        let token = creds.access_token;
        if token.is_empty() {
            continue;
        }
        let domain = creds.domain;
        let eid = creds.enterprise_id;
        out.push(crate::api_server::pool::WbSyncAccount {
            uid: a.id.clone(),
            name: if a.nickname.is_empty() { a.id.clone() } else { a.nickname.clone() },
            token,
            domain: domain.clone(),
            enterprise_id: eid,
            global_region: domain.contains(".workbuddy.ai"),
            credits: a.credits_balance,
            credits_expire_at: a.credits_expire_at,
            // 条目级快照采集时间（issue #91 缓存保鲜）：宽容解析 → unix 秒（B1），
            // 池用于 reserve 过滤新鲜度判断（陈旧快照 fail-open）。
            // 此前裸用 parse_from_rfc3339，对历史无偏移裸串恒失败 → 恒 None
            credits_fetched_at: a
                .credits_fetched_at
                .as_deref()
                .and_then(crate::fs_utils::parse_ts_loose),
            needs_relogin: a.needs_relogin || crate::tasks::wb_session::health(&state.data_dir, &a.id).status == "auth_required",
            group_id: a.group_id.clone(),
        });
    }
    out
}

// ── 账号库导入导出扩展（F-46 扩展，批次3 T3.6）────────────────────────────

/// 导出账号池（F-46 扩展）：元数据必含；include_credentials=true 时附工具侧凭证副本
///（迁移场景用；导出文件等同密码，由调用方提示）。与 Trae accounts_export_raw 同语义。
#[tauri::command(async)]
pub fn workbuddy_accounts_export(state: State<AppState>, include_credentials: Option<bool>) -> Result<serde_json::Value, String> {
    let pool = load_pool(&state);
    let include_cred = include_credentials.unwrap_or(false);
    let store: serde_json::Value = if include_cred {
        crate::tasks::wb_common::load_token_store(&state)
    } else {
        serde_json::json!({})
    };
    let tokens = store.get("tokens").cloned().unwrap_or(serde_json::Value::Null);
    let tag_map = crate::store::docs::account_tags_map(&crate::store::db(&state.data_dir));
    let accounts: Vec<serde_json::Value> = pool
        .accounts
        .iter()
        .map(|a| {
            let mut v = serde_json::json!({
                "id": a.id,
                "uid": a.uid,
                "nickname": a.nickname,
                "phone_masked": a.phone_masked,
                "edition_type": a.edition_type,
                "access_token_expires_at": a.access_token_expires_at,
                "refresh_token_expires_at": a.refresh_token_expires_at,
                "group_id": a.group_id,
                "note": a.note,
                // 共享标签绑定（tag id 数组；标签定义属全局数据不随文件携带）
                "tags": tag_map.get(&a.id).cloned().unwrap_or_default(),
            });
            if include_cred {
                v["credential"] = tokens.get(&a.id).cloned().unwrap_or(serde_json::Value::Null);
            }
            v
        })
        .collect();
    Ok(serde_json::json!({
        "kind": "aiwork-workbuddy-pool",
        "version": 1,
        "exported_at": fs_utils::now_iso(),
        "include_credentials": include_cred,
        "accounts": accounts,
    }))
}

/// 账号库导入（F-46 扩展）：解析导出文件 → 逐账号按 uid 幂等入池
///（已存在（同 uid，可能换 token/异机 id）原位更新并保留原 id，不重复入池）+ 凭证回写 token store。
/// P2：tags 字段恢复共享标签绑定（仅本机已存在的 tag id，未知 id 忽略）。
#[tauri::command(async)]
pub fn workbuddy_accounts_import(
    state: State<AppState>,
    runtime: State<'_, std::sync::Mutex<Option<crate::commands::api_server::ApiServerRuntime>>>,
    payload: serde_json::Value,
) -> Result<serde_json::Value, String> {
    if payload.get("kind").and_then(|v| v.as_str()) != Some("aiwork-workbuddy-pool") {
        return Err("文件格式无法识别（缺少 aiwork-workbuddy-pool 标记）".into());
    }
    let accounts = payload
        .get("accounts")
        .and_then(|v| v.as_array())
        .ok_or("导出文件缺少 accounts 数组")?;
    let mut added = 0usize;
    let mut updated = 0usize;
    let mut with_cred = 0usize;
    // 审查 P1：被拒绝条目逐条标注原因（id 非法/缺失的不入池，避免注入任意目录名）
    let mut rejected: Vec<serde_json::Value> = Vec::new();
    // 读-改-写互斥：罩住 load→逐条合并→save 全段，防并发写方用旧副本整文档覆盖丢账号；
    // 循环内 upsert_token_store 与池锁无嵌套，持锁调用无死锁（同 qoder data_io 先例）
    let _guard = state.wb_pool_lock.lock().unwrap_or_else(|e| e.into_inner());
    let mut pool = load_pool(&state);
    // 幽灵分组防御（对齐 qoder 导入侧）：预载共享分组定义，新增条目的 group_id
    // 指向不存在分组（异机导入未随载荷携带定义/本地已删分组）时回落「未分组」——
    // 聚合落空的幽灵引用不如显式未分组。与 tag_defs 同一 defs 短锁读取：
    // 池锁不隔离 defs 写方（defs 写方只持 group_defs_lock），须靠 defs 短锁
    // 保证与并发标签/分组增删的 RMW 快照一致（与 qoder 侧同构）
    let (tag_defs, valid_group_ids) = {
        let _defs = state
            .group_defs_lock
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let tag_defs =
            crate::store::docs::tags_load(&crate::store::db(&state.data_dir));
        let valid_group_ids: std::collections::HashSet<String> =
            crate::store::docs::group_defs_load(&crate::store::db(&state.data_dir))
                .into_iter()
                .map(|g| g.id)
                .collect();
        (tag_defs, valid_group_ids)
    };
    let mut tag_bindings: Vec<(String, Vec<String>)> = Vec::new();
    let mut tags_ignored = 0usize;
    // 凭证先收集、save_pool 成功后统一落库逐条聚合（对齐 qoder 先例）：原实现
    // 循环内 upsert_token_store 带 ?，某条失败整次导入中断且无明细，账号半成品
    let mut cred_writes: Vec<(String, serde_json::Value)> = Vec::new();
    for a in accounts {
        let id = a.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        if id.is_empty() {
            rejected.push(serde_json::json!({ "id": id, "reason": "缺少 id" }));
            continue;
        }
        // id 安全校验（审查 P1）：字符集白名单，杜绝 `..`/绝对路径/分隔符注入
        if let Err(e) = fs_utils::ensure_uid_safe(&id) {
            rejected.push(serde_json::json!({ "id": id, "reason": e }));
            continue;
        }
        // id 规则校验（审查 P1）：须与池内生成规则一致——wb-<sha256 前 12 位十六进制小写>
        let hex = id.strip_prefix("wb-").unwrap_or("");
        let id_valid = hex.len() == 12 && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
        if !id_valid {
            rejected.push(serde_json::json!({ "id": id, "reason": "id 不符合 wb-<12位十六进制小写> 规则" }));
            continue;
        }
        // 按 uid 幂等（uid 优先，id 兜底，同 find_uid_or_id）：命中 → 原位更新并保留原 id
        // （换 token / 异机 id 的同账号不再产生重复条目，快照与分组引用不悬空）。
        // group_id/note/phone_masked 属本地/用户可编辑数据，更新时不覆盖，仅新增时采用。
        let entry_uid = a.get("uid").and_then(|v| v.as_str()).unwrap_or_default().to_string();
        if let Some(x) = find_uid_or_id(&mut pool, &entry_uid, &id) {
            if !entry_uid.is_empty() {
                x.uid = entry_uid;
            }
            let nickname = a.get("nickname").and_then(|v| v.as_str()).unwrap_or_default().to_string();
            if !nickname.is_empty() {
                x.nickname = nickname;
            }
            let edition = a.get("edition_type").and_then(|v| v.as_str()).unwrap_or_default().to_string();
            if !edition.is_empty() {
                x.edition_type = edition;
            }
            if let Some(v) = a.get("access_token_expires_at").and_then(|v| v.as_i64()) {
                x.access_token_expires_at = Some(v);
            }
            if let Some(v) = a.get("refresh_token_expires_at").and_then(|v| v.as_i64()) {
                x.refresh_token_expires_at = Some(v);
            }
            x.auth_saved_at = Some(chrono::Utc::now().timestamp());
            updated += 1;
            crate::commands::tags::collect_import_tags(a, &x.id, &tag_defs, &mut tag_bindings, &mut tags_ignored);
            // 凭证副本收集（导出时含凭证才有效）：写回保留的原 id 名下，与池条目对齐
            if let Some(cred) = a.get("credential").filter(|c| c.is_object()) {
                cred_writes.push((x.id.clone(), cred.clone()));
            }
            continue;
        }
        // 新增条目 group_id 指向不存在分组 → 回落「未分组」（空串）
        let entry_gid = a.get("group_id").and_then(|v| v.as_str()).unwrap_or_default().to_string();
        let entry_gid = if entry_gid.is_empty() || valid_group_ids.contains(&entry_gid) {
            entry_gid
        } else {
            String::new()
        };
        pool.accounts.push(WorkBuddyAccount {
            id: id.clone(),
            uid: entry_uid,
            nickname: a.get("nickname").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
            phone_masked: a.get("phone_masked").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
            edition_type: a.get("edition_type").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
            access_token_expires_at: a.get("access_token_expires_at").and_then(|v| v.as_i64()),
            refresh_token_expires_at: a.get("refresh_token_expires_at").and_then(|v| v.as_i64()),
            group_id: entry_gid,
            note: a.get("note").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
            auth_saved_at: Some(chrono::Utc::now().timestamp()),
            ..Default::default()
        });
        added += 1;
        crate::commands::tags::collect_import_tags(a, &id, &tag_defs, &mut tag_bindings, &mut tags_ignored);
        // 凭证副本收集（导出时含凭证才有效）
        if let Some(cred) = a.get("credential").filter(|c| c.is_object()) {
            cred_writes.push((id.clone(), cred.clone()));
        }
    }
    save_pool(&state, &pool)?;
    // 标签绑定落 account_tags 表（uid 键跨池通用；池锁仍持有——账号与标签同批恢复）：
    // 走 account_tags_set_inner（持 group_defs_lock 校验标签存在性）而非盲写——
    // 收集→落库窗口内被并发删除的标签按「未知标签忽略」记数 + 日志，不再写入
    // 指向已删标签的幽灵绑定。持池锁期间取 defs 锁（锁序池锁→defs锁）
    let mut tags_restored = 0usize;
    for (uid, ids) in &tag_bindings {
        match crate::commands::tags::account_tags_set_inner(&state, uid, ids) {
            Ok(()) => tags_restored += ids.len(),
            Err(e) if e.starts_with("标签不存在") => {
                tags_ignored += ids.len();
                fs_utils::app_log(
                    &state.data_dir,
                    &format!("workbuddy 导入: 标签绑定跳过（落库时标签已被并发删除，按未知标签忽略）uid={uid}: {e}"),
                );
            }
            Err(e) => rejected.push(serde_json::json!({
                "id": uid,
                "reason": format!("账号已入池，但标签绑定落库失败：{e}（重新导入可补齐）"),
            })),
        }
    }
    // 凭证统一落库逐条聚合：单条失败不中断（账号已入池，重新导入可补齐）
    for (final_id, rec) in cred_writes {
        match upsert_token_store(&state, &final_id, &rec) {
            Ok(()) => {
                with_cred += 1;
                if !crate::tasks::wb_common::creds_of(&rec).access_token.is_empty() {
                    if let Some(account) = pool.accounts.iter_mut().find(|account| account.id == final_id) {
                        account.needs_relogin = false;
                        account.relogin_reason.clear();
                    }
                }
            }
            Err(e) => rejected.push(serde_json::json!({
                "id": final_id,
                "reason": format!("账号已入池，但凭证落库失败：{e}（重新导入可补齐）"),
            })),
        }
    }
    save_pool(&state, &pool)?;
    fs_utils::app_log(
        &state.data_dir,
        &format!("workbuddy: 账号库导入 新增 {added} / 更新 {updated} / 带凭证 {with_cred} / 拒绝 {} / 标签绑定恢复 {tags_restored}（未知标签忽略 {tags_ignored}）", rejected.len()),
    );
    crate::api_server::org_index::invalidate(&state.data_dir);
    // 网关池热重载（对齐 Trae accounts_import / qoder data_io 口径）：新增/更新
    // 账号凭证后不重载，运行中的网关看不到新凭证。先释放池锁（锁序纪律）
    drop(_guard);
    crate::commands::api_server::reload_pools_if_running(&state, runtime.inner());
    // skipped 保留为旧版前端兼容字段：旧语义「同 id 跳过」已改为按 uid 原位合并（updated）
    Ok(serde_json::json!({ "added": added, "updated": updated, "skipped": 0, "with_credentials": with_cred, "rejected": rejected, "tags_restored": tags_restored, "tags_ignored": tags_ignored }))
}

#[cfg(test)]
mod tests {
    use super::{
        accounts_list_inner, as_str, as_ts_seconds, extract_access_token, lazy_renew_skippable,
        merge_auth_entry, save_pool, uid_from_snapshot_slot, AuthMerge, WorkBuddyAccount, WbPool,
    };
    // auth_key_names 仅测试直用（非测试代码已改经 extract_access_token 间接引用）
    use super::super::common::auth_key_names;
    use crate::fs_utils;
    use crate::state::AppState;
    use std::sync::{Arc, Mutex};

    fn entry(id: &str, uid: &str, nickname: &str) -> WorkBuddyAccount {
        WorkBuddyAccount {
            id: id.into(),
            uid: uid.into(),
            nickname: nickname.into(),
            ..Default::default()
        }
    }

    /// 临时 AppState（独立 SQLite/KV；调用方清理目录）
    fn tmp_state(tag: &str) -> AppState {
        let d = std::env::temp_dir().join(format!("wb_accounts_view_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("data")).unwrap();
        std::fs::create_dir_all(d.join("conf")).unwrap();
        std::fs::create_dir_all(d.join("logs")).unwrap();
        AppState {
            data_dir: d,
            jwt_refresh_lock: Arc::new(Mutex::new(())),
            qoder_pool_lock: Arc::new(Mutex::new(())),
            wb_pool_lock: Arc::new(Mutex::new(())),
            doubao_pool_lock: Arc::new(Mutex::new(())),
            group_defs_lock: Arc::new(Mutex::new(())),
        }
    }

    #[test]
    fn managed_cleanup_includes_rollbacks_chats_and_matching_marker_only() {
        let state = tmp_state("cleanup");
        for root in ["profiles_workbuddy", "profiles_codebuddy", "workbuddy_chats", "codebuddy_chats"] {
            for suffix in ["", ".bak", ".bak2"] {
                std::fs::create_dir_all(state.data_dir.join("data").join(root).join(format!("wb-test{suffix}"))).unwrap();
            }
            std::fs::create_dir_all(state.data_dir.join("data").join(root).join("wb-other")).unwrap();
        }
        let profiles = state.data_dir.join("data").join("profiles_workbuddy");
        std::fs::write(profiles.join("current_account.txt"), "wb-test").unwrap();
        let other_profiles = state.data_dir.join("data").join("profiles_codebuddy");
        std::fs::write(other_profiles.join("current_account.txt"), "wb-other").unwrap();
        super::clean_managed_account_files(&state.data_dir, "wb-test").unwrap();
        super::clean_managed_account_files(&state.data_dir, "wb-test").unwrap();
        assert!(!profiles.join("wb-test.bak2").exists());
        assert!(!profiles.join("current_account.txt").exists());
        assert!(other_profiles.join("current_account.txt").exists());
        assert!(profiles.join("wb-other").exists());
        assert!(super::clean_managed_account_files(&state.data_dir, "../escape").is_err());
        let _ = std::fs::remove_dir_all(&state.data_dir);
    }

    #[test]
    fn upstream_rejects_conflicting_identity_and_deleted_credentials() {
        let state = tmp_state("upstream_identity");
        save_pool(&state, &WbPool { accounts: vec![entry("wb-test", "uid-a", "A")] }).unwrap();
        let db = crate::store::db(&state.data_dir);
        crate::store::docs::wb_token_store_upsert(&db, "wb-test", &serde_json::json!({"uid": "uid-b", "access_token": "synthetic-b"})).unwrap();
        assert!(super::wb_upstream_accounts(&state).is_empty());
        assert!(crate::tasks::wb_common::stored_creds(&state.data_dir, "wb-test").access_token.is_empty());
        crate::store::docs::wb_token_store_upsert(&db, "wb-test", &serde_json::json!({"access_token": "synthetic-a"})).unwrap();
        assert_eq!(super::wb_upstream_accounts(&state)[0].token, "synthetic-a");
        assert_eq!(crate::tasks::wb_common::stored_creds(&state.data_dir, "wb-test").uid, "uid-a");
        save_pool(&state, &WbPool::default()).unwrap();
        assert!(crate::tasks::wb_common::stored_creds(&state.data_dir, "wb-test").access_token.is_empty());
        let _ = std::fs::remove_dir_all(&state.data_dir);
    }

    #[test]
    fn deleted_account_rejects_late_refresh_and_import_writes() {
        let state = tmp_state("deleted_write");
        let creds = crate::tasks::wb_common::Creds { access_token: "synthetic-late".into(), ..Default::default() };
        assert!(crate::tasks::wb_common::save_token_store(&state, "gone", &creds).is_err());
        assert!(super::upsert_token_store(&state, "gone", &serde_json::json!({"access_token": "synthetic-late"})).is_err());
        assert!(crate::store::db(&state.data_dir).row_get("wb_tokens", "gone").unwrap().is_none());
        let _ = std::fs::remove_dir_all(&state.data_dir);
    }

    /// 列表视图必须透出积分到期字段：账号列表「积分到期」列依赖该字段，
    /// 回归防止 WorkBuddyAccountView 再次漏掉 credits_expire_at。
    #[test]
    fn accounts_view_exposes_credits_expire_at() {
        let state = tmp_state("credits_expire");
        let mut acc = entry("wb-expire000001", "u-expire", "积分到期");
        acc.credits_balance = Some(2100.0);
        acc.credits_expire_at = Some(1_794_921_410); // 2026-11-17 21:16:50（UTC+8）
        save_pool(&state, &WbPool { accounts: vec![acc] }).unwrap();

        let views = accounts_list_inner(&state).unwrap();
        assert_eq!(views.len(), 1);
        assert_eq!(views[0].credits_balance, Some(2100.0));
        assert_eq!(views[0].credits_expire_at, Some(1_794_921_410));
        let json = serde_json::to_value(&views[0]).unwrap();
        assert_eq!(
            json.get("credits_expire_at").and_then(|v| v.as_i64()),
            Some(1_794_921_410)
        );
        let _ = std::fs::remove_dir_all(&state.data_dir);
    }

    /// 需重登（登录态失效）账号不展示历史余额/到期：陈旧值无法刷新，作为实时值
    /// 展示会误导（用户实测「积分不对」根因）。仅视图层置空，池内存储值保留，
    /// 重登成功后由积分刷新链路回填恢复。
    #[test]
    fn accounts_view_blanks_stale_credits_when_needs_relogin() {
        let state = tmp_state("needs_relogin_blank");
        let mut acc = entry("wb-stale0000001", "u-stale", "需重登");
        acc.credits_balance = Some(2492.67);
        acc.credits_expire_at = Some(1_794_793_707);
        acc.needs_relogin = true;
        save_pool(&state, &WbPool { accounts: vec![acc] }).unwrap();

        let views = accounts_list_inner(&state).unwrap();
        assert_eq!(views.len(), 1);
        assert_eq!(views[0].credits_balance, None);
        assert_eq!(views[0].credits_expire_at, None);
        // 池内存储值不被清空（重登后仍可恢复）
        assert_eq!(super::load_pool(&state).accounts[0].credits_balance, Some(2492.67));
        let _ = std::fs::remove_dir_all(&state.data_dir);
    }

    /// 建一个临时快照槽根目录（调用方负责清理）
    fn tmp_profiles(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("wb_uid_bf_{tag}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// 快照槽取 uid：meta.json 优先
    #[test]
    fn slot_uid_prefers_meta() {
        let p = tmp_profiles("meta");
        let slot = p.join("wb-1");
        std::fs::create_dir_all(slot.join("auth")).unwrap();
        std::fs::write(
            slot.join("meta.json"),
            r#"{"layout":"authfile","uid":"from-meta"}"#,
        );
        // auth 文件给的是另一个值——不应被采用
        std::fs::write(
            slot.join("auth").join("workbuddy-desktop.info"),
            r#"{"uid":"from-authfile"}"#,
        )
        .unwrap();
        assert_eq!(
            uid_from_snapshot_slot(&p, "wb-1").as_deref(),
            Some("from-meta")
        );
        let _ = std::fs::remove_dir_all(&p);
    }

    /// meta 缺失/空值时回退到槽内 auth 文件，且走四键兼容
    /// （身份存于 account.uid 也能读出——这是顶层 uid 读不到的形态）
    #[test]
    fn slot_uid_falls_back_to_authfile_with_nested_key() {
        let p = tmp_profiles("fallback");
        let slot = p.join("wb-2");
        std::fs::create_dir_all(slot.join("auth")).unwrap();
        std::fs::write(
            slot.join("auth").join("workbuddy-desktop.info"),
            r#"{"account":{"uid":"nested-uid"},"nickname":"n"}"#,
        )
        .unwrap();
        // meta 存在但 uid 为空 → 必须回退，不能拿空串当结果
        std::fs::write(slot.join("meta.json"), r#"{"uid":""}"#).unwrap();
        assert_eq!(
            uid_from_snapshot_slot(&p, "wb-2").as_deref(),
            Some("nested-uid")
        );
        let _ = std::fs::remove_dir_all(&p);
    }

    /// 两级真源都没有 → None（调用方据此把该账号列入「仍需快照」）
    #[test]
    fn slot_uid_none_when_slot_missing() {
        let p = tmp_profiles("empty");
        assert_eq!(uid_from_snapshot_slot(&p, "wb-3"), None);
        let _ = std::fs::remove_dir_all(&p);
    }

    fn auth_merge(id: &str, uid: &str, exp: Option<i64>) -> AuthMerge {
        AuthMerge {
            id: id.into(),
            uid: uid.into(),
            nickname: "new-nick".into(),
            edition_type: "pro".into(),
            access_token_expires_at: exp,
            refresh_token_expires_at: None,
        }
    }

    /// 重复导入同 token：池大小不变、id 不变、字段以新值覆盖
    #[test]
    fn merge_same_token_keeps_pool_size_and_id() {
        let mut pool = WbPool { accounts: vec![entry("wb-aaaaaaaaaaaa", "u1", "old")] };
        let target_id = merge_auth_entry(&mut pool, auth_merge("wb-aaaaaaaaaaaa", "u1", Some(123)));
        assert_eq!(pool.accounts.len(), 1);
        assert_eq!(target_id, "wb-aaaaaaaaaaaa");
        assert_eq!(pool.accounts[0].id, "wb-aaaaaaaaaaaa");
        assert_eq!(pool.accounts[0].nickname, "new-nick");
        assert_eq!(pool.accounts[0].edition_type, "pro");
        assert_eq!(pool.accounts[0].access_token_expires_at, Some(123));
        assert!(!pool.accounts[0].needs_relogin);
    }

    /// 同 uid 不同 token（客户端换发 token）：池大小不变、凭证（过期时间）更新、id 保持旧值
    #[test]
    fn merge_same_uid_new_token_updates_and_keeps_old_id() {
        let mut pool = WbPool { accounts: vec![entry("wb-oldoldoldold", "u1", "old")] };
        pool.accounts[0].needs_relogin = true;
        pool.accounts[0].relogin_reason = "old failure".into();
        let target_id = merge_auth_entry(&mut pool, auth_merge("wb-newnewnewnew", "u1", Some(456)));
        assert_eq!(pool.accounts.len(), 1);
        assert_eq!(pool.accounts[0].id, "wb-oldoldoldold"); // 保留旧 id：快照/分组引用不悬空
        assert_eq!(target_id, "wb-oldoldoldold"); // 凭证副本须写回旧 id 名下
        assert_eq!(pool.accounts[0].uid, "u1");
        assert_eq!(pool.accounts[0].access_token_expires_at, Some(456));
        assert!(!pool.accounts[0].needs_relogin);
        assert!(pool.accounts[0].relogin_reason.is_empty());
    }

    /// 新 uid（新账号）：新增条目，沿用传入 id
    #[test]
    fn merge_new_uid_appends_entry() {
        let mut pool = WbPool { accounts: vec![entry("wb-aaaaaaaaaaaa", "u1", "a")] };
        let target_id = merge_auth_entry(&mut pool, auth_merge("wb-bbbbbbbbbbbb", "u2", None));
        assert_eq!(pool.accounts.len(), 2);
        assert_eq!(target_id, "wb-bbbbbbbbbbbb");
        assert_eq!(pool.accounts[1].id, "wb-bbbbbbbbbbbb");
        assert_eq!(pool.accounts[1].uid, "u2");
        assert_eq!(pool.accounts[1].nickname, "new-nick");
    }

    /// 同 id 但 uid 漂移（池内旧 uid 与 auth 文件不一致）：按 id 兜底匹配，不产生第二条，
    /// 且 uid 以 auth 文件新值修正
    #[test]
    fn merge_same_id_drifted_uid_does_not_duplicate() {
        let mut pool = WbPool { accounts: vec![entry("wb-cccccccccccc", "u-old", "old")] };
        let target_id = merge_auth_entry(&mut pool, auth_merge("wb-cccccccccccc", "u-new", Some(789)));
        assert_eq!(pool.accounts.len(), 1);
        assert_eq!(target_id, "wb-cccccccccccc");
        assert_eq!(pool.accounts[0].uid, "u-new");
    }

    /// auth 文件缺 uid（uid 为空）：按 id 匹配原位更新，且不以空 uid 回填覆盖旧 uid
    #[test]
    fn merge_empty_uid_falls_back_to_id_and_keeps_old_uid() {
        let mut pool = WbPool { accounts: vec![entry("wb-dddddddddddd", "u1", "old")] };
        let target_id = merge_auth_entry(&mut pool, auth_merge("wb-dddddddddddd", "", None));
        assert_eq!(pool.accounts.len(), 1);
        assert_eq!(target_id, "wb-dddddddddddd");
        assert_eq!(pool.accounts[0].uid, "u1");
    }

    // ── 惰性续期门（lazy_renew_skippable）边界测试 ──────────────────────────
    // 语义：true = 可跳过续期（access_token 剩余 >24h）；false = 需要续期。
    // 与 Trae 的 lazy_refresh_needed 方向相反，注意断言极性。

    /// 基准时刻（固定 now，不依赖真实时钟）
    const WB_T0: i64 = 1_700_000_000;
    /// 阈值 24h（秒），须与 WB_LAZY_RENEW_MIN_SECS 一致
    const WB_H24: i64 = 24 * 3600;

    /// expires_at 缺失（旧版 auth 文件/字段解析失败）：无法判断时效 → 保守放行续期
    #[test]
    fn lazy_renew_none_expires_at_conservatively_renews() {
        assert!(!lazy_renew_skippable(None, WB_T0));
    }

    /// 剩余 25h：跳过 /access_token/refresh，减少一次上游取号
    #[test]
    fn lazy_renew_over_24h_skips() {
        assert!(lazy_renew_skippable(Some(WB_T0 + WB_H24 + 3600), WB_T0));
    }

    /// 剩余恰好 24h：严格大于（> 24h）→ 不算充足 → 续期
    #[test]
    fn lazy_renew_exactly_24h_boundary_renews() {
        assert!(!lazy_renew_skippable(Some(WB_T0 + WB_H24), WB_T0));
    }

    /// 剩余不足 24h：续期
    #[test]
    fn lazy_renew_under_24h_renews() {
        assert!(!lazy_renew_skippable(Some(WB_T0 + WB_H24 - 1), WB_T0));
    }

    /// 已过期（负剩余）：必须续期
    #[test]
    fn lazy_renew_expired_renews() {
        assert!(!lazy_renew_skippable(Some(WB_T0 - 3600), WB_T0));
    }

    // ── auth 文件键名诊断 + 提取键对齐（issue #51）─────────────────────────

    /// 诊断键名：顶层 + auth/account 子对象键名（带前缀），绝不包含值
    #[test]
    fn auth_key_names_lists_top_and_child_keys() {
        let raw = serde_json::json!({
            "domain": "wb.example.com",
            "auth": { "accessToken": "secret-token", "refreshToken": "r" },
            "account": { "uid": "u1", "nickname": "n" }
        });
        let s = auth_key_names(&raw);
        assert!(s.contains("domain"));
        assert!(s.contains("auth.accessToken"));
        assert!(s.contains("auth.refreshToken"));
        assert!(s.contains("account.uid"));
        assert!(!s.contains("secret-token"), "诊断信息绝不能包含值");
    }

    /// 诊断键名：非对象 / 空对象兜底
    #[test]
    fn auth_key_names_non_object_and_empty() {
        assert_eq!(auth_key_names(&serde_json::json!("str")), "（非 JSON 对象）");
        assert_eq!(auth_key_names(&serde_json::json!([1])), "（非 JSON 对象）");
        assert_eq!(auth_key_names(&serde_json::json!({})), "（对象无键）");
    }

    /// 诊断键名附值类型（issue #58）：null 一眼可辨 → 未登录而非结构变更；仍绝不包含值
    #[test]
    fn auth_key_names_includes_value_type() {
        let raw = serde_json::json!({
            "auth": { "accessToken": null },
            "account": { "uid": "u1" }
        });
        let s = auth_key_names(&raw);
        assert!(s.contains("auth.accessToken（null）"));
        assert!(s.contains("account.uid（string）"));
        assert!(!s.contains("u1"), "诊断信息绝不能包含值");
    }

    // ── access token 提取分类报错（issue #58）──────────────────────────────

    /// 键存在但值为 null：报「未登录」而非「结构可能已变更」（原实现误报，issue #58 根因）
    #[test]
    fn extract_access_token_null_value_reports_login_hint() {
        let raw = serde_json::json!({
            "auth": { "accessToken": null, "refreshToken": null },
            "account": { "uid": "u1" }
        });
        let err = extract_access_token(&raw).unwrap_err();
        assert!(err.contains("null"), "错误应标注值类型: {err}");
        assert!(err.contains("登录"), "null 应提示登录而非结构变更: {err}");
        assert!(!err.contains("结构可能已变更"), "null 不应误报结构变更: {err}");
    }

    /// 键存在但值为空字符串：报「为空（可能未登录）」
    #[test]
    fn extract_access_token_empty_string_reports_empty() {
        let raw = serde_json::json!({ "auth": { "accessToken": "" } });
        let err = extract_access_token(&raw).unwrap_err();
        assert!(err.contains("为空字符串"), "{err}");
        assert!(!err.contains("结构可能已变更"), "{err}");
    }

    /// 信封下钻命中非空字符串：正常提取
    #[test]
    fn extract_access_token_envelope_hit() {
        let raw = serde_json::json!({ "auth": { "accessToken": "tok-1" } });
        assert_eq!(extract_access_token(&raw).unwrap(), "tok-1");
    }

    /// 5.8.2+ 静态加密包装：auth.accessToken 为 object 时透明解出明文（合成信封，
    /// DEK 为测试注入；不触碰真机 keyblob/凭证），不再误报「值为 object 请重新登录」
    #[test]
    fn extract_access_token_encrypted_wrapper_decrypts() {
        let wrapped =
            super::super::atrest::wrapped_field_value("tok-synthetic-encrypted-9f3k");
        let raw = serde_json::json!({
            "auth": { "accessToken": wrapped, "refreshToken": null },
            "account": { "uid": "u-syn-1" }
        });
        assert_eq!(extract_access_token(&raw).unwrap(), "tok-synthetic-encrypted-9f3k");

        // 透明解密辅助同样适用于 refreshToken / nickname
        let refresh_wrapped = super::super::atrest::wrapped_field_value("rt-synthetic-77");
        let raw2 = serde_json::json!({
            "auth": { "refreshToken": refresh_wrapped },
            "account": { "nickname": super::super::atrest::wrapped_field_value("昵称-合成") }
        });
        assert_eq!(
            super::super::common::dig_plain_string(&raw2, &["refreshToken", "refresh_token"]),
            Some("rt-synthetic-77".into())
        );
        assert_eq!(
            super::super::common::dig_plain_string(&raw2, &["nickname", "displayName", "name"]),
            Some("昵称-合成".into())
        );
    }

    /// 候选键全未命中：保留「结构可能已变更」+ 键名诊断（含类型）
    #[test]
    fn extract_access_token_missing_reports_structure_change() {
        let raw = serde_json::json!({ "foo": 1 });
        let err = extract_access_token(&raw).unwrap_err();
        assert!(err.contains("结构可能已变更"), "{err}");
        assert!(err.contains("foo（number）"), "{err}");
    }

    /// P2 键对齐：通用 `token` 键与 accessTokenExpiresAtMs 经 dig 可命中（原键列表会漏），
    /// 毫秒级到期经 as_ts_seconds 归一为秒
    #[test]
    fn dig_hits_generic_token_and_new_expiry_keys() {
        let raw = serde_json::json!({
            "auth": { "token": "t", "accessTokenExpiresAtMs": 1_700_000_000_000i64 }
        });
        assert_eq!(
            as_str(fs_utils::dig(&raw, &["accessToken", "access_token", "token"])),
            Some("t".into())
        );
        assert_eq!(
            as_ts_seconds(fs_utils::dig(
                &raw,
                &["expiresAtMs", "expires_at_ms", "expiresAt", "expires_in_ms", "accessTokenExpiresAtMs"]
            )),
            Some(1_700_000_000)
        );
    }
}
