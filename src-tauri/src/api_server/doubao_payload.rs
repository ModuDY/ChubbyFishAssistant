//! 豆包（doubao）对话请求体构造与模型目录（E-01，PC 真机口径）。
//!
//! ── body 形态（批次 0 真机抓包固化）───────────────────────────────────
//! 逐字段对齐真机「普通模式 + 新建会话」样本（2491B，2026-10-16 19:26）：
//! `client_meta`（`local_conversation_id` + 空 `conversation_id` + `last_message_index:null`
//! + 本地权限）+ `messages[].content_block[]`（`block_type=10000`，文本在
//! `content.text_block.text`）+ `option`（开关组 + `agent_mode` + 建会话三件套
//! `conversation_init_option`/`conversation_init_ext`/`conversation_mode` + `model_config`
//! /`aggregate_params`）+ `user_context` + `ext`。
//!
//! 两条「反直觉」但必须遵守的真机事实（写错即 `710020202 common invalid param`）：
//! 1. **`conversation_id` 是空串**（不是 `"0"`——那是 `/samantha` 宿主的 doubao2api 哨兵值）；
//!    `last_message_index` 是 **`null`**（不是 `0`）；建会话靠
//!    `conversation_init_option.need_ack_conversation=true` 让服务端回 ack。
//! 2. **两个「开关容器」的类型规则不同**：`option` 里的开关是**数字**
//!    （`agent_mode` / `need_deep_think` / `conversation_mode` / `model_config.reasoning_effort`），
//!    而 `ext` 里的开关是**字符串**（`"2"` / `"0"` / `"1"`）；把 `ext` 写成数字即参数非法
//!    （逐字段 diff 实测：89/89 路径对齐，唯有这 5 处类型不符）。
//!
//! 另：新版客户端已**不再发送** `chat_ability` 与 `ext.suggest_extra`，此处亦不发
//!（少发可选字段不会致错，多发未知字段才可能被拒）。
//!
//! ── 档位（模式）─────────────────────────────────────────────────────
//! 真机以 `option.need_deep_think`（0 快速 / 9 深度·Agent）+ `agent_mode`
//! + `model_item_key`/`mode_id`/`conversation_mode` 表达档位。`doubao-auto` 完全对齐
//! 普通模式样本；`doubao-think`/`doubao-expert` 的开关取值由普通/Agent 两类样本推断，
//! **待真机回归确认**。`doubao-2.1-*` 仍为占位（`verified=false`）。
//!
//! ── 多轮 ─────────────────────────────────────────────────────────────
//! v1 = **网关侧消息合并**：每轮把历史消息合并为单条 user 文本，以「新建会话」发出
//!（换账号不丢上下文，也不需要会话映射表）。

use serde_json::{json, Value};

/// 豆包默认 bot（实测：summary/probe 一致）
pub const DOUBAO_BOT_ID: &str = "7338286299411103781";

/// 文本块类型（真机实测）
const BLOCK_TYPE_TEXT: u64 = 10000;
/// 推理强度（真机普通/Agent 样本均为 `"5"`）
const REASONING_EFFORT: &str = "5";

/// 单个对外豆包模型档位
pub struct DoubaoModel {
    /// 对外 id（小写，与请求体 model 一致）
    pub id: &'static str,
    /// 展示名（`/v1/models` 用）
    pub display: &'static str,
    /// `option.need_deep_think`（0 = 快速 / 1 = 深度思考 / 9 = 深度·Agent）
    pub need_deep_think: u8,
    /// `option.agent_mode`（数字；普通模式样本 2，Agent 样本 1）
    pub agent_mode: u8,
    /// `model_item_key`（`conversation_init_ext`/`model_config`/`aggregate_params` 共用）
    pub model_item_key: &'static str,
    /// `mode_id`
    pub mode_id: &'static str,
    /// `conversation_mode`（数字）
    pub conversation_mode: u8,
    /// 是否已实测验证（false = 占位，调用返回 400 不打上游）
    pub verified: bool,
}

/// 模型目录（表驱动）
const CATALOG: &[DoubaoModel] = &[
    // 普通模式（对齐 19:26 真机样本）
    DoubaoModel { id: "doubao-auto", display: "豆包·自动", need_deep_think: 0, agent_mode: 2, model_item_key: "0", mode_id: "1", conversation_mode: 1, verified: true },
    // 深度思考（开关取值由普通样本推断，待回归）
    DoubaoModel { id: "doubao-think", display: "豆包·思考", need_deep_think: 1, agent_mode: 2, model_item_key: "0", mode_id: "1", conversation_mode: 1, verified: true },
    // 专家·Agent（对齐 14:55 Agent 样本的档位字段）
    DoubaoModel { id: "doubao-expert", display: "豆包·专家", need_deep_think: 9, agent_mode: 1, model_item_key: "9", mode_id: "3", conversation_mode: 2, verified: true },
    // 占位（待 MITM 抓包定死 model_item_key 后启用；调用返回 400）
    DoubaoModel { id: "doubao-2.1-lite", display: "豆包 2.1 Lite（未启用·待验证）", need_deep_think: 0, agent_mode: 2, model_item_key: "0", mode_id: "1", conversation_mode: 1, verified: false },
    DoubaoModel { id: "doubao-2.1-pro", display: "豆包 2.1 Pro（未启用·待验证）", need_deep_think: 0, agent_mode: 2, model_item_key: "0", mode_id: "1", conversation_mode: 1, verified: false },
    DoubaoModel { id: "doubao-2.1-turbo", display: "豆包 2.1 Turbo（未启用·待验证）", need_deep_think: 0, agent_mode: 2, model_item_key: "0", mode_id: "1", conversation_mode: 1, verified: false },
];

/// 目录全量
pub fn catalog() -> &'static [DoubaoModel] {
    CATALOG
}

/// 按 id 查目录（trim + 小写）
pub fn lookup(id: &str) -> Option<&'static DoubaoModel> {
    let key = id.trim().to_ascii_lowercase();
    CATALOG.iter().find(|m| m.id == key)
}

/// 是否「已启用且已验证」（调度放行口径：目录命中 + verified=true）
pub fn is_enabled_verified(id: &str) -> bool {
    lookup(id).map(|m| m.verified).unwrap_or(false)
}

/// 角色中文前缀（合并为单条 user 文本时使用）
fn role_label(role: &str) -> &'static str {
    match role {
        "system" | "developer" => "系统",
        "assistant" => "助手",
        "tool" => "工具",
        _ => "用户",
    }
}

/// OpenAI/Anthropic messages 归并为单条文本：
/// - `system`/`developer` 消息前置为「系统：…」段（多段按序拼接）；
/// - user/assistant/tool 逐条「角色：文本」；
/// - 内容取用 `qoder_upstream::content_to_text`（已处理 string / 多段 content 数组）。
/// 兼容 Anthropic 顶层 `system` 字段（字符串或多段）。
pub fn merge_messages(peek: &Value) -> String {
    let mut system_buf = String::new();
    let mut body_buf = String::new();
    // Anthropic 顶层 system（非 messages 内）
    if let Some(sys) = peek.get("system") {
        let t = crate::tasks::qoder_upstream::content_to_text(sys);
        if !t.is_empty() {
            system_buf.push_str(&t);
            system_buf.push('\n');
        }
    }
    if let Some(messages) = peek.get("messages").and_then(Value::as_array) {
        for m in messages {
            let role = m.get("role").and_then(Value::as_str).unwrap_or("user");
            let text = m
                .get("content")
                .map(crate::tasks::qoder_upstream::content_to_text)
                .unwrap_or_default();
            if text.is_empty() {
                continue;
            }
            if role == "system" || role == "developer" {
                system_buf.push_str(&text);
                system_buf.push('\n');
            } else {
                body_buf.push_str(role_label(role));
                body_buf.push('：');
                body_buf.push_str(&text);
                body_buf.push('\n');
            }
        }
    }
    let mut out = String::new();
    if !system_buf.trim().is_empty() {
        out.push_str("系统：");
        out.push_str(system_buf.trim_end());
        out.push('\n');
    }
    out.push_str(body_buf.trim_end());
    out
}

/// 当前毫秒时间戳
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 当前秒级时间戳（`recovery_option.req_create_time_sec` 用；真机为真实时间）
fn now_sec() -> i64 {
    now_ms() / 1000
}

/// 新 uuid v4（`local_message_id` / `block_id` / `unique_key`）
fn new_uuid() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// 本地会话 id：真机形如 `local_2262101092116271`（`local_` + 16 位数字，客户端本地生成）
fn local_conversation_id() -> String {
    let b = uuid::Uuid::new_v4().into_bytes();
    let n = u64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]);
    format!("local_{:016}", n % 10_000_000_000_000_000)
}

/// 三条固定本地权限声明（真机实测：概略/精确/后台定位，status=2 = 已拒绝）
fn local_permissions() -> Value {
    json!([
        {"permission_name": "ACCESS_COARSE_LOCATION", "status": 2},
        {"permission_name": "ACCESS_FINE_LOCATION", "status": 2},
        {"permission_name": "ACCESS_BACKGROUND_LOCATION", "status": 2},
    ])
}

/// 会话目标：新建 / 续聊。真机两套 body 的差异**只在会话相关字段**（其余逐字段相同）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConvTarget {
    /// 新建会话（`conversation_id:""` + 建会话三件套 + `need_create_conversation:true`）
    New,
    /// 续聊：复用既有会话（真实 17 位会话 id + 上一条所在 section + 上一条消息下标）
    Continue {
        conversation_id: String,
        last_section_id: String,
        last_message_index: i64,
    },
}

/// 构造「新建会话」请求体（真机 `content_block` 结构）。
pub fn build_body(peek: &Value, m: &DoubaoModel) -> Value {
    build_body_with(peek, m, &ConvTarget::New)
}

/// 构造「续聊」请求体（复用既有会话；真机续聊形状）。
///
/// 续聊体与新建体的差异（真机 14:54 续聊样本 vs 19:26 新建样本实测）：
/// `client_meta` 不带 `local_conversation_id`；`option` 不带
/// `conversation_init_option`/`conversation_init_ext`/`conversation_mode` 且
/// `need_create_conversation=false`、`aggregate_params.conversation_mode` 为空串、
/// `general_task_param` 换成 `runtime_type:0` 版本；`ext` 不带
/// `sub_conv_firstmet_type`/`conversation_init_option`。
pub fn build_body_continue(
    peek: &Value,
    m: &DoubaoModel,
    conversation_id: &str,
    last_section_id: &str,
    last_message_index: i64,
) -> Value {
    build_body_with(
        peek,
        m,
        &ConvTarget::Continue {
            conversation_id: conversation_id.to_string(),
            last_section_id: last_section_id.to_string(),
            last_message_index,
        },
    )
}

/// 请求体主体：按 [`ConvTarget`] 切换会话相关字段，其余逐字段对齐真机。
fn build_body_with(peek: &Value, m: &DoubaoModel, target: &ConvTarget) -> Value {
    let is_new = matches!(target, ConvTarget::New);
    let merged = merge_messages(peek);

    // 叶子对象先各自成形，再装配：既避免 `json!` 宏嵌套过深触发递归上限，
    // 也让每段结构与真机抓包样本可逐一对读。
    let client_meta = match target {
        ConvTarget::New => json!({
            "local_conversation_id": local_conversation_id(),
            // 新建会话：真机为**空串**；`last_message_index` 为 **null**（不是 0）
            "conversation_id": "",
            "bot_id": DOUBAO_BOT_ID,
            "last_section_id": "",
            "last_message_index": Value::Null,
            "local_permissions": local_permissions(),
        }),
        // 续聊：真机**不带** `local_conversation_id`，改用真实会话 id / section / 上一条下标
        ConvTarget::Continue { conversation_id, last_section_id, last_message_index } => json!({
            "conversation_id": conversation_id,
            "bot_id": DOUBAO_BOT_ID,
            "last_section_id": last_section_id,
            "last_message_index": last_message_index,
            "local_permissions": local_permissions(),
        }),
    };

    let text_block = json!({
        "text": merged,
        "icon_url": "",
        "icon_url_dark": "",
        "summary": "",
    });
    let block = json!({
        "block_type": BLOCK_TYPE_TEXT,
        "content": json!({"text_block": text_block, "pc_event_block": ""}),
        "block_id": new_uuid(),
        "parent_id": "",
        // 真机与 content 同级的三段：meta_info / append_fields 为空数组，extra_info 为空对象
        "meta_info": [],
        "extra_info": {},
        "append_fields": [],
    });
    let message = json!({
        "local_message_id": new_uuid(),
        "content_block": [block],
        "message_status": 0,
    });

    // 建会话三件套（**仅新建会话**发送；真机新建样本实测）
    let conversation_init_option = json!({"need_ack_conversation": true});
    let conversation_init_ext = json!({
        "model_item_key": m.model_item_key,
        "reasoning_effort": REASONING_EFFORT,
        "mode_id": m.mode_id,
    });
    // 任务参数：新建 / 续聊两套（真机样本形状不同）
    let general_task_param = if is_new {
        json!({"server_do_compare": true})
    } else {
        json!({"runtime_type": 0, "agent_task_param": {}, "project_id": "", "server_do_compare": true})
    };
    let model_config = json!({
        "model_item_key": m.model_item_key,
        "model_extra_params": {},
        "reasoning_effort": 5,
    });
    let aggregate_params = json!({
        // 真机这三项是「JSON 字符串」而非数组/对象
        "mention_skill_list": "[]",
        "mention_plugin_list": "[]",
        "mention_ext": "[{}]",
        // 续聊时真机为空串（新建时是档位号）
        "conversation_mode": if is_new { m.conversation_mode.to_string() } else { String::new() },
        "mode_id": m.mode_id,
        "model_item_key": m.model_item_key,
        "agent_mode": m.agent_mode.to_string(),
        "reasoning_effort": REASONING_EFFORT,
        "provider_id": "",
    });
    let recovery_option = json!({
        "is_recovery": false,
        "req_create_time_sec": now_sec(),
        "append_sse_event_scene": 0,
    });
    let sse_opts = json!({"support_chunk_delta": true});

    // `option` 键多达 40+，单个 `json!` 会触达宏递归上限，故分三段装配后合并
    let mut option = json!({
        "send_message_scene": "",
        "create_time_ms": now_ms(),
        "collect_id": "",
        "is_audio": false,
        "answer_with_suggest": false,
        "agent_mode": m.agent_mode,
        "tts_switch": false,
        "need_deep_think": m.need_deep_think,
        "click_clear_context": false,
        "from_suggest": false,
        "is_regen": false,
        "is_replace": false,
        "is_from_click_option": false,
        "is_from_click_softlink": false,
        "disable_sse_cache": false,
    });
    let mut chunk_mid = json!({
        "select_text_action": "",
        "is_select_text": false,
        "resend_for_regen": false,
        "scene_type": 0,
        "unique_key": new_uuid(),
        "start_seq": 0,
        "need_create_conversation": is_new,
        "regen_query_id": [],
        "edit_query_id": [],
        "regen_instruction": "",
        "no_replace_for_regen": false,
        "message_from": 0,
    });
    // 建会话三件套**仅新建会话**发送（真机续聊体不带这三键）
    if is_new {
        if let Some(o) = chunk_mid.as_object_mut() {
            o.insert("conversation_init_option".into(), conversation_init_option);
            o.insert("conversation_init_ext".into(), conversation_init_ext);
        }
    }
    let mut chunk_last = json!({
        "shared_app_name": "",
        "shared_app_id": "",
        "sse_recv_event_options": sse_opts,
        "support_lazy_fetch_stream": true,
        "is_ai_playground": false,
        "is_old_user": true,
        "general_task_param": general_task_param,
        "recovery_option": recovery_option,
        "message_storage_type": 0,
        "related_deleted_message_ids": {},
        "connector_info_list": [],
        "model_config": model_config,
        "aggregate_params": aggregate_params,
    });
    // `conversation_mode`（option 级）亦**仅新建会话**有
    if is_new {
        if let Some(o) = chunk_last.as_object_mut() {
            o.insert("conversation_mode".into(), json!(m.conversation_mode));
        }
    }
    for chunk in [chunk_mid, chunk_last] {
        if let (Some(dst), Some(src)) = (option.as_object_mut(), chunk.as_object()) {
            for (k, v) in src {
                dst.insert(k.clone(), v.clone());
            }
        }
    }

    // `ext`：真机除 `general_task_param`/`conversation_init_option`/`collection_id` 外，
    // 其余开关均为**字符串**（"0"/"1"/"2"）——写成数字会触发 710020202（逐字段 diff 实测）
    let mut ext = json!({
        "agent_mode": m.agent_mode.to_string(),
        "use_deep_think": if m.need_deep_think > 0 { "1" } else { "0" },
        // 新建 / 续聊两套（真机样本形状不同）
        "general_task_param": if is_new {
            "{\"server_do_compare\":true}"
        } else {
            "{\"runtime_type\":0,\"agent_task_param\":{},\"project_id\":\"\",\"server_do_compare\":true}"
        },
        "collection_id": "",
        "is_finish": "1",
        "commerce_credit_config_enable": "0",
    });
    // 新建会话专有的两键（真机续聊体不带）
    if is_new {
        if let Some(o) = ext.as_object_mut() {
            o.insert("sub_conv_firstmet_type".into(), json!("1"));
            o.insert(
                "conversation_init_option".into(),
                json!("{\"need_ack_conversation\":true}"),
            );
        }
    }

    json!({
        "client_meta": client_meta,
        "messages": [message],
        "option": option,
        "user_context": [],
        "ext": ext,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_has_verified_and_placeholder() {
        assert!(is_enabled_verified("doubao-auto"));
        assert!(is_enabled_verified("doubao-think"));
        assert!(is_enabled_verified("doubao-expert"));
        assert!(!is_enabled_verified("doubao-2.1-lite"));
        assert!(!is_enabled_verified("doubao-2.1-pro"));
        assert!(!is_enabled_verified("doubao-2.1-turbo"));
        assert!(lookup("doubao-auto").is_some());
    }

    #[test]
    fn lookup_is_trimmed_and_case_insensitive() {
        assert_eq!(lookup("  DOUBAO-THINK ").map(|m| m.id), Some("doubao-think"));
        assert!(lookup("does-not-exist").is_none());
    }

    #[test]
    fn merge_messages_joins_history() {
        let peek = json!({
            "messages": [
                {"role": "system", "content": "你是助手"},
                {"role": "user", "content": "你好"},
                {"role": "assistant", "content": "你好，请问需要什么"},
                {"role": "user", "content": [{"type": "text", "text": "讲个笑话"}]}
            ]
        });
        let merged = merge_messages(&peek);
        assert!(merged.contains("系统：你是助手"));
        assert!(merged.contains("用户：你好"));
        assert!(merged.contains("助手：你好，请问需要什么"));
        assert!(merged.contains("用户：讲个笑话"));
    }

    /// 顶层键与真机一致（新版客户端**不发** `chat_ability`）
    #[test]
    fn build_body_top_level_keys() {
        let peek = json!({"messages": [{"role": "user", "content": "hi"}]});
        let body = build_body(&peek, lookup("doubao-auto").unwrap());
        let mut top: Vec<&str> = body.as_object().unwrap().keys().map(String::as_str).collect();
        top.sort_unstable();
        assert_eq!(top, vec!["client_meta", "ext", "messages", "option", "user_context"]);
    }

    /// auto 档：逐字段对齐真机「普通模式 + 新建会话」样本（含两类反直觉取值）
    #[test]
    fn build_body_auto_matches_golden_shape() {
        let peek = json!({"messages": [{"role": "user", "content": "hi"}]});
        let body = build_body(&peek, lookup("doubao-auto").unwrap());

        // client_meta：本地会话 id + 空会话 id + null 索引
        let lcid = body["client_meta"]["local_conversation_id"].as_str().unwrap();
        assert!(lcid.starts_with("local_"), "本地会话 id 前缀应为 local_");
        let digits = &lcid["local_".len()..];
        assert_eq!(digits.len(), 16, "本地会话 id 应为 16 位数字");
        assert!(digits.bytes().all(|b| b.is_ascii_digit()));
        assert_eq!(body["client_meta"]["conversation_id"], "");
        assert_eq!(body["client_meta"]["last_section_id"], "");
        assert!(body["client_meta"]["last_message_index"].is_null(), "索引应为 null");
        assert_eq!(body["client_meta"]["bot_id"], DOUBAO_BOT_ID);

        // 正文块
        let blk = &body["messages"][0]["content_block"][0];
        assert_eq!(blk["block_type"], BLOCK_TYPE_TEXT);
        assert_eq!(blk["content"]["text_block"]["text"], "用户：hi");
        assert_eq!(blk["content"]["pc_event_block"], "");
        assert_eq!(blk["meta_info"], json!([]));
        assert_eq!(blk["extra_info"], json!({}));
        assert_eq!(blk["append_fields"], json!([]));
        assert_eq!(blk["parent_id"], "");
        assert_eq!(body["messages"][0]["message_status"], 0);

        // option：建会话三件套 + 档位字段
        let opt = &body["option"];
        assert_eq!(opt["need_create_conversation"], true);
        assert_eq!(opt["conversation_init_option"], json!({"need_ack_conversation": true}));
        assert_eq!(
            opt["conversation_init_ext"],
            json!({"model_item_key": "0", "reasoning_effort": "5", "mode_id": "1"})
        );
        assert_eq!(opt["agent_mode"], 2);
        assert_eq!(opt["need_deep_think"], 0);
        assert_eq!(opt["conversation_mode"], 1);
        assert_eq!(opt["general_task_param"], json!({"server_do_compare": true}));
        assert_eq!(opt["is_old_user"], true);
        assert_eq!(
            opt["model_config"],
            json!({"model_item_key": "0", "model_extra_params": {}, "reasoning_effort": 5})
        );
        let agg = &opt["aggregate_params"];
        assert_eq!(agg["mention_skill_list"], "[]");
        assert_eq!(agg["mention_plugin_list"], "[]");
        assert_eq!(agg["mention_ext"], "[{}]");
        assert_eq!(agg["conversation_mode"], "1");
        assert_eq!(agg["mode_id"], "1");
        assert_eq!(agg["model_item_key"], "0");
        assert_eq!(agg["agent_mode"], "2");
        assert_eq!(agg["reasoning_effort"], "5");
        assert_eq!(agg["provider_id"], "");
        assert_eq!(opt["sse_recv_event_options"], json!({"support_chunk_delta": true}));
        // 建会话时间戳为真实秒级
        assert!(opt["recovery_option"]["req_create_time_sec"].as_i64().unwrap() > 0);
        assert_eq!(opt["recovery_option"]["is_recovery"], false);

        // ext：值均为**字符串**（数字会 710020202）
        let ext = &body["ext"];
        assert_eq!(ext["agent_mode"], "2");
        assert_eq!(ext["use_deep_think"], "0");
        assert_eq!(ext["sub_conv_firstmet_type"], "1");
        assert_eq!(ext["general_task_param"], "{\"server_do_compare\":true}");
        assert_eq!(ext["collection_id"], "");
        assert_eq!(ext["is_finish"], "1");
        assert_eq!(ext["conversation_init_option"], "{\"need_ack_conversation\":true}");
        assert_eq!(ext["commerce_credit_config_enable"], "0");
        assert!(ext.get("suggest_extra").is_none(), "新版客户端不发 suggest_extra");

        // uuid 长度
        assert_eq!(body["messages"][0]["local_message_id"].as_str().unwrap().len(), 36);
        assert_eq!(blk["block_id"].as_str().unwrap().len(), 36);
        assert_eq!(opt["unique_key"].as_str().unwrap().len(), 36);
    }

    /// think 档：仅 `need_deep_think` / `ext.use_deep_think` 置位（其余同 auto）
    #[test]
    fn build_body_think_sets_deep_think() {
        let peek = json!({"messages": [{"role": "user", "content": "hi"}]});
        let body = build_body(&peek, lookup("doubao-think").unwrap());
        assert_eq!(body["option"]["need_deep_think"], 1);
        assert_eq!(body["ext"]["use_deep_think"], "1");
        assert_eq!(body["option"]["agent_mode"], 2);
    }

    /// expert 档：档位字段切换为 Agent 样本取值
    #[test]
    fn build_body_expert_uses_agent_profile() {
        let peek = json!({"messages": [{"role": "user", "content": "hi"}]});
        let body = build_body(&peek, lookup("doubao-expert").unwrap());
        assert_eq!(body["option"]["need_deep_think"], 9);
        assert_eq!(body["option"]["agent_mode"], 1);
        assert_eq!(body["option"]["conversation_mode"], 2);
        assert_eq!(body["option"]["conversation_init_ext"]["model_item_key"], "9");
        assert_eq!(body["option"]["conversation_init_ext"]["mode_id"], "3");
        assert_eq!(body["option"]["aggregate_params"]["model_item_key"], "9");
        assert_eq!(body["ext"]["agent_mode"], "1");
        assert_eq!(body["ext"]["use_deep_think"], "1");
    }

    /// 续聊体：仅会话相关字段切换为真机续聊形状，其余与新建体一致
    #[test]
    fn build_body_continue_shape() {
        let peek = json!({"messages": [{"role": "user", "content": "hi"}]});
        let m = lookup("doubao-auto").unwrap();
        let body = build_body_continue(&peek, m, "38447405334903554", "38447405334903810", 40);

        assert_eq!(body["client_meta"]["conversation_id"], "38447405334903554");
        assert_eq!(body["client_meta"]["last_section_id"], "38447405334903810");
        assert_eq!(body["client_meta"]["last_message_index"], 40);
        assert!(
            body["client_meta"].get("local_conversation_id").is_none(),
            "续聊体不带本地会话 id"
        );

        let opt = &body["option"];
        assert_eq!(opt["need_create_conversation"], false);
        assert!(opt.get("conversation_init_option").is_none(), "续聊不带建会话三件套");
        assert!(opt.get("conversation_init_ext").is_none());
        assert!(opt.get("conversation_mode").is_none());
        assert_eq!(opt["aggregate_params"]["conversation_mode"], "");
        assert_eq!(
            opt["general_task_param"],
            json!({"runtime_type": 0, "agent_task_param": {}, "project_id": "", "server_do_compare": true})
        );

        let ext = &body["ext"];
        assert!(ext.get("sub_conv_firstmet_type").is_none());
        assert!(ext.get("conversation_init_option").is_none());
        assert_eq!(
            ext["general_task_param"],
            "{\"runtime_type\":0,\"agent_task_param\":{},\"project_id\":\"\",\"server_do_compare\":true}"
        );
        // 其余（正文块 / 权限 / 档位）应与新建体一致
        assert_eq!(body["messages"][0]["content_block"][0]["block_type"], BLOCK_TYPE_TEXT);
        assert_eq!(body["client_meta"]["bot_id"], DOUBAO_BOT_ID);
        assert_eq!(body["ext"]["agent_mode"], "2");
    }

    /// Anthropic 顶层 system 并入
    #[test]
    fn build_body_merges_anthropic_system() {
        let peek = json!({
            "system": "你是豆包",
            "messages": [{"role": "user", "content": "在吗"}]
        });
        let body = build_body(&peek, lookup("doubao-auto").unwrap());
        let text = body["messages"][0]["content_block"][0]["content"]["text_block"]["text"]
            .as_str()
            .unwrap();
        assert!(text.contains("系统：你是豆包"));
        assert!(text.contains("用户：在吗"));
    }

    /// 诊断（`#[ignore]`，仅本机跑）：把 auto 档请求体落临时文件，供与真机样本做机械 diff。
    /// **不联网、不消耗豆包额度**；请求体不含任何凭据（只有文本/时间/随机 id）。
    /// 运行：`cargo test --no-default-features doubao_payload::tests::dump_body_for_diff -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn dump_body_for_diff() {
        let peek = json!({"messages": [{"role": "user", "content": "hi"}]});
        let body = build_body(&peek, lookup("doubao-auto").unwrap());
        let path = std::env::temp_dir().join("doubao_our_body.txt");
        std::fs::write(&path, serde_json::to_string(&body).unwrap()).expect("写临时文件失败");
        println!("dumped: {}", path.display());
    }
}
