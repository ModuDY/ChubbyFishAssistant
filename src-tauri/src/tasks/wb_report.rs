//! WorkBuddy 成长任务判据事件上报（P2 第一批：纯事件型任务）。
//!
//! 背景：官方成长任务按「行为事件」计分，不同任务认不同客户端指纹——
//! - 桌面包（RichMeow / Buddy_App / automation / template / playbook / canvas）：
//!   `copilot.tencent.com/v2/report` + `extName=workbuddy-desktop` 事件族；
//! - Web 包（Library_read 等页面行为）：`www.workbuddy.cn/v2/report` +
//!   `x-client-platform: web` + 浏览器形状字段；
//! - 对话包（chat_5 / 领养前置）：`{billing}/v2/report` 的 `chat_request_send`。
//!
//! 事件链形状来自参考实现实测样本（三账号点亮验证），本模块只做「构造 + 上报」，
//! 是否计分由上游决定：调用方应在上报后重新拉任务进度再领奖。

use serde_json::{json, Map, Value};

use super::wb_common::{self, Creds};

/// 当前 Unix 毫秒（事件时间戳口径）
fn now_ms() -> i64 {
    chrono::Local::now().timestamp_millis()
}

/// 桌面端实测 UA（5.5.6 内嵌 CLI 2.137.1）
const DESKTOP_UA: &str = "WorkBuddy/5.5.6 WorkBuddy/5.5.6 CLI/2.137.1";
/// Web 端浏览器 UA（页面行为类事件形状）
const WEB_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/152.0.0.0 Safari/537.36";
/// 桌面事件上报路径（chat 域）
const REPORT_PATH: &str = "/v2/report";

/// 由 uid 稳定派生 36 位 hex 设备标识（machineId / sessionId 复用）。
/// 幂等：同账号恒相同，模拟固定设备；不使用随机值，避免每次上报换设备。
pub fn derive_id(uid: &str, salt: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(format!("{salt}:{uid}").as_bytes());
    let d = h.finalize();
    d[..18].iter().map(|b| format!("{b:02x}")).collect()
}

fn mk(event_code: &str, extra: Value) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("eventCode".into(), json!(event_code));
    if let Some(o) = extra.as_object() {
        for (k, v) in o {
            m.insert(k.clone(), v.clone());
        }
    }
    m
}

/// 桌面端公共指纹（每个事件注入；业务字段同名时覆盖）
fn desktop_fingerprint(creds: &Creds) -> Map<String, Value> {
    let now = now_ms();
    let mut m = Map::new();
    m.insert("timezone".into(), json!("Asia/Shanghai"));
    m.insert("reportDelay".into(), json!(2000));
    m.insert("userId".into(), json!(creds.uid));
    m.insert("username".into(), json!(creds.nickname));
    m.insert("userNickname".into(), json!(creds.nickname));
    m.insert("product".into(), json!("SaaS"));
    m.insert("releaseDate".into(), json!(1789036585355i64));
    m.insert("commit".into(), json!("5f9692923c93033111c51ad7b003eb80204a9b75"));
    m.insert("ideName".into(), json!("WorkBuddy"));
    m.insert("ideType".into(), json!("WorkBuddy"));
    m.insert("ideVersion".into(), json!("5.5.6"));
    m.insert("machineId".into(), json!(derive_id(&creds.uid, "machine")));
    m.insert("sessionId".into(), json!(derive_id(&creds.uid, "session")));
    m.insert("extName".into(), json!("workbuddy-desktop"));
    m.insert("extVersion".into(), json!("5.5.6"));
    m.insert("os".into(), json!("win32"));
    m.insert("arch".into(), json!("x64"));
    m.insert("osVersion".into(), json!("10.0.26220"));
    m.insert("cpuCores".into(), json!(20));
    m.insert("memorySize".into(), json!(24));
    m.insert("timestamp".into(), json!(now));
    m.insert("presentAt".into(), json!(now));
    m
}

/// 以桌面指纹批量上报（事件公共指纹由本函数注入，业务字段优先）
pub fn report_desktop(
    agent: &ureq::Agent,
    creds: &Creds,
    events: Vec<Map<String, Value>>,
) -> Result<u16, String> {
    if events.is_empty() {
        return Err("no events".into());
    }
    let fp = desktop_fingerprint(creds);
    let arr: Vec<Value> = events
        .into_iter()
        .map(|ev| {
            let mut m = fp.clone();
            for (k, v) in ev {
                m.insert(k, v);
            }
            Value::Object(m)
        })
        .collect();
    let base = wb_common::region_chat_base(&creds.domain);
    let url = format!("{base}{REPORT_PATH}");
    let headers = vec![
        ("Authorization".to_string(), format!("Bearer {}", creds.access_token)),
        ("Accept".to_string(), "application/json, text/plain, */*".to_string()),
        ("Content-Type".to_string(), "application/json;charset=UTF-8".to_string()),
        ("User-Agent".to_string(), DESKTOP_UA.to_string()),
        ("X-Domain".to_string(), base.to_string()),
        ("X-Product".to_string(), "SaaS".to_string()),
        (
            "X-Request-ID".to_string(),
            format!("{}{}", derive_id(&creds.uid, "req"), now_ms()),
        ),
        ("X-User-Id".to_string(), creds.uid.clone()),
    ];
    let (st, _b, raw) = wb_common::post_json_raw(agent, &url, &headers, &json!(arr));
    if !(200..=201).contains(&st) {
        return Err(format!("HTTP {st} {}", raw.chars().take(80).collect::<String>()));
    }
    Ok(st)
}

/// 以 Web 指纹上报单条页面事件（Library_read 类）
pub fn report_web_event(
    agent: &ureq::Agent,
    creds: &Creds,
    event_code: &str,
    page_url: &str,
    element_id: &str,
    element_name: &str,
) -> Result<u16, String> {
    let web = wb_common::region_web_base(&creds.domain);
    let now = now_ms();
    let ev = json!([{
        "eventCode": event_code,
        "timestamp": now,
        "reportDelay": 0,
        "pageURL": page_url,
        "elementId": element_id,
        "elementName": element_name,
        "os": "Win32",
        "arch": "",
        "osVersion": "10.0",
        "userAgent": WEB_UA,
        "machineId": derive_id(&creds.uid, "webmachine"),
        "userId": creds.uid,
        "userNickname": creds.nickname,
        "enterpriseId": "",
    }]);
    let mut headers = wb_common::web_flavor(&[], web);
    headers.push(("User-Agent".to_string(), WEB_UA.to_string()));
    headers.push(("Referer".to_string(), page_url.to_string()));
    headers.push(("Authorization".to_string(), format!("Bearer {}", creds.access_token)));
    let url = format!("{web}{REPORT_PATH}");
    let (st, _b, raw) = wb_common::post_json_raw(agent, &url, &headers, &ev);
    if !(200..=201).contains(&st) {
        return Err(format!("HTTP {st} {}", raw.chars().take(80).collect::<String>()));
    }
    Ok(st)
}

/// 一次「桌面端成功对话」的完整事件链（点亮 RichMeow_Chat 等）
/// agent_task_created → chat_message_send → chat_request_send →
/// chat_message_response(isSuccessful) → chat_message_status → chat_request_response
pub fn chat_sequence(conv: &str, req: &str, msg: &str, model_id: &str, model_name: &str) -> Vec<Map<String, Value>> {
    let now = now_ms();
    vec![
        mk(
            "agent_task_created",
            json!({
                "source": "LOCAL", "name": "working", "task_target": "local", "mode": "craft",
                "requestModelId": model_id, "requestModelName": model_name,
                "has_repo": false, "repo_type": "none", "workspace_type": "empty",
                "has_connector": false, "connector_types": [],
                "has_mention": false, "mention_types": [],
                "has_template": false, "action": "", "template_name": "",
                "has_expert": false, "expert_id": "", "expert_name": "", "expert_industry_id": "",
                "has_skill": false, "skill_names": [],
                "conversationId": conv, "messageId": msg, "buddyId": "", "buddyName": "",
            }),
        ),
        mk(
            "chat_message_send",
            json!({
                "messageId": format!("{msg}-assistant"), "historyCount": 0,
                "isContextTruncated": false, "currentStepCount": 1,
                "traceId": req, "rootRequestId": req, "parentConversationId": conv,
                "agentName": "cli", "agentType": "main",
            }),
        ),
        mk(
            "chat_request_send",
            json!({
                "inputLength": 24, "isPlan": false, "isAutoExecuteTerminal": false,
                "isAutoModify": false, "codebaseEnable": false, "maxToken": 0,
                "maxSteps": 500, "temperature": 0, "maxRetries": 0,
                "mentionContexts": [], "knowledgeId": [], "knowledgeName": [],
                "codebaseId": "", "mentionContextCount": 0, "command": "",
                "recommendId": "", "skillId": "", "skillCount": 0, "totalCount": 0,
                "traceId": req, "rootRequestId": req, "parentConversationId": conv,
                "agentName": "cli", "agentType": "main",
                "codebuddy.session_id": conv, "codebuddy.conversation_request_id": req,
            }),
        ),
        mk(
            "chat_message_response",
            json!({
                "messageId": format!("{msg}-assistant"), "responseModelId": model_id,
                "inputToken": 120, "outputToken": 80, "totalToken": 200,
                "cachedTokens": 0, "cachedWriteTokens": 0, "cachedMissTokens": 0,
                "isSuccessful": true, "messageErrorCode": "", "finishReason": "stop",
                "firstTokenAt": now, "traceId": req, "conversationId": conv,
                "rootRequestId": req, "parentConversationId": conv,
                "agentName": "cli", "agentType": "main",
                "codebuddy.session_id": conv, "codebuddy.conversation_request_id": req,
            }),
        ),
        mk(
            "chat_message_status",
            json!({
                "messageId": format!("{msg}-assistant"), "messageErrorCode": "0",
                "traceId": req, "rootRequestId": req, "parentConversationId": conv,
                "agentName": "cli", "agentType": "main",
            }),
        ),
        mk(
            "chat_request_response",
            json!({
                "mode": "craft", "toolCallCount": 0,
                "inputToken": 120, "outputToken": 80, "totalToken": 200,
                "cachedTokens": 0, "cachedWriteTokens": 0, "cachedMissTokens": 0,
                "isSuccessful": true, "messageErrorCode": "", "finishReason": "stop",
                "rootRequestId": req, "parentConversationId": conv,
            }),
        ),
    ]
}

/// 「进入 Buddy 应用」五连事件（同时满足 Buddy_App 与 Buddy_App_QQ）
pub fn buddy_app_sequence(buddy_id: &str, buddy_name: &str) -> Vec<Map<String, Value>> {
    let mk_b = |code: &str, extra: Value| {
        let mut base = json!({
            "mode": "LOCAL", "buddyId": buddy_id, "buddyName": buddy_name,
        });
        if let (Some(dst), Some(src)) = (base.as_object_mut(), extra.as_object()) {
            for (k, v) in src {
                dst.insert(k.clone(), v.clone());
            }
        }
        mk(code, base)
    };
    vec![
        mk_b("buddyapp_discover_click", json!({})),
        mk_b(
            "buddyapp_show",
            json!({"elementId": buddy_id, "elementName": buddy_name, "position": 2}),
        ),
        mk_b(
            "buddyapp_enter_click",
            json!({"elementId": buddy_id, "elementName": buddy_name, "position": 2, "isFirstPage": "1"}),
        ),
        mk_b(
            "buddyapp_auth_confirm_click",
            json!({"elementId": buddy_id, "elementName": buddy_name}),
        ),
        mk_b(
            "buddyapp_bindaccount_skip_click",
            json!({"elementId": buddy_id, "elementName": buddy_name}),
        ),
    ]
}

/// 「定时任务创建成功」事件（点亮 automation_1）
pub fn automation_create_event(name: &str) -> Map<String, Value> {
    mk(
        "automated_task_create_suc",
        json!({
            "name": name, "source": "manually", "modelId": "fast-model",
            "modelIsThinking": true, "connectorCount": 0, "skills": "", "skillCount": 0,
            "scheduleType": "once", "mode": "LOCAL",
        }),
    )
}

/// 「使用模板创建任务」事件组（一条完整对话链 + 模板两事件）
pub fn template_use_sequence(
    conv: &str,
    req: &str,
    template_id: &str,
    template_name: &str,
) -> Vec<Map<String, Value>> {
    let mut ev = chat_sequence(conv, req, &format!("msg-{template_id}"), "fast-model", "fast-model");
    ev.push(mk(
        "agent_task_created_with_template",
        json!({
            "mode": "working", "isCustomModel": false, "id": template_id,
            "name": template_name, "requestId": req,
        }),
    ));
    ev.push(mk(
        "template_used",
        json!({"template_id": template_id, "task_mode": "working"}),
    ));
    ev
}

/// 「灵感案例做同款」事件组（对话链 + 3 个 playbook 事件）
pub fn playbook_prompt_sequence(
    conv: &str,
    req: &str,
    case_id: &str,
    case_name: &str,
) -> Vec<Map<String, Value>> {
    let mut ev = chat_sequence(conv, req, "msg-pb", "fast-model", "fast-model");
    let payload = json!({
        "id": case_id, "name": case_name, "type": "document",
        "categoryId": "", "categoryName": "",
    });
    ev.push(mk(
        "web_element_click",
        json!({
            "pageName": "playbook_detail", "elementId": "playbook_ctaClick",
            "elementName": case_name, "source": "discover",
        }),
    ));
    let mut cta = json!({"source": "discover", "position": 0});
    if let (Some(dst), Some(src)) = (cta.as_object_mut(), payload.as_object()) {
        for (k, v) in src {
            dst.insert(k.clone(), v.clone());
        }
    }
    ev.push(mk("playbook_cta_click", cta));
    let mut send = json!({"conversationId": conv, "requestId": req});
    if let (Some(dst), Some(src)) = (send.as_object_mut(), payload.as_object()) {
        for (k, v) in src {
            dst.insert(k.clone(), v.clone());
        }
    }
    ev.push(mk("playbook_prompt_send", send));
    ev
}

/// 「设计创意画布」事件组（对话链 + 建画布/打开画布）
pub fn canvas_sequence(conv: &str, req: &str) -> Vec<Map<String, Value>> {
    let mut ev = chat_sequence(conv, req, "msg-canvas", "fast-model", "fast-model");
    let tail: String = req.chars().rev().take(8).collect::<Vec<_>>().into_iter().rev().collect();
    ev.push(mk(
        "wbx_design_canvas_task_create",
        json!({
            "conversationId": conv, "requestId": req, "source": "summon_keyword",
            "cost": 12000, "isSuccessful": true,
        }),
    ));
    ev.push(mk(
        "wbx_design_canvas_open",
        json!({
            "conversationId": conv, "requestId": req, "id": format!("ardot-file-{tail}"),
            "source": "summon_keyword", "type": "page", "cost": 13000, "isSuccessful": true,
        }),
    ));
    ev
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derive_id_is_stable_and_36_hex() {
        let a = derive_id("user-1", "machine");
        let b = derive_id("user-1", "machine");
        assert_eq!(a, b);
        assert_eq!(a.len(), 36);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, derive_id("user-1", "session"));
        assert_ne!(a, derive_id("user-2", "machine"));
    }

    #[test]
    fn chat_sequence_has_six_events_with_required_fields() {
        let ev = chat_sequence("conv", "req", "msg", "fast-model", "fast-model");
        assert_eq!(ev.len(), 6);
        assert_eq!(ev[0]["eventCode"], json!("agent_task_created"));
        assert_eq!(ev[2]["eventCode"], json!("chat_request_send"));
        assert_eq!(ev[3]["isSuccessful"], json!(true));
        assert_eq!(ev[3]["conversationId"], json!("conv"));
    }

    #[test]
    fn buddy_app_sequence_covers_five_steps() {
        let ev = buddy_app_sequence("cb_1", "企鹅教师助手");
        assert_eq!(ev.len(), 5);
        assert!(ev.iter().all(|e| e["buddyId"] == json!("cb_1")));
        assert_eq!(ev[0]["eventCode"], json!("buddyapp_discover_click"));
    }

    #[test]
    fn template_playbook_canvas_sequences_append_business_events() {
        assert_eq!(template_use_sequence("c", "r", "t1", "模板").len(), 8);
        assert_eq!(playbook_prompt_sequence("c", "r", "p1", "案例").len(), 9);
        let canvas = canvas_sequence("c", "r");
        assert_eq!(canvas.len(), 8);
        assert!(canvas.last().unwrap()["id"]
            .as_str()
            .unwrap()
            .starts_with("ardot-file-"));
    }

    #[test]
    fn automation_event_shape() {
        let ev = automation_create_event("测试任务");
        assert_eq!(ev["eventCode"], json!("automated_task_create_suc"));
        assert_eq!(ev["scheduleType"], json!("once"));
    }
}
