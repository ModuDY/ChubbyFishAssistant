use crate::state::AppState;
use tauri::{Manager, State};

#[tauri::command]
pub fn backup_unlock(password: String) -> Result<String, String> {
    let password = zeroize::Zeroizing::new(password);
    crate::backup::unlock(&password)
}

#[tauri::command]
pub async fn backup_action(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    session: String,
    request: crate::backup::Action,
) -> Result<serde_json::Value, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let restoring = matches!(&request, crate::backup::Action::Restore { .. });
        let result = crate::backup::action(&state, &session, request)?;
        if restoring && result.get("added").and_then(serde_json::Value::as_u64).is_some_and(|n| n > 0) {
            crate::commands::api_server::reload_pools_if_running(&state, app.state::<std::sync::Mutex<Option<crate::commands::api_server::ApiServerRuntime>>>().inner());
        }
        Ok(result)
    })
        .await
        .map_err(|_| "备份任务中断，请检查历史记录后重试".to_string())?
}
