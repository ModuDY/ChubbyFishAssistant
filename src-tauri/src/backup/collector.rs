use crate::{state::AppState, store::Store};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, HashSet},
    path::Path,
};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    Trae,
    Buddy,
    Qoder,
    Doubao,
}

impl Platform {
    fn table(self) -> &'static str {
        match self {
            Self::Trae => "accounts",
            Self::Buddy => "wb_accounts",
            Self::Qoder => "qoder_accounts",
            Self::Doubao => "doubao_accounts",
        }
    }
    fn token_table(self) -> Option<&'static str> {
        match self {
            Self::Buddy => Some("wb_tokens"),
            Self::Qoder => Some("qoder_tokens"),
            _ => None,
        }
    }
    fn vault_key(self, id: &str) -> String {
        match self {
            Self::Trae => id.into(),
            Self::Buddy => format!("ns:wb:{id}"),
            Self::Qoder => format!("ns:qoder:{id}"),
            Self::Doubao => format!("ns:doubao:{id}"),
        }
    }
    fn fields(self) -> &'static [&'static str] {
        match self {
            Self::Trae => &["jwt", "refresh_token"],
            Self::Buddy => &["access_token", "refresh_token"],
            Self::Qoder => &["access_token", "refresh_token", "pat", "machine_token"],
            Self::Doubao => &["session_id", "sid_guard", "ttwid"],
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub platform: Platform,
    pub id: String,
    pub account: Value,
    pub token: Value,
    pub secret: Value,
    pub device: Option<Value>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Archive {
    pub kind: String,
    pub version: u32,
    pub created_at: String,
    pub app_version: String,
    pub accounts: Vec<Entry>,
}

pub fn healthy(dir: &Path) -> Result<(), String> {
    if crate::store::db(dir).health().mode != crate::store::StorageMode::Persistent {
        return Err("当前存储为恢复或内存模式，账号备份和恢复已暂停；修复存储并重启后重试".into());
    }
    Ok(())
}

fn raw_rows(
    c: &rusqlite::Connection,
    table: &str,
) -> rusqlite::Result<Vec<(Option<String>, String)>> {
    // table only comes from this module's fixed enum or device_map.
    let sql = if table == "accounts" {
        "SELECT user_id, data FROM accounts ORDER BY seq".into()
    } else {
        format!("SELECT pk, data FROM {table} ORDER BY pk")
    };
    let mut stmt = c.prepare(&sql)?;
    let values = stmt
        .query_map([], |r| {
            Ok((r.get::<_, Option<String>>(0)?, r.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(values)
}

fn decode_rows(raw: Vec<(Option<String>, String)>) -> Result<BTreeMap<String, Value>, String> {
    raw.into_iter()
        .map(|(id, raw)| {
            let id = id
                .filter(|s| !s.is_empty())
                .ok_or("账号缺少稳定标识，备份已中止")?;
            let v: Value = serde_json::from_str(&raw).map_err(|_| "账号数据损坏，备份已中止")?;
            if !v.is_object() {
                return Err("账号数据格式错误，备份已中止".into());
            }
            Ok((id, v))
        })
        .collect()
}

fn rows(store: &Store, table: &str) -> Result<BTreeMap<String, Value>, String> {
    decode_rows(store.with_conn(|c| raw_rows(c, table))?)
}

fn nonempty(v: &Value, field: &str) -> Option<String> {
    v.get(field)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .map(str::to_owned)
}

// Re-serialize known DTOs so arbitrary input properties cannot turn into plaintext on restore.
fn typed_account(platform: Platform, value: Value) -> Result<Value, String> {
    fn roundtrip<T: serde::de::DeserializeOwned + Serialize>(v: Value) -> Result<Value, String> {
        let typed: T = serde_json::from_value(v).map_err(|_| "账号元数据格式不正确")?;
        serde_json::to_value(typed).map_err(|_| "账号元数据编码失败".into())
    }
    match platform {
        Platform::Trae => roundtrip::<crate::models::RawAccount>(value),
        Platform::Buddy => roundtrip::<crate::commands::workbuddy::WorkBuddyAccount>(value),
        Platform::Qoder => roundtrip::<crate::commands::qoder::QoderAccount>(value),
        Platform::Doubao => roundtrip::<crate::commands::doubao::DoubaoAccount>(value),
    }
}

impl Entry {
    pub fn has_credential(&self) -> bool {
        self.platform
            .fields()
            .iter()
            .any(|f| !["machine_token", "ttwid"].contains(f) && nonempty(&self.secret, f).is_some())
    }
    fn normalize(&mut self) -> Result<(), String> {
        crate::fs_utils::ensure_uid_safe(&self.id).map_err(|_| "备份账号标识不合法")?;
        // Bare Trae vault keys must never collide with the namespace prefix.
        if self.id.len() > 256 || self.id.contains(':') {
            return Err("备份账号标识无效".into());
        }
        if self.platform == Platform::Qoder
            && (self.id.len() != 15
                || !self.id.starts_with("qd-")
                || !self.id[3..]
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
        {
            return Err("Qoder 账号标识格式不正确".into());
        }
        self.account = typed_account(self.platform, self.account.clone())?;
        let id_field = match self.platform {
            Platform::Trae => "UserID",
            Platform::Doubao => "user_id",
            _ => "id",
        };
        if self.account.get(id_field).and_then(Value::as_str) != Some(&self.id) {
            return Err("账号标识与元数据不一致".into());
        }
        if !self.secret.is_object() || !self.token.is_object() {
            return Err("备份凭据结构不正确".into());
        }
        let mut secrets = json!({});
        for &field in self.platform.fields() {
            let value = self.secret.get(field).filter(|v| !v.is_null());
            if value.is_some_and(|v| !v.is_string()) {
                return Err("凭据字段格式不正确".into());
            }
            if let Some(value) = value {
                secrets[field] = value.clone();
            }
            if self.account.get(field).is_some() {
                self.account[field] = Value::Null;
            }
        }
        // RawAccount.jwt is required String even when stored as placeholder.
        if self.platform == Platform::Trae {
            self.account["jwt"] = json!("");
        }
        self.secret = secrets;
        let mut token = json!({});
        for field in [
            "uid",
            "domain",
            "nickname",
            "edition",
            "kind",
            "machine_id",
            "expires_at_ms",
            "refresh_expires_at_ms",
            "updated_at",
            "enterprise_id",
            "enterpriseId",
            "expiresAtMs",
            "refreshExpiresAtMs",
        ] {
            if let Some(value) = self
                .token
                .get(field)
                .filter(|v| v.is_string() || v.is_number() || v.is_null())
            {
                token[field] = value.clone();
            }
        }
        // Consumers expect canonical placeholders; vault supplies their values.
        for &field in self.platform.fields() {
            token[field] = json!("");
        }
        self.token = token;
        if let Some(device) = self.device.take() {
            let typed: crate::models::DeviceEntry =
                serde_json::from_value(device).map_err(|_| "设备资料格式不正确")?;
            self.device = Some(serde_json::to_value(typed).map_err(|_| "设备资料编码失败")?);
        }
        Ok(())
    }

    fn identity(&self) -> &str {
        match self.platform {
            Platform::Buddy | Platform::Qoder => self
                .account
                .get("uid")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .unwrap_or(&self.id),
            _ => &self.id,
        }
    }
}

pub fn collect(state: &AppState) -> Result<Archive, String> {
    healthy(&state.data_dir)?;
    let _jwt = state
        .jwt_refresh_lock
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let _wb = state.wb_pool_lock.lock().unwrap_or_else(|e| e.into_inner());
    let _qd = state
        .qoder_pool_lock
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let _db = state
        .doubao_pool_lock
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let _wt = crate::tasks::wb_common::WB_TOKEN_STORE_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let _qt = crate::tasks::qoder_common::TOKEN_STORE_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let db = crate::store::db(&state.data_dir);
    // One SQLite read transaction; release the connection before opening Stronghold.
    let snapshot = db.with_conn(|c| {
        let tx = c.unchecked_transaction()?;
        let mut data = Vec::new();
        for table in [
            "device_map",
            "accounts",
            "wb_accounts",
            "wb_tokens",
            "qoder_accounts",
            "qoder_tokens",
            "doubao_accounts",
        ] {
            data.push((table, raw_rows(&tx, table)?));
        }
        tx.commit()?;
        Ok(data)
    })?;
    let snapshot = snapshot
        .into_iter()
        .map(|(t, raw)| decode_rows(raw).map(|v| (t, v)))
        .collect::<Result<BTreeMap<_, _>, _>>()?;
    let devices = &snapshot["device_map"];
    let mut entries = Vec::new();
    for platform in [
        Platform::Trae,
        Platform::Buddy,
        Platform::Qoder,
        Platform::Doubao,
    ] {
        let empty = BTreeMap::new();
        let tokens = platform
            .token_table()
            .map(|t| &snapshot[t])
            .unwrap_or(&empty);
        for (id, account) in snapshot[platform.table()].clone() {
            entries.push(Entry {
                platform,
                token: tokens.get(&id).cloned().unwrap_or_else(|| json!({})),
                device: if platform == Platform::Trae {
                    devices.get(&id).cloned()
                } else {
                    None
                },
                id,
                account,
                secret: json!({}),
            });
        }
    }
    let keys: Vec<_> = entries
        .iter()
        .map(|e| e.platform.vault_key(&e.id))
        .collect();
    let secrets = crate::vault::backup_read(&state.data_dir, &keys)?;
    for (entry, vaulted) in entries.iter_mut().zip(secrets) {
        let vaulted = vaulted.unwrap_or_else(|| json!({}));
        if !vaulted.is_object() {
            return Err("保险库凭据结构损坏，备份已中止".into());
        }
        for &field in entry.platform.fields() {
            let alias = match field {
                "access_token" => "accessToken",
                "refresh_token" => "refreshToken",
                _ => field,
            };
            for value in [
                entry.account.get(field),
                entry.token.get(field),
                entry.token.get(alias),
                vaulted.get(field),
            ]
            .into_iter()
            .flatten()
            {
                if !value.is_null() && !value.is_string() {
                    return Err("凭据字段格式损坏，备份已中止".into());
                }
            }
            // Current DB plaintext, when present, wins over vault (same contract as runtime).
            if let Some(v) = nonempty(&entry.account, field)
                .or_else(|| nonempty(&entry.token, field))
                .or_else(|| nonempty(&entry.token, alias))
                .or_else(|| nonempty(&vaulted, field))
            {
                entry.secret[field] = json!(v);
            }
        }
        entry.normalize()?;
    }
    let mut archive = Archive {
        kind: "aiwork-account-backup".into(),
        version: 1,
        created_at: chrono::Utc::now().to_rfc3339(),
        app_version: env!("CARGO_PKG_VERSION").into(),
        accounts: entries,
    };
    validate_archive(&mut archive)?;
    Ok(archive)
}

pub fn parse(plain: &[u8]) -> Result<Archive, String> {
    let mut archive: Archive = serde_json::from_slice(plain).map_err(|_| "备份内容格式不正确")?;
    validate_archive(&mut archive)?;
    Ok(archive)
}

fn validate_archive(archive: &mut Archive) -> Result<(), String> {
    if archive.kind != "aiwork-account-backup"
        || archive.version != 1
        || archive.accounts.len() > 100_000
    {
        return Err("不支持的备份版本或账号数过多".into());
    }
    let mut ids = HashSet::new();
    let mut identities = HashSet::new();
    for e in &mut archive.accounts {
        e.normalize()?;
        if !ids.insert((e.platform, e.id.clone()))
            || !identities.insert((e.platform, e.identity().to_owned()))
        {
            return Err("备份包含重复账号".into());
        }
    }
    Ok(())
}

fn new_entries(state: &AppState, archive: &Archive) -> Result<Vec<Entry>, String> {
    let db = crate::store::db(&state.data_dir);
    let mut selected = Vec::new();
    for platform in [
        Platform::Trae,
        Platform::Buddy,
        Platform::Qoder,
        Platform::Doubao,
    ] {
        let existing = rows(&db, platform.table())?;
        for e in archive.accounts.iter().filter(|e| e.platform == platform) {
            let same_uid = existing.values().any(|v| {
                v.get("uid")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    == Some(e.identity())
            });
            if !existing.contains_key(&e.id) && !same_uid {
                selected.push(e.clone());
            }
        }
    }
    Ok(selected)
}

pub fn preview(state: &AppState, archive: &Archive) -> Result<Value, String> {
    healthy(&state.data_dir)?;
    let new = new_entries(state, archive)?;
    Ok(
        json!({ "created_at": archive.created_at, "total": archive.accounts.len(), "new_accounts": new.len(),
        "skipped": archive.accounts.len() - new.len(), "accounts": archive.accounts.iter().map(|e| {
            let has_credential = e.has_credential();
            json!({ "platform": e.platform, "name": e.account.get("name").or_else(|| e.account.get("nickname")).and_then(Value::as_str).unwrap_or(&e.id),
                "has_credential": has_credential, "has_refresh_token": nonempty(&e.secret, "refresh_token").is_some(),
                "expires_at": if e.platform == Platform::Trae { json!(crate::jwt::parse(e.secret.get("jwt").and_then(Value::as_str).unwrap_or("")).exp_timestamp) }
                    else { e.account.get("token_expires_at").or_else(|| e.account.get("access_token_expires_at")).or_else(|| e.account.get("session_expire_at")).cloned().unwrap_or(Value::Null) },
                "will_add": new.iter().any(|n| n.platform == e.platform && n.id == e.id) })
        }).collect::<Vec<_>>() }),
    )
}

pub fn restore(state: &AppState, archive: &Archive) -> Result<Value, String> {
    healthy(&state.data_dir)?;
    // Same pool → token-store → vault → SQLite lock order used by account writers.
    let _jwt = state
        .jwt_refresh_lock
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let _wb = state.wb_pool_lock.lock().unwrap_or_else(|e| e.into_inner());
    let _qd = state
        .qoder_pool_lock
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let _db = state
        .doubao_pool_lock
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let _wt = crate::tasks::wb_common::WB_TOKEN_STORE_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let _qt = crate::tasks::qoder_common::TOKEN_STORE_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let _dev = crate::store::docs::DEVICE_MAP_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let mut entries = new_entries(state, archive)?;
    for e in &mut entries {
        // Groups are local definitions, not part of a credential backup.
        if e.account.get("group_id").is_some() {
            e.account["group_id"] = json!("");
        }
    }
    let secrets: Vec<_> = entries
        .iter()
        .map(|e| (e.platform.vault_key(&e.id), e.secret.clone()))
        .collect();
    if !entries.is_empty() {
        crate::vault::backup_restore(&state.data_dir, &secrets, || {
            crate::store::db(&state.data_dir).with_conn(|c| {
                let tx = c.unchecked_transaction()?;
                for e in &entries {
                    let data = e.account.to_string();
                    if e.platform == Platform::Trae {
                        tx.execute("INSERT INTO accounts(user_id,data) VALUES(?1,?2)", rusqlite::params![e.id, data])?;
                    } else {
                        tx.execute(&format!("INSERT INTO {}(pk,data) VALUES(?1,?2)", e.platform.table()), rusqlite::params![e.id, data])?;
                    }
                    if let Some(table) = e.platform.token_table() {
                        tx.execute(&format!("INSERT INTO {table}(pk,data) VALUES(?1,?2) ON CONFLICT(pk) DO UPDATE SET data=excluded.data"), rusqlite::params![e.id, e.token.to_string()])?;
                    }
                    if let Some(device) = &e.device {
                        tx.execute("INSERT OR IGNORE INTO device_map(pk,data) VALUES(?1,?2)", rusqlite::params![e.id, device.to_string()])?;
                    }
                }
                tx.commit()
            }).map_err(|_| "账号恢复事务失败，未修改账号；保险库将回滚".into())
        })?;
    }
    Ok(json!({ "added": entries.len(), "skipped": archive.accounts.len() - entries.len() }))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn state() -> AppState {
        use std::sync::{Arc, Mutex};
        AppState {
            data_dir: std::env::temp_dir()
                .join(format!("aiwork-backup-test-{}", uuid::Uuid::new_v4())),
            jwt_refresh_lock: Arc::new(Mutex::new(())),
            wb_pool_lock: Arc::new(Mutex::new(())),
            qoder_pool_lock: Arc::new(Mutex::new(())),
            doubao_pool_lock: Arc::new(Mutex::new(())),
            group_defs_lock: Arc::new(Mutex::new(())),
        }
    }

    #[test]
    fn encrypted_cross_machine_restore_is_add_only_and_compensates_vault_failure() {
        let source = state();
        let target = state();
        let db = crate::store::db(&source.data_dir);
        db.row_upsert(
            "wb_accounts",
            "wb-test",
            &json!({"id":"wb-test","uid":"buddy-user","nickname":"Buddy Test"}),
        )
        .unwrap();
        db.row_upsert("wb_tokens", "wb-test", &json!({"access_token":"","refresh_token":"","domain":"test-domain","expires_at_ms":1900000000000i64})).unwrap();
        crate::vault::ns_set(
            &source.data_dir,
            "wb",
            "wb-test",
            &json!({"access_token":"synthetic-access","refresh_token":"synthetic-refresh"}),
        )
        .unwrap();
        db.row_upsert("qoder_accounts", "qd-0123456789ab", &json!({"id":"qd-0123456789ab","uid":"qoder-user","nickname":"Qoder Test","device_profile":{"machine_id":"source-fingerprint","device_id":"source-device","umid":"source-umid"}})).unwrap();
        crate::vault::ns_set(
            &source.data_dir,
            "qoder",
            "qd-0123456789ab",
            &json!({"pat":"synthetic-source-pat"}),
        )
        .unwrap();
        let mut trae = crate::models::AccountsFile {
            accounts: vec![crate::models::RawAccount {
                name: "Trae Test".into(),
                user_id: Some("trae-user".into()),
                jwt: "synthetic-jwt".into(),
                refresh_token: Some("synthetic-trae-refresh".into()),
                ..Default::default()
            }],
        };
        crate::vault::save_accounts(&source, &mut trae).unwrap();
        db.row_upsert(
            "doubao_accounts",
            "doubao-user",
            &json!({"user_id":"doubao-user","name":"Doubao Test","session_id":null}),
        )
        .unwrap();
        crate::vault::ns_set(&source.data_dir,"doubao","doubao-user",&json!({"session_id":"synthetic-cookie","sid_guard":"synthetic-guard","ttwid":"synthetic-ttwid"})).unwrap();

        let archive = collect(&source).unwrap();
        assert_eq!(archive.accounts.len(), 4);
        let sealed =
            super::super::crypto::seal(&serde_json::to_vec(&archive).unwrap(), "fixture-password")
                .unwrap();
        assert!(!sealed
            .windows(b"synthetic".len())
            .any(|w| w == b"synthetic"));
        let archive =
            parse(&super::super::crypto::open(&sealed, "fixture-password").unwrap()).unwrap();
        let target_db = crate::store::db(&target.data_dir);
        target_db.row_upsert("qoder_accounts", "qd-aaaaaaaaaaaa", &json!({"id":"qd-aaaaaaaaaaaa","uid":"qoder-user","device_profile":{"machine_id":"existing-fingerprint"}})).unwrap();
        crate::vault::ns_set(
            &target.data_dir,
            "qoder",
            "qd-aaaaaaaaaaaa",
            &json!({"pat":"newer-existing-pat"}),
        )
        .unwrap();
        assert_eq!(preview(&target, &archive).unwrap()["new_accounts"], 3);
        let report = restore(&target, &archive).unwrap();
        assert_eq!(report["added"], 3);
        assert_eq!(report["skipped"], 1);
        assert_eq!(restore(&target, &archive).unwrap()["added"], 0);
        let wb = crate::vault::backup_read(&target.data_dir, &["ns:wb:wb-test".into()]).unwrap();
        assert_eq!(
            wb[0].as_ref().unwrap()["refresh_token"],
            "synthetic-refresh"
        );
        assert_eq!(
            rows(&target_db, "wb_tokens").unwrap()["wb-test"]["access_token"],
            ""
        );
        let stored = rows(&target_db, "accounts").unwrap();
        assert_eq!(stored["trae-user"]["jwt"], "");
        assert!(!serde_json::to_string(&stored)
            .unwrap()
            .contains("synthetic-jwt"));
        assert_eq!(
            rows(&target_db, "qoder_accounts").unwrap()["qd-aaaaaaaaaaaa"]["device_profile"]
                ["machine_id"],
            "existing-fingerprint"
        );
        let unchanged =
            crate::vault::backup_read(&target.data_dir, &["ns:qoder:qd-aaaaaaaaaaaa".into()])
                .unwrap();
        assert_eq!(unchanged[0].as_ref().unwrap()["pat"], "newer-existing-pat");

        let failure: Result<(), String> = crate::vault::backup_restore(
            &target.data_dir,
            &[(
                "ns:wb:wb-test".into(),
                json!({"access_token":"should-rollback"}),
            )],
            || Err("synthetic SQLite failure".into()),
        );
        assert!(failure.is_err());
        let after = crate::vault::backup_read(&target.data_dir, &["ns:wb:wb-test".into()]).unwrap();
        assert_eq!(after, wb);
        // Malformed vault data is an error, not an empty credential backup.
        crate::vault::ns_set(
            &source.data_dir,
            "wb",
            "wb-test",
            &json!({"access_token":123}),
        )
        .unwrap();
        assert!(collect(&source).is_err());
        crate::vault::ns_set(
            &source.data_dir,
            "wb",
            "wb-test",
            &json!({"access_token":"fixture"}),
        )
        .unwrap();
        db.with_conn(|c| {
            c.execute("UPDATE wb_accounts SET data='not-json'", [])
                .map(|_| ())
        })
        .unwrap();
        assert!(collect(&source).is_err());
        let _ = std::fs::remove_dir_all(&source.data_dir);
        let _ = std::fs::remove_dir_all(&target.data_dir);
    }

    #[test]
    fn restore_sanitizes_secrets_and_preserves_fingerprint() {
        let file = json!({"kind":"aiwork-account-backup","version":1,"created_at":"test","app_version":"test","accounts":[{
            "platform":"qoder","id":"qd-0123456789ab","account":{"id":"qd-0123456789ab","uid":"user1","device_profile":{"machine_id":"fixed-machine","device_id":"fixed-device","umid":"fixed-umid"},"injected":"synthetic-secret"},
            "token":{"access_token":"must-be-cleared","machine_id":"real-machine","unknown_secret":"must-be-dropped"},"secret":{"pat":"synthetic-pat"},"device":null}]});
        let archive = parse(&serde_json::to_vec(&file).unwrap()).unwrap();
        let e = &archive.accounts[0];
        assert_eq!(e.secret["pat"], "synthetic-pat");
        assert_eq!(e.account["device_profile"]["machine_id"], "fixed-machine");
        assert_eq!(e.token["machine_id"], "real-machine");
        assert_eq!(e.token["access_token"], "");
        assert!(e.token.get("unknown_secret").is_none());
        assert!(e.account.get("injected").is_none());
        let mut bad = file;
        bad["accounts"][0]["id"] = json!("../escape");
        assert!(parse(&serde_json::to_vec(&bad).unwrap()).is_err());
    }
}
