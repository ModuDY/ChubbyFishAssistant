//! chat 路径的 tool_call ↔ tool 结果配对清理（出站前最后一道防线）。
//!
//! OpenAI 兼容协议要求：带 `tool_calls` 的 assistant 消息，其**每一个** `tool_call_id`
//! 都必须有对应的 `role: tool` 结果消息；反之 `role: tool` 消息也必须有前置调用。
//! 任一侧缺失，上游会以 400（11101/11148）拒绝**整个请求**——而这类坏历史通常由
//! 客户端在工具执行失败后写入会话，之后每次重试都原样重放，整条会话报废。
//!
//! 网关侧做对称裁剪让会话自愈：宁可丢这一轮工具上下文，也好过整条会话打不开。
//! 规则（对齐参考实现 `tool_pairing.go`）：
//! - 相邻的「assistant + tool_calls」先合并成一条（客户端回放并行调用时常拆成多条，
//!   中间夹一条 assistant 会让上游判 `tool_call_sequence_broken`）；
//! - `keep = 有对应结果 且 function.name 非空` 的调用 id；
//! - assistant.tool_calls 按 keep 裁剪（全被裁掉则删键）；
//! - `role: tool` 按 keep 裁剪（孤儿结果整条丢弃）。
//!
//! 纯函数、无 IO，便于单测。

use serde_json::Value;

/// 裁剪统计（诊断/日志用）
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PairingStats {
    /// 被合并掉的相邻 assistant 消息数
    pub merged: usize,
    /// 被裁掉的 tool_call 数
    pub dropped_calls: usize,
    /// 被丢弃的孤儿 tool 结果数
    pub dropped_results: usize,
}

impl PairingStats {
    pub fn changed(&self) -> bool {
        self.merged > 0 || self.dropped_calls > 0 || self.dropped_results > 0
    }
}

fn role_of(m: &Value) -> &str {
    m.get("role").and_then(Value::as_str).unwrap_or("")
}

/// 取 tool_call 的 id（无则空串）
fn call_id(tc: &Value) -> &str {
    tc.get("id").and_then(Value::as_str).unwrap_or("")
}

/// 取 tool_call 的 function 名（兼容 function / function_call 两种命名）
fn call_name(tc: &Value) -> &str {
    tc.pointer("/function/name")
        .or_else(|| tc.pointer("/function_call/name"))
        .and_then(Value::as_str)
        .unwrap_or("")
}

/// 原地清理消息数组的 tool 配对；返回裁剪统计。
pub fn normalize_tool_pairing(messages: &mut Vec<Value>) -> PairingStats {
    let mut stats = PairingStats::default();
    merge_adjacent_tool_calls(messages, &mut stats);
    if !has_tool_traffic(messages) {
        return stats;
    }
    let keep = keep_calls(messages);
    trim_calls(messages, &keep, &mut stats);
    drop_orphan_results(messages, &keep, &mut stats);
    stats
}

/// 相邻的 assistant(tool_calls) 合并成一条：并行调用被拆成多条时会判 11148
fn merge_adjacent_tool_calls(messages: &mut Vec<Value>, stats: &mut PairingStats) {
    let mut i = 0usize;
    while i + 1 < messages.len() {
        let both_assistant_calls = role_of(&messages[i]) == "assistant"
            && role_of(&messages[i + 1]) == "assistant"
            && has_calls(&messages[i])
            && has_calls(&messages[i + 1]);
        if !both_assistant_calls {
            i += 1;
            continue;
        }
        let next_calls = messages[i + 1]
            .get("tool_calls")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if let Some(arr) = messages[i].get_mut("tool_calls").and_then(Value::as_array_mut) {
            arr.extend(next_calls);
        }
        messages.remove(i + 1);
        stats.merged += 1;
        // 不前进：可能还有第三条紧邻
    }
}

fn has_calls(m: &Value) -> bool {
    m.get("tool_calls")
        .and_then(Value::as_array)
        .is_some_and(|a| !a.is_empty())
}

/// 请求里是否真的出现工具调用/结果（纯文本会话零改动快速返回）
fn has_tool_traffic(messages: &[Value]) -> bool {
    messages
        .iter()
        .any(|m| has_calls(m) || role_of(m) == "tool")
}

/// 双侧齐全且 function.name 非空的调用 id 集合
fn keep_calls(messages: &[Value]) -> std::collections::HashSet<String> {
    let mut results: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for m in messages {
        if role_of(m) == "tool" {
            if let Some(id) = m.get("tool_call_id").and_then(Value::as_str) {
                if !id.is_empty() {
                    results.insert(id);
                }
            }
        }
    }
    let mut keep = std::collections::HashSet::new();
    for m in messages {
        let Some(calls) = m.get("tool_calls").and_then(Value::as_array) else {
            continue;
        };
        for tc in calls {
            let id = call_id(tc);
            // name 为空：上游 11133 直接拒，带不带结果都一样，一律不保留
            if !id.is_empty() && !call_name(tc).trim().is_empty() && results.contains(id) {
                keep.insert(id.to_string());
            }
        }
    }
    keep
}

fn trim_calls(
    messages: &mut [Value],
    keep: &std::collections::HashSet<String>,
    stats: &mut PairingStats,
) {
    for m in messages.iter_mut() {
        if role_of(m) != "assistant" {
            continue;
        }
        let Some(calls) = m.get("tool_calls").and_then(Value::as_array) else {
            continue;
        };
        let before = calls.len();
        let kept: Vec<Value> = calls
            .iter()
            .filter(|tc| keep.contains(call_id(tc)))
            .cloned()
            .collect();
        if kept.len() == before {
            continue;
        }
        stats.dropped_calls += before - kept.len();
        if let Some(obj) = m.as_object_mut() {
            if kept.is_empty() {
                obj.remove("tool_calls");
            } else {
                obj.insert("tool_calls".into(), Value::Array(kept));
            }
        }
    }
}

fn drop_orphan_results(
    messages: &mut Vec<Value>,
    keep: &std::collections::HashSet<String>,
    stats: &mut PairingStats,
) {
    let mut i = 0usize;
    while i < messages.len() {
        if role_of(&messages[i]) == "tool" {
            let id = messages[i]
                .get("tool_call_id")
                .and_then(Value::as_str)
                .unwrap_or("");
            if !keep.contains(id) {
                messages.remove(i);
                stats.dropped_results += 1;
                continue;
            }
        }
        i += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn msgs(v: Value) -> Vec<Value> {
        v.as_array().cloned().unwrap_or_default()
    }

    fn call(id: &str, name: &str) -> Value {
        json!({"id": id, "type": "function", "function": {"name": name, "arguments": "{}"}})
    }

    /// 纯文本会话零改动
    #[test]
    fn plain_conversation_untouched() {
        let mut m = msgs(json!([
            {"role": "system", "content": "s"},
            {"role": "user", "content": "hi"},
            {"role": "assistant", "content": "hello"}
        ]));
        let before = m.clone();
        let st = normalize_tool_pairing(&mut m);
        assert_eq!(m, before);
        assert!(!st.changed());
    }

    /// 调用有结果是完整配对：零改动
    #[test]
    fn complete_pairing_untouched() {
        let mut m = msgs(json!([
            {"role": "assistant", "tool_calls": [call("c1", "f")]},
            {"role": "tool", "tool_call_id": "c1", "content": "ok"}
        ]));
        let st = normalize_tool_pairing(&mut m);
        assert_eq!(st, PairingStats::default());
        assert_eq!(m[0]["tool_calls"].as_array().unwrap().len(), 1);
        assert_eq!(m[1]["tool_call_id"], json!("c1"));
    }

    /// 调用无结果（工具执行失败的历史）：裁掉该调用，避免整条会话 400
    #[test]
    fn call_without_result_is_trimmed() {
        let mut m = msgs(json!([
            {"role": "user", "content": "q"},
            {"role": "assistant", "tool_calls": [call("c1", "f")]},
            {"role": "user", "content": "again"}
        ]));
        let st = normalize_tool_pairing(&mut m);
        assert_eq!(st.dropped_calls, 1);
        assert!(m[1].get("tool_calls").is_none(), "无结果的调用必须裁掉");
    }

    /// 部分有结果：按 id 对称裁剪，保留有结果的那条
    #[test]
    fn partial_batch_trims_symmetrically() {
        let mut m = msgs(json!([
            {"role": "assistant", "tool_calls": [call("c1", "f"), call("c2", "f")]},
            {"role": "tool", "tool_call_id": "c1", "content": "ok"}
        ]));
        let st = normalize_tool_pairing(&mut m);
        assert_eq!(st.dropped_calls, 1);
        assert_eq!(st.dropped_results, 0);
        let kept = m[0]["tool_calls"].as_array().unwrap();
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0]["id"], json!("c1"));
        assert_eq!(m[1]["tool_call_id"], json!("c1"));
    }

    /// 孤儿结果（无对应调用）整条丢弃
    #[test]
    fn orphan_result_dropped() {
        let mut m = msgs(json!([
            {"role": "user", "content": "q"},
            {"role": "tool", "tool_call_id": "ghost", "content": "x"},
            {"role": "assistant", "content": "a"}
        ]));
        let st = normalize_tool_pairing(&mut m);
        assert_eq!(st.dropped_results, 1);
        assert_eq!(m.len(), 2);
        assert_eq!(m[1]["role"], json!("assistant"));
    }

    /// 相邻 assistant(tool_calls) 合并：并行调用被拆成多条会判 11148
    #[test]
    fn adjacent_calls_merged() {
        let mut m = msgs(json!([
            {"role": "assistant", "tool_calls": [call("c1", "f")]},
            {"role": "assistant", "tool_calls": [call("c2", "f")]},
            {"role": "tool", "tool_call_id": "c1", "content": "r1"},
            {"role": "tool", "tool_call_id": "c2", "content": "r2"}
        ]));
        let st = normalize_tool_pairing(&mut m);
        assert_eq!(st.merged, 1);
        assert_eq!(m.len(), 3);
        assert_eq!(m[0]["tool_calls"].as_array().unwrap().len(), 2);
        // 合并后两条调用都有结果 → 不再裁剪
        assert_eq!(st.dropped_calls, 0);
        assert_eq!(st.dropped_results, 0);
    }

    /// function 名为空的调用一律不保留（上游 11133 必拒）
    #[test]
    fn empty_name_call_dropped_even_with_result() {
        let mut m = msgs(json!([
            {"role": "assistant", "tool_calls": [call("c1", "  ")]},
            {"role": "tool", "tool_call_id": "c1", "content": "ok"}
        ]));
        let st = normalize_tool_pairing(&mut m);
        assert_eq!(st.dropped_calls, 1);
        assert_eq!(st.dropped_results, 1, "调用被裁后其结果成为孤儿");
        assert_eq!(m.len(), 1);
    }

    /// 兼容 function_call 命名的历史形态（SOLO 内部格式）
    #[test]
    fn supports_function_call_naming() {
        let mut m = msgs(json!([
            {"role": "assistant", "tool_calls": [
                {"id": "c1", "type": "function", "function_call": {"name": "f", "arguments": "{}"}}
            ]},
            {"role": "tool", "tool_call_id": "c1", "content": "ok"}
        ]));
        let st = normalize_tool_pairing(&mut m);
        assert_eq!(st.dropped_calls, 0, "function_call 命名应同样识别");
        assert_eq!(st.dropped_results, 0);
    }
}
