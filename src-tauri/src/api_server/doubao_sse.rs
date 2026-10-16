//! 豆包（PC 口径 `/chat/completion`）SSE → 标准 OpenAI `data:{chunk}` 流投影（E-01）。
//!
//! ── 上游真机帧形态（批次 0 抓包固化）────────────────────────────────────
//! 每行 `data: <json>`，**不是** `{event_type, event_data}` 信封，而是「快照 + 补丁」流：
//! ```text
//! {}                                  心跳/占位
//! {query_list,ack_client_meta,timeout_conf}   ack（含 conversation_id / section_id）
//! {message,message_attr}               用户提问的快照回显（sender=用户，**不得当作回答**）
//! {content,meta,attr}                  富块快照（loading / 引用等，无正文增量）
//! {message_id,patch_op:[…]}            正文增量：patch_op[].patch_value.content_block[0]
//!                                      且 block_type==10000 时 content.text_block.text 为**增量片段**
//! {end_type=1,msg_finish_attr}         消息收尾（`brief` 是摘要，非全文）
//! {end_type=2,answer_finish_attr}      回答收尾
//! {end_type=3}                         **整条流结束**
//! ```
//! `patch_op[].patch_object`：`1` = 内容块（正文/思考），`111` = `tts_content`（与正文**同长的
//! 语音副本**，必须忽略，否则内容翻倍）。
//!
//! ── 正文/思考判别 ─────────────────────────────────────────────────────
//! 正文块 `block_type = 10000`；思考链块的 `block_type` 真机样本未覆盖（战术笔记口径为
//! 2008），故此处按 `10000 → 正文 / 2008 → 思考` 映射，**待思考档真机回归确认**。
//!
//! **省量点**：本模块只把豆包增量投影为标准 OpenAI chunk 行（`data: {…}` + `data: [DONE]`），
//! 下游复用 `wb_sse::{stream_forward_ex, aggregate}` 一次性拿到 OpenAI / text / Anthropic /
//! Responses 四协议投影、keep-alive 与断线检测，**无需自写协议转换**。

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use super::doubao_upstream::{detect_risk, RiskCode};

/// 正文块类型（真机实测）
const BLOCK_TYPE_TEXT: u64 = 10000;
/// 思考链块类型（战术笔记口径；真机思考档样本未覆盖，待回归确认）
const BLOCK_TYPE_REASONING: u64 = 2008;
/// `patch_op[].patch_object`：内容块
const PATCH_OBJECT_CONTENT: u64 = 1;

/// 豆包单个 SSE 事件（已解析）
#[derive(Debug, Clone, PartialEq)]
pub enum DoubaoEvent {
    /// 文本/思考链增量
    Delta { content: Option<String>, reasoning: Option<String> },
    /// ack（含会话 id；仅用于记录，不产生输出）
    Ack { conversation_id: String },
    /// 结束（`end_type=3`）
    Done { conversation_id: String },
    /// 风控码（账号级）
    Risk(RiskCode),
    /// 其他错误帧
    Error { code: i64, msg: String },
}

/// 共享风控槽（translate 写入，路由读取决定冷却/禁用）
pub type RiskSlot = Arc<Mutex<Option<RiskCode>>>;

/// 逐层剥离「JSON 字符串包裹 JSON」（最多 4 层，防循环）
fn peel(mut v: Value) -> Value {
    for _ in 0..4 {
        match v {
            Value::String(s) => match serde_json::from_str::<Value>(&s) {
                Ok(parsed) => v = parsed,
                Err(_) => return Value::String(s),
            },
            other => return other,
        }
    }
    v
}

/// 宽松取数字（数值 / 数字字符串）
fn to_u64(v: &Value) -> Option<u64> {
    v.as_u64()
        .or_else(|| v.as_i64().filter(|n| *n >= 0).map(|n| n as u64))
        .or_else(|| v.as_str().and_then(|s| s.trim().parse::<u64>().ok()))
}

/// 递归查找数值字段（usage 可能藏在任意层）
fn find_num(v: &Value, key: &str) -> Option<u64> {
    match v {
        Value::Object(map) => {
            if let Some(x) = map.get(key).and_then(to_u64) {
                return Some(x);
            }
            map.values().find_map(|c| find_num(c, key))
        }
        Value::Array(items) => items.iter().find_map(|c| find_num(c, key)),
        _ => None,
    }
}

/// 从内容块取 `content.text_block.text`（content 可能是对象或 JSON 字符串）
fn block_text(block: &Value) -> Option<String> {
    let content = block.get("content")?.clone();
    let content = peel(content);
    let tb = content.get("text_block").cloned().map(peel)?;
    tb.get("text").and_then(Value::as_str).map(str::to_string)
}

/// 内容块 → (正文 / 思考) 增量（非文本块返回 `None`）
fn block_delta(block: &Value) -> Option<(Option<String>, Option<String>)> {
    let bt = block.get("block_type").and_then(to_u64)?;
    let text = block_text(block)?;
    if text.is_empty() {
        return None;
    }
    match bt {
        BLOCK_TYPE_TEXT => Some((Some(text), None)),
        BLOCK_TYPE_REASONING => Some((None, Some(text))),
        _ => None,
    }
}

/// 解析单行原始 SSE 载荷 → [`DoubaoEvent`]（容错：未知形态返回 `None`）。
///
/// 折叠一帧内的多个内容块：正文与思考各自**按出现顺序拼接**，一次返回。
pub fn parse_line(raw: &str) -> Option<DoubaoEvent> {
    let line = raw.trim();
    let line = line.strip_prefix("data:").unwrap_or(line).trim();
    if line.is_empty() || line == "[DONE]" {
        return None;
    }
    // 风控码可能出现在任意文本里（含被 JSON 字符串包裹的层）
    if let Some(risk) = detect_risk(200, line) {
        return Some(DoubaoEvent::Risk(risk));
    }
    let outer: Value = serde_json::from_str(line).ok()?;

    // end_type：1 消息收尾 / 2 回答收尾 / 3 整条流结束（仅 3 视为 Done）
    if let Some(end) = outer.get("end_type").and_then(to_u64) {
        return if end == 3 {
            Some(DoubaoEvent::Done {
                conversation_id: String::new(),
            })
        } else {
            None
        };
    }

    // 错误帧（真机 `event: STREAM_ERROR`）：`{error_code, error_msg, extra}`。
    // 非风控码（如 710020202 common invalid param）投影为通用错误，**不得静默当作空回答**。
    if let Some(code) = outer.get("error_code").and_then(to_u64) {
        let msg = outer
            .get("error_msg")
            .and_then(Value::as_str)
            .unwrap_or("doubao upstream error")
            .to_string();
        return match detect_risk(200, &msg) {
            Some(risk) => Some(DoubaoEvent::Risk(risk)),
            None => Some(DoubaoEvent::Error {
                code: code as i64,
                msg,
            }),
        };
    }

    // ack：取会话 id（不产生输出）
    if let Some(conv) = outer
        .get("ack_client_meta")
        .and_then(|m| m.get("conversation_id"))
        .and_then(Value::as_str)
    {
        return Some(DoubaoEvent::Ack {
            conversation_id: conv.to_string(),
        });
    }

    // patch_op：正文增量主体
    if let Some(ops) = outer.get("patch_op").and_then(Value::as_array) {
        let mut content = String::new();
        let mut reasoning = String::new();
        for op in ops {
            // 只取内容块补丁；tts_content（patch_object=111）是语音副本，必须忽略
            if op.get("patch_object").and_then(to_u64) != Some(PATCH_OBJECT_CONTENT) {
                continue;
            }
            let Some(blocks) = op
                .get("patch_value")
                .and_then(|v| v.get("content_block"))
                .and_then(Value::as_array)
            else {
                continue;
            };
            for block in blocks {
                if let Some((c, r)) = block_delta(block) {
                    if let Some(c) = c {
                        content.push_str(&c);
                    }
                    if let Some(r) = r {
                        reasoning.push_str(&r);
                    }
                }
            }
        }
        if content.is_empty() && reasoning.is_empty() {
            return None;
        }
        return Some(DoubaoEvent::Delta {
            content: (!content.is_empty()).then_some(content),
            reasoning: (!reasoning.is_empty()).then_some(reasoning),
        });
    }

    // content 快照帧：真机流中通常是 loading / 引用等富块（不承载正文）。若其中出现文本块，
    // 仅并入 **patch_type != 2（非「替换」语义）** 的块，避免与 patch_op 增量重复。
    if let Some(blocks) = outer
        .get("content")
        .and_then(|c| c.get("content_block"))
        .and_then(Value::as_array)
    {
        let mut content = String::new();
        for block in blocks {
            if block.get("patch_type").and_then(to_u64) == Some(2) {
                continue;
            }
            if let Some((c, _)) = block_delta(block) {
                if let Some(c) = c {
                    content.push_str(&c);
                }
            }
        }
        if content.is_empty() {
            return None;
        }
        return Some(DoubaoEvent::Delta {
            content: Some(content),
            reasoning: None,
        });
    }

    None
}

/// OpenAI chat.completion.chunk 行
fn chunk_line(delta: Value) -> String {
    let chunk = json!({
        "choices": [{ "index": 0, "delta": delta, "finish_reason": null }],
    });
    format!("data: {chunk}")
}

/// 豆包行迭代器 → OpenAI SSE 行的迭代器
pub struct DoubaoTranslate<L: Iterator<Item = String>> {
    lines: L,
    risk_slot: RiskSlot,
    pending: VecDeque<String>,
    done: bool,
    /// ack 记录的会话 id（Done 时带出）
    conversation_id: String,
    emitted_content: bool,
    usage_in: Option<u64>,
    usage_out: Option<u64>,
    finished: bool,
}

impl<L: Iterator<Item = String>> DoubaoTranslate<L> {
    pub fn new(lines: L, risk_slot: RiskSlot) -> Self {
        Self {
            lines,
            risk_slot,
            pending: VecDeque::new(),
            done: false,
            conversation_id: String::new(),
            emitted_content: false,
            usage_in: None,
            usage_out: None,
            finished: false,
        }
    }

    fn record_risk(&self, code: RiskCode) {
        if let Ok(mut slot) = self.risk_slot.lock() {
            *slot = Some(code);
        }
    }

    /// 处理一条原始行 → 推入 pending
    fn feed(&mut self, raw: &str) {
        // usage 采集（宽口径，任意帧；本方言样本未见，保留兜底）
        if let Ok(v) = serde_json::from_str::<Value>(
            raw.trim().strip_prefix("data:").unwrap_or(raw).trim(),
        ) {
            if let Some(n) = find_num(&v, "input_tokens").or_else(|| find_num(&v, "prompt_tokens")) {
                self.usage_in = Some(n);
            }
            if let Some(n) = find_num(&v, "output_tokens").or_else(|| find_num(&v, "completion_tokens"))
            {
                self.usage_out = Some(n);
            }
        }
        match parse_line(raw) {
            Some(DoubaoEvent::Delta { content, reasoning }) => {
                if let Some(c) = content {
                    if !c.is_empty() {
                        self.emitted_content = true;
                        self.pending.push_back(chunk_line(json!({ "content": c })));
                    }
                }
                if let Some(r) = reasoning {
                    if !r.is_empty() {
                        self.pending.push_back(chunk_line(json!({ "reasoning_content": r })));
                    }
                }
            }
            Some(DoubaoEvent::Ack { conversation_id }) => {
                self.conversation_id = conversation_id;
            }
            Some(DoubaoEvent::Risk(code)) => {
                self.record_risk(code);
                let err = json!({"error": {"code": code.code(), "message": format!("豆包风控：{}", code.code()), "type": "upstream_error"}});
                self.pending.push_back(format!("data: {err}"));
            }
            Some(DoubaoEvent::Error { code, msg }) => {
                let err = json!({"error": {"code": code, "message": msg, "type": "upstream_error"}});
                self.pending.push_back(format!("data: {err}"));
            }
            Some(DoubaoEvent::Done { .. }) => self.finish(),
            None => {}
        }
    }

    fn finish(&mut self) {
        if self.finished {
            return;
        }
        self.finished = true;
        let mut tail = json!({
            "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }],
        });
        if let (Some(i), Some(o)) = (self.usage_in, self.usage_out) {
            tail["usage"] = json!({ "input_tokens": i, "output_tokens": o });
        }
        self.pending.push_back(format!("data: {tail}"));
        self.pending.push_back("data: [DONE]".to_string());
    }
}

impl<L: Iterator<Item = String>> Iterator for DoubaoTranslate<L> {
    type Item = String;
    fn next(&mut self) -> Option<String> {
        loop {
            if let Some(line) = self.pending.pop_front() {
                return Some(line);
            }
            if self.done {
                return None;
            }
            match self.lines.next() {
                Some(raw) => self.feed(&raw),
                None => {
                    self.done = true;
                    self.finish();
                }
            }
        }
    }
}

/// 豆包行迭代器 → OpenAI SSE 行迭代器（供 `InterruptibleLines::from_iterator` 包装）
pub fn translate<L: Iterator<Item = String> + Send + 'static>(
    lines: L,
    risk_slot: RiskSlot,
) -> Box<dyn Iterator<Item = String> + Send> {
    Box::new(DoubaoTranslate::new(lines, risk_slot))
}

/// 非流式聚合：豆包行 → 完整 OpenAI `chat.completion`
/// 复用 `wb_sse::aggregate`（其吃标准 OpenAI 行）
pub fn aggregate<L: Iterator<Item = String>>(
    lines: L,
    chat_id: &str,
    risk_slot: RiskSlot,
) -> (Option<Value>, Option<(i64, String)>, Option<RiskCode>) {
    let translated = DoubaoTranslate::new(lines, risk_slot.clone());
    let (completion, err) = super::wb_sse::aggregate(translated, chat_id);
    let risk = risk_slot.lock().ok().and_then(|g| *g);
    (completion, err, risk)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot() -> RiskSlot {
        Arc::new(Mutex::new(None))
    }

    /// 合成 patch_op 帧（真机形态：patch_object=1，block_type=10000，增量文本）
    fn patch_frame(text: &str) -> String {
        let frame = json!({
            "message_id": "00000000000000000",
            "patch_op": [{
                "patch_object": PATCH_OBJECT_CONTENT,
                "patch_type": 1,
                "patch_value": {
                    "content_block": [{
                        "block_type": BLOCK_TYPE_TEXT,
                        "block_id": "00000000000000000",
                        "content": {"text_block": {"text": text}},
                        "is_finish": false,
                        "patch_type": 1
                    }]
                }
            }]
        });
        format!("data: {frame}")
    }

    /// 正文增量：patch_op 帧投影为 content
    #[test]
    fn parse_patch_op_text_delta() {
        match parse_line(&patch_frame("你好")) {
            Some(DoubaoEvent::Delta { content, reasoning }) => {
                assert_eq!(content.as_deref(), Some("你好"));
                assert!(reasoning.is_none());
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    /// 思考块（block_type=2008）→ reasoning；tts_content 补丁（patch_object=111）被忽略
    #[test]
    fn parse_reasoning_and_ignore_tts() {
        let reasoning_frame = json!({
            "message_id": "0",
            "patch_op": [{
                "patch_object": PATCH_OBJECT_CONTENT,
                "patch_type": 1,
                "patch_value": {"content_block": [{
                    "block_type": BLOCK_TYPE_REASONING,
                    "content": {"text_block": {"text": "思考中"}}
                }]}
            }]
        });
        match parse_line(&format!("data: {reasoning_frame}")) {
            Some(DoubaoEvent::Delta { content, reasoning }) => {
                assert!(content.is_none());
                assert_eq!(reasoning.as_deref(), Some("思考中"));
            }
            other => panic!("unexpected: {other:?}"),
        }

        let tts_only = json!({
            "message_id": "0",
            "patch_op": [{"patch_object": 111, "patch_type": 1, "patch_value": {"tts_content": "语音"}}]
        });
        assert_eq!(
            parse_line(&format!("data: {tts_only}")),
            None,
            "tts_content 补丁不得当作正文"
        );
    }

    /// 用户提问回显（message 帧）不得当作回答
    #[test]
    fn ignores_user_echo_and_loading() {
        let echo = json!({
            "message": {"content": "{\"text_block\":{\"text\":\"用户的提问\"}}", "sender_id": "u"},
            "message_attr": {}
        });
        assert_eq!(parse_line(&format!("data: {echo}")), None, "提问回显不得输出");

        let loading = json!({
            "content": {"content_status": 101, "content_type": 9999,
                "content_block": [{"block_type": 10101, "content": {"loading_block": {"scene": 2}}}]},
            "meta": {}, "attr": {}
        });
        assert_eq!(parse_line(&format!("data: {loading}")), None, "loading 块不得输出");
    }

    /// ack / end_type 分派：ack 记录会话 id，end_type=3 为 Done，1/2 忽略
    #[test]
    fn parse_ack_and_end() {
        let ack = json!({"query_list": [], "ack_client_meta": {"conversation_id": "c9"}, "timeout_conf": {}});
        assert_eq!(
            parse_line(&format!("data: {ack}")),
            Some(DoubaoEvent::Ack { conversation_id: "c9".into() })
        );
        assert_eq!(parse_line("data: {\"end_type\":1,\"msg_finish_attr\":{}}"), None);
        assert_eq!(parse_line("data: {\"end_type\":2}"), None);
        assert_eq!(
            parse_line("data: {\"end_type\":3}"),
            Some(DoubaoEvent::Done { conversation_id: String::new() })
        );
        assert_eq!(parse_line("data: {}"), None, "空心跳不得输出");
    }

    /// 上游错误帧（真机 `event: STREAM_ERROR`）→ Error；风控码仍归 Risk
    #[test]
    fn parse_stream_error_frame() {
        assert_eq!(
            parse_line(
                "data: {\"error_code\":710020202,\"error_msg\":\"common invalid param\",\"extra\":{\"is_replay\":\"false\"}}"
            ),
            Some(DoubaoEvent::Error {
                code: 710020202,
                msg: "common invalid param".into()
            })
        );
        // 风控码（顶层文本即命中）→ Risk，不得落到 Error
        assert_eq!(
            parse_line("data: {\"error_code\":710022004,\"error_msg\":\"need captcha\"}"),
            Some(DoubaoEvent::Risk(RiskCode::Captcha))
        );
    }

    /// 风控码（任意文本层）
    #[test]
    fn parse_risk_code() {
        assert_eq!(
            parse_line("data: {\"msg\":\"code 710022004\"}"),
            Some(DoubaoEvent::Risk(RiskCode::Captcha))
        );
        assert_eq!(
            parse_line("data: {\"msg\":\"710012001\"}"),
            Some(DoubaoEvent::Risk(RiskCode::Revoked))
        );
    }

    /// 整条流：ack → 3 段增量 → end_type=3 → finish + [DONE]
    #[test]
    fn translate_projects_openai_lines_and_done() {
        let lines = vec![
            format!("data: {}", json!({"query_list": [], "ack_client_meta": {"conversation_id": "c1"}})),
            patch_frame("你"),
            patch_frame("好"),
            "data: {\"end_type\":1,\"msg_finish_attr\":{\"brief\":\"摘要\"}}".to_string(),
            "data: {\"end_type\":3}".to_string(),
        ];
        let out: Vec<String> = translate(lines.into_iter(), slot()).collect();
        let joined = out.join("\n");
        assert!(joined.contains("\"content\":\"你\""), "首段增量应投影");
        assert!(joined.contains("\"content\":\"好\""), "次段增量应投影");
        assert!(!joined.contains("摘要"), "brief 摘要不得混入正文");
        assert!(out.iter().any(|s| s.contains("\"finish_reason\":\"stop\"")));
        assert_eq!(out.last().unwrap(), "data: [DONE]");
    }

    /// EOF 兜底：无 end_type 时流结束仍补 finish
    #[test]
    fn eof_fallback_emits_finish() {
        let out: Vec<String> = translate(vec![patch_frame("hi")].into_iter(), slot()).collect();
        assert_eq!(out.last().unwrap(), "data: [DONE]");
        assert!(out.iter().any(|s| s.contains("\"finish_reason\":\"stop\"")));
    }
}
