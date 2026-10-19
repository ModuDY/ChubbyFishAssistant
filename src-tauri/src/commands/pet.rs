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

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use tauri::{
    AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, PhysicalPosition, WebviewWindow,
};

use crate::state::AppState;

/// 桌宠窗口 label（与 tauri.conf.json 的 app.windows 项一致）
pub const PET_LABEL: &str = "pet";

/// 基准逻辑尺寸（素材 640×360，窗口按 1:1 逻辑像素装下角色）
const BASE_W: f64 = 300.0;
const BASE_H: f64 = 360.0;
/// 窗口右侧额外留白：右键菜单要浮在角色**右边**且不被窗口裁剪，
/// 而这部分区域全透明——点击穿透（start_hit_watcher）会让它正常放行鼠标。
/// 必须与 `pet.css` 的 `--pet-menu-space` 保持一致。
const MENU_SPACE: f64 = 160.0;
/// 默认位置距屏幕右下角的间距（给任务栏与视觉留白）
const EDGE_RIGHT: f64 = 24.0;
const EDGE_BOTTOM: f64 = 64.0;

/// 隐藏窗口前记录的原生扩展样式（诊断用）
const GWL_EXSTYLE: i32 = -20;
/// 分层窗口（透明窗口必需）；Tauri 的 transparent(true) 会加上它
const WS_EX_LAYERED: i32 = 0x0008_0000;
/// 工具窗口：不出现在 Alt+Tab 与任务栏（Tauri 的 skipTaskbar 会加上它）
const WS_EX_TOOLWINDOW: i32 = 0x0000_0080;
/// 应用窗口位：会把窗口钉在任务栏。实测正式窗口会同时带 TOOLWINDOW 与 APPWINDOW，
/// 需显式清掉后者，否则任务栏可能留一个按钮（详见设计方案 T0/§10.9）
const WS_EX_APPWINDOW: i32 = 0x0004_0000;

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
    // w = 角色舞台宽度（视频按它铺满），窗口再额外加一段右侧留白给菜单
    let w = BASE_W * scale;
    let h = BASE_H * scale;
    let win_w = w + MENU_SPACE;

    if let Err(e) = window.set_size(LogicalSize::new(win_w, h)) {
        crate::fs_utils::app_log(&st.data_dir, &format!("桌宠设置尺寸失败: {e}"));
    }

    // 位置：优先用户记忆的物理坐标；否则按当前显示器右下角落位
    match (settings.pet_x, settings.pet_y) {
        (Some(x), Some(y)) => {
            let _ = window.set_position(PhysicalPosition::new(x, y));
        }
        _ => {
            if let Some(pos) = default_position(&window, win_w, h) {
                let _ = window.set_position(pos);
            }
        }
    }

    let _ = window.set_always_on_top(true);

    if settings.pet_enabled {
        let _ = window.show();
        // show 时系统/Tauri 可能补回 WS_EX_APPWINDOW（任务栏图标）；显式清掉，
        // 与 skipTaskbar 的意图保持一致
        let _ = and_not_ex_style(&window, WS_EX_APPWINDOW as i64);
    } else {
        let _ = window.hide();
    }

    // 托盘「显示/隐藏桌宠」菜单文本在此同步——`apply_settings` 是所有显隐路径的公共出口
    // （托盘开关 / 桌宠右键菜单 / 设置页开关 / 关闭窗口 / 启动），一处覆盖全部来源
    sync_tray_pet_text(app, settings.pet_enabled);

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
            // P3：AI 档位与人格随设置即时下发（改完不必重启）
            "aiMode": settings.pet_ai_mode.clone().unwrap_or_else(|| "off".into()),
            "persona": settings.pet_persona.clone().unwrap_or_else(|| "whale".into()),
        }),
    );
}

/// 同步托盘「显示桌宠 / 隐藏桌宠」菜单文本。
///
/// 托盘菜单是**原生、持久**控件，不会自动感知状态变化——只有显式 `set_text` 才会刷新。
/// 因此把它收敛成单一咽喉点：凡改变 `pet_enabled` 的路径都必须经 `set_enabled`，
/// 由后者统一同步，覆盖「桌宠右键菜单 / 设置页 / 托盘 / 关闭窗口」四条来源，
/// 避免出现「从桌宠菜单隐藏后，托盘仍写着『隐藏桌宠』」这类两侧不一致。
pub fn sync_tray_pet_text(app: &AppHandle, enabled: bool) {
    if let Some(tray) = app.try_state::<crate::TrayMenu>() {
        let _ = tray.pet_item.set_text(tray_item_text(enabled));
    }
}

/// 设置启用态 + 落盘 + 应用窗口状态，返回新的启用态。
///
/// 托盘菜单文本不在这里同步——统一由 `apply_settings` 负责（见其注释），
/// 这样设置页直接改 `pet_enabled` 的路径也能覆盖。
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
        // P3：AI 档位/人格（首启桌宠窗口读一次，避免只依赖 pet-update 事件）
        "aiMode": settings.pet_ai_mode.clone().unwrap_or_else(|| "off".into()),
        "persona": settings.pet_persona.clone().unwrap_or_else(|| "whale".into()),
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

/// 清除窗口的扩展样式位（`or_ex_style` 的对称操作）。
///
/// 用于抹掉系统/Tauri 在 show 时给置顶透明窗误补的 `WS_EX_APPWINDOW`，
/// 否则桌宠会在任务栏留下一个按钮，与 `skipTaskbar` 的意图相反。
#[cfg(windows)]
fn and_not_ex_style(window: &WebviewWindow, bits: i64) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetWindowLongPtrW, SetWindowLongPtrW};
    let Ok(hwnd) = window.hwnd() else {
        return false;
    };
    let hwnd = hwnd.0 as _;
    unsafe {
        let cur = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as i64;
        let next = cur & !bits;
        if next == cur {
            return true;
        }
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, next as _);
        true
    }
}

#[cfg(not(windows))]
fn and_not_ex_style(_window: &WebviewWindow, _bits: i64) -> bool {
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

// ───────────────── 透明区域点击穿透（Windows） ─────────────────
//
// 问题：窗口是「视频宽 × 气泡留白」的矩形，角色只占底部一块，其余区域 alpha=0
// 视觉上不存在，但原生窗口仍然**整块接收鼠标消息**——点在角色旁边的空白上不会落到
// 下层窗口，表现为「角色周围有一层看不见的遮罩挡住点击」（用户实测反馈）。
//
// 方案：前端按当前画面 alpha 采样出「可点击格子」（PetSprite 的 mask 上报），
// Rust 侧用一个轻量后台线程跟随光标：光标落在透明格子时给窗口加 WS_EX_TRANSPARENT
// （Tauri 的 set_ignore_cursor_events），鼠标消息即穿透到下层窗口；回到角色格子再撤掉。
//
// 为什么不用 SetWindowRgn：窗口区域会**连同渲染一起裁剪**，角色头顶的气泡会被切掉。

/// 前端上报的「可点击格子」掩码（窗口百分比网格，1=角色 0=透明）
#[cfg(windows)]
struct HitMask {
    cols: u16,
    rows: u16,
    /// 行优先排列，长度 = cols × rows
    cells: Vec<u8>,
}

#[cfg(windows)]
static HIT_MASK: std::sync::Mutex<Option<HitMask>> = std::sync::Mutex::new(None);

/// 上报「可点击格子」掩码：角色换动作/重新采样时由前端调用（幂等，覆盖旧值）。
#[tauri::command]
#[cfg_attr(not(windows), allow(unused_variables))]
pub fn pet_set_hit_mask(cols: u16, rows: u16, cells: Vec<u8>) {
    #[cfg(windows)]
    {
        if cols == 0 || rows == 0 || cells.len() != cols as usize * rows as usize {
            return;
        }
        // 全透明（采样失败/首帧未解码）一律不采用：否则整窗穿透，用户再也点不到角色
        if !cells.iter().any(|c| *c != 0) {
            return;
        }
        if let Ok(mut slot) = HIT_MASK.lock() {
            *slot = Some(HitMask { cols, rows, cells });
        }
    }
}

/// 启动光标跟随线程（只需调用一次；重复调用为空操作）。
pub fn start_hit_watcher(app: AppHandle) {
    #[cfg(windows)]
    {
        use std::sync::atomic::{AtomicBool, Ordering};
        static STARTED: AtomicBool = AtomicBool::new(false);
        if STARTED.swap(true, Ordering::SeqCst) {
            return;
        }

        std::thread::spawn(move || {
            let mut ignoring = false;
            // 光标贴着角色边缘时逐帧可能反复横跳；状态需连续两帧一致才切换，
            // 避免 WS_EX_LAYERED 反复增删引起的闪烁
            let mut pending: Option<bool> = None;
            loop {
                // 24ms ≈ 40Hz：足够跟手，单次只有一次 GetCursorPos + 一次查表
                std::thread::sleep(std::time::Duration::from_millis(24));

                let Some(window) = pet_window(&app) else {
                    continue;
                };
                if !window.is_visible().unwrap_or(false) {
                    if ignoring {
                        let _ = window.set_ignore_cursor_events(false);
                        ignoring = false;
                    }
                    pending = None;
                    continue;
                }
                // 左键按住期间（拖拽中/点住角色）一律不切换：
                // 切到穿透会打断 startDragging 的原生拖拽循环
                if left_button_down() {
                    pending = None;
                    continue;
                }

                let want = transparent_under_cursor(&window);
                if want == ignoring {
                    pending = None;
                    continue;
                }
                if pending == Some(want) {
                    if window.set_ignore_cursor_events(want).is_ok() {
                        ignoring = want;
                    }
                    pending = None;
                } else {
                    pending = Some(want);
                }
            }
        });
    }
    #[cfg(not(windows))]
    {
        let _ = app;
    }
}

#[cfg(windows)]
fn left_button_down() -> bool {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};
    unsafe { (GetAsyncKeyState(VK_LBUTTON as i32) as u16 & 0x8000) != 0 }
}

// ───────────────── 置顶 watchdog：前台全屏自动降级隐藏 ─────────────────
//
// 问题：桌宠是 always-on-top 窗口，前台跑全屏应用（游戏 / 全屏视频 / 演示）时会一直压在
// 上面，既碍事又可能挡住画面。
//
// 方案：后台线程低频轮询前台窗口，若其矩形覆盖整个显示器（判为全屏），就让桌宠临时**隐藏**；
// 全屏退出后按 `pet_enabled` 自动恢复。只对账窗口显隐，**不写设置**——用户「显示桌宠」的意愿
// （pet_enabled）不被覆盖（区别于 pet_show/pet_hide，那两个会落盘）。

/// 启动置顶 watchdog（只需调用一次；重复调用为空操作）。
pub fn start_top_watchdog(app: AppHandle) {
    #[cfg(windows)]
    {
        use std::sync::atomic::{AtomicBool, Ordering};
        static STARTED: AtomicBool = AtomicBool::new(false);
        if STARTED.swap(true, Ordering::SeqCst) {
            return;
        }

        std::thread::spawn(move || loop {
            // 1s 一轮：全屏切换不是高频事件，低频即可，单轮只有两次廉价查询
            std::thread::sleep(std::time::Duration::from_millis(1000));

            let Some(window) = pet_window(&app) else {
                continue;
            };
            // 期望可见 = 用户设置要显示 且 前台不是全屏应用；与实际状态对账（幂等，可自愈）
            let want =
                app.state::<AppState>().settings().pet_enabled && !foreground_is_fullscreen();
            let visible = window.is_visible().unwrap_or(false);
            if want == visible {
                continue;
            }

            let patch = if want {
                let _ = window.show();
                // 与 apply_settings 一致：show 后清掉任务栏位
                let _ = and_not_ex_style(&window, WS_EX_APPWINDOW as i64);
                serde_json::json!({ "visible": true })
            } else {
                let _ = window.hide();
                serde_json::json!({ "visible": false })
            };
            // 同步前端播放状态：窗口隐藏期间 WebView 可能冻结定时器，恢复后需事件纠偏
            let _ = app.emit_to(PET_LABEL, "pet-update", patch);
        });
    }
    #[cfg(not(windows))]
    {
        let _ = app;
    }
}

/// 前台窗口是否为「铺满整个显示器」的全屏应用（游戏 / 全屏视频等）。
///
/// 判据：前台窗口可见、未最小化、不属于本进程，且矩形覆盖其所在显示器的外沿。
/// 排除桌面外壳（Progman/WorkerW）与任务栏，否则在桌面点一下就会被误判成全屏。
#[cfg(windows)]
fn foreground_is_fullscreen() -> bool {
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetClassNameW, GetForegroundWindow, GetWindowRect, GetWindowThreadProcessId, IsIconic,
        IsWindowVisible,
    };

    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_null() {
            return false;
        }
        if IsWindowVisible(hwnd) == 0 || IsIconic(hwnd) != 0 {
            return false;
        }

        // 本进程窗口（main / pet）不参与判定，避免自己的窗口触发降级
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid == std::process::id() {
            return false;
        }

        // 桌面外壳：点击桌面时前台会变成它，其矩形覆盖整屏
        let mut buf = [0u16; 64];
        let n = GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
        if n > 0 {
            let class = String::from_utf16_lossy(&buf[..n as usize]);
            if class == "Progman" || class == "WorkerW" || class == "Shell_TrayWnd" {
                return false;
            }
        }

        let mut rect = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        if GetWindowRect(hwnd, &mut rect) == 0 {
            return false;
        }

        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        if monitor.is_null() {
            return false;
        }
        let mut mi = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            rcMonitor: RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            },
            rcWork: RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            },
            dwFlags: 0,
        };
        if GetMonitorInfoW(monitor, &mut mi) == 0 {
            return false;
        }

        // 容差 1px：部分全屏窗带 1 像素边框偏差
        const TOL: i32 = 1;
        let m = mi.rcMonitor;
        rect.left <= m.left + TOL
            && rect.top <= m.top + TOL
            && rect.right >= m.right - TOL
            && rect.bottom >= m.bottom - TOL
    }
}

#[cfg(not(windows))]
fn foreground_is_fullscreen() -> bool {
    false
}

/// 光标是否落在角色的**透明**区域上。
///
/// 一切异常（不在窗口内、掩码未就绪、坐标越界）都返回 false，即保持窗口可交互——
/// 宁可多挡一下，也不能让用户点不到角色。
#[cfg(windows)]
fn transparent_under_cursor(window: &WebviewWindow) -> bool {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;

    let (Ok(origin), Ok(size)) = (window.inner_position(), window.inner_size()) else {
        return false;
    };
    let mut point = POINT { x: 0, y: 0 };
    if unsafe { GetCursorPos(&mut point) } == 0 {
        return false;
    }
    // inner_position/inner_size 均为物理像素，与 GetCursorPos 同一坐标系
    let lx = point.x - origin.x;
    let ly = point.y - origin.y;
    if lx < 0 || ly < 0 || lx >= size.width as i32 || ly >= size.height as i32 {
        return false;
    }

    let Ok(guard) = HIT_MASK.lock() else {
        return false;
    };
    let Some(mask) = guard.as_ref() else {
        return false;
    };
    let cols = mask.cols as u32;
    let rows = mask.rows as u32;
    let cx = (lx as u32 * cols / size.width.max(1)).min(cols - 1);
    let cy = (ly as u32 * rows / size.height.max(1)).min(rows - 1);
    mask.cells[(cy * cols + cx) as usize] == 0
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

// ───────────────────────── 桌宠 AI 对话（P3） ─────────────────────────
//
// 目标：让桌宠气泡说由**本机已有 API 网关**生成的短句，真流式（逐字长出），并把上游
// `reasoning_content` 接「深度思考碎碎念」（前端按首个 reasoning delta 播 mumble-deep）。
//
// 为什么走**进程内**而不是回环 HTTP：网关所有路由经 `auth::bearer_auth`（仅 /health
// 环回豁免），回环调用需要 API Key 且会污染 Key 用量；且仓库无 reqwest。这里直接拿
// `ApiServerRuntime.shared`（`Arc<ApiSharedState>`）调 `routes::run_chat_completion`，
// 不带 Key、不经 socket —— 零新增外发，与"仅经本机网关到上游"红线一致。
//
// 竞态说明（有意如此，勿误判为 bug）：进程内调用持有 `Arc<ApiSharedState>`，用户在网关
// 页面点「停止 API 网关」**不会**打断已开始的这一流；下一次 `pet_say` 因 runtime 为
// None 返回 Err("网关未启动")，前端回落本地台词。

/// 桌宠 AI 请求在用量/日志里的记账标识（与真实 Key 区分，便于用量页单独审计）
const PET_SAY_KEY: &str = "internal:pet";
/// 两次发言最小间隔（ms）：防连点/事件风暴把额度打空
const PET_SAY_MIN_INTERVAL_MS: u64 = 5_000;
/// 每日发言上限（超出后一律回落本地台词）
const PET_SAY_DAILY_CAP: u32 = 200;
/// 首字节等待上限（秒）
const PET_SAY_FIRST_BYTE_SECS: u64 = 15;
/// 后续分片读等待上限（秒）
const PET_SAY_READ_SECS: u64 = 30;
/// 单次回复总时长上限（秒）
const PET_SAY_TOTAL_SECS: u64 = 60;
/// 单次回复最大生成 token（成本闸门）
///
/// 2026-10-19 从 120 调到 400：思考链与正文**共用**这个预算，120 时模型把额度
/// 全花在「复述要求 + 打草稿」上，正文被截断为空——用户看到的就是一整段思考。
/// 400 足够容纳少量思考 + 一句话，日上限 200 次下成本仍然可控。
const PET_SAY_MAX_TOKENS: u32 = 400;
/// 每日计数落盘键
const PET_SAY_STATS_KV: &str = "pet_say_stats";

/// 当前在跑的流式任务（新的盖旧的：起新任务前 abort）
static SAY_TASK: Mutex<Option<tauri::async_runtime::JoinHandle<()>>> = Mutex::new(None);
/// 会话序号：前端按 sessionId 丢弃过期事件（与 abort 形成双保险）
static SAY_GEN: AtomicU64 = AtomicU64::new(0);
/// 上次发言时间（ms）
static SAY_LAST_MS: Mutex<u64> = Mutex::new(0);

/// 上游 SSE 增量（转发给前端的语义单元）
#[derive(Debug, PartialEq)]
enum PetSseDelta {
    Text(String),
    Reasoning(String),
    Error(String),
    Done,
}

/// 在字节缓冲里找子串位置（仅用于找 SSE 帧分隔符 `\n\n`）
fn find_sub(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    (0..=hay.len() - needle.len()).find(|&i| &hay[i..i + needle.len()] == needle)
}

/// 增量解析 OpenAI 兼容 SSE。
///
/// 按 `\n\n` 分帧——**在字节层**分帧，天然抗"UTF-8 半字被切断"（只在整帧完成后再解码）；
/// `:` 开头为 keep-alive 注释跳过；剥 `data:` 前缀（多行 data 用 `\n` 连接）；
/// `[DONE]` → Done；顶层 `error` 帧 → Error（否则会把"思考完就停"当成正常收尾）；
/// 其余取 `choices[0].delta.content` / `.reasoning_content`。
fn pump_sse(buf: &mut Vec<u8>, chunk: &[u8], out: &mut Vec<PetSseDelta>) {
    buf.extend_from_slice(chunk);
    while let Some(pos) = find_sub(buf, b"\n\n") {
        let block: Vec<u8> = buf.drain(..pos + 2).collect();
        let text = String::from_utf8_lossy(&block[..pos]);
        let mut data_lines: Vec<&str> = Vec::new();
        for line in text.lines() {
            if line.starts_with(':') {
                continue; // keep-alive 注释行
            }
            if let Some(rest) = line.strip_prefix("data:") {
                data_lines.push(rest.trim_start());
            }
        }
        if data_lines.is_empty() {
            continue;
        }
        let data = data_lines.join("\n");
        if data.trim() == "[DONE]" {
            out.push(PetSseDelta::Done);
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&data) else {
            continue;
        };
        if let Some(err) = v.get("error") {
            let msg = err
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("上游返回错误");
            out.push(PetSseDelta::Error(msg.to_string()));
            continue;
        }
        if let Some(delta) = v.pointer("/choices/0/delta") {
            if let Some(r) = delta.get("reasoning_content").and_then(|x| x.as_str()) {
                if !r.is_empty() {
                    out.push(PetSseDelta::Reasoning(r.to_string()));
                }
            }
            if let Some(c) = delta.get("content").and_then(|x| x.as_str()) {
                if !c.is_empty() {
                    out.push(PetSseDelta::Text(c.to_string()));
                }
            }
        }
    }
}

/// 人格档位 → system prompt（表驱动；未知档位回落默认「憨憨大肥鱼」）。
///
/// 2026-10-19 修订（对话「不够灵性」的主因）：
/// - **不再写格式规则**（原来那句「只输出 1 句中文，不超过 40 字；不使用 Markdown…」）。
///   模型会先「规划怎么满足要求」，思考链因此膨胀、正文被截断，气泡里只剩一大段
///   「用户说现在是晚上，让我随便说一句话。要求：1 句中文，40 字内…」——
///   那是它在复述规则，不是它想说的话。
/// - 改为「只描述是谁、怎么说话」+ 一句「直接开口，不要解释或复述设定」；
/// - 每个档位给一条短示例（few-shot 比规则更能锁口吻）；
/// - 安全约束保留，但用自然语言写在最后。
fn persona_system_prompt(persona: Option<&str>) -> String {
    let (body, sample) = match persona.unwrap_or("whale") {
        "tsundere" => (
            "你是桌面上一只傲娇的大肥鱼：嘴上不饶人、其实很在意对方，句子短促，偶尔哼一声。",
            "才、才不是在等你呢，顺手看一眼而已。",
        ),
        "butler" => (
            "你是桌面上一只正经的管家鱼：克制、汇报式、用敬语，不卖萌。",
            "今日 6 项已办妥，1 项还等您确认。",
        ),
        "zen" => (
            "你是桌面上一只禅意的大肥鱼：惜字如金，半文半白，像在自言自语。",
            "水满则溢，事毕则安。",
        ),
        _ => (
            "你是桌面上一只憨憨的大肥鱼（自称「大肥鱼」）：口语化、有点黏人、爱撒娇，\
偶尔冒出「吃 Token」的梗，但不堆颜文字。",
            "今天也辛苦啦，大肥鱼给你揉揉肩～",
        ),
    };
    format!(
        "{body}\n直接说出那句话就行，不要解释你在做什么，也不要复述或评论这些设定。\
口吻参考「{sample}」\n\
通常就一两句、别再长；不提账号名、邮箱、金额等敏感信息；不编造没给过你的数据；\
拿不准就直说不知道。"
    )
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn today_local() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

/// 今日已发言次数（跨日归零）
fn say_count_today(data_dir: &std::path::Path) -> u32 {
    let store = crate::store::db(data_dir);
    let v: serde_json::Value = store.kv_get(PET_SAY_STATS_KV);
    let day = today_local();
    if v.get("day").and_then(|d| d.as_str()) != Some(day.as_str()) {
        return 0;
    }
    v.get("count").and_then(|c| c.as_u64()).unwrap_or(0) as u32
}

/// 今日已发言次数 +1（跨日自动归零），返回新计数；读写失败不阻塞发言
fn say_count_bump(data_dir: &std::path::Path) -> u32 {
    let store = crate::store::db(data_dir);
    let next = say_count_today(data_dir).saturating_add(1);
    let v = serde_json::json!({ "day": today_local(), "count": next });
    let _ = store.kv_set(PET_SAY_STATS_KV, &v);
    next
}

fn emit_say(app: &AppHandle, event: &str, payload: serde_json::Value) {
    let _ = app.emit_to(PET_LABEL, event, payload);
}

fn emit_say_error(app: &AppHandle, session: u64, code: &str, message: &str) {
    emit_say(
        app,
        "pet-say-error",
        serde_json::json!({ "sessionId": session, "code": code, "message": message }),
    );
}

/// 短文案：尽量抽 JSON 里的 message 字段，抽不到就原样截断（避免整段错误体塞气泡）
fn short_msg(s: &str) -> String {
    let t = s.trim();
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(t) {
        if let Some(m) = v
            .pointer("/error/message")
            .or_else(|| v.get("message"))
            .and_then(|x| x.as_str())
        {
            return m.chars().take(80).collect();
        }
    }
    t.chars().take(80).collect()
}

/// 实际执行一次流式发言：取共享状态 → 跑补全 → 泵 SSE → 逐 delta 下发
async fn run_say(app: AppHandle, session: u64, body_vec: Vec<u8>, trigger: String, model: String) {
    let shared = {
        let rt = app.state::<Mutex<Option<crate::commands::api_server::ApiServerRuntime>>>();
        let guard = rt.lock().unwrap_or_else(|e| e.into_inner());
        guard.as_ref().map(|r| r.shared.clone())
    };
    let Some(shared) = shared else {
        emit_say_error(&app, session, "gateway_down", "网关未启动");
        return;
    };

    let resp = crate::api_server::routes::run_chat_completion(
        shared,
        PET_SAY_KEY.to_string(),
        body_vec,
        crate::api_server::routes::Protocol::OpenAi,
    )
    .await;

    let status = resp.status();
    if !status.is_success() {
        let bytes = axum::body::to_bytes(resp.into_body(), 4096)
            .await
            .unwrap_or_default();
        let raw = String::from_utf8_lossy(&bytes);
        let code = if status.as_u16() == 503 { "no_healthy" } else { "upstream" };
        emit_say_error(&app, session, code, &short_msg(&raw));
        return;
    }

    emit_say(
        &app,
        "pet-say-start",
        serde_json::json!({ "sessionId": session, "trigger": trigger, "model": model }),
    );

    let t0 = Instant::now();
    let mut buf: Vec<u8> = Vec::new();
    let mut text = String::new();
    let mut reasoning = String::new();
    let mut stream = resp.into_body().into_data_stream();
    let mut first = true;
    let mut ended = false;

    loop {
        if t0.elapsed() > Duration::from_secs(PET_SAY_TOTAL_SECS) {
            emit_say_error(&app, session, "timeout", "回复超时");
            return;
        }
        let wait = if first { PET_SAY_FIRST_BYTE_SECS } else { PET_SAY_READ_SECS };
        match tokio::time::timeout(Duration::from_secs(wait), stream.next()).await {
            Err(_) => {
                emit_say_error(&app, session, "timeout", "回复超时");
                return;
            }
            Ok(None) => break,
            Ok(Some(Err(_))) => {
                emit_say_error(&app, session, "upstream", "流读取失败");
                return;
            }
            Ok(Some(Ok(chunk))) => {
                first = false;
                let mut deltas = Vec::new();
                pump_sse(&mut buf, &chunk, &mut deltas);
                for d in deltas {
                    match d {
                        PetSseDelta::Text(s) => {
                            text.push_str(&s);
                            emit_say(
                                &app,
                                "pet-say-delta",
                                serde_json::json!({ "sessionId": session, "kind": "text", "delta": s }),
                            );
                        }
                        PetSseDelta::Reasoning(s) => {
                            reasoning.push_str(&s);
                            emit_say(
                                &app,
                                "pet-say-delta",
                                serde_json::json!({ "sessionId": session, "kind": "reasoning", "delta": s }),
                            );
                        }
                        PetSseDelta::Error(m) => {
                            emit_say_error(&app, session, "upstream", &short_msg(&m));
                            return;
                        }
                        PetSseDelta::Done => ended = true,
                    }
                }
                if ended {
                    break;
                }
            }
        }
    }

    // 收尾：处理没有以 `\n\n` 结束的残余帧
    if !buf.is_empty() {
        let mut deltas = Vec::new();
        pump_sse(&mut buf, b"\n\n", &mut deltas);
        for d in deltas {
            match d {
                PetSseDelta::Text(s) => text.push_str(&s),
                PetSseDelta::Reasoning(s) => reasoning.push_str(&s),
                PetSseDelta::Error(m) => {
                    emit_say_error(&app, session, "upstream", &short_msg(&m));
                    return;
                }
                PetSseDelta::Done => {}
            }
        }
    }

    if text.trim().is_empty() {
        emit_say_error(&app, session, "empty", "这次没说出话");
        return;
    }

    emit_say(
        &app,
        "pet-say-done",
        serde_json::json!({
            "sessionId": session,
            "ok": true,
            "text": text,
            "reasoning": reasoning,
            "model": model,
            "ms": t0.elapsed().as_millis() as u64,
        }),
    );
}

/// 桌宠说一句 AI 话（P3）。返回 sessionId；同步错误直接给前端做本地台词兜底。
///
/// 同步快速失败（任一命中即 `Err`，前端立刻本地台词、不弹 AI 气泡）：
/// 档位为 off / trigger 与档位不匹配 / 网关未启动 / 距上次发言过近 / 当日超上限。
#[tauri::command]
pub async fn pet_say(
    app: AppHandle,
    prompt: String,
    context: Option<String>,
    trigger: Option<String>,
) -> Result<u64, String> {
    let st = app.state::<AppState>();
    let settings = st.settings();
    let mode = settings.pet_ai_mode.clone().unwrap_or_else(|| "off".into());
    if mode == "off" {
        return Err("AI 对话未开启".into());
    }
    let trig = trigger.unwrap_or_else(|| "user".into());
    if trig == "auto" && mode != "auto" {
        return Err("自动点评未开启".into());
    }

    // 网关运行态（None = 未启动）；顺带取默认模型名
    let model = {
        let rt = app.state::<Mutex<Option<crate::commands::api_server::ApiServerRuntime>>>();
        let guard = rt.lock().unwrap_or_else(|e| e.into_inner());
        guard.as_ref().map(|r| r.shared.default_model.clone())
    };
    let Some(model) = model else {
        return Err("网关未启动".into());
    };

    // 节流 + 日上限
    let now = now_ms();
    {
        let mut last = SAY_LAST_MS.lock().unwrap_or_else(|e| e.into_inner());
        if *last != 0 && now.saturating_sub(*last) < PET_SAY_MIN_INTERVAL_MS {
            return Err("刚说过，缓一缓".into());
        }
        *last = now;
    }
    if say_count_today(&st.data_dir) >= PET_SAY_DAILY_CAP {
        return Err("今天聊够啦".into());
    }
    say_count_bump(&st.data_dir);

    // 组装请求体（只这三类字段，不碰任何嵌套契约字段名）
    let system = persona_system_prompt(settings.pet_persona.as_deref());
    let user = match context.as_deref() {
        Some(c) if !c.trim().is_empty() => format!("{prompt}\n（背景：{c}）"),
        _ => prompt,
    };
    let body = serde_json::json!({
        "model": &model,
        "stream": true,
        "max_tokens": PET_SAY_MAX_TOKENS,
        "reasoning_effort": "low",
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": user },
        ],
    });
    let body_vec = serde_json::to_vec(&body).map_err(|e| e.to_string())?;

    // 新的盖旧的：abort 上一轮流，避免并发占用账号并发位
    if let Some(old) = SAY_TASK.lock().unwrap_or_else(|e| e.into_inner()).take() {
        old.abort();
    }
    let session = SAY_GEN.fetch_add(1, Ordering::SeqCst) + 1;
    let app2 = app.clone();
    let task = tauri::async_runtime::spawn(async move {
        run_say(app2, session, body_vec, trig, model).await;
    });
    *SAY_TASK.lock().unwrap_or_else(|e| e.into_inner()) = Some(task);

    Ok(session)
}

#[cfg(test)]
mod say_tests {
    use super::*;

    /// 跨 chunk 分帧 + UTF-8 半字切断：整帧完成前不产出，完成后能正确解码多字节字符
    #[test]
    fn pump_survives_split_frame_and_split_utf8() {
        let full = "data: {\"choices\":[{\"delta\":{\"content\":\"你好\"}}]}\n\n";
        let bytes = full.as_bytes();
        // 在「你」（3 字节）中间切开
        let mid = full.find('你').unwrap() + 1;
        let mut buf = Vec::new();
        let mut out = Vec::new();
        pump_sse(&mut buf, &bytes[..mid], &mut out);
        assert!(out.is_empty(), "帧未完成时不应产出");
        pump_sse(&mut buf, &bytes[mid..], &mut out);
        assert_eq!(out, vec![PetSseDelta::Text("你好".to_string())]);
    }

    /// keep-alive 注释行跳过；`[DONE]` 识别为结束
    #[test]
    fn pump_skips_keepalive_and_detects_done() {
        let mut buf = Vec::new();
        let mut out = Vec::new();
        pump_sse(&mut buf, b": keep-alive\n\ndata: [DONE]\n\n", &mut out);
        assert_eq!(out, vec![PetSseDelta::Done]);
    }

    /// 顶层 error 帧 → Error（否则会被当成正常收尾）
    #[test]
    fn pump_reports_error_frame() {
        let mut buf = Vec::new();
        let mut out = Vec::new();
        pump_sse(
            &mut buf,
            b"data: {\"error\":{\"message\":\"boom\",\"code\":\"x\"}}\n\n",
            &mut out,
        );
        assert_eq!(out, vec![PetSseDelta::Error("boom".to_string())]);
    }

    /// 思考链先于正文：同一帧内 reasoning 在 text 之前
    #[test]
    fn pump_emits_reasoning_before_text() {
        let mut buf = Vec::new();
        let mut out = Vec::new();
        let payload = "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"想\",\"content\":\"说\"}}]}\n\n";
        pump_sse(&mut buf, payload.as_bytes(), &mut out);
        assert_eq!(
            out,
            vec![
                PetSseDelta::Reasoning("想".to_string()),
                PetSseDelta::Text("说".to_string()),
            ]
        );
    }

    /// 人格档位：每个档位都带公共安全/长度约束，且都给了口吻示例；未知档位回落默认
    #[test]
    fn persona_prompt_has_hard_rules_and_default_fallback() {
        for p in [None, Some("whale"), Some("tsundere"), Some("butler"), Some("zen"), Some("nope")] {
            let s = persona_system_prompt(p);
            assert!(s.contains("不编造没给过你的数据"), "档位 {p:?} 缺少『不编造』约束");
            assert!(s.contains("不提账号名、邮箱、金额"), "档位 {p:?} 缺少隐私约束");
            assert!(s.contains("拿不准就直说不知道"), "档位 {p:?} 缺少不确定兜底");
            assert!(s.contains("通常就一两句"), "档位 {p:?} 缺少长度约束");
            // few-shot 口吻示例：比抽象规则更能锁住语气
            assert!(s.contains("口吻参考「"), "档位 {p:?} 缺少口吻示例");
        }
        assert!(persona_system_prompt(Some("nope")).contains("大肥鱼"));
        assert!(persona_system_prompt(None).contains("大肥鱼"));
    }
}
