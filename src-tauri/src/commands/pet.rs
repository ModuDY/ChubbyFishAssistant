//! 桌宠窗口（pet）：可见性、位置记忆、缩放与诊断。
//!
//! 设计要点（详见 `docs/pending/桌宠结合设计方案.md`）：
//! - 窗口在 `tauri.conf.json` 中**声明式**创建（label = `pet`，透明/置顶/不进任务栏/默认隐藏），
//!   本模块只负责运行时状态，不在 Rust 里 builder 建窗；
//! - 业务数据一律不写：桌宠只订阅前端事件（签到/积分/切换），Rust 侧只碰窗口与 `pet_*` 设置；
//! - `pet_enabled=false` 时必须**真正 hide**，而不是只把画面透明——留着的透明窗口仍会截点击；
//! - 位置以**物理像素**落盘（`pet_x/pet_y`），避免不同 DPI 下逻辑坐标换算漂移。
//!
//! 文件末尾保留了渲染路线验证期的诊断命令（`?probe=1` 页面使用），不影响正式链路。

use tauri::{
    AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, PhysicalPosition, WebviewWindow,
};

use crate::state::AppState;

/// 桌宠窗口 label（与 tauri.conf.json 的 app.windows 项一致）
pub const PET_LABEL: &str = "pet";

/// 基准逻辑尺寸（素材 640×360，窗口按 1:1 逻辑像素装下角色）
const BASE_W: f64 = 300.0;
const BASE_H: f64 = 360.0;
/// 默认位置距屏幕右下角的间距（给任务栏与视觉留白）
const EDGE_RIGHT: f64 = 24.0;
const EDGE_BOTTOM: f64 = 64.0;

/// 隐藏窗口前记录的原生扩展样式（诊断用）
const GWL_EXSTYLE: i32 = -20;
/// 分层窗口（透明窗口必需）；Tauri 的 transparent(true) 会加上它
const WS_EX_LAYERED: i32 = 0x0008_0000;
/// 工具窗口：不出现在 Alt+Tab 与任务栏（Tauri 的 skipTaskbar 会加上它）
const WS_EX_TOOLWINDOW: i32 = 0x0000_0080;

// ─────────────────────────── 设置读写 ───────────────────────────

/// 合并写入 `app_settings`（与 `settings_set` 命令同一 kv 文档、同一 patch 语义）。
///
/// 为什么不复用 `commands::misc::settings_set`：那是 `#[tauri::command]`，Rust 内部调用
/// 需要传 `State`，而托盘/窗口事件里只有 `AppHandle`；这里读写的是同一份 SQLite kv。
fn patch_settings(app: &AppHandle, patch: serde_json::Value) {
    let st = app.state::<AppState>();
    let store = crate::store::db(&st.data_dir);
    let mut current: serde_json::Value = store.kv_get("app_settings");
    if !current.is_object() {
        current = serde_json::json!({});
    }
    if let (Some(obj), Some(p)) = (current.as_object_mut(), patch.as_object()) {
        for (k, v) in p {
            obj.insert(k.clone(), v.clone());
        }
    }
    if let Err(e) = store.kv_set("app_settings", &current) {
        crate::fs_utils::app_log(&st.data_dir, &format!("桌宠设置写入失败: {e}"));
    }
}

fn scale_of(settings: &crate::models::Settings) -> f64 {
    settings
        .pet_scale
        .map(|s| s as f64)
        .unwrap_or(1.0)
        .clamp(0.6, 1.5)
}

pub fn pet_window(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window(PET_LABEL)
}

/// 托盘菜单项文本（按当前启用态动态切换，沿用 API/代理菜单的同款做法）
pub fn tray_item_text(enabled: bool) -> &'static str {
    if enabled {
        "隐藏桌宠"
    } else {
        "显示桌宠"
    }
}

/// 未设置位置时的默认位：主屏右下角（考虑 DPI 缩放）。
fn default_position(window: &WebviewWindow, w: f64, h: f64) -> Option<LogicalPosition<f64>> {
    let monitor = window
        .current_monitor()
        .ok()
        .flatten()
        .or_else(|| window.primary_monitor().ok().flatten())?;
    let sf = monitor.scale_factor();
    let size = monitor.size().to_logical::<f64>(sf);
    let pos = monitor.position().to_logical::<f64>(sf);
    Some(LogicalPosition::new(
        pos.x + size.width - w - EDGE_RIGHT,
        pos.y + size.height - h - EDGE_BOTTOM,
    ))
}

/// 按设置应用窗口状态：尺寸、位置、置顶、显隐。
///
/// 调用时机：① 启动 setup；② `settings_set` 保存了 `pet_*` 字段之后；③ 托盘显隐。
/// 幂等，可重复调用。
pub fn apply_settings(app: &AppHandle) {
    let Some(window) = pet_window(app) else {
        return;
    };
    let st = app.state::<AppState>();
    let settings = st.settings();
    let scale = scale_of(&settings);
    let w = BASE_W * scale;
    let h = BASE_H * scale;

    if let Err(e) = window.set_size(LogicalSize::new(w, h)) {
        crate::fs_utils::app_log(&st.data_dir, &format!("桌宠设置尺寸失败: {e}"));
    }

    // 位置：优先用户记忆的物理坐标；否则按当前显示器右下角落位
    match (settings.pet_x, settings.pet_y) {
        (Some(x), Some(y)) => {
            let _ = window.set_position(PhysicalPosition::new(x, y));
        }
        _ => {
            if let Some(pos) = default_position(&window, w, h) {
                let _ = window.set_position(pos);
            }
        }
    }

    let _ = window.set_always_on_top(true);

    if settings.pet_enabled {
        let _ = window.show();
    } else {
        let _ = window.hide();
    }

    // 状态变化主动通知页面：窗口隐藏期间 WebView 的 rAF/定时器可能被冻结，
    // 恢复后靠这条事件立刻纠正播放状态（前端同时监听 visibilitychange 兜底）；
    // 顺带同步气泡/透明度/打盹时长，避免设置页改完要重启
    let _ = app.emit_to(
        PET_LABEL,
        "pet-update",
        serde_json::json!({
            "visible": settings.pet_enabled,
            "bubble": settings.pet_bubble.unwrap_or(true),
            "opacity": settings.pet_opacity.unwrap_or(1.0),
            "scale": scale,
            "sleepMinutes": settings.pet_sleep_minutes.unwrap_or(5),
        }),
    );
}

/// 设置启用态 + 落盘 + 应用窗口状态，返回新的启用态。
fn set_enabled(app: &AppHandle, enabled: bool) -> bool {
    patch_settings(app, serde_json::json!({ "pet_enabled": enabled }));
    apply_settings(app);
    enabled
}

// ─────────────────────────── 正式命令 ───────────────────────────

/// 显示桌宠并持久化 `pet_enabled=true`。
#[tauri::command]
pub fn pet_show(app: AppHandle) -> bool {
    set_enabled(&app, true)
}

/// 隐藏桌宠并持久化 `pet_enabled=false`。
#[tauri::command]
pub fn pet_hide(app: AppHandle) -> bool {
    set_enabled(&app, false)
}

/// 切换桌宠显隐，返回切换后的可见状态（供前端提示）。
#[tauri::command]
pub fn pet_toggle(app: AppHandle) -> bool {
    let visible = pet_window(&app)
        .and_then(|w| w.is_visible().ok())
        .unwrap_or(false);
    set_enabled(&app, !visible)
}

/// 当前桌宠状态（设置页/调试页读取）。
#[tauri::command]
pub fn pet_status(app: AppHandle) -> serde_json::Value {
    let settings = app.state::<AppState>().settings();
    let window = pet_window(&app);
    let visible = window
        .as_ref()
        .and_then(|w| w.is_visible().ok())
        .unwrap_or(false);
    // 位置以窗口实际值为准（可能与设置里的记忆值不同：用户刚拖过、还没落盘）
    let pos = window
        .as_ref()
        .and_then(|w| w.outer_position().ok())
        .map(|p| (p.x, p.y))
        .unwrap_or((settings.pet_x.unwrap_or(0), settings.pet_y.unwrap_or(0)));
    serde_json::json!({
        "enabled": settings.pet_enabled,
        "visible": visible,
        "x": pos.0,
        "y": pos.1,
        "scale": scale_of(&settings),
        "opacity": settings.pet_opacity.unwrap_or(1.0),
        "bubble": settings.pet_bubble.unwrap_or(true),
        "sleepMinutes": settings.pet_sleep_minutes.unwrap_or(5),
    })
}

/// 记录当前窗口位置（前端在 `moved` 事件去抖 500ms 后调用，无参数）。
///
/// 为什么由 Rust 读窗口位置：前后端 DPI 换算口径不同会累积漂移，
/// 直接取 `outer_position()`（物理像素）落盘，下次启动原样 set_position。
#[tauri::command]
pub fn pet_save_position(app: AppHandle) -> Result<(), String> {
    let Some(window) = pet_window(&app) else {
        return Ok(());
    };
    let pos = window.outer_position().map_err(|e| e.to_string())?;
    patch_settings(
        &app,
        serde_json::json!({ "pet_x": pos.x, "pet_y": pos.y }),
    );
    Ok(())
}

/// 双击桌宠：呼出并聚焦主窗口（复用 main.rs 的最大化兜底逻辑）。
#[tauri::command]
pub fn pet_focus_main(app: AppHandle) {
    crate::show_main_window(&app);
}

// ───────────────────── 渲染路线验证期诊断命令（保留） ─────────────────────

/// 读取窗口当前的扩展样式位（供前端对比"隐藏→恢复"前后是否被改写）。
/// 返回 (样式位原始值, 是否含 WS_EX_LAYERED, 是否含 WS_EX_TOOLWINDOW)。
#[cfg(windows)]
fn read_ex_style(window: &WebviewWindow) -> Option<(i64, bool, bool)> {
    use windows_sys::Win32::UI::WindowsAndMessaging::GetWindowLongPtrW;
    let hwnd = window.hwnd().ok()?;
    let ex = unsafe { GetWindowLongPtrW(hwnd.0 as _, GWL_EXSTYLE) } as i64;
    Some((ex, ex & WS_EX_LAYERED as i64 != 0, ex & WS_EX_TOOLWINDOW as i64 != 0))
}

#[cfg(not(windows))]
fn read_ex_style(_window: &WebviewWindow) -> Option<(i64, bool, bool)> {
    None
}

/// 恢复窗口的扩展样式位（仅置位，不清除既有位，避免破坏 Tauri 自身状态）。
#[cfg(windows)]
fn or_ex_style(window: &WebviewWindow, bits: i64) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetWindowLongPtrW, SetWindowLongPtrW};
    let Ok(hwnd) = window.hwnd() else {
        return false;
    };
    let hwnd = hwnd.0 as _;
    unsafe {
        let cur = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as i64;
        let next = cur | bits;
        if next == cur {
            return true;
        }
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, next as _);
        true
    }
}

#[cfg(not(windows))]
fn or_ex_style(_window: &WebviewWindow, _bits: i64) -> bool {
    true
}

/// 诊断页首次挂载时上报窗口状态：透明、置顶、无边框、不在任务栏。
#[tauri::command]
pub fn pet_probe_ready(window: WebviewWindow) -> Result<serde_json::Value, String> {
    let (ex, layered, toolwin) = read_ex_style(&window).unwrap_or((0, false, false));
    Ok(serde_json::json!({
        "label": window.label(),
        "transparent": window.label() == PET_LABEL,
        "alwaysOnTop": window.is_always_on_top().unwrap_or(false),
        "decorations": window.is_decorated().unwrap_or(false),
        "visible": window.is_visible().unwrap_or(false),
        "exStyle": ex,
        "exLayered": layered,
        "exToolWindow": toolwin,
    }))
}

/// 【诊断】隐藏 pet 窗口 1.5 秒后由 Rust 强制 show + 还原置顶。
///
/// 保留原因：HTML5 允许窗口不可见时暂停 rAF/定时器，该行为一旦发生，前端**无法自行恢复**
/// （页面根本没在执行）。这条命令是排查"窗口隐藏后回不来"的诊断手段。
#[tauri::command]
pub async fn pet_probe_hide_and_restore(window: WebviewWindow) -> Result<serde_json::Value, String> {
    let before = read_ex_style(&window).unwrap_or((0, false, false));
    let top_before = window.is_always_on_top().unwrap_or(false);

    window.hide().map_err(|e| e.to_string())?;
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;

    window.show().map_err(|e| e.to_string())?;
    let _ = window.set_always_on_top(true);

    // 若透明/工具窗口位在隐藏-恢复过程中被系统或 Tauri 改写，这里补回
    let repaired = or_ex_style(&window, (WS_EX_LAYERED | WS_EX_TOOLWINDOW) as i64);
    let after = read_ex_style(&window).unwrap_or((0, false, false));

    Ok(serde_json::json!({
        "hideMs": 1500,
        "alwaysOnTopBefore": top_before,
        "alwaysOnTopAfter": window.is_always_on_top().unwrap_or(false),
        "exBefore": before.0,
        "exAfter": after.0,
        "layeredAfter": after.1,
        "toolWindowAfter": after.2,
        "styleRepaired": repaired,
    }))
}

/// 把诊断页的检测结论落盘，返回文件绝对路径。
///
/// 为什么落盘而不是 console.log：Tauri dev 下 WebView 的 console 不转发到终端，
/// 落盘是唯一能稳定取回读数的通道。
#[tauri::command]
pub fn pet_probe_report(report: String) -> Result<String, String> {
    let path = std::env::temp_dir().join("pet-probe-report.json");
    std::fs::write(&path, report).map_err(|e| e.to_string())?;
    Ok(path.to_string_lossy().to_string())
}
