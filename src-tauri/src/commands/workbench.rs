//! 工作台（P0）：布局持久化与"待办规则"的服务端读取口。
//!
//! 设计口径（见 `docs/pending/工作台-工作流-知识库方案.md` §4）：
//! - 工作台是**跨应用固定入口**，不是某个应用的子页；
//! - 布局是**纯 UI 偏好**，存 kv 文档 `workbench_layout`，**不进 `Settings`**
//!   （Settings 已是 100+ 字段的大结构，把 UI 偏好塞进去会让每次
//!   settings_get 的序列化都背这份负载，且布局结构将来会演进）；
//! - 待办由规则表在**前端**从既有 store 数据推导（零额外 IPC）；
//!   本模块只提供布局读写 + 默认布局常量，不做业务判断。
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::state::AppState;

/// 单个区块布局项（与前端 `WorkbenchWidgetLayout` 对齐）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkbenchWidgetLayout {
    /// 区块 key（status / todos / actions / recent / capture）
    pub key: String,
    /// 栅格占列数 1~3（越界由读取侧 clamp，不报错）
    pub span: u8,
    /// 是否隐藏
    pub hidden: bool,
}

/// 布局文档（存 kv `workbench_layout`）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkbenchLayout {
    /// 布局语义版本：将来顺序/字段变更时用于迁移，缺失按 1 处理
    #[serde(default = "default_layout_version")]
    pub version: u32,
    pub widgets: Vec<WorkbenchWidgetLayout>,
}

fn default_layout_version() -> u32 {
    1
}

/// 默认布局：与方案 §4.2 的区块顺序一致。
///
/// 顺序即优先级（越靠前越"每天必看"）：
/// ① 今日状态条 → ② 待办（核心）→ ③ 一键动作 → ④ 最近&常用 → ⑤ 快速捕获
fn default_widgets() -> Vec<WorkbenchWidgetLayout> {
    vec![
        WorkbenchWidgetLayout { key: "status".into(), span: 3, hidden: false },
        WorkbenchWidgetLayout { key: "todos".into(), span: 2, hidden: false },
        WorkbenchWidgetLayout { key: "actions".into(), span: 1, hidden: false },
        WorkbenchWidgetLayout { key: "recent".into(), span: 2, hidden: false },
        WorkbenchWidgetLayout { key: "capture".into(), span: 1, hidden: false },
    ]
}

/// 合法的区块 key 白名单。
///
/// 只校验 key 不校验 span：span 越界是 UI 容错问题（clamp 即可），
/// 而未知 key 说明前端与后端版本不一致（例如降级安装旧版），必须丢弃——
/// 否则前端注册表里没有对应组件，渲染时会直接抛错白屏。
const WIDGET_KEYS: &[&str] = &["status", "todos", "actions", "recent", "capture"];

impl WorkbenchLayout {
    /// 归一化：补全缺失区块 / 丢弃未知 key / clamp span / 去重（保留首次出现位置）
    fn normalized(mut self) -> Self {
        // 先按 key 去重（保留首次出现，避免同一区块渲染两次）
        let mut seen = std::collections::HashSet::new();
        self.widgets.retain(|w| seen.insert(w.key.clone()));

        // 丢弃未知 key 与重复 key 之外的脏数据
        self.widgets.retain(|w| WIDGET_KEYS.contains(&w.key.as_str()));

        // clamp span 到 1..=3
        for w in self.widgets.iter_mut() {
            w.span = w.span.clamp(1, 3);
        }

        // 补全缺失区块（新版新增区块时，老布局文档不会丢它们），追加到末尾
        for key in WIDGET_KEYS {
            if !self.widgets.iter().any(|w| w.key == *key) {
                let span = if *key == "status" { 3 } else { 2 };
                self.widgets.push(WorkbenchWidgetLayout {
                    key: (*key).into(),
                    span,
                    hidden: false,
                });
            }
        }

        if self.version == 0 {
            self.version = default_layout_version();
        }
        self
    }
}

/// 读取工作台布局：未保存过 → 返回默认布局（不写库，纯读路径幂等）。
///
/// 前端首屏就要用，必须**永不失败**：库异常时回落默认布局而非报错，
/// 否则整个工作台白屏（对齐 `evolution_metrics` 的"不报错"口径）。
#[tauri::command]
pub fn workbench_layout_get(state: State<AppState>) -> WorkbenchLayout {
    let store = crate::store::db(&state.data_dir);
    store
        .kv_get::<Option<WorkbenchLayout>>("workbench_layout")
        .unwrap_or_else(|| WorkbenchLayout {
            version: default_layout_version(),
            widgets: default_widgets(),
        })
        .normalized()
}

/// 保存工作台布局：整体替换（布局是 UI 偏好，无并发合并语义）。
#[tauri::command]
pub fn workbench_layout_set(
    state: State<AppState>,
    layout: WorkbenchLayout,
) -> Result<(), String> {
    let store = crate::store::db(&state.data_dir);
    store.kv_set("workbench_layout", &layout.normalized())
}

/// 恢复默认布局（前端"重置布局"按钮）。
#[tauri::command]
pub fn workbench_layout_reset(state: State<AppState>) -> Result<WorkbenchLayout, String> {
    let layout = WorkbenchLayout {
        version: default_layout_version(),
        widgets: default_widgets(),
    };
    crate::store::db(&state.data_dir).kv_set("workbench_layout", &layout)?;
    Ok(layout)
}
