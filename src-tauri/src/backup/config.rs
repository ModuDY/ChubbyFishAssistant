use serde::{Deserialize, Serialize};
use std::path::Path;

pub const DEFAULT_PASSWORD: &str = "zing@123";
/// 内置默认 SMTP 授权码：仅在本机保险库从未保存过授权码时兜底，做到开箱即用。
/// 用户在面板点「清除授权码」后会显式写入空值，此后不再回退默认值（等于停用发送）。
pub const DEFAULT_AUTH_CODE: &str = "geiquyhmwgmdbcej";

#[derive(Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub enabled: bool,
    pub sender: String,
    pub recipient: String,
    pub smtp_host: String,
    pub smtp_port: u16,
    /// 0 = 每日；6 / 12 = 小时间隔。
    pub interval_hours: u8,
    pub daily_time: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: true,
            sender: "840520767@qq.com".into(),
            recipient: "zezeqiqiwudie@qq.com".into(),
            smtp_host: "smtp.qq.com".into(),
            smtp_port: 465,
            interval_hours: 0,
            daily_time: "15:30".into(),
        }
    }
}

impl Config {
    pub fn validate(&self) -> Result<(), String> {
        self.sender
            .parse::<lettre::Address>()
            .map_err(|_| "发件邮箱格式不正确")?;
        self.recipient
            .parse::<lettre::Address>()
            .map_err(|_| "收件邮箱格式不正确")?;
        if ![465, 587].contains(&self.smtp_port) {
            return Err("仅支持 TLS 465 或 STARTTLS 587".into());
        }
        if self.smtp_host.is_empty()
            || self.smtp_host.len() > 253
            || !self
                .smtp_host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
        {
            return Err("SMTP 主机名不正确".into());
        }
        if ![0, 6, 12].contains(&self.interval_hours)
            || self.daily_time.len() != 5
            || chrono::NaiveTime::parse_from_str(&self.daily_time, "%H:%M").is_err()
        {
            return Err("备份频率或每日时间不正确".into());
        }
        Ok(())
    }
}

pub fn read<T: serde::de::DeserializeOwned + Default>(dir: &Path, key: &str) -> Result<T, String> {
    crate::store::db(dir)
        .kv_get_raw_checked(key)?
        .map(|raw| {
            serde_json::from_str(&raw)
                .map_err(|_| "备份配置或历史损坏，请先修复，自动任务已暂停".into())
        })
        .unwrap_or_else(|| Ok(T::default()))
}

pub fn load(dir: &Path) -> Result<Config, String> {
    let config: Config = read(dir, "account_backup_config")?;
    config.validate()?;
    Ok(config)
}

/// 读取 SMTP 授权码：保险库有记录则用记录值（显式空串 = 停用），
/// 从未保存过记录时回退内置默认授权码（开箱即用）。
pub fn mail_secret(dir: &Path) -> Result<Option<String>, String> {
    match stored_mail_secret(dir)? {
        Some(StoredSecret::Saved(code)) => Ok(Some(code)),
        Some(StoredSecret::Cleared) => Ok(None),
        None => Ok(Some(DEFAULT_AUTH_CODE.to_owned())),
    }
}

/// 当前授权码是否来自内置默认值（未保存过自定义授权码），用于面板提示。
pub fn mail_secret_builtin(dir: &Path) -> Result<bool, String> {
    Ok(stored_mail_secret(dir)?.is_none())
}

enum StoredSecret {
    Saved(String),
    /// 用户显式「清除授权码」写入的空值，需停止自动发送。
    Cleared,
}

fn stored_mail_secret(dir: &Path) -> Result<Option<StoredSecret>, String> {
    let values = crate::vault::backup_read(dir, &["ns:backup:mail".into()])?;
    match values.into_iter().next().flatten() {
        None => Ok(None),
        Some(v) => v
            .get("authorization_code")
            .and_then(serde_json::Value::as_str)
            .map(|s| {
                if s.is_empty() {
                    StoredSecret::Cleared
                } else {
                    StoredSecret::Saved(s.to_owned())
                }
            })
            .map(Some)
            .ok_or_else(|| "邮件授权配置损坏".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn defaults_and_tls_validation() {
        let mut c = Config::default();
        assert!(c.enabled);
        assert_eq!(c.sender, "840520767@qq.com");
        assert_eq!(c.recipient, "zezeqiqiwudie@qq.com");
        assert_eq!(c.daily_time, "15:30");
        assert!(c.validate().is_ok());
        c.smtp_port = 25;
        assert!(c.validate().is_err());
        c.smtp_port = 587;
        c.daily_time = "25:00".into();
        assert!(c.validate().is_err());
    }
}
