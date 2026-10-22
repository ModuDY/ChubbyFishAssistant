//! 豆包网关编排与账号级令牌桶（E-01 v1）。
//!
//! **简化镜像** `qoder_route`：不含慢请求对冲 / 排队重试 / 模型级冷却；
//! 豆包为单源，每轮以**网关侧消息合并**新建会话。v1 核心关注三件事：
//! ① 多账号轮换（单账号吞吐极低）；② 固定窗口令牌桶（实证 ~8 次/小时/账号）；
//! ③ 风控码处置（验证码墙 → 长冷却 + needs_captcha；吊销 → 禁用；指纹 → 换号）。
//!
//! 下游协议投影完全复用 `wb_sse`：本模块把豆包 SSE 经 `doubao_sse::translate`
//! 投影为标准 OpenAI 行，再交 `wb_sse::{stream_forward_ex, aggregate}`，一次性
//! 拿到 OpenAI / text / Anthropic / Responses 四协议投影 + keep-alive + 断连检测。

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use axum::body::Body;
use axum::http::StatusCode;
use axum::response::Response;
use serde_json::{json, Value};
use tokio_stream::wrappers::ReceiverStream;

use super::doubao_sse::RiskSlot;
use super::doubao_upstream::{self, RiskCode};
use super::wb_sse;
use super::wb_upstream::InterruptibleLines;
use super::{ApiSharedState, ErrKind, InflightGuard};
use crate::api_server::routes::{anthropic_error, openai_error, stream_msg_or_fallback, Protocol};

/// 单账号固定窗口配额（实证 ~8 次/小时/账号）
const HOURLY_LIMIT: u32 = 8;
/// 固定窗口长度（秒）
const WINDOW_SECS: i64 = 3600;
/// 验证码墙长冷却（秒）：命中即视为需人工过验证码
const CAPTCHA_COOLDOWN_SECS: i64 = 6 * 3600;

fn now_ts() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn safe_lock<'a, T>(m: &'a Mutex<T>) -> std::sync::MutexGuard<'a, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// 错误文本截断脱敏（同 HTTP 分支口径；不越 UTF-8 边界）
fn safe_slice(s: &str, n: usize) -> &str {
    if s.len() <= n {
        return s;
    }
    let mut end = n;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

fn log_path(proto: Protocol) -> &'static str {
    match proto {
        Protocol::OpenAi => "/v1/chat/completions",
        Protocol::OpenAiText => "/v1/completions",
        Protocol::Anthropic => "/v1/messages",
        Protocol::Responses => "/v1/responses",
    }
}

/// HTTP 状态码 → 账号错误分类（v1 简化，无同号退避）
fn classify_http(status: u16) -> ErrKind {
    match status {
        401 => ErrKind::SessionDead,
        403 => ErrKind::Forbidden,
        429 => ErrKind::SoftRate,
        s if s >= 500 => ErrKind::Server,
        _ => ErrKind::Client,
    }
}

// ==================== 账号级令牌桶（固定窗口 8 次/小时） ====================

#[derive(Default)]
struct DoubaoAcct {
    /// 当前窗口起点（Unix 秒）
    window_start: i64,
    /// 窗口内已用次数
    hits: u32,
    /// 冷却截止（验证码长冷却；0 = 无）
    until: i64,
    /// 是否需人工过验证码（`/status` 可见）
    needs_captcha: bool,
}

/// 供 `/status` 展示的单账号配额快照
#[derive(Clone)]
pub struct DoubaoQuotaInfo {
    pub uid: String,
    pub used: u32,
    pub limit: u32,
    /// 冷却截止（Unix 秒；0 = 未冷却）
    pub until: i64,
    pub needs_captcha: bool,
}

/// 豆包池状态合并视图（`/status` 展示）：账号运行时状态 + 小时配额 + 验证码标记。
/// `needs_captcha` 经 `DoubaoQuota::snapshot()` 透出（池自身不保存该标记）
#[derive(Clone, serde::Serialize)]
pub struct DoubaoPoolStatus {
    pub uid: String,
    pub name: String,
    pub cooling: bool,
    pub cooldown_until: Option<i64>,
    pub cooldown_reason: Option<String>,
    pub disabled: bool,
    pub state: String,
    pub inflight: u32,
    pub hourly_used: u32,
    pub hourly_limit: u32,
    /// 配额冷却截止（Unix 秒；0 = 无）
    pub quota_until: i64,
    pub needs_captcha: bool,
}

impl DoubaoPoolStatus {
    /// 合并池运行时状态与配额快照（无配额记录的账号按未使用/未冷却默认值）
    pub fn merge(pool: &[crate::models::PoolStatus], quota: &[DoubaoQuotaInfo]) -> Vec<Self> {
        let qmap: HashMap<&str, &DoubaoQuotaInfo> =
            quota.iter().map(|q| (q.uid.as_str(), q)).collect();
        pool.iter()
            .map(|p| {
                let q = qmap.get(p.uid.as_str());
                Self {
                    uid: p.uid.clone(),
                    name: p.name.clone(),
                    cooling: p.cooling,
                    cooldown_until: p.cooldown_until,
                    cooldown_reason: p.cooldown_reason.clone(),
                    disabled: p.disabled,
                    state: p.state.clone(),
                    inflight: p.inflight,
                    hourly_used: q.map_or(0, |q| q.used),
                    hourly_limit: q.map_or(HOURLY_LIMIT, |q| q.limit),
                    quota_until: q.map_or(0, |q| q.until),
                    needs_captcha: q.map_or(false, |q| q.needs_captcha),
                }
            })
            .collect()
    }
}

/// 豆包账号级令牌桶（固定窗口 + 验证码长冷却）
pub struct DoubaoQuota {
    inner: Mutex<HashMap<String, DoubaoAcct>>,
}

impl DoubaoQuota {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
        }
    }

    /// 尝试占用一次额度：冷却中 / 窗口已满 → false；成功计一次
    pub fn try_acquire(&self, uid: &str) -> bool {
        let now = now_ts();
        let mut map = safe_lock(&self.inner);
        let acct = map.entry(uid.to_string()).or_default();
        if acct.until > now {
            return false;
        }
        if acct.window_start == 0 || now - acct.window_start >= WINDOW_SECS {
            acct.window_start = now;
            acct.hits = 0;
        }
        if acct.hits >= HOURLY_LIMIT {
            return false;
        }
        acct.hits += 1;
        true
    }

    /// 命中验证码墙：长冷却 + 标记需人工过验证码
    pub fn note_captcha(&self, uid: &str) {
        let now = now_ts();
        let mut map = safe_lock(&self.inner);
        let acct = map.entry(uid.to_string()).or_default();
        acct.until = now + CAPTCHA_COOLDOWN_SECS;
        acct.needs_captcha = true;
    }

    /// 快照（供 `/status` 展示 needs_captcha / 窗口用量）
    pub fn snapshot(&self) -> Vec<DoubaoQuotaInfo> {
        let map = safe_lock(&self.inner);
        map.iter()
            .map(|(uid, a)| DoubaoQuotaInfo {
                uid: uid.clone(),
                used: a.hits,
                limit: HOURLY_LIMIT,
                until: a.until,
                needs_captcha: a.needs_captcha,
            })
            .collect()
    }
}

impl Default for DoubaoQuota {
    fn default() -> Self {
        Self::new()
    }
}

/// 风控码处置：写入冷却/禁用（换号由主循环继续）
fn handle_risk(state: &Arc<ApiSharedState>, uid: &str, rc: RiskCode) {
    match rc {
        RiskCode::Captcha => {
            state.doubao_quota.note_captcha(uid);
            // 长冷却（12h），与 needs_captcha 标记共同生效
            state.doubao_pool.note_error(uid, ErrKind::PlanLimit);
        }
        RiskCode::Revoked => {
            // sessionid 吊销 → 禁用（需重新登录）
            state.doubao_pool.note_error(uid, ErrKind::Forbidden);
        }
        RiskCode::Fingerprint => {
            // v1 简化：换号（10min 冷却）
            state.doubao_pool.note_error(uid, ErrKind::Client);
        }
    }
}

// ==================== 流式入口 ====================

/// 豆包流式对话（routes.rs 各协议端点 TargetPool::Doubao 分支调用）
pub fn doubao_stream_chat(
    state: Arc<ApiSharedState>,
    body_vec: Vec<u8>,
    model: String,
    start_ts: Instant,
    proto: Protocol,
    key_id: String,
    guard: InflightGuard,
) -> Response {
    let (tx, rx) = tokio::sync::mpsc::channel(64);

    // 线程隔离：流任务迁入专用阻塞池（同 qoder/wb）
    super::stream_runtime().spawn_blocking(move || {
        let chat_id = match proto {
            Protocol::OpenAi => format!("chatcmpl-{}", now_ts()),
            Protocol::OpenAiText => format!("cmpl-{}", now_ts()),
            Protocol::Anthropic => format!("msg_{}", now_ts()),
            Protocol::Responses => format!("resp_{}", now_ts()),
        };

        // SSE keep-alive 15s（watch + DoneSignal；主任务结束/panic 展开置 done 放行流终结）
        let (done_tx, mut done_rx) = tokio::sync::watch::channel(false);
        let _done = super::routes::DoneSignal(done_tx);
        {
            let tx2 = tx.clone();
            tokio::spawn(async move {
                let mut tick = tokio::time::interval(std::time::Duration::from_secs(15));
                tick.tick().await; // 首个 tick 立即返回，跳过
                loop {
                    tokio::select! {
                        _ = tick.tick() => {
                            if tx2
                                .send(Ok(bytes::Bytes::from(": keep-alive\n\n")))
                                .await
                                .is_err()
                            {
                                break;
                            }
                        }
                        _ = done_rx.changed() => break,
                    }
                }
            });
        }

        run_doubao_stream(&state, &body_vec, &model, proto, &key_id, &chat_id, &tx, start_ts, guard);
    });

    let stream = ReceiverStream::new(rx);
    Response::builder()
        .header("content-type", "text/event-stream")
        .header("cache-control", "no-cache")
        .header("connection", "keep-alive")
        // 显式禁用中间层缓冲（nginx 默认缓冲 SSE 会切碎流）
        .header("x-accel-buffering", "no")
        .body(Body::from_stream(stream))
        .unwrap_or_else(|_| internal_error_response())
}

#[allow(clippy::too_many_arguments)]
fn run_doubao_stream(
    state: &Arc<ApiSharedState>,
    body_vec: &[u8],
    model: &str,
    proto: Protocol,
    key_id: &str,
    chat_id: &str,
    tx: &tokio::sync::mpsc::Sender<Result<bytes::Bytes, std::io::Error>>,
    start_ts: Instant,
    mut guard: InflightGuard,
) {
    let key_name = super::api_keys::key_name_for(&state.data_dir, key_id);
    let path = log_path(proto);
    let peek: Value = serde_json::from_slice(body_vec).unwrap_or(json!({}));

    // ① 目录条目校验（dispatch 已短路，此处为防御性兜底）
    let Some(m) = super::doubao_payload::lookup(model) else {
        let msg = format!("model {model} not in doubao catalog");
        state.logger.log_request(
            "doubao", "POST", path, model, true, 404, "-",
            start_ts.elapsed().as_millis() as u64, &key_name, "", Some(&msg),
        );
        send_stream_error(tx, proto, 404, &msg);
        return;
    };
    if !m.verified {
        let msg = format!("model {model} 未启用（待验证）");
        state.logger.log_request(
            "doubao", "POST", path, model, true, 400, "-",
            start_ts.elapsed().as_millis() as u64, &key_name, "", Some(&msg),
        );
        send_stream_error(tx, proto, 400, &msg);
        return;
    }

    // ①⁺ 签名材料（安装级）前置校验：缺失（未抓包/无 a_bogus 模板）即明确报错、不进入
    // 账号轮换——否则每个账号都会被误判为上游故障而白白冷却
    if let Err(e) = super::doubao_upstream::ensure_signer(&state.data_dir) {
        let msg = format!("豆包签名材料不可用：{e}（先经代理发一次真机消息以生成采集文件）");
        *safe_lock(&state.last_error) = Some(msg.clone());
        state.logger.log_request(
            "doubao", "POST", path, model, true, 500, "-",
            start_ts.elapsed().as_millis() as u64, &key_name, "", Some(&msg),
        );
        send_stream_error(tx, proto, 500, &msg);
        return;
    }

    // ② F-35 子 Key 约束（doubao 池作用域）
    let (allowed_set, dedicated) = super::api_keys::constraints_for(&state.data_dir, key_id)
        .and_then(|k| k.pool_constraints("doubao"))
        .map_or((None, None), |c| (c.allowed, c.dedicated));

    let mut tried: HashSet<String> = HashSet::new();
    let mut empty_hits: usize = 0;

    loop {
        // 客户端断连检测：通道关闭即终止轮换（补记失败用量，防统计缺口）
        if tx.is_closed() {
            state.record_usage_doubao_abort(
                model, "none", key_id, true,
                start_ts.elapsed().as_millis() as u64,
            );
            return;
        }

        // ③ 取号：按 Key 约束 + 调度策略
        let picked = match state
            .doubao_pool
            .pick_excluding_constrained_ev(&tried, allowed_set.as_ref(), dedicated.as_deref())
        {
            Some((p, ev)) => {
                if let Some(ev) = ev {
                    state.logger.log_sched_event(&ev);
                }
                p
            }
            None => {
                let duration_ms = start_ts.elapsed().as_millis() as u64;
                state.record_usage_doubao(model, "none", key_id, false, true, duration_ms, 0, 0, None);
                let (healthy_total, _) = state.doubao_pool.selectable_stats_in(None);
                let (exhaust_msg, _tag) =
                    super::routes::exhaust_message(empty_hits, tried.len(), healthy_total);
                state.logger.log_request(
                    "doubao", "POST", path, model, true, 503, "none", duration_ms,
                    &key_name, "", Some(&exhaust_msg),
                );
                let err_body = json!({
                    "error": {"message": exhaust_msg, "type": "api_error", "code": "no_healthy_account"}
                });
                let _ = tx.blocking_send(Ok(bytes::Bytes::from(format!("data: {err_body}\n\n"))));
                let _ = tx.blocking_send(Ok(bytes::Bytes::from("data: [DONE]\n\n")));
                return;
            }
        };
        tried.insert(picked.uid.clone());

        // ④ F-77 账号级在途计数绑定（换号自动解绑旧账号；CAS 复核并发上限）
        let bound = if picked.over_limit {
            guard = guard.bind_account(state.doubao_pool.inflight_handle(&picked.uid));
            true
        } else {
            guard.bind_account_cas(
                state.doubao_pool.inflight_handle(&picked.uid),
                state.doubao_pool.concurrency_limit(),
            )
        };
        if !bound {
            continue;
        }
        *safe_lock(&state.active_uid) = Some(picked.uid.clone());

        // ⑤ 令牌桶：超限换号（不冷却——限速是账号常态而非故障）
        if !state.doubao_quota.try_acquire(&picked.uid) {
            state.logger.log_request(
                "doubao", "POST", path, model, true, 429, &picked.uid,
                start_ts.elapsed().as_millis() as u64, &key_name,
                &state.doubao_pool.name_of(&picked.uid),
                Some("账号小时配额已用尽，换号"),
            );
            continue;
        }

        // ⑥ 凭证解析（回调缺失 → 换号不冷却；报错 → 禁用后换号）
        let creds: doubao_upstream::DoubaoCreds = match state.doubao_identity.as_ref() {
            None => {
                state.logger.log_request(
                    "doubao", "POST", path, model, true, 503, &picked.uid,
                    start_ts.elapsed().as_millis() as u64, &key_name,
                    &state.doubao_pool.name_of(&picked.uid), Some("doubao identity 回调未注入"),
                );
                continue;
            }
            Some(resolve) => match resolve(&picked.uid) {
                Ok(c) => c,
                Err(e) => {
                    state.doubao_pool.note_error(&picked.uid, ErrKind::SessionDead);
                    *safe_lock(&state.last_error) = Some(format!(
                        "doubao identity uid={} err={}",
                        picked.uid,
                        safe_slice(&e, 200)
                    ));
                    state.logger.log_request(
                        "doubao", "POST", path, model, true, 503, &picked.uid,
                        start_ts.elapsed().as_millis() as u64, &key_name,
                        &state.doubao_pool.name_of(&picked.uid), Some("凭证解析失败"),
                    );
                    continue;
                }
            },
        };

        // ⑦ 请求体 + 发请求（每轮新建会话；多轮由网关侧消息合并承载）
        let body = super::doubao_payload::build_body(&peek, m);
        let cookie = doubao_upstream::cookie_of(&creds);

        let ttfb_start = Instant::now();
        match doubao_upstream::make_doubao_request(&cookie, &body, &state.data_dir) {
            Ok(resp) => {
                // 首字超时 10s：超时视为上游故障 → 换号（同 qoder/wb 语义）
                let lines = match super::wb_upstream::lines_with_first_byte_timeout_interruptible(resp.reader) {
                    Ok(l) => l,
                    Err(()) => {
                        // 冷却仍按 Server 处理，观测归因单列 timeout
                        state.doubao_pool.note_error_as(
                            &picked.uid,
                            ErrKind::Server,
                            super::metrics::Outcome::Timeout,
                        );
                        break;
                    }
                };
                let ttfb_ms = ttfb_start.elapsed().as_millis() as u64;

                let risk_slot: RiskSlot = Arc::new(Mutex::new(None));
                let translated = super::doubao_sse::translate(lines, risk_slot.clone());
                let ilines = InterruptibleLines::from_iterator(translated);
                let (error_info, sent_any, failed_inline, usage) =
                    wb_sse::stream_forward_ex(ilines, tx, proto, chat_id, model, &HashSet::new());
                let duration_ms = start_ts.elapsed().as_millis() as u64;
                let risk = *safe_lock(&risk_slot);
                let (pt, ct) = usage.as_ref().map(wb_sse::u64_pair).unwrap_or((0, 0));

                if let Some(rc) = risk {
                    handle_risk(state, &picked.uid, rc);
                }

                match error_info {
                    Some((code, msg)) => {
                        if !sent_any {
                            // 流未开始：错误不下发，允许换号重试（中间态不记 usage）
                            if risk.is_none() {
                                state.doubao_pool.note_error(&picked.uid, classify_http(code as u16));
                                if super::is_empty_completion(code, &msg) {
                                    empty_hits += 1;
                                }
                            }
                            *safe_lock(&state.last_error) = Some(format!(
                                "doubao uid={} model={} code={} msg={}",
                                picked.uid, model, code, safe_slice(&msg, 200)
                            ));
                            break;
                        }
                        // 已有数据流出：就地收尾（部分成功按实际 usage 落账）
                        state.record_usage_doubao(
                            model, &picked.uid, key_id, false, true,
                            duration_ms, pt, ct, Some(ttfb_ms),
                        );
                        state.logger.log_request_ttfb(
                            "doubao", "POST", path, model, true, 200, &picked.uid,
                            duration_ms, Some(ttfb_ms), &key_name,
                            &state.doubao_pool.name_of(&picked.uid), Some(&msg),
                        );
                        return;
                    }
                    None => {
                        if failed_inline {
                            // 流内失败已就地透传客户端：不重试不记账冷却（同 wb/qoder）
                            state.logger.log_request_ttfb(
                                "doubao", "POST", path, model, true, 200, &picked.uid,
                                duration_ms, Some(ttfb_ms), &key_name,
                                &state.doubao_pool.name_of(&picked.uid),
                                Some("流内失败已透传客户端"),
                            );
                            return;
                        }
                        if risk.is_some() {
                            // 风控帧已收录但零内容：视同流未产出 → 换号
                            state.record_usage_doubao(
                                model, &picked.uid, key_id, false, true,
                                duration_ms, pt, ct, Some(ttfb_ms),
                            );
                            state.logger.log_request_ttfb(
                                "doubao", "POST", path, model, true, 200, &picked.uid,
                                duration_ms, Some(ttfb_ms), &key_name,
                                &state.doubao_pool.name_of(&picked.uid), Some("豆包风控"),
                            );
                            return;
                        }
                        if !sent_any {
                            // EOF 零完成（空回放/影子风控）：计数后换号
                            empty_hits += 1;
                            state.doubao_pool.note_error(&picked.uid, ErrKind::Server);
                            break;
                        }
                        // 成功终局
                        state.doubao_pool.note_success(&picked.uid);
                        state.record_usage_doubao(
                            model, &picked.uid, key_id, true, true,
                            duration_ms, pt, ct, Some(ttfb_ms),
                        );
                        state.logger.log_request_ttfb(
                            "doubao", "POST", path, model, true, 200, &picked.uid,
                            duration_ms, Some(ttfb_ms), &key_name,
                            &state.doubao_pool.name_of(&picked.uid), None,
                        );
                        return;
                    }
                }
            }
            Err((status, resp_body, _retry_after)) => {
                // HTTP 层错误：先识别风控码，其次按状态分类
                if let Some(rc) = doubao_upstream::detect_risk(status, &resp_body) {
                    handle_risk(state, &picked.uid, rc);
                } else {
                    let kind = classify_http(status);
                    state.doubao_pool.note_error(&picked.uid, kind);
                }
                *safe_lock(&state.last_error) = Some(format!(
                    "doubao uid={} model={} status={} body={}",
                    picked.uid, model, status, safe_slice(&resp_body, 200)
                ));
                state.logger.log_request(
                    "doubao", "POST", path, model, true, status, &picked.uid,
                    start_ts.elapsed().as_millis() as u64, &key_name,
                    &state.doubao_pool.name_of(&picked.uid),
                    Some(&format!("upstream status={status}")),
                );
                // Fatal（请求级错误）：不再换号，直接透传
                if status == 401 || status == 403 {
                    let msg = format!("upstream {} error: {}", status, safe_slice(&resp_body, 300));
                    state.record_usage_doubao(
                        model, &picked.uid, key_id, false, true,
                        start_ts.elapsed().as_millis() as u64, 0, 0, None,
                    );
                    send_stream_error(tx, proto, status as i64, &msg);
                    return;
                }
                break; // 换号
            }
        }
    }
}

// ==================== 非流式（上游只回 SSE → 本地聚合） ====================

/// 豆包非流式对话：聚合 + 协议投影（routes.rs 各协议端点调用）
#[allow(clippy::too_many_arguments)]
pub async fn doubao_aggregate_chat(
    state: Arc<ApiSharedState>,
    body_vec: Vec<u8>,
    model: String,
    stream: bool,
    start_ts: Instant,
    proto: Protocol,
    key_id: String,
    guard: InflightGuard,
) -> Response {
    let model_out = model.clone();
    let result = super::stream_runtime().spawn_blocking(move || {
        let mut guard = guard;
        let key_name = super::api_keys::key_name_for(&state.data_dir, &key_id);
        let path = log_path(proto);
        let chat_id = format!("chatcmpl-{}", now_ts());
        let peek: Value = serde_json::from_slice(&body_vec).unwrap_or(json!({}));

        let Some(m) = super::doubao_payload::lookup(&model) else {
            return Err(format!("upstream 404 error: model {model} not in doubao catalog"));
        };
        if !m.verified {
            return Err(format!("upstream 400 error: model {model} 未启用（待验证）"));
        }
        // ①⁺ 签名材料（安装级）前置校验：缺失即明确报错，不进入账号轮换（同流式口径）
        if let Err(e) = super::doubao_upstream::ensure_signer(&state.data_dir) {
            return Err(format!(
                "upstream 500 error: 豆包签名材料不可用：{e}（先经代理发一次真机消息以生成采集文件）"
            ));
        }

        let (allowed_set, dedicated) =
            super::api_keys::constraints_for(&state.data_dir, &key_id)
                .and_then(|k| k.pool_constraints("doubao"))
                .map_or((None, None), |c| (c.allowed, c.dedicated));

        let mut tried: HashSet<String> = HashSet::new();
        let mut empty_hits: usize = 0;

        loop {
            let picked = match state.doubao_pool.pick_excluding_constrained_ev(
                &tried,
                allowed_set.as_ref(),
                dedicated.as_deref(),
            ) {
                Some((p, ev)) => {
                    if let Some(ev) = ev {
                        state.logger.log_sched_event(&ev);
                    }
                    p
                }
                None => {
                    state.record_usage_doubao(
                        &model, "none", &key_id, false, stream,
                        start_ts.elapsed().as_millis() as u64, 0, 0, None,
                    );
                    let (healthy_total, _) = state.doubao_pool.selectable_stats_in(None);
                    let (exhaust_msg, _tag) =
                        super::routes::exhaust_message(empty_hits, tried.len(), healthy_total);
                    return Err(format!("upstream 503 error: {exhaust_msg}"));
                }
            };
            tried.insert(picked.uid.clone());

            let bound = if picked.over_limit {
                guard = guard.bind_account(state.doubao_pool.inflight_handle(&picked.uid));
                true
            } else {
                guard.bind_account_cas(
                    state.doubao_pool.inflight_handle(&picked.uid),
                    state.doubao_pool.concurrency_limit(),
                )
            };
            if !bound {
                continue;
            }

            if !state.doubao_quota.try_acquire(&picked.uid) {
                state.logger.log_request(
                    "doubao", "POST", path, &model, stream, 429, &picked.uid,
                    start_ts.elapsed().as_millis() as u64, &key_name,
                    &state.doubao_pool.name_of(&picked.uid),
                    Some("账号小时配额已用尽，换号"),
                );
                continue;
            }

            let creds: doubao_upstream::DoubaoCreds = match state.doubao_identity.as_ref() {
                None => continue,
                Some(resolve) => match resolve(&picked.uid) {
                    Ok(c) => c,
                    Err(e) => {
                        state.doubao_pool.note_error(&picked.uid, ErrKind::SessionDead);
                        *safe_lock(&state.last_error) = Some(format!(
                            "doubao identity uid={} err={}",
                            picked.uid,
                            safe_slice(&e, 200)
                        ));
                        continue;
                    }
                },
            };

            let body = super::doubao_payload::build_body(&peek, m);
            let cookie = doubao_upstream::cookie_of(&creds);

            let ttfb_start = Instant::now();
            match doubao_upstream::make_doubao_request(&cookie, &body, &state.data_dir) {
                Ok(resp) => {
                    let lines = match super::wb_upstream::lines_with_first_byte_timeout_interruptible(resp.reader) {
                        Ok(l) => l,
                        Err(()) => {
                            // 冷却仍按 Server 处理，观测归因单列 timeout
                            state.doubao_pool.note_error_as(
                                &picked.uid,
                                ErrKind::Server,
                                super::metrics::Outcome::Timeout,
                            );
                            continue;
                        }
                    };
                    let ttfb_ms = ttfb_start.elapsed().as_millis() as u64;
                    let risk_slot: RiskSlot = Arc::new(Mutex::new(None));
                    let (completion, err, risk) =
                        super::doubao_sse::aggregate(lines, &chat_id, risk_slot);
                    let duration_ms = start_ts.elapsed().as_millis() as u64;
                    let (pt, ct) = completion.as_ref().map(wb_sse::u64_pair).unwrap_or((0, 0));

                    if let Some(rc) = risk {
                        handle_risk(&state, &picked.uid, rc);
                    }

                    if let Some(r) = completion {
                        if let Some(rc) = risk {
                            state.record_usage_doubao(
                                &model, &picked.uid, &key_id, false, stream,
                                duration_ms, pt, ct, Some(ttfb_ms),
                            );
                            state.logger.log_request_ttfb(
                                "doubao", "POST", path, &model, stream, 200, &picked.uid,
                                duration_ms, Some(ttfb_ms), &key_name,
                                &state.doubao_pool.name_of(&picked.uid), Some("豆包风控"),
                            );
                            return Err(format!("upstream 200 error: 豆包风控 {}", rc.code()));
                        }
                        state.doubao_pool.note_success(&picked.uid);
                        state.record_usage_doubao(
                            &model, &picked.uid, &key_id, true, stream,
                            duration_ms, pt, ct, Some(ttfb_ms),
                        );
                        state.logger.log_request_ttfb(
                            "doubao", "POST", path, &model, stream, 200, &picked.uid,
                            duration_ms, Some(ttfb_ms), &key_name,
                            &state.doubao_pool.name_of(&picked.uid), None,
                        );
                        return Ok(r);
                    }

                    match err {
                        Some((code, msg)) => {
                            if risk.is_none() {
                                state.doubao_pool.note_error(&picked.uid, ErrKind::Server);
                            }
                            state.logger.log_request_ttfb(
                                "doubao", "POST", path, &model, stream, 200, &picked.uid,
                                duration_ms, Some(ttfb_ms), &key_name,
                                &state.doubao_pool.name_of(&picked.uid), Some(&msg),
                            );
                            *safe_lock(&state.last_error) = Some(format!(
                                "doubao uid={} model={} code={} msg={}",
                                picked.uid, model, code, safe_slice(&msg, 200)
                            ));
                            continue;
                        }
                        None => {
                            // EOF 零完成
                            empty_hits += 1;
                            state.doubao_pool.note_error(&picked.uid, ErrKind::Server);
                            state.logger.log_request(
                                "doubao", "POST", path, &model, stream, 502, &picked.uid,
                                start_ts.elapsed().as_millis() as u64, &key_name,
                                &state.doubao_pool.name_of(&picked.uid),
                                Some("empty response"),
                            );
                            continue;
                        }
                    }
                }
                Err((status, resp_body, _retry_after)) => {
                    if let Some(rc) = doubao_upstream::detect_risk(status, &resp_body) {
                        handle_risk(&state, &picked.uid, rc);
                    } else {
                        state.doubao_pool.note_error(&picked.uid, classify_http(status));
                    }
                    state.logger.log_request(
                        "doubao", "POST", path, &model, stream, status, &picked.uid,
                        start_ts.elapsed().as_millis() as u64, &key_name,
                        &state.doubao_pool.name_of(&picked.uid),
                        Some(&format!("upstream status={status}")),
                    );
                    *safe_lock(&state.last_error) = Some(format!(
                        "doubao uid={} model={} status={} body={}",
                        picked.uid, model, status, safe_slice(&resp_body, 200)
                    ));
                    // Fatal：请求级错误直接透传
                    if (400..500).contains(&status) && !matches!(status, 401 | 403 | 429) {
                        state.record_usage_doubao(
                            &model, &picked.uid, &key_id, false, stream,
                            start_ts.elapsed().as_millis() as u64, 0, 0, None,
                        );
                        return Err(format!(
                            "upstream {} error: {}",
                            status,
                            safe_slice(&resp_body, 300)
                        ));
                    }
                    continue; // 换号
                }
            }
        }
    })
    .await;

    match result {
        Ok(Ok(resp)) => {
            let body = match proto {
                Protocol::Anthropic => {
                    wb_sse::completion_to_anthropic(&resp, &format!("msg_{}", now_ts()), &model_out)
                }
                Protocol::OpenAiText => wb_sse::completion_to_text(&resp, &model_out),
                Protocol::Responses => super::wb_responses::completion_to_responses(
                    &resp,
                    &format!("resp_{}", now_ts()),
                    &model_out,
                    &HashSet::new(),
                ),
                Protocol::OpenAi => resp,
            };
            Response::builder()
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap_or_else(|_| internal_error_response())
        }
        Ok(Err(msg)) => {
            // Fatal 错误透传上游状态码（对齐 qoder/wb 聚合路径）
            let upstream_status = msg
                .strip_prefix("upstream ")
                .and_then(|rest| rest.split(' ').next())
                .and_then(|s| s.parse::<u16>().ok());
            let status = match upstream_status {
                Some(s) if (400..500).contains(&s) && !matches!(s, 401 | 403) => {
                    StatusCode::from_u16(s).unwrap_or(StatusCode::SERVICE_UNAVAILABLE)
                }
                _ => StatusCode::SERVICE_UNAVAILABLE,
            };
            match proto {
                Protocol::Anthropic => anthropic_error(status, "api_error", &msg),
                _ => openai_error(status, "upstream_error", &msg),
            }
        }
        Err(e) => openai_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            &format!("task join error: {}", e),
        ),
    }
}

// ==================== 小工具 ====================

/// 流内错误帧下发（形态同 qoder/wb；私有不可复用，故自抄）
fn send_stream_error(
    tx: &tokio::sync::mpsc::Sender<Result<bytes::Bytes, std::io::Error>>,
    proto: Protocol,
    code: i64,
    msg: &str,
) {
    let msg = stream_msg_or_fallback(msg, code);
    match proto {
        Protocol::Anthropic => {
            let err = json!({"type":"error","error":{"type":"api_error","message":msg}});
            let _ = tx.blocking_send(Ok(bytes::Bytes::from(format!(
                "event: error\ndata: {}\n\n",
                err
            ))));
        }
        Protocol::Responses => {
            let resp = json!({
                "id": format!("resp_{}", now_ts()),
                "object": "response",
                "status": "failed",
                "output": [],
                "error": {"code": code.to_string(), "message": msg},
            });
            let body = json!({"type": "response.failed", "response": resp});
            let _ = tx.blocking_send(Ok(bytes::Bytes::from(format!(
                "event: response.failed\ndata: {}\n\n",
                body
            ))));
        }
        _ => {
            let body = json!({"error": { "message": msg, "type": "api_error", "code": code }});
            let _ = tx.blocking_send(Ok(bytes::Bytes::from(format!("data: {}\n\n", body))));
            let _ = tx.blocking_send(Ok(bytes::Bytes::from("data: [DONE]\n\n")));
        }
    }
}

fn internal_error_response() -> Response {
    Response::builder()
        .status(StatusCode::INTERNAL_SERVER_ERROR)
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"error": {"message": "内部错误", "type": "internal_error"}}).to_string(),
        ))
        .unwrap_or_else(|_| Response::new(Body::empty()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quota_allows_eight_then_blocks() {
        let q = DoubaoQuota::new();
        for i in 0..HOURLY_LIMIT {
            assert!(q.try_acquire("u1"), "第 {} 次应放行", i + 1);
        }
        assert!(!q.try_acquire("u1"), "第 9 次应被拦");
        // 其他账号不受影响
        assert!(q.try_acquire("u2"));
    }

    #[test]
    fn captcha_marks_long_cooldown() {
        let q = DoubaoQuota::new();
        assert!(q.try_acquire("u1"));
        q.note_captcha("u1");
        // 冷却期内不再放行
        assert!(!q.try_acquire("u1"));
        let snap = q.snapshot();
        let info = snap.iter().find(|i| i.uid == "u1").unwrap();
        assert!(info.needs_captcha);
        assert!(info.until > now_ts());
        assert_eq!(info.limit, HOURLY_LIMIT);
    }

    #[test]
    fn quota_window_resets_after_hour() {
        let q = DoubaoQuota::new();
        {
            let mut map = safe_lock(&q.inner);
            let a = map.entry("u1".to_string()).or_default();
            a.window_start = now_ts() - WINDOW_SECS - 1;
            a.hits = HOURLY_LIMIT; // 旧窗口已用尽
        }
        // 新窗口开启 → 放行
        assert!(q.try_acquire("u1"));
    }

    #[test]
    fn classify_http_maps_expected_kinds() {
        assert_eq!(classify_http(401), ErrKind::SessionDead);
        assert_eq!(classify_http(403), ErrKind::Forbidden);
        assert_eq!(classify_http(429), ErrKind::SoftRate);
        assert_eq!(classify_http(500), ErrKind::Server);
        assert_eq!(classify_http(400), ErrKind::Client);
    }
}
