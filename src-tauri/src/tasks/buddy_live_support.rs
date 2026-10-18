use std::path::PathBuf;

use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use tauri_plugin_stronghold::stronghold::Stronghold;

use super::wb_common::{self, Creds};

pub(crate) struct LiveAccount {
    pub ordinal: usize,
    pub creds: Creds,
    pub cached_balance: Option<f64>,
}

pub(crate) fn accounts() -> Result<Vec<LiveAccount>, String> {
    if std::env::var("AIWORK_BUDDY_LIVE_READONLY").as_deref() != Ok("1") {
        return Err("只读实测未启用；需要 AIWORK_BUDDY_LIVE_READONLY=1".into());
    }
    let root = PathBuf::from(std::env::var("AIWORK_BUDDY_LIVE_DATA_DIR")
        .map_err(|_| "需要显式指定 AIWORK_BUDDY_LIVE_DATA_DIR")?);
    let database = root.join("data/aiwork.sqlite");
    let snapshot = root.join("conf/vault.stronghold");
    let key = root.join("conf/vault_key.bin");
    if !database.is_file() || !snapshot.is_file() || !key.is_file() {
        return Err("现有数据库、保险库或密钥缺失；禁止创建".into());
    }
    let connection = Connection::open_with_flags(database, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|_| "只读打开数据库失败")?;
    let password = crate::vault::dpapi::unprotect(&std::fs::read(key).map_err(|_| "读取密钥失败")?)
        .map_err(|_| "本机密钥解密失败")?;
    let stronghold = Stronghold::new(&snapshot, password).map_err(|_| "只读加载保险库失败")?;
    stronghold.load_client(b"main".to_vec()).map_err(|_| "现有保险库客户端不可用")?;
    let client = stronghold.get_client(b"main".to_vec()).map_err(|_| "获取保险库客户端失败")?;
    let mut statement = connection.prepare(
        "SELECT a.pk, a.data, t.data FROM wb_accounts a LEFT JOIN wb_tokens t ON t.pk = a.pk ORDER BY a.pk LIMIT 3",
    ).map_err(|_| "账号查询失败")?;
    let rows = statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, Option<String>>(2)?))
    }).map_err(|_| "读取账号失败")?;
    let mut accounts = Vec::new();
    for (index, row) in rows.enumerate() {
        let (id, pool, token) = row.map_err(|_| "读取账号行失败")?;
        let pool: Value = serde_json::from_str(&pool).map_err(|_| "账号格式错误")?;
        let mut token: Value = serde_json::from_str(token.as_deref().unwrap_or("{}"))
            .map_err(|_| "凭证元数据格式错误")?;
        let secret = client.store().get(format!("ns:wb:{id}").as_bytes())
            .map_err(|_| "保险库读取失败")?;
        let secret: Value = secret.as_deref().map(serde_json::from_slice).transpose()
            .map_err(|_| "保险库记录格式错误")?.unwrap_or(Value::Null);
        let access_token = [token.get("access_token"), token.get("accessToken"), secret.get("access_token")]
            .into_iter().flatten().filter_map(Value::as_str).find(|value| !value.is_empty())
            .unwrap_or_default().to_string();
        let metadata = token.as_object_mut().ok_or("凭证元数据不是对象")?;
        for field in ["accessToken", "refreshToken", "refresh_token"] { metadata.remove(field); }
        metadata.insert("access_token".into(), Value::String(access_token));
        let mut creds = wb_common::creds_of(&token);
        let uid = pool.get("uid").and_then(Value::as_str).unwrap_or_default();
        let ordinal = index + 1;
        if uid.is_empty() || (!creds.uid.is_empty() && creds.uid != uid) {
            return Err(format!("account#{ordinal}: 账号身份缺失或冲突，停止查询"));
        }
        creds.uid = uid.to_string();
        if creds.access_token.is_empty() {
            println!("BUDDY_LIVE account#{ordinal}: skipped=no_access_token");
            continue;
        }
        let domain = creds.domain.trim().trim_end_matches('/');
        creds.domain = match domain {
            "" | "workbuddy.cn" | "www.workbuddy.cn" | "https://www.workbuddy.cn" |
            "codebuddy.cn" | "www.codebuddy.cn" | "https://www.codebuddy.cn" => "https://www.workbuddy.cn",
            "workbuddy.ai" | "www.workbuddy.ai" | "https://www.workbuddy.ai" => "https://www.workbuddy.ai",
            _ => return Err(format!("account#{ordinal}: 非官方域名，停止查询")),
        }.into();
        accounts.push(LiveAccount { ordinal, creds, cached_balance: pool.get("credits_balance").and_then(Value::as_f64) });
    }
    if accounts.is_empty() { return Err("没有可只读查询的账号".into()); }
    Ok(accounts)
}
