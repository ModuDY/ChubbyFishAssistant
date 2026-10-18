//! 桌宠渲染路线验证（P0 技术验证，临时模块）
//!
//! 目的：用一个最小 demo 回答"透明 WebM 到底该怎么播"，而不是先写业务代码。
//! 本文件只做**验证期需要的窗口能力**，验证结束后按下述结论保留或删除：
//!   - 若走 `<video>` 路线：保留 show/hide，删除 style probe 与 watchdog 相关部分
//!   - 若走帧序列路线：整体保留（隐藏即停与样式还原仍然必需）
//!
//! 为什么不把这些写在 main.rs：main.rs 已 900+ 行且承担启动/托盘/命令注册，
//! 验证代码是**一次性、可能被删**的，独立成文件便于整体回滚。

use tauri::{Manager, WebviewWindow};

/// 验证期窗口 label（与 tauri.conf.json 的 app.windows[1].label 一致）
const PET_LABEL: &str = "pet";

/// 隐藏窗口前记录的原生扩展样式（验证用）
const GWL_EXSTYLE: i32 = -20;
/// 分层窗口（透明窗口必需）；Tauri 的 transparent(true) 会加上它
const WS_EX_LAYERED: i32 = 0x0008_0000;
/// 工具窗口：不出现在 Alt+Tab 与任务栏（Tauri 的 skipTaskbar 会加上它）
const WS_EX_TOOLWINDOW: i32 = 0x0000_0080;

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
        // MSDN：SetWindowLongPtrW 后建议调用 SetWindowPos 使样式变更生效
        true
    }
}

#[cfg(not(windows))]
fn or_ex_style(_window: &WebviewWindow, _bits: i64) -> bool {
    true
}

/// 验证页首次挂载时上报自身状态：确认 pet 窗口已透明、置顶、不属于任务栏，
/// 并记录当前原生扩展样式，作为后续"隐藏→恢复"对比的基线。
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

/// 【关键验证】隐藏 pet 窗口 → 停留 1.5 秒（让前端观察 rAF/定时器是否被冻结）
/// → 由 **Rust 强制 show + set_always_on_top(还原)**。
///
/// 之所以必须由 Rust 兜底：HTML5 规范允许浏览器在窗口不可见时暂停 `requestAnimationFrame`
/// 与定时器。若该行为在 WebView2 上生效，前端**无法自行恢复**（页面根本没在执行），
/// 窗口将永久消失。此处用 Rust 侧的 sleep 保证一定能把窗口拉回来。
#[tauri::command]
pub async fn pet_probe_hide_and_restore(window: WebviewWindow) -> Result<serde_json::Value, String> {
    let before = read_ex_style(&window).unwrap_or((0, false, false));
    let top_before = window.is_always_on_top().unwrap_or(false);

    window.hide().map_err(|e| e.to_string())?;

    // 验证期间给前端留出观察窗口：1.5s 足够看两次 rAF tick 差
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

/// 切换 pet 窗口显示/隐藏（验证期由前端按钮调用；正式版将由托盘项驱动）。
#[tauri::command]
pub fn pet_toggle(window: WebviewWindow) -> Result<bool, String> {
    let visible = window.is_visible().unwrap_or(false);
    if visible {
        window.hide().map_err(|e| e.to_string())?;
        Ok(false)
    } else {
        window.show().map_err(|e| e.to_string())?;
        let _ = window.set_always_on_top(true);
        Ok(true)
    }
}

/// 关闭 pet 窗口（验证期用）。**不退出应用**——与 main.rs 中
/// "仅 main 窗口的 CloseRequested 才 exit(0)" 的判定保持一致。
#[tauri::command]
pub fn pet_close(app: tauri::AppHandle) -> Result<(), String> {
    if let Some(w) = app.get_webview_window(PET_LABEL) {
        w.close().map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 把验证页的五项检测结论落盘，返回文件绝对路径。
///
/// 为什么落盘而不是只 console.log：Tauri dev 下 WebView 的 `console.log`
/// **不会**转发到终端（需要额外插件），落盘是唯一能稳定取回读数的通道。
#[tauri::command]
pub fn pet_probe_report(report: String) -> Result<String, String> {
    let path = std::env::temp_dir().join("pet-probe-report.json");
    std::fs::write(&path, report).map_err(|e| e.to_string())?;
    Ok(path.to_string_lossy().to_string())
}
