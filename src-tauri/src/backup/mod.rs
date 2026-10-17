//! 本应用账号凭据的加密邮件备份。入口受短期后端会话保护，后台调度不依赖 WebView。
mod collector;
mod config;
mod crypto;
mod mail;
mod service;

use crate::state::AppState;
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex, MutexGuard, TryLockError,
    },
    time::{Duration, Instant},
};
use tauri::Manager;

static OPERATION: Mutex<()> = Mutex::new(());
static SESSION: Mutex<Option<Session>> = Mutex::new(None);
static AUTH: Mutex<(u8, Option<Instant>)> = Mutex::new((0, None));
struct Session {
    token: String,
    expires: Instant,
    preview: Option<(String, Instant, collector::Archive)>,
}

fn operation() -> Result<MutexGuard<'static, ()>, String> {
    match OPERATION.try_lock() {
        Ok(guard) => Ok(guard),
        Err(TryLockError::Poisoned(error)) => Ok(error.into_inner()),
        Err(TryLockError::WouldBlock) => Err("备份任务正在执行，请稍后重试".into()),
    }
}

fn expire_session() {
    let mut session = SESSION.lock().unwrap_or_else(|e| e.into_inner());
    if session
        .as_ref()
        .is_some_and(|s| Instant::now() >= s.expires)
    {
        *session = None;
    } else if let Some(s) = session.as_mut() {
        if s.preview
            .as_ref()
            .is_some_and(|(_, created, _)| created.elapsed() > Duration::from_secs(300))
        {
            s.preview = None;
        }
    }
}

pub fn unlock(password: &str) -> Result<String, String> {
    let mut auth = AUTH.lock().unwrap_or_else(|e| e.into_inner());
    if auth.1.is_some_and(|until| Instant::now() < until) {
        return Err("尝试次数过多，请 30 秒后重试".into());
    }
    if password != config::DEFAULT_PASSWORD {
        auth.0 += 1;
        if auth.0 >= 5 {
            auth.0 = 0;
            auth.1 = Some(Instant::now() + Duration::from_secs(30));
        }
        return Err("入口密码错误".into());
    }
    *auth = (0, None);
    let token = uuid::Uuid::new_v4().to_string();
    *SESSION.lock().unwrap_or_else(|e| e.into_inner()) = Some(Session {
        token: token.clone(),
        expires: Instant::now() + Duration::from_secs(900),
        preview: None,
    });
    Ok(token)
}

fn session_mut<'a>(
    session: &'a mut Option<Session>,
    token: &str,
) -> Result<&'a mut Session, String> {
    if session
        .as_ref()
        .is_some_and(|s| Instant::now() >= s.expires)
    {
        *session = None;
    }
    session
        .as_mut()
        .filter(|s| s.token == token)
        .ok_or_else(|| "备份面板已锁定或会话过期，请重新输入入口密码".into())
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    Status,
    Save {
        config: config::Config,
        authorization_code: Option<String>,
    },
    TestMail,
    Run,
    Retry {
        id: String,
    },
    Export {
        path: String,
    },
    Preview {
        path: String,
        password: String,
    },
    Restore {
        preview_id: String,
    },
    Lock,
}

pub fn action(state: &AppState, token: &str, action: Action) -> Result<Value, String> {
    session_mut(
        &mut SESSION.lock().unwrap_or_else(|e| e.into_inner()),
        token,
    )?;
    // Closing a panel always revokes access, even while a scheduler operation is running.
    if matches!(action, Action::Lock) {
        *SESSION.lock().unwrap_or_else(|e| e.into_inner()) = None;
        return Ok(json!({}));
    }
    let _op = operation()?;
    match action {
        Action::Status => service::status(state),
        Action::Save {
            config,
            authorization_code,
        } => {
            collector::healthy(&state.data_dir)?;
            config.validate()?;
            if config.sender != config::load(&state.data_dir)?.sender
                && authorization_code
                    .as_ref()
                    .is_none_or(|c| c.trim().is_empty())
            {
                return Err("更改发件邮箱时请重新填写对应的 SMTP 授权码".into());
            }
            if let Some(code) = authorization_code {
                let code = zeroize::Zeroizing::new(code);
                if code.len() > 512 || code.contains(['\r', '\n']) {
                    return Err("授权码格式不正确".into());
                }
                crate::vault::ns_set(
                    &state.data_dir,
                    "backup",
                    "mail",
                    &json!({"authorization_code": code.trim()}),
                )
                .map_err(|_| "SMTP 授权码加密保存失败")?;
            }
            crate::store::db(&state.data_dir).kv_set("account_backup_config", &config)?;
            let mut history = service::history(&state.data_dir)?;
            for run in &mut history {
                run.next_retry = None;
                if run.status == "auth_error" {
                    run.status = "failed".into();
                    run.message = "配置已重新保存，可手动重试".into();
                }
            }
            crate::store::db(&state.data_dir).kv_set("account_backup_history", &history)?;
            service::status(state)
        }
        Action::TestMail => {
            let config = config::load(&state.data_dir)?;
            let code = zeroize::Zeroizing::new(
                config::mail_secret(&state.data_dir)?.ok_or("请先保存 SMTP 授权码")?,
            );
            mail::send(&config, &code, &uuid::Uuid::new_v4().to_string(), None, 0)
                .map_err(|e| e.message.to_string())?;
            Ok(json!({"message":"测试邮件已被 SMTP 接受，请检查收件箱"}))
        }
        Action::Run => {
            let run = service::create(state, None)?;
            let run = if config::mail_secret(&state.data_dir)?.is_some() {
                service::deliver(state, &run.id, &config::load(&state.data_dir)?)?
            } else {
                run
            };
            serde_json::to_value(run).map_err(|_| "结果编码失败".into())
        }
        Action::Retry { id } => serde_json::to_value(service::deliver(
            state,
            &id,
            &config::load(&state.data_dir)?,
        )?)
        .map_err(|_| "结果编码失败".into()),
        Action::Export { path } => service::export(state, std::path::Path::new(&path)),
        Action::Preview { path, password } => {
            let password = zeroize::Zeroizing::new(password);
            let password = if password.is_empty() {
                config::DEFAULT_PASSWORD
            } else {
                &password
            };
            let bytes = service::read_file(std::path::Path::new(&path))?;
            let archive = collector::parse(&crypto::open(&bytes, password)?)?;
            let mut view = collector::preview(state, &archive)?;
            let id = uuid::Uuid::new_v4().to_string();
            view["preview_id"] = json!(id);
            session_mut(
                &mut SESSION.lock().unwrap_or_else(|e| e.into_inner()),
                token,
            )?
            .preview = Some((id, Instant::now(), archive));
            Ok(view)
        }
        Action::Restore { preview_id } => {
            let mut session = SESSION.lock().unwrap_or_else(|e| e.into_inner());
            let s = session_mut(&mut session, token)?;
            let (id, created, archive) = s.preview.take().ok_or("请先选择文件并预览")?;
            if id != preview_id || created.elapsed() > Duration::from_secs(300) {
                return Err("恢复预览已失效，请重新选择文件".into());
            }
            drop(session);
            collector::restore(state, &archive)
        }
        Action::Lock => unreachable!(),
    }
}

pub fn start(app: tauri::AppHandle) {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(90));
        loop {
            expire_session();
            if let Ok(_guard) = operation() {
                // Status and sanitized failure details are available only in the unlocked panel.
                // Avoid logging any metadata that could accidentally contain credentials.
                let state = app.state::<AppState>();
                let outcome = service::tick(&state);
                let _ = crate::store::db(&state.data_dir).kv_set(
                    "account_backup_runtime",
                    &json!({
                        "checked_at": chrono::Utc::now().to_rfc3339(), "error": outcome.err()
                    }),
                );
            }
            std::thread::sleep(Duration::from_secs(60));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sessions_require_backend_unlock_and_expire() {
        assert!(unlock("wrong-password").is_err());
        let token = unlock(config::DEFAULT_PASSWORD).unwrap();
        let mut session = SESSION.lock().unwrap_or_else(|e| e.into_inner());
        assert!(session_mut(&mut session, "wrong-token").is_err());
        assert!(session_mut(&mut session, &token).is_ok());
        session.as_mut().unwrap().expires = Instant::now() - Duration::from_secs(1);
        assert!(session_mut(&mut session, &token).is_err());
        assert!(session.is_none());
    }
}
