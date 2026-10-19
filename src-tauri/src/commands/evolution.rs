//! 自我进化 P1：网关观测层的读取接口。
//!
//! 目前**只有读**：把按池累计的成功 / 失败 / 耗时 / token 以快照形式暴露给前端。
//! 数据不参与任何路由决策——决策（P2 自适应调参）要等真实分布积累出来再做，
//! 否则调出来的参数没有依据。见 `docs/pending/自我进化方案.md`。
//!
//! 指标口径见 `api_server/metrics.rs`：
//! - `ok/fail/total` 进程内累计；
//! - `ewmaSuccess` / `ewmaLatencyMs` 指数滑动（α=0.05），抗尖峰；
//! - 进程重启清零（P1 阶段不做持久化）。
use serde_json::Value;
use std::sync::Mutex;
use tauri::State;

use crate::commands::api_server::ApiServerRuntime;

/// 网关观测快照：
/// `{ running, pools: { trae|wb|qoder|doubao|custom: {...} }, total: {...} }`
///
/// 网关未运行时返回 `running:false` 与空池，而不是报错——面板/调试随时可调用。
#[tauri::command]
pub fn evolution_metrics(runtime: State<'_, Mutex<Option<ApiServerRuntime>>>) -> Value {
    let guard = runtime.lock().unwrap_or_else(|e| e.into_inner());
    match guard.as_ref() {
        Some(rt) => {
            let mut snap = rt.shared.metrics.snapshot();
            if let Some(obj) = snap.as_object_mut() {
                obj.insert("running".into(), serde_json::json!(true));
            }
            snap
        }
        None => serde_json::json!({
            "running": false,
            "pools": {},
            "total": { "ok": 0, "fail": 0, "tokens": 0 },
        }),
    }
}
