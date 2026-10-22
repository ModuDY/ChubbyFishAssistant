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

/// 观测 2.0 窗口快照（5 分钟 / 1 小时，池汇总 + 账号维度）：
/// `{ running, windows: { "5m": { windowMinutes, pools }, "1h": {...} } }`
///
/// 网关未运行时返回同构空窗口，前端无需分支处理。
#[tauri::command]
pub fn evolution_metrics_v2(runtime: State<'_, Mutex<Option<ApiServerRuntime>>>) -> Value {
    let guard = runtime.lock().unwrap_or_else(|e| e.into_inner());
    match guard.as_ref() {
        Some(rt) => {
            let mut snap = rt.shared.metrics.snapshot_v2();
            if let Some(obj) = snap.as_object_mut() {
                obj.insert("running".into(), serde_json::json!(true));
            }
            snap
        }
        None => serde_json::json!({
            "running": false,
            "windows": {
                "5m": { "windowMinutes": 5, "pools": {} },
                "1h": { "windowMinutes": 60, "pools": {} },
            },
        }),
    }
}

/// 进化历史（P2 影子模式才产生数据）。
///
/// P1.5 只做观测，不计算建议，因此这里返回空列表并显式标记 `enabled:false`；
/// 面板据此显示「未启用」而不是渲染空图。历史表结构见方案 §6.4。
#[tauri::command]
pub fn evolution_history() -> Value {
    serde_json::json!({ "enabled": false, "items": [] })
}
