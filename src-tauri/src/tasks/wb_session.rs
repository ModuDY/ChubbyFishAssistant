use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::wb_common::{self, Creds, RefreshFail};

#[derive(Debug, Clone)]
pub struct RefreshError {
    pub kind: RefreshFail,
    pub http_status: Option<u16>,
    pub retry_after_ms: Option<i64>,
}

impl RefreshError {
    pub fn new(kind: RefreshFail) -> Self {
        Self { kind, http_status: None, retry_after_ms: None }
    }

    pub fn message(&self) -> String {
        let reason = match self.kind {
            RefreshFail::Auth => "续期凭证被拒绝，需重新登录",
            RefreshFail::NoRefreshToken => "无可用续期凭证，需重新登录",
            RefreshFail::RateLimited => "续期被限流，稍后自动重试",
            RefreshFail::Network => "续期网络或服务端暂时异常，稍后自动重试",
            RefreshFail::Rejected => "续期请求被拒绝，暂未确认凭证失效，稍后重试",
            RefreshFail::BadResponse => "续期响应异常，稍后自动重试",
            RefreshFail::Busy => "该账号正在续期，请稍后重试",
            RefreshFail::Cooldown => "该账号正在恢复冷却，请稍后重试",
            RefreshFail::Persistence => "续期结果保存失败，请检查存储状态",
        };
        match self.http_status {
            Some(status) => format!("{reason}（refresh HTTP {status}）"),
            None => reason.to_string(),
        }
    }

    pub fn note(&self) -> &'static str {
        match self.kind {
            RefreshFail::Auth | RefreshFail::NoRefreshToken => "auth_required",
            RefreshFail::Persistence => "persist_failed",
            RefreshFail::Busy => "refresh_busy",
            RefreshFail::Cooldown => "refresh_cooldown",
            _ => "refresh_failed",
        }
    }

    pub fn permanent(&self) -> bool {
        matches!(self.kind, RefreshFail::Auth | RefreshFail::NoRefreshToken)
    }
}

#[derive(Default, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionHealth {
    pub status: String,
    pub last_refresh_at: Option<i64>,
    pub last_attempt_at: Option<i64>,
    pub next_retry_at: Option<i64>,
    pub http_status: Option<u16>,
    pub message: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    generation: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    retired_generations: Vec<String>,
    failures: u32,
}

pub enum RefreshMode {
    Due(i64),
    Recover,
    Force,
}

fn health_key(account_id: &str) -> String {
    format!("wb_session:{account_id}")
}

fn generation(creds: &Creds) -> String {
    let mut digest = Sha256::new();
    for value in [&creds.uid, &creds.domain, &creds.access_token, &creds.refresh_token] {
        digest.update((value.len() as u64).to_le_bytes());
        digest.update(value.as_bytes());
    }
    format!("{:x}", digest.finalize())
}

pub fn health(data_dir: &Path, account_id: &str) -> SessionHealth {
    health_checked(data_dir, account_id).unwrap_or_default()
}

fn health_checked(data_dir: &Path, account_id: &str) -> Result<SessionHealth, RefreshError> {
    let raw = crate::store::db(data_dir).kv_get_raw_checked(&health_key(account_id))
        .map_err(|_| RefreshError::new(RefreshFail::Persistence))?;
    raw.map(|text| serde_json::from_str(&text).map_err(|_| RefreshError::new(RefreshFail::Persistence)))
        .transpose().map(|health| health.unwrap_or_default())
}

pub fn health_view(data_dir: &Path, account_id: &str, needs_relogin: bool, access_expiry: Option<i64>, refresh_expiry: Option<i64>) -> serde_json::Value {
    let mut health = health(data_dir, account_id);
    if needs_relogin { health.status = "auth_required".into(); }
    else if health.status.is_empty() {
        let threshold = chrono::Utc::now().timestamp().saturating_add(86_400);
        health.status = if [access_expiry, refresh_expiry].into_iter().flatten().any(|expiry| expiry <= threshold) {
            "expiring"
        } else { "unverified" }.into();
    }
    let mut value = serde_json::to_value(health).unwrap_or_default();
    if let Some(fields) = value.as_object_mut() {
        fields.remove("generation"); fields.remove("retired_generations"); fields.remove("failures");
    }
    value
}

fn save_health(data_dir: &Path, account_id: &str, health: &SessionHealth) -> Result<(), RefreshError> {
    let text = serde_json::to_string(health).map_err(|_| RefreshError::new(RefreshFail::Persistence))?;
    crate::store::db(data_dir).kv_set_raw(&health_key(account_id), &text)
        .map_err(|_| RefreshError::new(RefreshFail::Persistence))
}

pub fn remove_health(data_dir: &Path, account_id: &str) -> Result<(), String> {
    crate::store::db(data_dir).with_conn(|connection| {
        connection.execute("DELETE FROM kv WHERE key=?1", [health_key(account_id)])?;
        Ok(())
    })
}

pub fn refresh_due(creds: &Creds, lazy_hours: i64, now_ms: i64) -> bool {
    let window = lazy_hours.max(0).saturating_mul(3_600_000);
    creds.expires_at_ms.is_none_or(|expiry| expiry.saturating_sub(now_ms) <= window)
        || (!creds.refresh_token.is_empty()
            && creds.refresh_expires_at_ms.is_some_and(|expiry| expiry.saturating_sub(now_ms) <= window))
}

fn blocked(health: &SessionHealth, creds: &Creds, force: bool, now_ms: i64) -> Option<RefreshError> {
    if health.generation != generation(creds) { return None; }
    if health.status == "auth_required" && !force {
        return Some(RefreshError { kind: RefreshFail::Auth, http_status: health.http_status, retry_after_ms: None });
    }
    if health.next_retry_at.is_some_and(|retry| retry > now_ms) {
        return Some(RefreshError::new(RefreshFail::Cooldown));
    }
    None
}

pub fn recovery_block(data_dir: &Path, account_id: &str, creds: &Creds) -> Option<RefreshError> {
    match crate::store::db(data_dir).row_get("wb_accounts", account_id) {
        Ok(Some(account)) if account.get("needs_relogin").and_then(serde_json::Value::as_bool) != Some(true) => {}
        Ok(_) => return Some(RefreshError::new(RefreshFail::Auth)),
        Err(_) => return Some(RefreshError::new(RefreshFail::Persistence)),
    }
    match health_checked(data_dir, account_id) {
        Ok(health) => blocked(&health, creds, false, chrono::Utc::now().timestamp_millis()),
        Err(error) => Some(error),
    }
}

fn failure_health(previous: SessionHealth, creds: &Creds, error: &RefreshError, now_ms: i64) -> SessionHealth {
    let failures = if previous.generation == generation(creds) { previous.failures.saturating_add(1) } else { 1 };
    let base = 60_000_i64.saturating_mul(1_i64 << failures.saturating_sub(1).min(7)).min(7_200_000);
    let jitter = i64::from(Sha256::digest(generation(creds).as_bytes())[0]) * 100;
    let delay = error.retry_after_ms.unwrap_or(0).max(base + jitter);
    SessionHealth {
        status: if error.permanent() { "auth_required" } else { "transient" }.into(),
        generation: generation(creds), failures,
        retired_generations: previous.retired_generations,
        last_refresh_at: previous.last_refresh_at,
        last_attempt_at: Some(now_ms),
        next_retry_at: if error.permanent() { None } else { Some(now_ms.saturating_add(delay)) },
        http_status: error.http_status,
        message: error.message(),
    }
}

/// Compare whole credential pairs. Only a recorded rotation proves that a candidate
/// is obsolete; a different, newer desktop login must remain eligible.
pub(crate) fn select_current(stored: Creds, candidate: Creds, health: &SessionHealth) -> Creds {
    if stored.access_token.is_empty() { return candidate; }
    let retired = health.generation == generation(&stored)
        && health.retired_generations.contains(&generation(&candidate));
    let stored_newer = stored.expires_at_ms.is_some_and(|expiry| {
        candidate.expires_at_ms.is_none_or(|other| expiry >= other)
    });
    if candidate.access_token.is_empty() || retired || stored_newer
        || (stored.expires_at_ms.is_none() && candidate.expires_at_ms.is_none()) {
        stored
    } else { candidate }
}

pub fn refresh(
    data_dir: &Path, account_id: &str, creds: &Creds, mode: RefreshMode, agent: &ureq::Agent,
) -> Result<(Creds, bool), RefreshError> {
    refresh_with(data_dir, account_id, creds, mode, |current| wb_common::refresh_request(agent, current))
}

fn refresh_with(
    data_dir: &Path, account_id: &str, candidate: &Creds, mode: RefreshMode,
    request: impl FnOnce(&Creds) -> Result<Creds, RefreshError>,
) -> Result<(Creds, bool), RefreshError> {
    let lock = wb_common::refresh_lock_for(account_id);
    let _guard = lock.lock().unwrap_or_else(|error| error.into_inner());
    let _cross = wb_common::cross_refresh_lock(data_dir, account_id).ok_or_else(|| RefreshError::new(RefreshFail::Busy))?;
    let store = crate::store::db(data_dir);
    let account = store.row_get("wb_accounts", account_id).map_err(|_| RefreshError::new(RefreshFail::Persistence))?
        .ok_or_else(|| RefreshError::new(RefreshFail::NoRefreshToken))?;
    let stored = wb_common::stored_creds(data_dir, account_id);
    let uid = account.get("uid").and_then(serde_json::Value::as_str).unwrap_or_default();
    if uid.is_empty() || (!candidate.uid.is_empty() && candidate.uid != uid) {
        return Err(RefreshError::new(RefreshFail::NoRefreshToken));
    }
    let now_ms = chrono::Utc::now().timestamp_millis();
    let previous = health_checked(data_dir, account_id)?;
    let current = select_current(stored, candidate.clone(), &previous);
    if let Some(error) = blocked(&previous, &current, matches!(mode, RefreshMode::Force), now_ms) { return Err(error); }
    if account.get("needs_relogin").and_then(serde_json::Value::as_bool) == Some(true)
        && !matches!(mode, RefreshMode::Force) {
        return Err(RefreshError::new(RefreshFail::Auth));
    }
    if matches!(mode, RefreshMode::Recover) && !current.access_token.is_empty() && current.access_token != candidate.access_token {
        return Ok((current, false));
    }
    if let RefreshMode::Due(window) = mode {
        if !refresh_due(&current, window, now_ms) { return Ok((current, false)); }
        if previous.generation == generation(&current)
            && previous.last_refresh_at.is_some_and(|last| now_ms.saturating_sub(last) < 21_600_000)
            && current.expires_at_ms.is_some_and(|expiry| expiry > now_ms.saturating_add(21_600_000)) {
            return Ok((current, false));
        }
        if current.refresh_token.is_empty() && current.expires_at_ms.is_some_and(|expiry| expiry > now_ms) {
            save_health(data_dir, account_id, &SessionHealth {
                status: "expiring".into(), generation: generation(&current),
                message: "AT 仍有效，但无续期凭证，请在到期前重新登录".into(),
                ..previous
            })?;
            return Ok((current, false));
        }
    }
    let result = if current.refresh_token.is_empty() {
        Err(RefreshError::new(RefreshFail::NoRefreshToken))
    } else { request(&current) };
    let new = match result {
        Ok(new) => new,
        Err(error) => {
            save_health(data_dir, account_id, &failure_health(previous, &current, &error, now_ms))?;
            crate::fs_utils::app_log(data_dir, &format!("[wb-renew] account={account_id} {}", error.message()));
            return Err(error);
        }
    };
    if wb_common::save_token_store_at(data_dir, account_id, &new).is_err() {
        let error = RefreshError::new(RefreshFail::Persistence);
        let stored_after = wb_common::stored_creds(data_dir, account_id);
        let blocked_creds = if stored_after.access_token.is_empty() { &current } else { &stored_after };
        save_health(data_dir, account_id, &failure_health(previous, blocked_creds, &error, now_ms))?;
        return Err(error);
    }
    let refreshed_at = chrono::Utc::now().timestamp_millis();
    let mut retired_generations = previous.retired_generations;
    if generation(&current) != generation(&new) {
        retired_generations.push(generation(&current));
        if retired_generations.len() > 16 { retired_generations.remove(0); }
    }
    let status = if new.refresh_expires_at_ms.is_some_and(|expiry| expiry <= refreshed_at.saturating_add(86_400_000)) {
        "expiring"
    } else { "healthy" };
    save_health(data_dir, account_id, &SessionHealth {
        status: status.into(), generation: generation(&new), last_refresh_at: Some(refreshed_at),
        retired_generations,
        last_attempt_at: Some(now_ms), message: if status == "expiring" { "已续期，但 RT 仍临期，请检查并准备重新登录".into() } else { "凭证已续期并保存".into() },
        ..Default::default()
    })?;
    Ok((new, true))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::{Arc, Barrier, atomic::{AtomicUsize, Ordering}};

    fn isolated(tag: &str) -> (std::path::PathBuf, Creds) {
        let directory = std::env::temp_dir().join(format!("wb-session-{tag}-{}-{}", std::process::id(), chrono::Utc::now().timestamp_nanos_opt().unwrap()));
        std::fs::create_dir_all(directory.join("data")).unwrap();
        crate::store::db(&directory).row_upsert("wb_accounts", "synthetic", &json!({"id":"synthetic", "uid":"synthetic-uid"})).unwrap();
        let creds = Creds { uid: "synthetic-uid".into(), access_token: "synthetic-access".into(), refresh_token: "synthetic-refresh".into(),
            expires_at_ms: Some(chrono::Utc::now().timestamp_millis() + 172_800_000), ..Default::default() };
        wb_common::token_store_upsert_secure(&directory, "synthetic", &serde_json::to_value(&creds).unwrap()).unwrap();
        (directory, creds)
    }

    #[test]
    fn session_due_checks_rt_even_with_long_lived_at_and_zero_is_not_force() {
        let mut creds = Creds { expires_at_ms: Some(300_000_000), refresh_token: "synthetic".into(), refresh_expires_at_ms: Some(100_000), ..Default::default() };
        assert!(refresh_due(&creds, 24, 1_000));
        assert!(!refresh_due(&creds, 0, 1_000));
        creds.refresh_expires_at_ms = None;
        assert!(!refresh_due(&creds, 24, 1_000));
        assert!(refresh_due(&creds, i64::MAX, 1_000));
        creds.expires_at_ms = None;
        assert!(refresh_due(&creds, 24, 1_000));
    }

    #[test]
    fn session_backoff_honors_retry_after_and_caps_exponential_delay() {
        let creds = Creds::default();
        let error = RefreshError { kind: RefreshFail::RateLimited, http_status: Some(429), retry_after_ms: Some(9_000_000) };
        let first = failure_health(SessionHealth::default(), &creds, &error, 100);
        assert_eq!(first.next_retry_at, Some(9_000_100));
        assert_eq!(first.status, "transient");
        let mut previous = first;
        previous.failures = u32::MAX;
        let capped = failure_health(previous, &creds, &RefreshError::new(RefreshFail::Network), 0);
        assert!(capped.next_retry_at.unwrap() >= 7_200_000);
        assert!(capped.next_retry_at.unwrap() < 7_226_000);
    }

    #[test]
    fn session_cooldown_survives_reload_and_blocks_all_modes() {
        let (directory, creds) = isolated("cooldown");
        let error = refresh_with(&directory, "synthetic", &creds, RefreshMode::Recover, |_| {
            Err(RefreshError { kind: RefreshFail::RateLimited, http_status: Some(429), retry_after_ms: Some(600_000) })
        }).err().unwrap();
        assert_eq!(error.kind, RefreshFail::RateLimited);
        for mode in [RefreshMode::Recover, RefreshMode::Due(24), RefreshMode::Force] {
            let result = refresh_with(&directory, "synthetic", &creds, mode, |_| panic!("cooldown must not send request"));
            assert_eq!(result.err().unwrap().kind, RefreshFail::Cooldown);
        }
        let disk = rusqlite::Connection::open(directory.join("data/aiwork.sqlite")).unwrap();
        let text: String = disk.query_row("SELECT content FROM kv WHERE key='wb_session:synthetic'", [], |row| row.get(0)).unwrap();
        let restored: SessionHealth = serde_json::from_str(&text).unwrap();
        assert!(blocked(&restored, &creds, false, chrono::Utc::now().timestamp_millis()).is_some());
        assert!(!text.contains("synthetic-refresh"));
        assert!(!text.contains("synthetic-access"));
        assert!(!error.message().contains("需重新登录"));
        assert_eq!(health_view(&directory, "synthetic", false, None, None)["status"], "transient");
    }

    #[test]
    fn session_permanent_failure_stops_future_attempts_but_new_login_clears_it() {
        let (directory, mut creds) = isolated("auth");
        refresh_with(&directory, "synthetic", &creds, RefreshMode::Recover, |_| Err(RefreshError::new(RefreshFail::Auth))).err().unwrap();
        assert!(refresh_with(&directory, "synthetic", &creds, RefreshMode::Recover, |_| panic!("auth failure must stop recovery")).is_err());
        creds.access_token = "synthetic-login-access".into();
        creds.refresh_token = "synthetic-login-refresh".into();
        wb_common::token_store_upsert_secure(&directory, "synthetic", &serde_json::to_value(&creds).unwrap()).unwrap();
        assert!(recovery_block(&directory, "synthetic", &creds).is_none());
        assert_eq!(health_view(&directory, "synthetic", false, None, None)["status"], "unverified");
    }

    #[test]
    fn session_parallel_recovery_refreshes_once_and_reuses_new_pair() {
        let (directory, creds) = isolated("parallel");
        let barrier = Arc::new(Barrier::new(2));
        let attempts = Arc::new(AtomicUsize::new(0));
        let handles: Vec<_> = (0..2).map(|_| {
            let (directory, creds, barrier, attempts) = (directory.clone(), creds.clone(), barrier.clone(), attempts.clone());
            std::thread::spawn(move || {
                barrier.wait();
                refresh_with(&directory, "synthetic", &creds, RefreshMode::Recover, |current| {
                    attempts.fetch_add(1, Ordering::SeqCst);
                    let mut new = current.clone();
                    new.access_token = "synthetic-rotated-access".into();
                    new.refresh_token = "synthetic-rotated-refresh".into();
                    new.expires_at_ms = Some(current.expires_at_ms.unwrap() + 3_600_000);
                    Ok(new)
                }).unwrap().0.access_token
            })
        }).collect();
        for handle in handles { assert_eq!(handle.join().unwrap(), "synthetic-rotated-access"); }
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        assert_eq!(wb_common::stored_creds(&directory, "synthetic").refresh_token, "synthetic-rotated-refresh");
        assert_eq!(health(&directory, "synthetic").status, "healthy");
    }

    #[test]
    fn session_persistence_failure_is_not_success_and_is_cooled() {
        let (directory, creds) = isolated("persistence");
        crate::store::db(&directory).kv_set_raw("wb_tokens_meta", "2").unwrap();
        let result = refresh_with(&directory, "synthetic", &creds, RefreshMode::Recover, |current| {
            Ok(Creds { access_token: "synthetic-unsaved-access".into(), ..current.clone() })
        });
        assert_eq!(result.err().unwrap().kind, RefreshFail::Persistence);
        assert_eq!(health(&directory, "synthetic").last_refresh_at, None);
        assert_eq!(refresh_with(&directory, "synthetic", &creds, RefreshMode::Recover, |_| panic!("must cool persistence failure")).err().unwrap().kind, RefreshFail::Cooldown);
    }

    #[test]
    fn session_deleted_account_never_sends_refresh_or_reappears() {
        let (directory, creds) = isolated("deleted");
        crate::store::db(&directory).with_conn(|connection| connection.execute("DELETE FROM wb_accounts WHERE pk='synthetic'", []).map(|_| ())).unwrap();
        assert!(refresh_with(&directory, "synthetic", &creds, RefreshMode::Recover, |_| panic!("deleted account must not refresh")).is_err());
        assert!(crate::store::db(&directory).row_get("wb_accounts", "synthetic").unwrap().is_none());
    }

    #[test]
    fn session_healthy_metadata_does_not_claim_online_validation() {
        let (directory, creds) = isolated("fresh");
        let (_, refreshed) = refresh_with(&directory, "synthetic", &creds, RefreshMode::Due(24), |_| panic!("fresh must not refresh")).unwrap();
        assert!(!refreshed);
        assert!(health(&directory, "synthetic").last_refresh_at.is_none());
        let view = health_view(&directory, "synthetic", false, None, None);
        assert_eq!(view["status"], "unverified");
        assert!(view.get("generation").is_none());
        assert!(view.get("retired_generations").is_none());
        assert!(view.get("failures").is_none());
    }

    #[test]
    fn session_health_removed_with_account_cleanup() {
        let (directory, creds) = isolated("cleanup");
        save_health(&directory, "synthetic", &failure_health(SessionHealth::default(), &creds, &RefreshError::new(RefreshFail::Network), 0)).unwrap();
        remove_health(&directory, "synthetic").unwrap();
        assert!(health(&directory, "synthetic").status.is_empty());
    }

    #[test]
    fn session_old_snapshot_cannot_replace_stored_pair_even_with_later_expiry() {
        let (directory, creds) = isolated("old-snapshot");
        let (rotated, _) = refresh_with(&directory, "synthetic", &creds, RefreshMode::Recover, |current| {
            Ok(Creds { access_token: "synthetic-rotated-access".into(), refresh_token: "synthetic-rotated-refresh".into(), ..current.clone() })
        }).unwrap();
        let candidate = Creds { expires_at_ms: Some(i64::MAX), ..creds };
        let (selected, refreshed) = refresh_with(&directory, "synthetic", &candidate, RefreshMode::Due(24), |_| panic!("stored pair is fresh")).unwrap();
        assert!(!refreshed);
        assert_eq!(selected.access_token, rotated.access_token);
        assert_eq!(selected.refresh_token, rotated.refresh_token);
        assert!(health_view(&directory, "synthetic", false, None, None).get("retired_generations").is_none());
    }

    #[test]
    fn session_newer_desktop_login_remains_eligible_after_stored_rotation() {
        let (directory, creds) = isolated("new-desktop");
        refresh_with(&directory, "synthetic", &creds, RefreshMode::Recover, |current| {
            Ok(Creds { access_token: "synthetic-rotated-access".into(), refresh_token: "synthetic-rotated-refresh".into(), ..current.clone() })
        }).unwrap();
        let desktop = Creds { access_token: "synthetic-desktop-access".into(), refresh_token: "synthetic-desktop-refresh".into(),
            expires_at_ms: Some(creds.expires_at_ms.unwrap() + 3_600_000), ..creds };
        let (selected, refreshed) = refresh_with(&directory, "synthetic", &desktop, RefreshMode::Recover, |current| {
            assert_eq!(current.access_token, desktop.access_token);
            assert_eq!(current.refresh_token, desktop.refresh_token);
            Ok(Creds { access_token: "synthetic-recovered-access".into(), ..current.clone() })
        }).unwrap();
        assert!(refreshed);
        assert_eq!(selected.access_token, "synthetic-recovered-access");
        assert_eq!(wb_common::stored_creds(&directory, "synthetic").refresh_token, desktop.refresh_token);
    }

    #[test]
    fn session_unrotated_store_does_not_override_newer_desktop_credentials() {
        let (directory, creds) = isolated("desktop-due");
        let desktop = Creds { access_token: "synthetic-desktop-access".into(), refresh_token: "synthetic-desktop-refresh".into(),
            expires_at_ms: Some(creds.expires_at_ms.unwrap() + 3_600_000), ..creds };
        let (selected, refreshed) = refresh_with(&directory, "synthetic", &desktop, RefreshMode::Due(24), |_| panic!("desktop is fresh")).unwrap();
        assert!(!refreshed);
        assert_eq!(selected.access_token, desktop.access_token);
        assert_eq!(selected.refresh_token, desktop.refresh_token);
    }

    #[test]
    fn session_pool_relogin_flag_blocks_business_steps_even_after_successful_refresh() {
        let (directory, creds) = isolated("pool-auth");
        refresh_with(&directory, "synthetic", &creds, RefreshMode::Recover, |current| Ok(current.clone())).unwrap();
        crate::store::db(&directory).row_upsert("wb_accounts", "synthetic", &json!({"id":"synthetic", "uid":creds.uid, "needs_relogin":true})).unwrap();
        assert_eq!(recovery_block(&directory, "synthetic", &creds).unwrap().kind, RefreshFail::Auth);
        assert_eq!(health_view(&directory, "synthetic", true, None, None)["status"], "auth_required");
    }

    #[test]
    fn session_valid_access_without_rt_remains_usable_until_expiry() {
        let (directory, mut creds) = isolated("no-rt");
        creds.refresh_token.clear();
        creds.expires_at_ms = Some(chrono::Utc::now().timestamp_millis() + 60_000);
        crate::vault::ns_remove_strict(&directory, "wb", "synthetic").unwrap();
        wb_common::token_store_upsert_secure(&directory, "synthetic", &serde_json::to_value(&creds).unwrap()).unwrap();
        let (_, refreshed) = refresh_with(&directory, "synthetic", &creds, RefreshMode::Due(24), |_| panic!("no rt cannot refresh")).unwrap();
        assert!(!refreshed);
        assert_eq!(health(&directory, "synthetic").status, "expiring");
        assert!(recovery_block(&directory, "synthetic", &creds).is_none());
    }
}
