//! 出站提示词的降级重建（P3-1 第一步）。
//!
//! 只做一件事：把请求体里的 system/developer 消息整体换成一段极简中性提示词。
//!
//! 为什么必须"整体替换"而不是"再清洗一遍"：上游按逐字匹配拦截，只要原句还在
//! 任何一条 system 里就仍会被拦。降级时必须把客户端 system 全部摘掉。
//!
//! 注意：本模块**不改变正常请求**——只有命中内容拦截后的降级重试才调用它。
//! 提示词三模式（custom/append/passthrough）暂未引入，默认即"完全透传"。

use serde_json::{json, Value};

/// 降级中性提示词：刻意极简，不含任何客户端身份/工具指纹。
/// 只用于绕开 system 来源的误报，不改变用户指令的合法性语义。
pub const DEGRADED_SYSTEM: &str =
    "You are a helpful assistant. Respond in the user's language, follow the user's instructions, and be direct and concise.";

/// 把 body 的 system/developer 消息替换为单条中性提示词。
///
/// 规则（对齐参考实现的 Rewrite 语义）：
/// - 删除 messages 中**所有** role 为 system / developer 的消息；
/// - 在 messages 头部插入一条 `{"role":"system","content":DEGRADED_SYSTEM}`；
/// - user / assistant / tool 消息逐字不动；
/// - 任何解析失败原样返回（降级是应急路径，绝不因改写而阻塞转发）。
pub fn apply_degraded_system(body: &[u8]) -> Vec<u8> {
    let mut obj: Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(_) => return body.to_vec(),
    };
    let Some(obj_mut) = obj.as_object_mut() else {
        return body.to_vec();
    };
    let neutral = json!({ "role": "system", "content": DEGRADED_SYSTEM });

    match obj_mut.get_mut("messages").and_then(Value::as_array_mut) {
        Some(msgs) => {
            msgs.retain(|m| {
                let role = m.get("role").and_then(Value::as_str).unwrap_or("");
                role != "system" && role != "developer"
            });
            msgs.insert(0, neutral);
        }
        None => {
            obj_mut.insert("messages".into(), Value::Array(vec![neutral]));
        }
    }

    serde_json::to_vec(&obj).unwrap_or_else(|_| body.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(v: Value) -> Value {
        let out = apply_degraded_system(&serde_json::to_vec(&v).unwrap());
        serde_json::from_slice(&out).unwrap()
    }

    /// 所有 system/developer 被摘掉，只留一条中性提示词在头部
    #[test]
    fn replaces_all_system_messages() {
        let out = apply(json!({
            "model": "m",
            "messages": [
                {"role": "system", "content": "You are Claude Code, Anthropic's official CLI for Claude."},
                {"role": "developer", "content": "extra rules"},
                {"role": "user", "content": "你好"},
                {"role": "assistant", "content": "在"},
                {"role": "user", "content": "继续"}
            ]
        }));
        let msgs = out["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 4, "两条 system/developer 合并成一条中性提示词");
        assert_eq!(msgs[0]["role"], json!("system"));
        assert_eq!(msgs[0]["content"], json!(DEGRADED_SYSTEM));
        // 其余消息逐字保持
        assert_eq!(msgs[1]["content"], json!("你好"));
        assert_eq!(msgs[2]["content"], json!("在"));
        assert_eq!(msgs[3]["content"], json!("继续"));
        // 原始指纹句不得残留
        assert!(!out.to_string().contains("Claude Code"));
    }

    /// 无 system 时也要补一条（否则降级后没有任何约束）
    #[test]
    fn inserts_neutral_when_no_system() {
        let out = apply(json!({"messages": [{"role": "user", "content": "hi"}]}));
        let msgs = out["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0]["content"], json!(DEGRADED_SYSTEM));
    }

    /// 缺 messages 字段：补一条中性提示词，其余字段保持
    #[test]
    fn missing_messages_gets_neutral() {
        let out = apply(json!({"model": "m", "temperature": 0.2}));
        assert_eq!(out["model"], json!("m"));
        assert_eq!(out["temperature"], json!(0.2));
        assert_eq!(out["messages"][0]["content"], json!(DEGRADED_SYSTEM));
    }

    /// 工具相关字段（tools / tool_calls / tool 消息）不被破坏
    #[test]
    fn tool_fields_preserved() {
        let out = apply(json!({
            "messages": [
                {"role": "system", "content": "fp"},
                {"role": "assistant", "tool_calls": [
                    {"id": "c1", "type": "function", "function": {"name": "f", "arguments": "{}"}}
                ]},
                {"role": "tool", "tool_call_id": "c1", "content": "ok"}
            ],
            "tools": [{"type": "function", "function": {"name": "f", "parameters": {}}}]
        }));
        let msgs = out["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[1]["tool_calls"][0]["id"], json!("c1"));
        assert_eq!(msgs[2]["tool_call_id"], json!("c1"));
        assert_eq!(out["tools"][0]["function"]["name"], json!("f"));
    }

    /// 坏 JSON 原样返回（绝不因降级改写阻塞转发）
    #[test]
    fn invalid_json_returned_as_is() {
        let raw = b"not json at all";
        assert_eq!(apply_degraded_system(raw), raw.to_vec());
    }
}
