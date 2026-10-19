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
