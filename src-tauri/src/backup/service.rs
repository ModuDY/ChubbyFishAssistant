use super::{
    collector,
    config::{self, Config},
    crypto, mail,
};
use crate::state::AppState;
use chrono::{DateTime, Local, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};
use zeroize::Zeroizing;

#[derive(Clone, Serialize, Deserialize)]
pub struct Run {
    pub id: String,
    pub created_at: String,
    pub status: String,
    pub total: usize,
    #[serde(default)]
    pub missing_credentials: usize,
    pub message: String,
    pub attempt: u8,
    pub next_retry: Option<i64>,
    pub schedule_key: Option<String>,
}

pub fn history(dir: &Path) -> Result<Vec<Run>, String> {
    config::read(dir, "account_backup_history")
}

fn save_history(dir: &Path, history: &mut Vec<Run>) -> Result<(), String> {
    history.truncate(50);
    crate::store::db(dir).kv_set("account_backup_history", history)
}

fn directory(dir: &Path) -> PathBuf {
    dir.join("backups").join("accounts")
}
fn path(dir: &Path, id: &str) -> Result<PathBuf, String> {
    uuid::Uuid::parse_str(id).map_err(|_| "备份编号无效")?;
    Ok(directory(dir).join(format!("aiwork-{id}.aiwbak")))
}

pub fn read_file(path: &Path) -> Result<Vec<u8>, String> {
    let file = fs::File::open(path).map_err(|_| "无法读取备份文件")?;
    let mut bytes = Vec::new();
    file.take(crypto::MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "读取备份文件失败")?;
    if bytes.len() > crypto::MAX_BYTES {
        return Err("文件超过 15 MiB 限制".into());
    }
    Ok(bytes)
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| "无法创建加密备份文件（文件可能已存在）")?;
    if file.write_all(bytes).and_then(|_| file.sync_all()).is_err() {
        let _ = fs::remove_file(path);
        return Err("加密备份写入失败".into());
    }
    Ok(())
}

fn retain(dir: &Path) -> Result<(), String> {
    let root = directory(dir);
    let mut files = fs::read_dir(&root)
        .map_err(|_| "无法读取本地备份目录")?
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter(|e| {
            e.file_name().to_str().is_some_and(|s| {
                s.strip_prefix("aiwork-")
                    .and_then(|s| s.strip_suffix(".aiwbak"))
                    .is_some_and(|s| uuid::Uuid::parse_str(s).is_ok())
            })
        })
        .collect::<Vec<_>>();
    files.sort_by_key(|e| std::cmp::Reverse(e.metadata().and_then(|m| m.modified()).ok()));
    // Preserve any pending retry file even if it exceeds the normal 10-file retention.
    let pending: Vec<String> = history(dir)?
        .into_iter()
        .filter(|r| r.next_retry.is_some())
        .map(|r| r.id)
        .collect();
    for file in files.into_iter().skip(10) {
        if !pending
            .iter()
            .any(|id| file.file_name() == format!("aiwork-{id}.aiwbak").as_str())
        {
            fs::remove_file(file.path()).map_err(|_| "旧加密备份清理失败")?;
        }
    }
    Ok(())
}

/// 空账号池的固定错误文案：自动任务据此判为「无事可做」而非故障。
const NO_ACCOUNTS: &str = "没有可备份的账号，未生成空备份";

pub fn create(state: &AppState, schedule_key: Option<String>) -> Result<Run, String> {
    let archive = collector::collect(state)?;
    if archive.accounts.is_empty() {
        return Err(NO_ACCOUNTS.into());
    }
    let plain = Zeroizing::new(serde_json::to_vec(&archive).map_err(|_| "备份编码失败")?);
    let encrypted = crypto::seal(&plain, config::DEFAULT_PASSWORD)?;
    let id = uuid::Uuid::new_v4().to_string();
    let destination = path(&state.data_dir, &id)?;
    fs::create_dir_all(directory(&state.data_dir)).map_err(|_| "无法创建本地备份目录")?;
    let temp = destination.with_extension("tmp");
    write_new(&temp, &encrypted)?;
    fs::rename(&temp, &destination).map_err(|_| "加密备份原子保存失败")?;
    let mut run = Run {
        id,
        created_at: Utc::now().to_rfc3339(),
        status: "local".into(),
        total: archive.accounts.len(),
        missing_credentials: archive
            .accounts
            .iter()
            .filter(|e| !e.has_credential())
            .count(),
        message: "已保存本地加密备份".into(),
        attempt: 0,
        next_retry: None,
        schedule_key,
    };
    let mut history = history(&state.data_dir)?;
    history.insert(0, run.clone());
    save_history(&state.data_dir, &mut history)?;
    if run.schedule_key.is_some() {
        // Keep the schedule cursor independent of bounded history/manual exports.
        crate::store::db(&state.data_dir)
            .kv_set("account_backup_last_schedule", &run.schedule_key)?;
    }
    if retain(&state.data_dir).is_err() {
        run.message = "已保存加密备份；旧文件清理失败，请检查目录权限".into();
        history[0] = run.clone();
        save_history(&state.data_dir, &mut history)?;
    }
    Ok(run)
}

fn result(run: &mut Run, outcome: Result<(), mail::Failure>, now: i64) {
    run.next_retry = None;
    match outcome {
        Ok(()) => {
            run.status = "accepted".into();
            run.message = "SMTP 已接受，请检查收件箱（不代表已投递）".into();
        }
        Err(error) => {
            run.status = if error.auth {
                "auth_error"
            } else if error.uncertain {
                "uncertain"
            } else {
                "failed"
            }
            .into();
            run.message = error.message.into();
            if error.retry && run.attempt <= 3 {
                run.next_retry =
                    Some(now + [300, 900, 3600][run.attempt.saturating_sub(1) as usize]);
            }
        }
    }
}

pub fn deliver(state: &AppState, id: &str, config: &Config) -> Result<Run, String> {
    deliver_with(state, id, config, mail::send)
}

fn deliver_with(
    state: &AppState,
    id: &str,
    config: &Config,
    send: impl FnOnce(&Config, &str, &str, Option<Vec<u8>>, usize) -> Result<(), mail::Failure>,
) -> Result<Run, String> {
    collector::healthy(&state.data_dir)?;
    let code = Zeroizing::new(
        config::mail_secret(&state.data_dir)?.ok_or("待配置：请先填写 SMTP 授权码")?,
    );
    let mut history = history(&state.data_dir)?;
    let index = history
        .iter()
        .position(|r| r.id == id)
        .ok_or("未找到备份记录")?;
    if history[index].status == "accepted" {
        return Err("此备份已被 SMTP 接受，无需重复发送".into());
    }
    let bytes = match read_file(&path(&state.data_dir, id)?) {
        Ok(bytes) => bytes,
        Err(error) => {
            history[index].next_retry = None;
            history[index].status = "failed".into();
            history[index].message = "本地加密附件不可读，已取消重试，请重新备份".into();
            save_history(&state.data_dir, &mut history)?;
            return Err(error);
        }
    };
    if crypto::open(&bytes, config::DEFAULT_PASSWORD).is_err() {
        history[index].next_retry = None;
        history[index].status = "failed".into();
        history[index].message = "本地附件认证失败，已取消发送，请重新备份".into();
        save_history(&state.data_dir, &mut history)?;
        return Err("本地附件损坏，禁止发送未经认证的文件".into());
    }
    let run = &mut history[index];
    run.attempt = run.attempt.saturating_add(1);
    run.status = "sending".into();
    run.message = "发送中；若应用中断，请先检查邮箱再手动重试".into();
    run.next_retry = None;
    let total = run.total;
    save_history(&state.data_dir, &mut history)?;
    let outcome = send(config, &code, id, Some(bytes), total);
    result(&mut history[index], outcome, Utc::now().timestamp());
    let finished = history[index].clone();
    save_history(&state.data_dir, &mut history)?;
    Ok(finished)
}

pub fn export(state: &AppState, destination: &Path) -> Result<Value, String> {
    let run = create(state, None)?;
    write_new(destination, &read_file(&path(&state.data_dir, &run.id)?)?)?;
    Ok(json!({"total": run.total, "message":"已导出加密备份"}))
}

pub fn status(state: &AppState) -> Result<Value, String> {
    let config = config::load(&state.data_dir)?;
    let configured = config::mail_secret(&state.data_dir)?.is_some();
    let builtin = config::mail_secret_builtin(&state.data_dir)?;
    let history = history(&state.data_dir)?;
    let auth_paused = history
        .iter()
        .find(|r| r.attempt > 0)
        .is_some_and(|r| r.status == "auth_error");
    let runtime: Value = config::read(&state.data_dir, "account_backup_runtime")?;
    let status = if collector::healthy(&state.data_dir).is_err() {
        "存储异常，已暂停"
    } else if !config.enabled {
        "已关闭"
    } else if !configured {
        "待配置 SMTP 授权码"
    } else if auth_paused {
        "身份验证失败，重新保存配置后恢复"
    } else {
        "已开启，等待计划时间"
    };
    Ok(
        json!({"config":config,"smtp_configured":configured,"smtp_builtin":builtin,"status":status,"history":history,"runtime":runtime,"scope":"all","retention":10}),
    )
}

pub fn schedule_key(config: &Config, now: DateTime<Local>) -> Option<String> {
    if !config.enabled {
        return None;
    }
    if config.interval_hours == 0 {
        let time = chrono::NaiveTime::parse_from_str(&config.daily_time, "%H:%M").ok()?;
        (now.time() >= time).then(|| format!("daily:{}", now.format("%Y-%m-%d")))
    } else {
        Some(format!(
            "{}h:{}",
            config.interval_hours,
            now.timestamp()
                .div_euclid(config.interval_hours as i64 * 3600)
        ))
    }
}

pub fn tick(state: &AppState) -> Result<(), String> {
    collector::healthy(&state.data_dir)?;
    let config = config::load(&state.data_dir)?;
    if !config.enabled || config::mail_secret(&state.data_dir)?.is_none() {
        return Ok(());
    }
    let history = history(&state.data_dir)?;
    if history
        .iter()
        .find(|r| r.attempt > 0)
        .is_some_and(|r| r.status == "auth_error")
    {
        return Ok(());
    }
    if let Some(run) = history
        .iter()
        .find(|r| r.next_retry.is_some_and(|n| n <= Utc::now().timestamp()))
    {
        deliver(state, &run.id, &config)?;
        return Ok(());
    }
    if let Some(key) = schedule_key(&config, Local::now()) {
        let last: Option<String> = config::read(&state.data_dir, "account_backup_last_schedule")?;
        if last.as_deref() != Some(&key)
            && !history
                .iter()
                .any(|r| r.schedule_key.as_deref() == Some(&key))
        {
            let run = match create(state, Some(key)) {
                Ok(run) => run,
                // 池内还没有账号：不算故障，等有账号后的下一轮再备份（避免每分钟刷面板报错）。
                Err(error) if error == NO_ACCOUNTS => return Ok(()),
                Err(error) => return Err(error),
            };
            deliver(state, &run.id, &config)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retention_preserves_pending_and_unrelated_files_and_export_never_overwrites() {
        let dir =
            std::env::temp_dir().join(format!("aiwork-backup-retention-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(directory(&dir)).unwrap();
        let mut ids = Vec::new();
        for index in 0..12 {
            let id = uuid::Uuid::new_v4().to_string();
            let file = path(&dir, &id).unwrap();
            write_new(&file, b"synthetic ciphertext fixture").unwrap();
            OpenOptions::new()
                .write(true)
                .open(&file)
                .unwrap()
                .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(100 + index))
                .unwrap();
            ids.push(id);
        }
        let unrelated = directory(&dir).join("personal-notes.txt");
        fs::write(&unrelated, b"preserve this file").unwrap();
        let pending = Run {
            id: ids[0].clone(),
            created_at: "fixture".into(),
            status: "failed".into(),
            total: 1,
            missing_credentials: 0,
            message: "fixture".into(),
            attempt: 1,
            next_retry: Some(100),
            schedule_key: None,
        };
        crate::store::db(&dir)
            .kv_set("account_backup_history", &vec![pending])
            .unwrap();
        retain(&dir).unwrap();
        assert!(path(&dir, &ids[0]).unwrap().exists());
        assert!(!path(&dir, &ids[1]).unwrap().exists());
        assert!(unrelated.exists());
        assert!(write_new(&unrelated, b"must not overwrite").is_err());
        assert_eq!(fs::read(&unrelated).unwrap(), b"preserve this file");
        crate::store::db(&dir)
            .kv_set("account_backup_history", &Vec::<Run>::new())
            .unwrap();
        retain(&dir).unwrap();
        assert!(!path(&dir, &ids[0]).unwrap().exists());
        assert!(ids[2..].iter().all(|id| path(&dir, id).unwrap().exists()));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn mock_smtp_reuses_ciphertext_and_never_resends_accepted_or_corrupt_files() {
        use std::sync::{Arc, Mutex};
        let state = AppState {
            data_dir: std::env::temp_dir()
                .join(format!("aiwork-backup-mail-test-{}", uuid::Uuid::new_v4())),
            jwt_refresh_lock: Arc::new(Mutex::new(())),
            wb_pool_lock: Arc::new(Mutex::new(())),
            qoder_pool_lock: Arc::new(Mutex::new(())),
            doubao_pool_lock: Arc::new(Mutex::new(())),
            group_defs_lock: Arc::new(Mutex::new(())),
        };
        let db = crate::store::db(&state.data_dir);
        // 未保存过授权码时使用内置默认值（开箱即用），显式清除后才停用。
        assert_eq!(
            config::mail_secret(&state.data_dir).unwrap().as_deref(),
            Some(config::DEFAULT_AUTH_CODE)
        );
        assert!(config::mail_secret_builtin(&state.data_dir).unwrap());
        // 内置授权码 + 空账号池：不生成附件、不落历史，也不算故障。
        tick(&state).unwrap();
        assert!(history(&state.data_dir).unwrap().is_empty());
        assert!(!directory(&state.data_dir).exists());
        crate::vault::ns_set(
            &state.data_dir,
            "backup",
            "mail",
            &json!({"authorization_code": ""}),
        )
        .unwrap();
        assert!(!config::mail_secret_builtin(&state.data_dir).unwrap());
        // 已清除授权码：自动任务不尝试 SMTP，也不生成空备份文件。
        tick(&state).unwrap();
        assert!(history(&state.data_dir).unwrap().is_empty());
        db.row_upsert(
            "wb_accounts",
            "wb-fixture",
            &json!({"id":"wb-fixture","uid":"fixture-user"}),
        )
        .unwrap();
        crate::vault::ns_set(
            &state.data_dir,
            "wb",
            "wb-fixture",
            &json!({"access_token":"synthetic-access"}),
        )
        .unwrap();
        crate::vault::ns_set(
            &state.data_dir,
            "backup",
            "mail",
            &json!({"authorization_code":"synthetic-code"}),
        )
        .unwrap();
        let config = Config::default();
        let run = create(&state, None).unwrap();
        let original = read_file(&path(&state.data_dir, &run.id).unwrap()).unwrap();
        let mut calls = 0;
        let failed = deliver_with(&state, &run.id, &config, |_, code, id, bytes, total| {
            calls += 1;
            assert_eq!(code, "synthetic-code");
            assert_eq!(id, run.id);
            assert_eq!(bytes.unwrap(), original);
            assert_eq!(total, 1);
            Err(mail::Failure {
                message: "synthetic temporary rejection",
                retry: true,
                uncertain: false,
                auth: false,
            })
        })
        .unwrap();
        assert_eq!(failed.status, "failed");
        assert!(failed.next_retry.is_some());
        let accepted = deliver_with(&state, &run.id, &config, |_, _, _, bytes, _| {
            calls += 1;
            assert_eq!(bytes.unwrap(), original);
            Ok(())
        })
        .unwrap();
        assert_eq!(accepted.status, "accepted");
        assert_eq!(accepted.attempt, 2);
        assert!(deliver_with(&state, &run.id, &config, |_, _, _, _, _| {
            calls += 1;
            Ok(())
        })
        .is_err());
        assert_eq!(calls, 2);
        let second = create(&state, None).unwrap();
        fs::write(
            path(&state.data_dir, &second.id).unwrap(),
            b"this-is-not-an-encrypted-backup",
        )
        .unwrap();
        assert!(deliver_with(&state, &second.id, &config, |_, _, _, _, _| {
            calls += 1;
            Ok(())
        })
        .is_err());
        assert_eq!(calls, 2);
        assert!(history(&state.data_dir).unwrap()[0].next_retry.is_none());
        let _ = fs::remove_dir_all(&state.data_dir);
    }

    #[test]
    fn scheduling_and_bounded_retry() {
        let mut c = Config::default();
        let before: DateTime<Local> = DateTime::parse_from_rfc3339("2026-10-17T09:29:00+08:00")
            .unwrap()
            .with_timezone(&Local);
        // Local timezone in CI may differ: construct local times explicitly.
        use chrono::TimeZone;
        let before = Local
            .from_local_datetime(&before.naive_local().date().and_hms_opt(9, 29, 0).unwrap())
            .single()
            .unwrap();
        assert!(schedule_key(&c, before).is_none());
        assert!(schedule_key(&c, before + chrono::Duration::minutes(1)).is_some());
        c.enabled = false;
        assert!(schedule_key(&c, before + chrono::Duration::hours(2)).is_none());
        let mut run = Run {
            id: "test".into(),
            created_at: "test".into(),
            status: "sending".into(),
            total: 1,
            missing_credentials: 0,
            message: String::new(),
            attempt: 1,
            next_retry: None,
            schedule_key: None,
        };
        for (attempt, delay) in [(1, 300), (2, 900), (3, 3600), (4, 0)] {
            run.attempt = attempt;
            result(
                &mut run,
                Err(mail::Failure {
                    message: "synthetic",
                    retry: true,
                    uncertain: false,
                    auth: false,
                }),
                100,
            );
            assert_eq!(
                run.next_retry,
                if delay == 0 { None } else { Some(100 + delay) }
            );
        }
        result(&mut run, Ok(()), 100);
        assert_eq!(run.status, "accepted");
        assert!(run.next_retry.is_none());
        result(
            &mut run,
            Err(mail::Failure {
                message: "unknown",
                retry: false,
                uncertain: true,
                auth: false,
            }),
            100,
        );
        assert_eq!(run.status, "uncertain");
        assert!(run.next_retry.is_none());
    }
}
