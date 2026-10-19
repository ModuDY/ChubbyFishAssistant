//! 桌宠素材包（pet_pack）：在 `%APPDATA%\<data_dir>\pets\<id>\` 下挂一套自己的形象。
//!
//! 目录结构（与内置素材完全一致，方便直接复制）：
//!
//! ```text
//! pets/<id>/manifest.json        { "canvas": {...}, "actions": [{ id,label,group,loop,file }] }
//! pets/<id>/videos/*.webm
//! ```
//!
//! 语义是**按动作 id 覆盖**：包里有 `idle-breathe` 就用包里的，没有的仍回落内置素材，
//! 所以一套新形象只要补齐需要替换的动作即可（例如只换 6 个核心态动作）。
//!
//! 前端拿到绝对路径后用 `convertFileSrc()` 播放，因此：
//! - `tauri.conf.json` 需要 `assetProtocol.enable = true` 且 scope 覆盖 `pets/**`；
//! - CSP 需要允许 `media-src ... asset: http://asset.localhost`（否则 video 加载被拦）。
use serde::Serialize;
use std::path::{Path, PathBuf};
use tauri::State;

use crate::state::AppState;

#[derive(Serialize)]
pub struct PackInfo {
    pub id: String,
    pub name: String,
    pub action_count: usize,
}

#[derive(Serialize)]
pub struct PackAction {
    pub id: String,
    pub label: String,
    pub group: String,
    #[serde(rename = "loop")]
    pub looping: bool,
    /// 素材绝对路径（前端 convertFileSrc 后可直接喂给 <video>）
    pub path: String,
}

#[derive(Serialize)]
pub struct PackManifest {
    pub id: String,
    pub canvas: serde_json::Value,
    pub actions: Vec<PackAction>,
}

fn packs_root(state: &AppState) -> PathBuf {
    state.data_dir.join("pets")
}

/// 只允许 `[A-Za-z0-9_-]{1,64}`：挡住 `..`、盘符、分隔符等目录穿越
fn is_safe_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn read_manifest(dir: &Path) -> Option<serde_json::Value> {
    let text = std::fs::read_to_string(dir.join("manifest.json")).ok()?;
    serde_json::from_str::<serde_json::Value>(&text).ok()
}

fn str_field<'a>(v: &'a serde_json::Value, key: &str) -> &'a str {
    v.get(key).and_then(|x| x.as_str()).unwrap_or_default()
}

/// 列出可用素材包（不含内置；前端自己把 `builtin` 放第一项）
#[tauri::command]
pub fn pet_pack_list(state: State<AppState>) -> Vec<PackInfo> {
    let root = packs_root(&state);
    let Ok(entries) = std::fs::read_dir(&root) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for entry in entries.flatten() {
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let id = entry.file_name().to_string_lossy().to_string();
        if !is_safe_id(&id) {
            continue;
        }
        let Some(manifest) = read_manifest(&entry.path()) else {
            continue;
        };
        let count = manifest
            .get("actions")
            .and_then(|a| a.as_array())
            .map(|a| a.len())
            .unwrap_or(0);
        if count == 0 {
            continue;
        }
        let name = str_field(&manifest, "name");
        out.push(PackInfo {
            id: id.clone(),
            name: if name.is_empty() { id } else { name.to_string() },
            action_count: count,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// 读取某个包的清单（只返回**文件确实存在**的动作；缺失的由前端回落内置）
#[tauri::command]
pub fn pet_pack_manifest(state: State<AppState>, id: String) -> Result<PackManifest, String> {
    if !is_safe_id(&id) {
        return Err("非法的素材包 id".into());
    }
    let dir = packs_root(&state).join(&id);
    let manifest = read_manifest(&dir).ok_or_else(|| format!("素材包 {id} 缺少 manifest.json"))?;

    let actions = manifest
        .get("actions")
        .and_then(|a| a.as_array())
        .ok_or_else(|| "manifest 缺少 actions".to_string())?;

    let videos = dir.join("videos");
    let mut out = Vec::new();
    for item in actions {
        let action_id = str_field(item, "id");
        let file = str_field(item, "file");
        if action_id.is_empty() || file.is_empty() {
            continue;
        }
        let path = videos.join(file);
        if !path.is_file() {
            continue;
        }
        let label = str_field(item, "label");
        let group = str_field(item, "group");
        out.push(PackAction {
            id: action_id.to_string(),
            label: if label.is_empty() { action_id.to_string() } else { label.to_string() },
            group: if group.is_empty() { "idle".to_string() } else { group.to_string() },
            looping: item.get("loop").and_then(|x| x.as_bool()).unwrap_or(true),
            path: path.to_string_lossy().to_string(),
        });
    }

    if out.is_empty() {
        return Err(format!("素材包 {id} 里没有可用的视频文件"));
    }

    Ok(PackManifest {
        id,
        canvas: manifest.get("canvas").cloned().unwrap_or(serde_json::Value::Null),
        actions: out,
    })
}
