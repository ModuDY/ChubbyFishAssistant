//! 本机 Wallpaper Engine 壁纸库：扫描与只读访问。
//!
//! 为什么放在宿主侧而不是前端：
//! 1. 需要读注册表（HKCU\Software\Valve\Steam）定位 Steam —— 浏览器没有这个能力；
//! 2. 需要遍历任意盘符下的 Steam 库（`libraryfolders.vdf` 里可能同时挂着 D:/E: 多个库）；
//! 3. 需要按名读**散装工程**里的资源（scene.json / .tex / shader / 模型），
//!    一次几十上百个文件，走 IPC 比让前端拼 HTTP 更直接（也不需要额外开端口）。
//!
//! 安全边界（硬约束）：
//! - 只读；不写、不删、不执行壁纸目录里的任何东西。
//! - `we_read` 的每个路径都必须 canonicalize 后仍落在传入的工程目录内，
//!   否则拒绝 —— 壁纸目录里的内容来自创意工坊（第三方作者），不能当成可信路径拼接。
//! - 不做 zip/pkg 解包：散装工程直接按名读，`scene.pkg` 整包交给前端引擎自己解析。

use std::path::{Path, PathBuf};

use serde::Serialize;
use tauri::ipc::Response;

const STEAM_APP_WALLPAPER_ENGINE: &str = "wallpaper_engine";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WeWallpaper {
    /// 目录名（官方工程名或创意工坊 id）
    pub id: String,
    pub title: String,
    /// scene / video / web / application（取自 project.json 的 type）
    pub kind: String,
    /// 工程目录绝对路径
    pub dir: String,
    /// project.json 的 file 字段（scene.json / xxx.mp4 / index.html）
    pub entry: Option<String>,
    /// 预览图文件名（preview.jpg 等），不存在则为 None
    pub preview: Option<String>,
    /// 是否是创意工坊订阅内容（前端据此显示来源）
    pub workshop: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WeScanResult {
    /// 找到的 Wallpaper Engine 安装根（projects/ 与 config.json 所在处）
    pub install_dir: Option<String>,
    /// 扫过的 Steam 库根（诊断用）
    pub library_roots: Vec<String>,
    pub items: Vec<WeWallpaper>,
}

/// 从注册表读 Steam 安装根（等价于 HKCU\Software\Valve\Steam 的 SteamPath）
fn steam_roots() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    #[cfg(windows)]
    {
        use windows_registry::CURRENT_USER;
        if let Ok(key) = CURRENT_USER.open("Software\\Valve\\Steam") {
            if let Ok(p) = key.get_string("SteamPath") {
                let p = PathBuf::from(p.replace('/', "\\"));
                if p.is_dir() {
                    roots.push(p);
                }
            }
            // SteamExe 形如 d:\game\steam\steam.exe，取父目录兜底
            if let Ok(exe) = key.get_string("SteamExe") {
                let p = PathBuf::from(exe.replace('/', "\\"));
                if let Some(parent) = p.parent() {
                    if parent.is_dir() && !roots.iter().any(|r| r == parent) {
                        roots.push(parent.to_path_buf());
                    }
                }
            }
        }
    }
    // 注册表拿不到时（绿色版 / 注册表被清）再试常见位置
    for guess in [
        r"C:\Program Files (x86)\Steam",
        r"C:\Program Files\Steam",
        r"D:\Steam",
        r"E:\Steam",
        r"D:\SteamLibrary",
        r"E:\SteamLibrary",
    ] {
        let p = PathBuf::from(guess);
        if p.is_dir() && !roots.iter().any(|r| r == &p) {
            roots.push(p);
        }
    }
    roots
}

/// 解析 libraryfolders.vdf，取出所有库根。
///
/// 只认 `"path"` 行：这是 Steam 真正用来定位库的字段，其余键（label/contentid/
/// apps 等）对扫描没有意义。VDF 里路径是双反斜杠转义，需要还原。
fn library_roots(steam_root: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = vec![steam_root.to_path_buf()];
    let vdf = steam_root.join("steamapps").join("libraryfolders.vdf");
    let Ok(text) = std::fs::read_to_string(&vdf) else {
        return out;
    };
    for line in text.lines() {
        let trimmed = line.trim_start();
        let Some(rest) = trimmed.strip_prefix("\"path\"") else {
            continue;
        };
        // 形如：  "path"    "D:\\game\\steam"
        let Some(first_quote) = rest.find('"') else { continue };
        let after = &rest[first_quote + 1..];
        let Some(end_quote) = after.find('"') else { continue };
        let raw = &after[..end_quote];
        let p = PathBuf::from(raw.replace("\\\\", "\\"));
        if p.is_dir() && !out.iter().any(|r| r == &p) {
            out.push(p);
        }
    }
    out
}

/// 读 project.json（容忍 UTF-8 BOM —— Wallpaper Engine 自己写出的文件有时带）
fn read_project_json(dir: &Path) -> Option<serde_json::Value> {
    let raw = std::fs::read(dir.join("project.json")).ok()?;
    let raw = if raw.starts_with(&[0xEF, 0xBB, 0xBF]) {
        &raw[3..]
    } else {
        &raw[..]
    };
    serde_json::from_slice(raw).ok()
}

fn preview_file(dir: &Path) -> Option<String> {
    // 官方工程与工坊条目的预览图命名不统一，按常见顺序挑第一个存在的
    for name in ["preview.jpg", "preview.png", "preview.gif", "preview.jpeg"] {
        if dir.join(name).is_file() {
            return Some(name.to_string());
        }
    }
    None
}

fn collect_from(parent: &Path, workshop: bool, out: &mut Vec<WeWallpaper>) {
    let Ok(entries) = std::fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let Some(project) = read_project_json(&dir) else {
            continue;
        };
        let kind = project
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("scene")
            .to_string();
        let entry_file = project.get("file").and_then(|v| v.as_str()).map(str::to_string);
        // 场景工程至少要有一个入口：散装 json 或 scene.pkg，否则是空壳
        let has_entry = entry_file
            .as_deref()
            .map(|f| dir.join(f).is_file())
            .unwrap_or(false)
            || dir.join("scene.pkg").is_file();
        if !has_entry && kind == "scene" {
            continue;
        }
        let id = dir
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        let title = project
            .get("title")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .unwrap_or(&id)
            .to_string();
        out.push(WeWallpaper {
            id,
            title,
            kind,
            dir: dir.to_string_lossy().to_string(),
            entry: entry_file,
            preview: preview_file(&dir),
            workshop,
        });
    }
}

fn scan_blocking() -> WeScanResult {
    let mut items: Vec<WeWallpaper> = Vec::new();
    let mut install_dir: Option<String> = None;
    let mut library_roots_out: Vec<String> = Vec::new();

    for steam in steam_roots() {
        for lib in library_roots(&steam) {
            let lib_str = lib.to_string_lossy().to_string();
            if !library_roots_out.iter().any(|r| r == &lib_str) {
                library_roots_out.push(lib_str);
            }
            // 1) 官方工程：<lib>/steamapps/common/wallpaper_engine/projects/{defaultprojects,myprojects}
            let we = lib
                .join("steamapps")
                .join("common")
                .join(STEAM_APP_WALLPAPER_ENGINE);
            if we.is_dir() {
                if install_dir.is_none() {
                    install_dir = Some(we.to_string_lossy().to_string());
                }
                for sub in ["defaultprojects", "myprojects"] {
                    collect_from(&we.join("projects").join(sub), false, &mut items);
                }
            }
            // 2) 创意工坊：<lib>/steamapps/workshop/content/431960/*
            let workshop = lib
                .join("steamapps")
                .join("workshop")
                .join("content")
                .join("431960");
            if workshop.is_dir() {
                collect_from(&workshop, true, &mut items);
            }
        }
    }

    // 同一张壁纸可能被多个库路径指到（软链/重复挂载），按 dir 去重
    items.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));
    items.dedup_by(|a, b| a.dir == b.dir);

    WeScanResult {
        install_dir,
        library_roots: library_roots_out,
        items,
    }
}

/// 扫描本机 Wallpaper Engine 壁纸库。
#[tauri::command]
pub async fn we_scan() -> Result<WeScanResult, String> {
    tauri::async_runtime::spawn_blocking(scan_blocking)
        .await
        .map_err(|e| format!("扫描壁纸库失败: {e}"))
}

/// 只读一个工程内文件，返回原始字节（前端用 ArrayBuffer 接）。
///
/// `name` 允许带子目录（如 `materials/foo.json`）；任何逃逸出工程目录的路径一律拒绝。
#[tauri::command]
pub async fn we_read(dir: String, name: String) -> Result<Response, String> {
    tauri::async_runtime::spawn_blocking(move || {
        // 拒绝明显的路径穿越字符（canonicalize 之前先挡一层，Windows 上 `..\` 也能拼出来）
        if name.contains("..") || name.starts_with('/') || name.starts_with('\\') {
            return Err(format!("非法资源名: {name}"));
        }
        let base = PathBuf::from(&dir)
            .canonicalize()
            .map_err(|e| format!("工程目录不可用: {e}"))?;
        let target = base
            .join(name.replace('\\', "/"))
            .canonicalize()
            .map_err(|e| format!("资源不存在: {e}"))?;
        if !target.starts_with(&base) {
            return Err("拒绝访问工程目录之外的路径".into());
        }
        if !target.is_file() {
            return Err("不是文件".into());
        }
        let bytes = std::fs::read(&target).map_err(|e| format!("读取失败: {e}"))?;
        Ok(Response::new(bytes))
    })
    .await
    .map_err(|e| format!("读取资源失败: {e}"))?
}
