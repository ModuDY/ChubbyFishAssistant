//! WorkBuddy 一键签到（F-15/F-16/F-55）+ 成长中心（T2.5/F-17）+ token 惰性续期（F-09）。
//! 原 src-python/workbuddy_checkin.py 的 Rust 移植，逐函数对齐：
//! - 签到：checkin_status（新路径回退旧路径）→ checkin_do（域名双探测 + 已签容错 + 奖励宽容解析）
//!   → 余额差值兜底（F-17，仅差值>0 采信）→ 401 刷新一次重试（F-09，禁二次刷新）
//! - 成长：travel（status→claim→config→depart）/ lottery（chances→draw 循环上限 20）/
//!   tasks（has_reward 过滤 → accept）各步独立容错，401 同款刷新重试
//! - NDJSON 事件契约（wb-checkin-progress 管线，前端逐行 JSON.parse）：
//!   start {type,total[,mode]} / account {user_id,name,status,message[,reward],index} /
//!   growth {type:"growth",...,index} / done {type:"done"[,mode],ok,already,failed}
//! - 结果 90 天滚动存储；刷新成功回写账号池过期时间（调度/到期日历数据源）
//! 红线：全程零 token 输出（凭证不入日志/事件）。

use serde_json::{json, Map, Value};

use crate::fs_utils;
use crate::state::AppState;

use super::http_agent;
use super::wb_common::{self, Creds};
use super::wb_report;

/// 盲盒抽取循环上限（防接口异常时死循环；正常 balance 会归零）
const LOTTERY_MAX_DRAWS: usize = 20;

/// 签到轮次参数（对齐 python --uid/--skip-checked/--skip-expired/--lazy-hours）
#[derive(Clone, Default)]
pub struct CheckinOpts {
    pub uids: Vec<String>,
    /// 跳过今日已签到账号（本地记录优先命中直接跳过、不发上游预检；
    /// 未命中则走 `checkin_status` 上游预检兜底，见 `checked_today_local`）
    pub skip_checked: bool,
    pub skip_expired: bool,
    pub lazy_hours: i64,
}

impl CheckinOpts {
    /// 每日计划任务默认（--json-stream --skip-checked，lazy 24h）
    pub fn daily() -> Self {
        Self {
            skip_checked: true,
            lazy_hours: 24,
            ..Default::default()
        }
    }
}

/// 成长中心开关（对齐 python --growth-travel/--growth-lottery/--growth-tasks）
#[derive(Clone, Copy, Default)]
pub struct GrowthOpts {
    pub travel: bool,
    pub lottery: bool,
    pub tasks: bool,
}

/// 区域端点表（对齐 python _urls_for；base 不带尾斜杠）
struct Urls {
    base: &'static str,
    checkin_status: String,
    checkin_status_old: String,
    checkin_do: String,
    travel_status: String,
    travel_claim: String,
    travel_config: String,
    travel_depart: String,
    lottery_chances: String,
    lottery_draw: String,
    tasks: String,
    tasks_accept: String,
    energy: String,
    streak: String,
    /// 连登档位兑换（POST {tier, client_token}；未解锁返回 403）
    streak_redeem: String,
    /// 对话活跃上报（点亮连登 + 解锁领养等成长任务）
    report: String,
    /// 领养协议 / 领取第一只 Buddy（first_buddy 任务）
    buddy_agreement: String,
    buddy_first: String,
}

fn urls_for(base: &'static str) -> Urls {
    let b = base;
    let g = format!("{b}/v2/activity/growth");
    Urls {
        base,
        checkin_status: format!("{b}/v2/billing/meter/checkin-activity-status"),
        checkin_status_old: format!("{b}/v2/billing/meter/checkin-status"),
        checkin_do: format!("{b}/v2/billing/meter/daily-checkin"),
        travel_status: format!("{g}/buddy/travel/status"),
        travel_claim: format!("{g}/buddy/travel/claim"),
        travel_config: format!("{g}/buddy/travel/config"),
        travel_depart: format!("{g}/buddy/travel/depart"),
        lottery_chances: format!("{g}/lottery/chances"),
        lottery_draw: format!("{g}/lottery/draw"),
        tasks: format!("{g}/tasks"),
        tasks_accept: format!("{g}/tasks/accept"),
        energy: format!("{g}/energy"),
        streak: format!("{g}/streak"),
        streak_redeem: format!("{g}/redeem"),
        report: format!("{b}/v2/report"),
        buddy_agreement: format!("{g}/buddy/agreement"),
        buddy_first: format!("{g}/buddy/first"),
    }
}

impl Urls {
    /// 备用域名端点表（§2.2 域名双探测）：主域名网络不可达时切换重试一次
    fn alt(&self) -> Urls {
        let bases = wb_common::billing_bases("");
        let alt = if bases[0] == self.base { bases[1] } else { bases[0] };
        urls_for(alt)
    }

    /// Web 端基址（领奖专用，与 billing 基址不同域）
    fn web_base(&self) -> &'static str {
        if self.base == wb_common::BILLING_BASE_GLOBAL {
            wb_common::WEB_BASE_GLOBAL
        } else {
            wb_common::WEB_BASE_CN
        }
    }
}

// ── 解析辅助（对齐 python _num_or_none / _scalar_of / bool(v)）─────────────

/// 奖励/余额数值归一：数字或纯数字字符串 → f64，其余 None（不递归，对齐 _num_or_none）
fn num_or_none(v: Option<&Value>) -> Option<f64> {
    let v = v?;
    match v {
        Value::Bool(_) => None,
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
}

/// python bool() 语义（0/""/null/false → false，其余 true）
fn py_truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().map_or(true, |f| f != 0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(_)) => true,
    }
}

/// 嵌套对象按常用数值键归一（energy/streak 兼容 {current:..} 等对象返回，
/// 修复前端「连签 [object Object] 天」展示）
fn scalar_of(v: &Value) -> Option<f64> {
    if let Some(m) = v.as_object() {
        for k in ["current", "days", "count", "value", "num", "total", "streak", "energy"] {
            if let Some(x) = m.get(k) {
                if !x.is_null() {
                    return scalar_of(x);
                }
            }
        }
        return None;
    }
    num_or_none(Some(v))
}

/// 递归深挖奖励数额：键名含奖励语义子串的数值字段（宽容解析，对齐 doubao_quota
/// deep dig 模式）。刻意不含 balance/total/remaining 语义——避免把账户余额误当本次奖励。
fn deep_reward_dig(v: &Value, depth: usize) -> Option<f64> {
    if depth > 8 {
        return None;
    }
    match v {
        Value::Object(m) => {
            for (k, val) in m {
                let kl = k.to_ascii_lowercase();
                let looks_reward = ["reward", "bonus", "earned", "add_integral", "add_credits", "addcredit", "integral_add", "gain", "obtain", "prize"]
                    .iter()
                    .any(|s| kl.contains(s));
                if looks_reward {
                    if let Some(n) = num_or_none(Some(val)) {
                        return Some(n);
                    }
                }
            }
            for val in m.values() {
                if val.is_object() || val.is_array() {
                    if let Some(n) = deep_reward_dig(val, depth + 1) {
                        return Some(n);
                    }
                }
            }
            None
        }
        Value::Array(a) => a.iter().find_map(|x| deep_reward_dig(x, depth + 1)),
        _ => None,
    }
}

/// 递归找余额数值字段：键名含 remaining/balance/capacity（大小写不敏感，排除 total）。
/// 用于 billing summary 结构未知时的余额差值兜底（pre/post 同结构，首命中稳定）。
fn deep_balance_dig(v: &Value, depth: usize) -> Option<f64> {
    if depth > 8 {
        return None;
    }
    match v {
        Value::Object(m) => {
            for (k, val) in m {
                let kl = k.to_ascii_lowercase();
                if (kl.contains("remaining") || kl.contains("balance") || kl.contains("capacity"))
                    && !kl.contains("total")
                {
                    if let Some(n) = num_or_none(Some(val)) {
                        return Some(n);
                    }
                }
            }
            for val in m.values() {
                if val.is_object() || val.is_array() {
                    if let Some(n) = deep_balance_dig(val, depth + 1) {
                        return Some(n);
                    }
                }
            }
            None
        }
        Value::Array(a) => a.iter().find_map(|x| deep_balance_dig(x, depth + 1)),
        _ => None,
    }
}

/// 奖励数额一律以接口返回为准，不硬编码（F-17 红线）；无奖励回退 raw 摘要
fn reward_text(body: Option<&Value>, raw: &str) -> String {
    let r = body.and_then(|b| fs_utils::dig(b, &["reward", "credits", "points", "amount", "value"]));
    match r {
        Some(v) if !v.is_null() => format!("+{v}"),
        _ => {
            let t: String = raw.chars().take(60).collect();
            if t.is_empty() { "ok".to_string() } else { t }
        }
    }
}

fn s_of(v: Option<&Value>) -> String {
    v.and_then(Value::as_str).unwrap_or("").to_string()
}

/// 取首个真值字符串（对齐 python `a or b or c`：空串视为假）
fn first_str(parts: &[Option<String>]) -> String {
    parts
        .iter()
        .flatten()
        .find(|s| !s.is_empty())
        .cloned()
        .unwrap_or_default()
}

// ── 签到端点（对齐 python checkin_status / checkin_do / fetch_balance）─────

/// 查询今日是否已签：新路径回退旧路径；返回 (today_checked_in, status_ok)
fn checkin_status(
    agent: &ureq::Agent,
    headers: &[(String, String)],
    urls: &Urls,
) -> (Option<bool>, bool) {
    for url in [&urls.checkin_status, &urls.checkin_status_old] {
        let (status, body) = wb_common::post_json(agent, url, headers, &json!({}));
        if status == 401 {
            return (None, false);
        }
        if status == 200 {
            if let Some(b) = body.filter(|b| b.is_object()) {
                let mut v = fs_utils::dig(&b, &["today_checked_in", "todayCheckedIn", "checked_in", "checkedIn"]);
                if v.is_none() {
                    // 部分响应以 0/1 表达
                    v = fs_utils::dig(&b, &["checked", "is_checked"]);
                }
                if let Some(v) = v {
                    return (Some(py_truthy(Some(v))), true);
                }
                return (None, true); // 200 但字段缺失：视为未知但不失败（执行时容错）
            }
        }
        // 其它状态码尝试旧路径
    }
    (None, false)
}

/// 执行签到。返回 (kind, message, reward)：success / already / fail（auth 由调用方刷新重试）
/// 签到执行。返回 (kind, message, reward, dbg_keys)：
/// success / already / fail（auth 由调用方刷新重试）；dbg_keys = 成功但奖励未识别时
/// 的响应键路径（仅键名不含值，供 app.log 诊断校准，见 process_account）
fn checkin_do(
    agent: &ureq::Agent,
    headers: &[(String, String)],
    urls: &Urls,
) -> (String, String, Option<f64>, Option<String>) {
    let (mut status, mut body, mut raw) =
        wb_common::post_json_raw(agent, &urls.checkin_do, headers, &json!({}));
    if status == 0 {
        // 域名双探测（§2.2）：主域名网络不可达 → 备用域名重试一次
        let alt = urls.alt().checkin_do;
        if alt != urls.checkin_do {
            let r = wb_common::post_json_raw(agent, &alt, headers, &json!({}));
            status = r.0;
            body = r.1;
            raw = r.2;
        }
    }
    if status == 401 {
        return ("auth".into(), "登录态失效（401）".into(), None, None);
    }
    let code: Option<i64> = body
        .as_ref()
        .and_then(|b| b.get("code"))
        .and_then(Value::as_i64)
        .or_else(|| body.as_ref().and_then(|b| fs_utils::dig(b, &["code"])).and_then(Value::as_i64));
    let message = body
        .as_ref()
        .and_then(|b| fs_utils::dig(b, &["message", "msg"]))
        .map(|v| s_of(Some(v)));
    if status == 0 {
        let head: String = raw.chars().take(120).collect();
        return ("fail".into(), format!("网络不可达: {head}"), None, None);
    }
    if (200..=201).contains(&status) || code.is_some_and(|c| c == 0 || c == 200) {
        // 奖励数额以接口返回为准，不硬编码（F-17）；精确键未命中走递归深挖，
        // 仍无则兜底走签到前后余额差值（process_account）
        let reward = body.as_ref().and_then(|b| {
            num_or_none(fs_utils::dig(
                b,
                &[
                    "reward", "credits", "points", "amount", "integral", "score", "bonus",
                    "reward_amount", "add_integral", "addCredits", "earned",
                ],
            ))
            .or_else(|| deep_reward_dig(b, 0))
        });
        // 奖励未识别时收集响应键路径（不含值）供诊断校准
        let dbg_keys = if reward.is_none() {
            body.as_ref().map(|b| {
                let mut paths = Vec::new();
                collect_key_paths(b, "", 0, &mut paths);
                paths.join(" | ")
            })
        } else {
            None
        };
        return ("success".into(), "签到成功".into(), reward, dbg_keys);
    }
    // 已签容错（F-15）：code:10001 / message 含「已签到」/「repeat」
    let msg_low = message.as_deref().unwrap_or("").to_ascii_lowercase();
    if code == Some(10001)
        || ["已签到", "repeat", "already"].iter().any(|k| msg_low.contains(k))
    {
        return ("already".into(), "今日已签到".into(), None, None);
    }
    let head: String = raw.chars().take(120).collect();
    let shown = message
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| if head.is_empty() { format!("HTTP {status}") } else { head });
    (
        "fail".into(),
        format!("{shown}（code={}）", code.map(|c| c.to_string()).unwrap_or_else(|| "None".into())),
        None,
        None,
    )
}

/// 收集 JSON 键路径（深度/数量受限；只采集键名不采集值——诊断用，避免敏感信息入日志）
fn collect_key_paths(v: &Value, prefix: &str, depth: usize, out: &mut Vec<String>) {
    if depth > 6 || out.len() >= 40 {
        return;
    }
    match v {
        Value::Object(m) => {
            for (k, val) in m {
                let p = if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") };
                out.push(p.clone());
                collect_key_paths(val, &p, depth + 1, out);
            }
        }
        Value::Array(a) => {
            for (i, x) in a.iter().enumerate().take(3) {
                collect_key_paths(x, &format!("{prefix}[{i}]"), depth + 1, out);
            }
        }
        _ => {}
    }
}

/// 查询当前通用积分余额（get-user-resource-summary，与积分页同口径）。
/// 网络失败/解析失败返回 None——仅用于签到获得积分差值兜底，不阻塞签到。
fn fetch_balance(agent: &ureq::Agent, headers: &[(String, String)], base: &str) -> Option<f64> {
    let path = "/billing/meter/get-user-resource-summary";
    let (mut status, mut body) = wb_common::post_json(agent, &format!("{base}{path}"), headers, &json!({}));
    if status == 0 {
        let alt = if base == wb_common::BILLING_BASE_CN {
            wb_common::BILLING_BASE_GLOBAL
        } else {
            wb_common::BILLING_BASE_CN
        };
        if alt != base {
            let r = wb_common::post_json(agent, &format!("{alt}{path}"), headers, &json!({}));
            status = r.0;
            body = r.1;
        }
    }
    if status != 200 {
        return None;
    }
    let b = body?;
    num_or_none(fs_utils::dig(
        &b,
        &["RemainingCapacity", "remaining", "TotalRemaining", "Balance", "balance"],
    ))
    .or_else(|| deep_balance_dig(&b, 0))
}

/// WB credits 域余额（WorkBuddy 积分账本）：billing meter 是通用积分账本，
/// 签到奖励入 WB 积分——billing 差值恒 0 时用 credits 差值兜底
fn fetch_wb_credits_balance(state: &AppState, aid: &str) -> Option<f64> {
    let parsed = crate::tasks::wb_credits::fetch_credits(state, Some(aid), true).ok()?;
    parsed
        .get("accounts")?
        .as_array()?
        .iter()
        .find(|a| a.get("user_id").and_then(Value::as_str) == Some(aid))
        .and_then(|a| a.get("balance"))
        .and_then(Value::as_f64)
}

// ── 账号池回写 / 结果存储 ──────────────────────────────────────────────────

/// 回写账号池的企业号标记（识别出来即落库，避免每轮重探）。
/// 只在该标记发生变化时写盘，识别失败（网络/POST 不可达）不误标
fn sync_pool_enterprise(state: &AppState, aid: &str, is_enterprise: bool) {
    let _guard = state.wb_pool_lock.lock().unwrap_or_else(|e| e.into_inner());
    let mut pool: Value = crate::store::docs::wb_pool_load(&crate::store::db(&state.data_dir));
    let mut changed = false;
    if let Some(accounts) = pool.get_mut("accounts").and_then(Value::as_array_mut) {
        for a in accounts.iter_mut() {
            if a.get("id").and_then(Value::as_str) != Some(aid) {
                continue;
            }
            if a.get("is_enterprise").and_then(Value::as_bool) == Some(is_enterprise) {
                break;
            }
            if let Some(obj) = a.as_object_mut() {
                obj.insert("is_enterprise".into(), json!(is_enterprise));
                changed = true;
            }
            break;
        }
    }
    if changed {
        let _ = crate::store::docs::wb_pool_save(&crate::store::db(&state.data_dir), &pool);
    }
}

/// 刷新成功后回写账号池 token 过期时间（调度/到期日历数据源）
fn sync_pool_expiry(state: &AppState, aid: &str, creds: &Creds) {
    // SQLite 化（P3）：wb_accounts 表
    // 读-改-写互斥：罩住 load→改→save 段，防命令/后台/调度线程旧副本整文档覆盖
    let _guard = state.wb_pool_lock.lock().unwrap_or_else(|e| e.into_inner());
    let mut pool: Value = crate::store::docs::wb_pool_load(&crate::store::db(&state.data_dir));
    let mut changed = false;
    if let Some(accounts) = pool.get_mut("accounts").and_then(Value::as_array_mut) {
        for a in accounts.iter_mut() {
            if a.get("id").and_then(Value::as_str) == Some(aid) {
                if let Some(exp) = creds.expires_at_ms {
                    a["access_token_expires_at"] = json!(exp.div_euclid(1000));
                }
                a["refresh_token_expires_at"] = json!(creds.refresh_expires_at_ms.map(|expiry| expiry.div_euclid(1000)));
                a["needs_relogin"] = json!(false);
                a["relogin_reason"] = json!("");
                changed = true;
            }
        }
    }
    if changed {
        let _ = crate::store::docs::wb_pool_save(&crate::store::db(&state.data_dir), &pool);
    }
}

/// 签到结果 90 天滚动存储（趋势/日志数据源，F-55/F-22；SQLite 化 P3：wb_checkin_results 表）
fn append_results(state: &AppState, events: &[Value]) {
    let store = crate::store::db(&state.data_dir);
    let mut data: Value = crate::store::docs::wb_checkin_results_load(&store);
    let results: Vec<Value> = data
        .get("results")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let cutoff = (chrono::Local::now().date_naive() - chrono::Duration::days(90))
        .format("%Y-%m-%d")
        .to_string();
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let mut kept: Vec<Value> = results
        .into_iter()
        .filter(|r| r.get("date").and_then(Value::as_str).unwrap_or("") >= cutoff.as_str())
        .collect();
    for ev in events {
        // skip = 按设计不适用（企业号无个人签到体系）：不落签到记录。
        // 日历/趋势把非 success/already 一律计入 fail，落库会让企业号
        // 每天在签到日历上留一个红色「失败」
        if ev.get("status").and_then(Value::as_str) == Some("skip") {
            continue;
        }
        let mut rec = json!({
            "date": today,
            "time": fs_utils::now_ts(),
            "user_id": ev.get("user_id").cloned().unwrap_or_default(),
            "name": ev.get("name").cloned().unwrap_or_default(),
            "status": ev.get("status").cloned().unwrap_or_default(),
            "message": ev.get("message").cloned().unwrap_or_default(),
        });
        if let Some(r) = ev.get("reward").filter(|r| !r.is_null()) {
            rec["reward"] = r.clone();
        }
        kept.push(rec);
    }
    data["results"] = json!(kept);
    let _ = crate::store::docs::wb_checkin_results_save(&store, &data);
}

// ── 签到主流程 ─────────────────────────────────────────────────────────────

/// 取本地记录中该账号**当日 success 记录的最大 reward**（无则 None）。
///
/// 语义（审查 P1-3）：结果表为纯 append，同日同账号可能多条——取最大值，
/// 防早前无奖励记录遮蔽后来的有效记录。供「已签回填」与「本地优先跳过」共用。
fn local_reward_today(state: &AppState, aid: &str) -> Option<f64> {
    let results: Value = crate::store::docs::wb_checkin_results_load(&crate::store::db(&state.data_dir));
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    results.get("results").and_then(Value::as_array).and_then(|arr| {
        arr.iter()
            .filter(|r| {
                r.get("date").and_then(Value::as_str) == Some(today.as_str())
                    && r.get("user_id").and_then(Value::as_str) == Some(aid)
                    && r.get("status").and_then(Value::as_str) == Some("success")
            })
            .filter_map(|r| r.get("reward").and_then(Value::as_f64))
            .fold(None::<f64>, |acc, v| Some(match acc {
                Some(a) if a >= v => a,
                _ => v,
            }))
    })
}

/// 处理单账号签到（含 401 刷新一次重试）。返回 account 事件（不含 index）。
/// `local_checked`：本地记录判定今日已签的账号集合（skip_checked 开启时非空）——
/// 命中即直接返回 already，不进任何上游请求（本地优先）；未命中则照常走
/// `checkin_status` 上游预检兜底（权威口径，覆盖本地记录缺失的情形）。
fn process_account(
    state: &AppState,
    agent: &ureq::Agent,
    acct: &Value,
    opts: &CheckinOpts,
    local_checked: &std::collections::HashSet<String>,
) -> Value {
    let aid = s_of(acct.get("id"));
    let name = first_str(&[
        acct.get("nickname").and_then(Value::as_str).map(str::to_string),
        Some(s_of(acct.get("uid")).chars().take(8).collect()),
        Some(aid.clone()),
    ]);
    let base_ev = json!({"user_id": aid, "name": name});
    if opts.skip_expired && py_truthy(acct.get("needs_relogin")) {
        return json!({ "user_id": aid, "name": base_ev["name"], "status": "fail", "message": "需重新登录，已跳过" });
    }
    // 本地优先：今日已有 success/already 记录 → 直接跳过（不发上游状态预检）。
    // 保留「回填当日积分」语义：从本地记录取当日 success 的最大 reward 一并展示
    if opts.skip_checked && local_checked.contains(&aid) {
        let reward = local_reward_today(state, &aid);
        let mut ev = json!({
            "user_id": aid, "name": base_ev["name"], "status": "already",
            "message": "今日已签到（本地记录，已跳过请求）"
        });
        if let Some(r) = reward {
            ev["reward"] = json!(r);
        }
        return ev;
    }
    let (creds, refreshed, note) = wb_common::ensure_fresh(state, agent, acct, opts.lazy_hours);
    if !matches!(note, "fresh" | "refreshed" | "no_credential") {
        return json!({ "user_id": aid, "name": base_ev["name"], "status": "fail", "message": renewal_note_message(state, &aid, note) });
    }
    if creds.access_token.is_empty() {
        return json!({ "user_id": aid, "name": base_ev["name"], "status": "fail", "message": format!("无可用凭证（{note}）") });
    }
    if refreshed {
        sync_pool_expiry(state, &aid, &creds);
    }
    // 企业号无个人成长体系：签到在上游返回 400 code 10001「企业账号不支持该操作」，
    // 整体跳过（不空发请求）。额度查询另走企业接口，不受影响
    if wb_common::is_enterprise_account(&creds) {
        sync_pool_enterprise(state, &aid, true);
        return json!({
            "user_id": aid, "name": base_ev["name"], "status": "skip",
            "message": "企业号无个人签到体系，已跳过"
        });
    }
    let base = wb_common::region_billing_base(&creds.domain);
    let urls = urls_for(base);
    let mut headers = wb_common::build_auth_headers(&creds, false);
    let (checked, ok) = checkin_status(agent, &headers, &urls);
    if ok && checked == Some(true) {
        return json!({ "user_id": aid, "name": base_ev["name"], "status": "already", "message": "今日已签到" });
    }
    // 签到前余额（获得积分差值兜底数据源；两级恒取：①billing meter 通用积分
    // ②WB credits 域 WorkBuddy 积分——查询失败不阻塞签到。恒取两级的原因：
    // billing 前值可能成功但余额恰好不变（签到加的是 WB 积分账本），差值恒 0 时
    // 仍需 WB credits 差值兜底）
    let pre_balance = fetch_balance(agent, &headers, base);
    let pre_wb_credits = fetch_wb_credits_balance(state, &aid);
    let (mut kind, mut message, mut reward, mut dbg_keys) = checkin_do(agent, &headers, &urls);
    if kind == "auth" {
        // 401：刷新一次仅重试失败分支（禁止二次刷新，F-09）。
        // H-1：加锁刷新 + 二次检查（他人已刷新落库直接复用）+ 内部落库
        let (new, fail_reason) = wb_common::refresh_token_once_locked(state, agent, &aid, &creds);
        match new {
            Some(new) => {
                sync_pool_expiry(state, &aid, &new);
                headers = wb_common::build_auth_headers(&new, false);
                let r = checkin_do(agent, &headers, &urls);
                kind = r.0;
                message = r.1;
                reward = r.2;
                dbg_keys = r.3;
                // 二次失败（审查 P1-4）：新凭证仍 401 → 标记重登录，归一为 fail
                if kind == "auth" {
                    wb_common::mark_needs_relogin(state, &aid, "签到重试后仍 401");
                    kind = "fail".into();
                    message = "登录态失效（刷新后仍 401），需重新登录".into();
                    reward = None;
                }
            }
            None => {
                // 凭证失效才标记重登录；网络故障/响应异常/他方进程刷新中不误标（审查 P1-4 + H-1）
                kind = "fail".into();
                message = failure_message(state, &aid, fail_reason);
                reward = None;
            }
        }
    }
    // 获得积分兜底（F-17）：接口未返回奖励数额时用签到前后余额差值；
    // 仅差值>0 才采信（防并发扣减/查询时点差造成负值误报）。
    // 两级数据源都尝试：①billing meter（通用积分）②WB credits（WorkBuddy 积分）
    let mut balance_shrunk = false; // 差值兜底期间检测到余额下降（并发扣减），用于诊断区分「无奖励」与「未能核实」
    if kind == "success" && reward.is_none() {
        if let Some(pre) = pre_balance {
            if let Some(post) = fetch_balance(agent, &headers, base) {
                if post > pre {
                    reward = Some(((post - pre) * 100.0).round() / 100.0);
                } else if post < pre {
                    balance_shrunk = true;
                }
            }
        }
        if reward.is_none() {
            if let Some(pre) = pre_wb_credits {
                if let Some(post) = fetch_wb_credits_balance(state, &aid) {
                    if post > pre {
                        reward = Some(((post - pre) * 100.0).round() / 100.0);
                    } else if post < pre {
                        balance_shrunk = true;
                    }
                }
            }
        }
    }
    let status_txt = match kind.as_str() {
        "success" => "success",
        "already" => "already",
        _ => "fail",
    };
    // 「已签」回填：签到奖励当日只发一次，今日早前成功记录若已捕获奖励则同步展示
    //（用户语义：无论成功还是已签，获取积分列都应显示当日所得）；取值口径见 local_reward_today
    if status_txt == "already" && reward.is_none() {
        reward = local_reward_today(state, &aid);
    }
    // 诊断（脱敏红线：只输出键路径不含值）：成功但三层提取（精确键/深挖/双差值）
    // 均未命中时记录响应键路径，便于按真实结构校准奖励键
    if kind == "success" && reward.is_none() {
        fs_utils::app_log(
            &state.data_dir,
            &format!(
                "wb 签到成功但奖励数额未识别（接口回显/递归深挖/双余额差值均未命中）: {aid}，响应键路径: {}{}",
                dbg_keys.as_deref().unwrap_or("（响应非 JSON）"),
                if balance_shrunk { "；差值兜底期间余额下降（并发扣减可能抵消奖励差值）" } else { "" },
            ),
        );
    }
    let mut ev = json!({ "user_id": aid, "name": base_ev["name"], "status": status_txt, "message": message });
    if let Some(r) = reward {
        ev["reward"] = json!(r);
    }
    if kind == "success" && note == "refreshed" {
        ev["message"] = json!(format!("{}（凭证已续期）", ev["message"].as_str().unwrap_or("")));
    }
    ev
}

/// 今日已签到账号集合（本地记录口径）：`wb_checkin_results` 中当天状态为
/// success / already 的 user_id。
///
/// 用途：`skip_checked` 开启时「本地优先」跳过——已签过的账号不再打上游
/// `checkin_status` 预检（对齐 Trae 侧 `skip_checked_in` 的过滤语义，省请求）。
/// 本地无记录（首用/记录被清）时仍走上游预检兜底，故不会误判。
fn checked_today_local(state: &AppState) -> std::collections::HashSet<String> {
    let data: Value = crate::store::docs::wb_checkin_results_load(&crate::store::db(&state.data_dir));
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    data.get("results")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter(|r| r.get("date").and_then(Value::as_str) == Some(today.as_str()))
                .filter(|r| {
                    matches!(
                        r.get("status").and_then(Value::as_str),
                        Some("success") | Some("already")
                    )
                })
                .filter_map(|r| r.get("user_id").and_then(Value::as_str).map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// 签到整轮：逐账号串行处理，事件经 emit 回调逐条输出（NDJSON 管线复用）。
/// 返回 done 事件（ok/already/failed 计数），供启动补签/托盘静默路径直接消费。
pub fn run_checkin_round(state: &AppState, opts: &CheckinOpts, emit: &mut dyn FnMut(&Value)) -> Value {
    let agent = http_agent(30);
    let pool: Value = crate::store::docs::wb_pool_load(&crate::store::db(&state.data_dir));
    let mut accounts: Vec<Value> = pool
        .get("accounts")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if !opts.uids.is_empty() {
        accounts.retain(|a| opts.uids.iter().any(|u| s_of(a.get("id")) == *u));
    }
    // skip_checked：本地已签账号本轮直接判 already（不发上游预检）——见 checked_today_local
    let local_checked = if opts.skip_checked {
        checked_today_local(state)
    } else {
        std::collections::HashSet::new()
    };
    emit(&json!({"type": "start", "total": accounts.len()}));

    // 多账号签到间隔（任务配置页可改，默认 3s，0=关闭）：防上游频控，账号间串行等待
    let gap_secs = crate::models::effective_checkin_gap(state.settings().wb_checkin_gap_secs);
    let mut events: Vec<Value> = Vec::new();
    for (i, acct) in accounts.iter().enumerate() {
        if i > 0 && gap_secs > 0 {
            std::thread::sleep(std::time::Duration::from_secs(gap_secs));
        }
        // 单账号失败不中断整轮（python try/except 语义；Rust 直调无异常路径）
        let mut ev = process_account(state, &agent, acct, opts, &local_checked);
        ev["index"] = json!(i + 1);
        emit(&ev);
        events.push(ev);
    }

    let ok = events.iter().filter(|e| e["status"] == "success").count();
    let already = events.iter().filter(|e| e["status"] == "already").count();
    // skip 是「按设计不适用」（企业号无个人签到体系）而非失败，不计入 failed
    let skipped = events.iter().filter(|e| e["status"] == "skip").count();
    let failed = events.len() - ok - already - skipped;
    let done = json!({"type": "done", "ok": ok, "already": already, "failed": failed});
    emit(&done);
    append_results(state, &events);
    done
}

// ── 成长中心自动化（T2.5/F-17）────────────────────────────────────────────

/// 401 刷新一次重试封装（对齐 python with_retry）：f 以 headers 为参，
/// 返回 (kind, msg)；kind=auth 时刷新凭证后重试一次。
fn with_retry<F>(state: &AppState, agent: &ureq::Agent, acct_id: &str, creds: &Creds, f: F) -> (String, String)
where
    F: Fn(&[(String, String)]) -> (String, String),
{
    let current = wb_common::effective_creds(state, acct_id, &creds.uid);
    if let Some(error) = super::wb_session::recovery_block(&state.data_dir, acct_id, &current) {
        return ("stop".into(), error.message());
    }
    let headers = wb_common::build_auth_headers(&current, false);
    let (kind, msg) = f(&headers);
    if kind == "auth" {
        // H-1：加锁刷新 + 二次检查 + 内部落库（原 refresh_token_once_ex + 手工 save）
        let (new, fail_reason) = wb_common::refresh_token_once_locked(state, agent, acct_id, &current);
        if let Some(new) = new {
            sync_pool_expiry(state, acct_id, &new);
            let h2 = wb_common::build_auth_headers(&new, false);
            let (kind2, msg2) = f(&h2);
            // 二次失败处理（审查 P1-4）：刷新成功但新凭证仍 401 → 凭证实际已失效，
            // 归一为 fail 并回写 needs_relogin（避免 auth 泄漏到上层结果）
            if kind2 == "auth" {
                wb_common::mark_needs_relogin(state, acct_id, "刷新后仍 401");
                return ("stop".into(), "登录态失效（刷新后仍 401），需重新登录".into());
            }
            return (kind2, msg2);
        }
        // 凭证失效才标记重登录；网络故障/响应异常/他方进程刷新中不误标（下次调度自然重试）
        return ("stop".into(), failure_message(state, acct_id, fail_reason));
    }
    (kind, msg)
}

fn failure_message(state: &AppState, account_id: &str, failure: Option<wb_common::RefreshFail>) -> String {
    let stored = super::wb_session::health(&state.data_dir, account_id);
    if !stored.message.is_empty() { return stored.message; }
    super::wb_session::RefreshError::new(failure.unwrap_or(wb_common::RefreshFail::Network)).message()
}

fn renewal_note_message(state: &AppState, account_id: &str, note: &str) -> String {
    if note == "auth_required" { return "需重新登录，已停止自动恢复".into(); }
    failure_message(state, account_id, Some(match note {
        "refresh_busy" => wb_common::RefreshFail::Busy,
        "refresh_cooldown" => wb_common::RefreshFail::Cooldown,
        "persist_failed" => wb_common::RefreshFail::Persistence,
        _ => wb_common::RefreshFail::Network,
    }))
}

fn growth_stopped(base: &Value, message: String) -> Value {
    json!({"type": "growth", "user_id": base["user_id"], "name": base["name"], "status": "fail", "message": message})
}

/// Buddy 旅行：status → arrived 则 claim → config → depart（各步独立容错）
fn growth_travel(agent: &ureq::Agent, headers: &[(String, String)], urls: &Urls) -> (String, String) {
    let (status, body, raw) = wb_common::get_json(agent, &urls.travel_status, headers);
    if status == 401 {
        return ("auth".into(), "登录态失效（401）".into());
    }
    if status != 200 || !body.as_ref().is_some_and(|b| b.is_object()) {
        let head: String = raw.chars().take(60).collect();
        let msg = if status != 0 {
            format!("travel/status 不可用（HTTP {status}）")
        } else {
            format!("travel/status 不可达: {head}")
        };
        return ("fail".into(), msg);
    }
    let b = body.as_ref().unwrap();
    let arrived = py_truthy(fs_utils::dig(b, &["arrived", "is_arrived", "has_arrived"]));
    let record_id = fs_utils::dig(b, &["record_id", "recordId"]).cloned();
    if !arrived {
        // 未到达：报告在途状态即可
        let dest = fs_utils::dig(b, &["destination", "name", "target"]).map(|v| s_of(Some(v)));
        let suffix = dest
            .filter(|d| !d.is_empty())
            .map(|d| format!("（{d}）"))
            .unwrap_or_default();
        return ("skip".into(), format!("旅行在途{suffix}"));
    }
    // 领奖
    let claim_body = match &record_id {
        Some(r) => json!({"record_id": r}),
        None => json!({}),
    };
    let (st1, b1, r1) = wb_common::post_json_raw(agent, &urls.travel_claim, headers, &claim_body);
    if st1 == 401 {
        return ("auth".into(), "登录态失效（401）".into());
    }
    let claim_txt = if (200..=201).contains(&st1) {
        reward_text(b1.as_ref(), &r1)
    } else {
        format!("claim 失败（HTTP {st1}）")
    };
    // 查目的地配置并出发（config 失败不阻塞 depart）
    let dest = wb_common::get_json(agent, &urls.travel_config, headers)
        .1
        .filter(|b2| b2.is_object())
        .and_then(|b2| fs_utils::dig(&b2, &["destination", "name", "target"]).cloned());
    let depart_body = match &dest {
        Some(d) => json!({"destination": d}),
        None => json!({}),
    };
    let (st3, _b3, _r3) = wb_common::post_json_raw(agent, &urls.travel_depart, headers, &depart_body);
    let dest_txt = dest
        .as_ref()
        .map(|d| format!("（{}）", value_display(d)))
        .unwrap_or_default();
    if (200..=201).contains(&st3) {
        return ("ok".into(), format!("领奖{claim_txt}，已出发{dest_txt}"));
    }
    ("ok".into(), format!("领奖{claim_txt}；depart 失败（HTTP {st3}）"))
}

/// 展示用值文本（数字去尾零、字符串原样）
fn value_display(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// 盲盒：chances(balance>0) → draw 循环（可开关）
fn growth_lottery(agent: &ureq::Agent, headers: &[(String, String)], urls: &Urls) -> (String, String) {
    let (status, body, raw) = wb_common::get_json(agent, &urls.lottery_chances, headers);
    if status == 401 {
        return ("auth".into(), "登录态失效（401）".into());
    }
    if status != 200 || !body.as_ref().is_some_and(|b| b.is_object()) {
        let head: String = raw.chars().take(60).collect();
        let msg = if status != 0 {
            format!("lottery/chances 不可用（HTTP {status}）")
        } else {
            format!("lottery/chances 不可达: {head}")
        };
        return ("fail".into(), msg);
    }
    let b = body.as_ref().unwrap();
    // 对齐 python int(balance)：数字（float 四舍五入）或整数字符串；小数字符串视为无效。
    // 审查 P1-4：不用 as i64 截断——JSON 浮点表示误差（3.0 实存 2.9999…）会少抽一次
    let balance = fs_utils::dig(b, &["balance", "chances", "count", "remain"]).and_then(|v| match v {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f.round() as i64)),
        Value::String(s) => s.trim().parse::<i64>().ok(),
        _ => None,
    });
    let Some(balance) = balance.filter(|n| *n > 0) else {
        return ("skip".into(), "无可用次数".into());
    };
    let mut balance = balance;
    let mut draws = 0usize;
    let mut rewards: Vec<String> = vec![];
    while balance > 0 && draws < LOTTERY_MAX_DRAWS {
        let (st, b, _r) = wb_common::post_json_raw(agent, &urls.lottery_draw, headers, &json!({}));
        if st == 401 {
            return ("auth".into(), format!("登录态失效（401，已抽 {draws} 次）"));
        }
        if !(200..=201).contains(&st) {
            break;
        }
        draws += 1;
        if let Some(rw) = b.as_ref().and_then(|bb| fs_utils::dig(bb, &["reward", "credits", "points", "amount"])) {
            if !rw.is_null() {
                rewards.push(value_display(rw));
            }
        }
        balance -= 1;
    }
    if draws == 0 {
        return ("fail".into(), "draw 不可用".into());
    }
    let joined = if rewards.is_empty() {
        "见响应".to_string()
    } else {
        rewards.join("/")
    };
    ("ok".into(), format!("抽取 {draws} 次（奖励 {joined}）"))
}

/// 任务码路径段编码（任务码形如 `first_buddy` / `Model_chat_GLM5.2`）
fn encode_path_seg(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// 任务进度 (current, target)：`progress{current,target}` 优先，回退平铺字段
fn task_progress(t: &Value) -> (f64, f64) {
    let flat = |k: &str| t.get(k).filter(|v| !v.is_null()).and_then(scalar_of);
    if let Some(p) = t.get("progress").filter(|p| p.is_object()) {
        let c = p.get("current").filter(|v| !v.is_null()).and_then(scalar_of);
        let g = p.get("target").filter(|v| !v.is_null()).and_then(scalar_of);
        if c.is_some() || g.is_some() {
            return (c.or_else(|| flat("current")).unwrap_or(0.0), g.or_else(|| flat("target")).unwrap_or(0.0));
        }
    }
    (flat("current").unwrap_or(0.0), flat("target").unwrap_or(0.0))
}

/// 单任务领奖（**Web 域**）：`POST {web}/activity/growth/tasks/{code}/claim`，无 body。
/// 返回 Ok((credit, energy))；`already_claimed` 视为成功但零到账。Err 携带 HTTP 状态。
fn claim_task(
    agent: &ureq::Agent,
    web_headers: &[(String, String)],
    urls: &Urls,
    code: &str,
) -> Result<(f64, f64), u16> {
    let url = format!(
        "{}/activity/growth/tasks/{}/claim",
        urls.web_base(),
        encode_path_seg(code)
    );
    let (st, body, _raw) = wb_common::post_json_raw(agent, &url, web_headers, &json!({}));
    if !(200..=201).contains(&st) {
        return Err(st);
    }
    let b = body.unwrap_or_else(|| json!({}));
    let credit = fs_utils::dig(&b, &["credit"]).and_then(scalar_of).unwrap_or(0.0);
    let energy = fs_utils::dig(&b, &["energy"]).and_then(scalar_of).unwrap_or(0.0);
    Ok((credit, energy))
}

/// 拉取成长任务列表（宽松解析 `data.tasks/list/records`，返回 owned 数组）
fn fetch_tasks(
    agent: &ureq::Agent,
    headers: &[(String, String)],
    urls: &Urls,
) -> Result<Vec<Value>, (u16, String)> {
    let (status, body, raw) = wb_common::get_json(agent, &urls.tasks, headers);
    if status == 401 {
        return Err((401, "登录态失效（401）".into()));
    }
    if status != 200 || !body.as_ref().is_some_and(|b| b.is_object()) {
        let head: String = raw.chars().take(60).collect();
        let msg = if status != 0 {
            format!("tasks 不可用（HTTP {status}）")
        } else {
            format!("tasks 不可达: {head}")
        };
        return Err((status, msg));
    }
    let b = body.as_ref().unwrap();
    Ok(fs_utils::dig(b, &["tasks", "list", "records"])
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default())
}

/// 取任务码（task_code / taskCode / code）
fn task_code_of(t: &Value) -> &str {
    t.get("task_code")
        .filter(|v| !v.is_null())
        .or_else(|| t.get("taskCode").filter(|v| !v.is_null()))
        .or_else(|| t.get("code").filter(|v| !v.is_null()))
        .and_then(|v| v.as_str())
        .unwrap_or("")
}

/// P2 第一批：纯事件型任务的判据补齐。
///
/// 只处理「未领取且进度未达标」的任务，按任务类型构造对应客户端指纹的事件链：
/// - 桌面包（RichMeow / Buddy_App / automation / template / playbook / canvas）
/// - Web 包（Library_read）
/// - 对话包（chat_5 / first_buddy 前置）
///
/// 上报 200 不等于计分：真正是否到账由随后重新拉进度 + 领奖决定
/// （调用方在本函数返回后 sleep 一段时间再走 `growth_tasks`）。
fn sync_task_reports(
    agent: &ureq::Agent,
    headers: &[(String, String)],
    creds: &Creds,
    urls: &Urls,
    tasks: &[Value],
) -> (String, String) {
    // 缺 uid 时整体跳过：桌面包与 Web 包两套指纹的 userId 都取自 creds.uid，
    // 空值会让服务端无法把事件归属到账号，上报必定不计分。此前这类上报照发
    // 且被计入 ok_codes，结果行谎报「已上报 N 个任务判据」——实际一个都没生效
    if !reports_effective(&creds.uid) {
        return (
            "skip".into(),
            "缺少 uid，跳过任务判据上报（空 userId 不计分）".into(),
        );
    }
    let mut ok_codes: Vec<&str> = Vec::new();
    let mut failed: Vec<String> = Vec::new();
    for t in tasks {
        let code = task_code_of(t);
        if code.is_empty() {
            continue;
        }
        let accept_status = t
            .get("accept_status")
            .filter(|v| !v.is_null())
            .or_else(|| fs_utils::dig(t, &["acceptStatus", "status"]))
            .map(value_display)
            .unwrap_or_default();
        let claimed = ["claimed", "已领取"].contains(&accept_status.as_str());
        let (cur, tgt) = task_progress(t);
        let met = tgt > 0.0 && cur >= tgt;
        if claimed || met {
            continue;
        }
        let ms = chrono::Local::now().timestamp_millis();
        let desktop = |evs| wb_report::report_desktop(agent, creds, evs).map(|_| ());
        let res: Result<(), String> = match code {
            // 对话活跃上报补足差额（每条独立会话 id）
            "chat_5" => {
                let need = if tgt > 0.0 { ((tgt - cur).max(0.0)).round() as usize } else { 5 };
                let mut first_err: Option<String> = None;
                for i in 0..need {
                    let conv = format!("aiwork-chat5-{ms}-{i}");
                    let (k, m) = report_chat_event(
                        agent, headers, urls, &creds.uid, &conv,
                        "deepseek-v4-flash", "DeepSeek V4 Flash",
                    );
                    if k == "auth" {
                        return ("auth".into(), m);
                    }
                    if k == "fail" {
                        first_err.get_or_insert(m);
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(300));
                }
                match first_err {
                    Some(e) => Err(e),
                    None => Ok(()),
                }
            }
            // 领养：活跃上报解锁 → 同意协议 → 领取
            "first_buddy" => {
                let conv = format!("aiwork-adopt-{ms}");
                let (k, m) = report_chat_event(
                    agent, headers, urls, &creds.uid, &conv,
                    "deepseek-v4-flash", "DeepSeek V4 Flash",
                );
                if k == "auth" {
                    return ("auth".into(), m);
                }
                std::thread::sleep(std::time::Duration::from_millis(800));
                let (st1, _b1, raw1) =
                    wb_common::post_json_raw(agent, &urls.buddy_agreement, headers, &json!({}));
                if st1 == 401 {
                    return ("auth".into(), "登录态失效（401）".into());
                }
                // 协议是领取的前置条件：它没生效时下游必然回
                // 「please accept buddy adoption agreement first」，把失败定位到
                // 真因（此前 st1 的结果被丢弃，用户只看到下游那句误导性的 400）
                if !(200..=201).contains(&st1) {
                    Err(format!(
                        "同意领养协议失败 HTTP {st1} {}",
                        body_head(&raw1)
                    ))
                } else {
                    let (st2, _b2, raw2) =
                        wb_common::post_json_raw(agent, &urls.buddy_first, headers, &json!({}));
                    if st2 == 401 {
                        return ("auth".into(), "登录态失效（401）".into());
                    }
                    if (200..=201).contains(&st2) {
                        Ok(())
                    } else {
                        Err(format!("领取 Buddy HTTP {st2} {}", body_head(&raw2)))
                    }
                }
            }
            // 桌面端完整对话链
            "RichMeow_Chat" => desktop(wb_report::chat_sequence(
                &format!("aiwork-rm-{ms}"),
                &format!("aiwork-rm-req-{ms}"),
                &format!("req-{ms}-user"),
                "fast-model",
                "fast-model",
            )),
            // 进入 Buddy 应用五连事件（同覆盖 Buddy_App_QQ）
            "Buddy_App" | "Buddy_App_QQ" => {
                desktop(wb_report::buddy_app_sequence("cb_y5Dy46tPQGGWtueMxXbe", "企鹅教师助手"))
            }
            "automation_1" => desktop(vec![wb_report::automation_create_event("aiwork 自动化")]),
            // Web 页面行为：资料库介绍点击
            "Library_read" => wb_report::report_web_event(
                agent,
                creds,
                "web_element_click",
                "https://www.workbuddy.cn/space/d/o0KWYeynteVv06UnAZqIFm",
                "library_doc_intro_click",
                "WorkBuddy资料库介绍",
            )
            .map(|_| ()),
            // 模板使用 ×5（不同模板 id）
            "template_5" => {
                let need = if tgt > 0.0 { ((tgt - cur).max(0.0)).round() as usize } else { 5 };
                let mut first_err: Option<String> = None;
                for i in 0..need.max(1) {
                    let evs = wb_report::template_use_sequence(
                        &format!("aiwork-tpl-{ms}-{i}"),
                        &format!("aiwork-tpl-req-{ms}-{i}"),
                        &format!("tpl-{i}"),
                        &format!("模板 {i}"),
                    );
                    if let Err(e) = desktop(evs) {
                        first_err.get_or_insert(e);
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(300));
                }
                match first_err {
                    Some(e) => Err(e),
                    None => Ok(()),
                }
            }
            "playbook_prompt" => desktop(wb_report::playbook_prompt_sequence(
                &format!("aiwork-pb-{ms}"),
                &format!("aiwork-pb-req-{ms}"),
                "pb-aiwork",
                "灵感案例",
            )),
            "create_canvas" => desktop(wb_report::canvas_sequence(
                &format!("aiwork-cv-{ms}"),
                &format!("aiwork-cv-req-{ms}"),
            )),
            // 其余任务需要真实对话/专家链路，留待下一批
            _ => continue,
        };
        match res {
            Ok(()) => ok_codes.push(code),
            Err(e) => failed.push(format!("{code}: {e}")),
        }
    }
    if ok_codes.is_empty() && failed.is_empty() {
        return ("skip".into(), "判据事件无需补齐".into());
    }
    let mut msg = String::new();
    if !ok_codes.is_empty() {
        msg.push_str(&format!("已上报 {} 个任务判据（{}）", ok_codes.len(), ok_codes.join("/")));
    }
    if !failed.is_empty() {
        if !msg.is_empty() {
            msg.push_str("；");
        }
        msg.push_str(&format!("失败 {}：{}", failed.len(), failed.join("；")));
    }
    let kind = if ok_codes.is_empty() { "fail" } else { "ok" };
    (kind.into(), msg)
}

/// 成长任务：拉列表 → 未接受则 accept → 进度达标且未领取则 **Web 域 claim 领奖**。
/// accept 只是「接任务」，奖励必须另走 claim 才会到账（此前只 accept 不 claim，
/// 奖励从未落袋）。
fn growth_tasks(
    agent: &ureq::Agent,
    headers: &[(String, String)],
    urls: &Urls,
) -> (String, String) {
    // 领奖走 Web 域：在（可能刚刷新过的）账号头上补 Web 端标记
    let web_headers = wb_common::web_flavor(headers, urls.web_base());
    let tasks = match fetch_tasks(agent, headers, urls) {
        Ok(t) => t,
        Err((401, msg)) => return ("auth".into(), msg),
        Err((_, msg)) => return ("fail".into(), msg),
    };
    let mut accepted = 0usize;
    let mut claimed = 0usize;
    let mut skipped = 0usize;
    let mut credit_total = 0.0;
    let mut energy_total = 0.0;
    for t in &tasks {
        let Some(obj) = t.as_object() else { continue };
        if !py_truthy(obj.get("has_reward")) && fs_utils::dig(t, &["hasReward"]).is_none() {
            continue;
        }
        let code = obj
            .get("task_code")
            .filter(|v| !v.is_null())
            .or_else(|| obj.get("taskCode").filter(|v| !v.is_null()))
            .or_else(|| obj.get("code").filter(|v| !v.is_null()))
            .and_then(|v| v.as_str().map(str::to_string));
        // accept_status：顶层缺省时 dig 兜底；空值归一 ""（对齐 python `or ""` + str()）
        let accept_status = obj
            .get("accept_status")
            .filter(|v| !v.is_null())
            .or_else(|| fs_utils::dig(t, &["acceptStatus", "status"]))
            .map(value_display)
            .unwrap_or_default();
        let already_claimed = ["claimed", "已领取"].contains(&accept_status.as_str());
        let accepted_already =
            ["1", "2", "accepted", "true", "True"].contains(&accept_status.as_str());

        // 1) 未接受 → accept（幂等）
        if !already_claimed && !accepted_already {
            let accept_body = match &code {
                Some(c) => json!({"task_code": c}),
                None => json!({}),
            };
            let (st, _bb, _r) =
                wb_common::post_json_raw(agent, &urls.tasks_accept, headers, &accept_body);
            if st == 401 {
                return ("auth".into(), format!("登录态失效（401，已领 {claimed} 项）"));
            }
            if (200..=201).contains(&st) {
                accepted += 1;
            }
        }

        // 2) 进度达标且未领取 → Web 域 claim
        let (cur, tgt) = task_progress(t);
        let done = tgt > 0.0 && cur >= tgt;
        if already_claimed || !done {
            skipped += 1;
            continue;
        }
        let Some(code) = code else { continue };
        match claim_task(agent, &web_headers, urls, &code) {
            Ok((c, e)) => {
                claimed += 1;
                credit_total += c;
                energy_total += e;
            }
            Err(401) => {
                return ("auth".into(), format!("登录态失效（401，已领 {claimed} 项）"));
            }
            Err(_) => continue, // 单项失败不影响其余任务
        }
    }
    let mut parts: Vec<String> = Vec::new();
    if accepted > 0 {
        parts.push(format!("接受 {accepted} 项"));
    }
    if claimed > 0 {
        parts.push(format!(
            "领取 {claimed} 项（+{:.0} 积分/+{:.0} 能量）",
            credit_total, energy_total
        ));
    }
    if parts.is_empty() {
        return ("skip".into(), format!("暂无可领取奖励（已领/未达标 {skipped} 项）"));
    }
    ("ok".into(), parts.join("，"))
}

/// 连登档位兑换：读 streak 的 `redemption_status` → 对「未锁定且未兑换」的档位
/// 逐个 POST redeem。未解锁返回 403「连续登录天数不足」属预期，静默跳过。
fn growth_redeem(agent: &ureq::Agent, headers: &[(String, String)], urls: &Urls) -> (String, String) {
    let (st, body, raw) = wb_common::get_json(agent, &urls.streak, headers);
    if st == 401 {
        return ("auth".into(), "登录态失效（401）".into());
    }
    if st != 200 || !body.as_ref().is_some_and(|b| b.is_object()) {
        let head: String = raw.chars().take(60).collect();
        return (
            "fail".into(),
            if st != 0 { format!("streak 不可用（HTTP {st}）") } else { format!("streak 不可达: {head}") },
        );
    }
    let b = body.as_ref().unwrap();
    // 档位清单：tiers[].tier 优先；缺省回退固定三档
    let mut tiers: Vec<String> = fs_utils::dig(b, &["tiers"])
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|t| t.get("tier").and_then(|v| v.as_str()).map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    if tiers.is_empty() {
        tiers = vec!["7d".into(), "14d".into(), "28d".into()];
    }
    let mut done = 0usize;
    let mut skipped = 0usize;
    for tier in &tiers {
        // 状态键：tier_7d_status / tier_14d_status / tier_28d_status
        let key = format!("tier_{}_status", tier.replace('-', "_"));
        let status = fs_utils::dig(b, &[key.as_str()])
            .map(value_display)
            .unwrap_or_default()
            .to_ascii_lowercase();
        // locked / claimed 明确不可兑换；其余（含空值）尝试一次，403 属预期
        if status == "locked" || status == "claimed" {
            skipped += 1;
            continue;
        }
        let payload = json!({"tier": tier, "client_token": uuid::Uuid::new_v4().to_string()});
        let (st, _rb, _rr) = wb_common::post_json_raw(agent, &urls.streak_redeem, headers, &payload);
        if st == 401 {
            return ("auth".into(), format!("登录态失效（401，已兑换 {done} 档）"));
        }
        if (200..=201).contains(&st) {
            done += 1;
        } else {
            skipped += 1; // 403 未满天数 / 已兑换：预期内
        }
    }
    if done == 0 {
        return ("skip".into(), format!("连登档位暂不可兑换（跳过 {skipped} 档）"));
    }
    ("ok".into(), format!("兑换连登档位 {done} 个"))
}

/// 对话活跃上报（`POST /v2/report`，事件码 chat_request_send）：
/// 一条即点亮当日连登，并解锁「领养 Buddy」等成长任务前置。每号每天 1 次。
/// `userId` 必填——缺失时上游 200 但静默丢弃。
fn growth_report(
    agent: &ureq::Agent,
    headers: &[(String, String)],
    urls: &Urls,
    uid: &str,
) -> (String, String) {
    let now = chrono::Utc::now().timestamp_millis();
    report_chat_event(agent, headers, urls, uid, &format!("aiwork-{now}"), "deepseek-v4-flash", "DeepSeek V4 Flash")
}

/// 事件上报能否真正计分。
///
/// 桌面包（`wb_report::desktop_fingerprint`）与 Web 包（`report_web_event`）
/// 的 `userId` 均取自 `creds.uid`；空值时服务端无法把事件归属到账号，
/// 上报必定不计分。宁可跳过也不能发出去——否则结果行会谎报成功。
fn reports_effective(uid: &str) -> bool {
    !uid.trim().is_empty()
}

/// 响应体摘要（错误定位用）：截断到 60 字符，防超长响应体污染结果行
fn body_head(raw: &str) -> String {
    raw.chars().take(60).collect()
}

/// 单条对话活跃事件上报（可指定会话 id 与模型，供 chat_5 / 模型体验类任务复用）
fn report_chat_event(
    agent: &ureq::Agent,
    headers: &[(String, String)],
    urls: &Urls,
    uid: &str,
    conv: &str,
    model_id: &str,
    model_name: &str,
) -> (String, String) {
    if !reports_effective(uid) {
        return ("skip".into(), "缺少 uid，跳过活跃上报".into());
    }
    let now = chrono::Utc::now().timestamp_millis();
    let event = json!({
        "eventCode": "chat_request_send",
        "timestamp": now,
        "reportDelay": 0,
        "mode": "craft",
        "conversationId": conv,
        "requestId": conv,
        "inputLength": 12,
        "requestModelId": model_id,
        "requestModelName": model_name,
        "isPlan": false,
        "isAutoExecuteTerminal": false,
        "isAutoModify": false,
        "codebaseEnable": false,
        "maxToken": 0,
        "maxSteps": 0,
        "temperature": 0,
        "maxRetries": 0,
        "mentionContexts": [],
        "knowledgeId": [],
        "knowledgeName": [],
        "codebaseId": "",
        "mentionContextCount": 0,
        "command": "",
        "expertId": "",
        "recommendId": "",
        "skillId": "",
        "skillCount": 0,
        "totalCount": 0,
        "fileUri": "",
        "presentAt": now,
        "traceId": "",
        "rootRequestId": conv,
        "parentConversationId": conv,
        "agentName": "default",
        "agentType": "conversation",
        "userId": uid,
    });
    let (st, _b, raw) = wb_common::post_json_raw(agent, &urls.report, headers, &json!([event]));
    if st == 401 {
        return ("auth".into(), "登录态失效（401）".into());
    }
    if (200..=201).contains(&st) {
        return ("ok".into(), "已上报对话活跃".into());
    }
    ("fail".into(), format!("活跃上报失败（HTTP {st}）{}", body_head(&raw)))
}

/// 能量与连签天数（页面附注展示；对象响应归一为标量，防 [object Object]）
fn growth_info(agent: &ureq::Agent, headers: &[(String, String)], urls: &Urls) -> Vec<(&'static str, f64)> {
    let mut info = vec![];
    let (st, b, _r) = wb_common::get_json(agent, &urls.energy, headers);
    if st == 200 {
        if let Some(v) = b
            .filter(|bb| bb.is_object())
            .and_then(|bb| fs_utils::dig(&bb, &["energy", "value", "balance", "num"]).cloned())
            .and_then(|v| scalar_of(&v))
        {
            info.push(("energy", v));
        }
    }
    let (st, b, _r) = wb_common::get_json(agent, &urls.streak, headers);
    if st == 200 {
        if let Some(v) = b
            .filter(|bb| bb.is_object())
            .and_then(|bb| fs_utils::dig(&bb, &["streak", "days", "continuous_days", "count"]).cloned())
            .and_then(|v| scalar_of(&v))
        {
            info.push(("streak", v));
        }
    }
    info
}

/// 单账号成长中心链式执行（旅行→盲盒→任务，各步独立容错；401 刷新一次重试）
fn process_account_growth(state: &AppState, agent: &ureq::Agent, acct: &Value, flags: &GrowthOpts) -> Value {
    let aid = s_of(acct.get("id"));
    let name = first_str(&[
        acct.get("nickname").and_then(Value::as_str).map(str::to_string),
        Some(s_of(acct.get("uid")).chars().take(8).collect()),
        Some(aid.clone()),
    ]);
    let base_ev = json!({"type": "growth", "user_id": aid, "name": name});
    let (creds, _refreshed, note) = wb_common::ensure_fresh(state, agent, acct, 24);
    if !matches!(note, "fresh" | "refreshed" | "no_credential") {
        return growth_stopped(&base_ev, renewal_note_message(state, &aid, note));
    }
    if creds.access_token.is_empty() {
        return json!({ "type": "growth", "user_id": aid, "name": base_ev["name"], "status": "fail", "message": format!("无可用凭证（{note}）") });
    }
    // 企业号无个人成长体系：growth 系列端点一律 403「growth system is only
    // available for personal users」，整体跳过而非逐项空发失败请求
    if wb_common::is_enterprise_account(&creds) {
        sync_pool_enterprise(state, &aid, true);
        return json!({
            "type": "growth", "user_id": aid, "name": base_ev["name"], "status": "ok",
            "tasks": "企业号无个人成长体系，已跳过"
        });
    }
    let urls = urls_for(wb_common::region_billing_base(&creds.domain));

    let mut result = Map::new();
    if flags.travel {
        let (kind, msg) = with_retry(state, agent, &aid, &creds, |h| growth_travel(agent, h, &urls));
        if kind == "stop" { return growth_stopped(&base_ev, msg); }
        result.insert("travel".into(), json!(if kind == "fail" { "fail" } else { msg.as_str() }));
    }
    if flags.lottery {
        let (kind, msg) = with_retry(state, agent, &aid, &creds, |h| growth_lottery(agent, h, &urls));
        if kind == "stop" { return growth_stopped(&base_ev, msg); }
        result.insert("lottery".into(), json!(if kind == "fail" { "fail" } else { msg.as_str() }));
    }
    if flags.tasks {
        // 成长任务链路：活跃上报（点亮连登）→ 连登档位兑换 → 任务接受/领奖。
        // 三步同属成长中心维护，随 growth_tasks 开关联动，避免再增一组设置项
        let (k1, m1) = with_retry(state, agent, &aid, &creds, |h| {
            growth_report(agent, h, &urls, &creds.uid)
        });
        if k1 == "stop" { return growth_stopped(&base_ev, m1); }
        result.insert("report".into(), json!(if k1 == "fail" { "fail" } else { m1.as_str() }));
        let (k2, m2) = with_retry(state, agent, &aid, &creds, |h| growth_redeem(agent, h, &urls));
        if k2 == "stop" { return growth_stopped(&base_ev, m2); }
        result.insert("redeem".into(), json!(if k2 == "fail" { "fail" } else { m2.as_str() }));
        // 判据事件补齐：先拉任务列表，按任务类型构造对应指纹的事件链上报；
        // 上报与计分异步，等待片刻再交给下面的 accept/claim 步骤
        let (k3, m3) = with_retry(state, agent, &aid, &creds, |h| {
            match fetch_tasks(agent, h, &urls) {
                Ok(tasks) => sync_task_reports(agent, h, &creds, &urls, &tasks),
                Err((401, msg)) => ("auth".into(), msg),
                Err((_, msg)) => ("fail".into(), msg),
            }
        });
        if k3 == "stop" { return growth_stopped(&base_ev, m3); }
        result.insert("actions".into(), json!(if k3 == "fail" { "fail" } else { m3.as_str() }));
        std::thread::sleep(std::time::Duration::from_millis(1500));
        let (kind, msg) = with_retry(state, agent, &aid, &creds, |h| growth_tasks(agent, h, &urls));
        if kind == "stop" { return growth_stopped(&base_ev, msg); }
        result.insert("tasks".into(), json!(if kind == "fail" { "fail" } else { msg.as_str() }));
    }
    // 能量/连签（不带刷新重试，纯展示）
    let current = wb_common::effective_creds(state, &aid, &creds.uid);
    let headers = wb_common::build_auth_headers(&current, false);
    for (k, v) in growth_info(agent, &headers, &urls) {
        result.insert(k.to_string(), json!(v));
    }

    let fails = ["travel", "lottery", "report", "redeem", "actions", "tasks"]
        .iter()
        .filter(|k| result.get(**k).and_then(Value::as_str) == Some("fail"))
        .count();
    let status = if fails > 0 { "fail" } else { "ok" };
    let mut ev = json!({ "type": "growth", "user_id": aid, "name": base_ev["name"], "status": status });
    for (k, v) in result {
        ev[k.as_str()] = v;
    }
    ev
}

/// 成长中心整轮：NDJSON 输出（wb-checkin-progress 管线复用）
pub fn run_growth_round(state: &AppState, flags: &GrowthOpts, uids: &[String], emit: &mut dyn FnMut(&Value)) {
    let agent = http_agent(30);
    let pool: Value = crate::store::docs::wb_pool_load(&crate::store::db(&state.data_dir));
    let mut accounts: Vec<Value> = pool
        .get("accounts")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if !uids.is_empty() {
        accounts.retain(|a| uids.iter().any(|u| s_of(a.get("id")) == *u));
    }
    emit(&json!({"type": "start", "total": accounts.len(), "mode": "growth"}));
    // 多账号成长间隔（与签到共用 wb_checkin_gap_secs，默认 3s，0=关闭）：防上游频控
    let gap_secs = crate::models::effective_checkin_gap(state.settings().wb_checkin_gap_secs);
    for (i, acct) in accounts.iter().enumerate() {
        if i > 0 && gap_secs > 0 {
            std::thread::sleep(std::time::Duration::from_secs(gap_secs));
        }
        let mut ev = process_account_growth(state, &agent, acct, flags);
        ev["index"] = json!(i + 1);
        emit(&ev);
    }
    emit(&json!({"type": "done", "mode": "growth"}));
}

// ── token 每周兜底续期（F-09，schtasks / --task-run wb-renew）──────────────

/// 检查 AT/RT 临期；lazy_hours=0 仅刷新到期项。
/// 返回末行 JSON 摘要（零 token 输出）。
pub fn run_renew_only(state: &AppState, lazy_hours: i64) -> Result<Value, String> {
    let agent = http_agent(30);
    let pool: Value = crate::store::docs::wb_pool_load(&crate::store::db(&state.data_dir));
    let accounts: Vec<Value> = pool
        .get("accounts")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut items: Vec<Value> = vec![];
    for acct in &accounts {
        let aid = s_of(acct.get("id"));
        if aid.is_empty() {
            continue;
        }
        let (creds, refreshed, note) = wb_common::ensure_fresh(state, &agent, acct, lazy_hours);
        if refreshed {
            sync_pool_expiry(state, &aid, &creds);
        }
        let mut item = json!({"user_id": aid, "refreshed": refreshed, "note": note});
        if creds.access_token.is_empty() {
            item["note"] = json!("no_credential");
        }
        items.push(item);
    }
    renewal_outcome(items)
}

fn renewal_outcome(items: Vec<Value>) -> Result<Value, String> {
    let count = |notes: &[&str]| items.iter().filter(|item| notes.contains(&item["note"].as_str().unwrap_or_default())).count();
    let refreshed = count(&["refreshed"]);
    let fresh = count(&["fresh"]);
    let auth_required = count(&["auth_required", "expired_needs_relogin", "no_credential"]);
    let retryable = items.len().saturating_sub(refreshed + fresh + auth_required);
    if retryable > 0 {
        return Err(format!("WB 续期：已续期 {refreshed}，无需续期 {fresh}，需重登 {auth_required}，暂时失败 {retryable}（稍后自动重试）"));
    }
    Ok(json!({"mode": "renew", "ok": auth_required == 0, "finished_at": fs_utils::now_ts(),
        "refreshed": refreshed, "fresh": fresh, "auth_required": auth_required, "retryable": retryable, "accounts": items}))
}

#[cfg(test)]
mod wb_checkin_tests {
    use super::*;

    #[test]
    fn session_renewal_temporary_failures_remain_retryable() {
        for note in ["refresh_failed", "refresh_cooldown", "refresh_busy", "persist_failed"] {
            assert!(renewal_outcome(vec![json!({"note":"fresh"}), json!({"note":note})]).is_err());
        }
    }

    #[test]
    fn session_renewal_permanent_failure_completes_without_claiming_success() {
        let result = renewal_outcome(vec![json!({"note":"refreshed"}), json!({"note":"fresh"}), json!({"note":"auth_required"})]).unwrap();
        assert_eq!(result["ok"], false);
        assert_eq!(result["refreshed"], 1);
        assert_eq!(result["fresh"], 1);
        assert_eq!(result["auth_required"], 1);
        assert_eq!(result["retryable"], 0);
        assert_eq!(renewal_outcome(vec![]).unwrap()["ok"], true);
    }

    #[test]
    fn session_growth_stops_before_business_step_when_recovery_is_blocked() {
        let directory = std::env::temp_dir().join(format!("wb-growth-stop-{}-{}", std::process::id(), chrono::Utc::now().timestamp_nanos_opt().unwrap()));
        let store = crate::store::db(&directory);
        store.row_upsert("wb_accounts", "synthetic", &json!({"id":"synthetic", "uid":"synthetic-uid", "needs_relogin":true})).unwrap();
        let state = AppState {
            data_dir: directory,
            jwt_refresh_lock: std::sync::Arc::new(std::sync::Mutex::new(())),
            qoder_pool_lock: std::sync::Arc::new(std::sync::Mutex::new(())),
            wb_pool_lock: std::sync::Arc::new(std::sync::Mutex::new(())),
            doubao_pool_lock: std::sync::Arc::new(std::sync::Mutex::new(())),
            group_defs_lock: std::sync::Arc::new(std::sync::Mutex::new(())),
        };
        let creds = Creds {uid:"synthetic-uid".into(), access_token:"synthetic-access".into(), refresh_token:"synthetic-refresh".into(), ..Default::default()};
        wb_common::token_store_upsert_secure(&state.data_dir, "synthetic", &serde_json::to_value(&creds).unwrap()).unwrap();
        let result = process_account_growth(&state, &http_agent(1), &json!({"id":"synthetic", "uid":"synthetic-uid"}), &GrowthOpts {travel:true, lottery:true, tasks:true});
        assert_eq!(result["status"], "fail");
        assert!(result["message"].as_str().unwrap().contains("需重新登录"));
        assert!(result.get("travel").is_none());
        assert!(result.get("tasks").is_none());
    }

    /// 空 uid 的事件不可计分：桌面/Web 两套指纹的 userId 都取自 creds.uid，
    /// 缺失时上报无法归属到账号（此前照发并被计入成功，结果行谎报「已上报」）
    #[test]
    fn reports_effective_requires_uid() {
        assert!(reports_effective("u-1"));
        assert!(!reports_effective(""));
        assert!(!reports_effective("   "));
        assert!(!reports_effective(" "));
        // 两端空白不影响判定
        assert!(reports_effective(" u-1 "));
    }

    /// 响应摘要截断到 60 字符（防超长响应体污染结果行），按字符而非字节切
    #[test]
    fn body_head_truncates_by_chars() {
        assert_eq!(body_head(""), "");
        assert_eq!(body_head("short"), "short");
        let long = "x".repeat(200);
        assert_eq!(body_head(&long).chars().count(), 60);
        // 多字节字符不被切成半个：中文按字符计数
        let zh = "错".repeat(100);
        assert_eq!(body_head(&zh).chars().count(), 60);
    }

    #[test]
    fn scalar_of_unwraps_common_keys() {
        // {current:..} 等对象返回归一为标量（修复前端 [object Object] 展示）
        assert_eq!(scalar_of(&json!({"current": 5})), Some(5.0));
        assert_eq!(scalar_of(&json!({"streak": {"days": "3"}})), Some(3.0));
        assert_eq!(scalar_of(&json!(7)), Some(7.0));
        assert_eq!(scalar_of(&json!("2.5")), Some(2.5));
        assert_eq!(scalar_of(&json!({"other": 1})), None);
        assert_eq!(scalar_of(&json!({"current": null})), None);
    }

    #[test]
    fn num_or_none_rejects_bools_and_garbage() {
        assert_eq!(num_or_none(Some(&json!(true))), None);
        assert_eq!(num_or_none(Some(&json!(10))), Some(10.0));
        assert_eq!(num_or_none(Some(&json!("12.5"))), Some(12.5));
        assert_eq!(num_or_none(Some(&json!("abc"))), None);
        assert_eq!(num_or_none(Some(&json!({"a": 1}))), None);
    }

    #[test]
    fn py_truthy_matches_python_bool_semantics() {
        assert!(!py_truthy(Some(&json!(0))));
        assert!(py_truthy(Some(&json!(1))));
        assert!(!py_truthy(Some(&json!(""))));
        assert!(py_truthy(Some(&json!("false")))); // python bool("false") == True
        assert!(!py_truthy(Some(&json!(null))));
        assert!(py_truthy(Some(&json!({"a": 1}))));
    }

    #[test]
    fn urls_cover_all_endpoints_and_alt_region() {
        let u = urls_for(wb_common::BILLING_BASE_CN);
        assert!(u.checkin_do.starts_with(wb_common::BILLING_BASE_CN));
        assert!(u.travel_status.contains("/v2/activity/growth/buddy/travel/status"));
        let alt = u.alt();
        assert_eq!(alt.base, wb_common::BILLING_BASE_GLOBAL);
        assert_ne!(alt.checkin_do, u.checkin_do);
    }

    /// 领奖走 Web 域：urls.web_base() 与 billing 基址不同域（CN=workbuddy.cn）
    #[test]
    fn claim_uses_web_domain_not_billing_domain() {
        let u = urls_for(wb_common::BILLING_BASE_CN);
        assert_eq!(u.web_base(), wb_common::WEB_BASE_CN);
        assert_eq!(wb_common::WEB_BASE_CN, "https://www.workbuddy.cn");
        assert!(u.streak_redeem.ends_with("/v2/activity/growth/redeem"));
        assert!(u.report.ends_with("/v2/report"));
    }

    /// 任务进度：progress 对象优先于平铺字段，字符串数字同样解析
    #[test]
    fn task_progress_prefers_progress_object() {
        let obj = json!({"progress": {"current": 3, "target": 5}, "current": 0, "target": 0});
        assert_eq!(task_progress(&obj), (3.0, 5.0));
        let flat = json!({"current": "2", "target": "4"});
        assert_eq!(task_progress(&flat), (2.0, 4.0));
        let none = json!({});
        assert_eq!(task_progress(&none), (0.0, 0.0));
    }

    /// 任务码路径段编码：合法字符原样，非法字符百分号转义
    #[test]
    fn encode_path_seg_escapes_unsafe_chars() {
        assert_eq!(encode_path_seg("Model_chat_GLM5.2"), "Model_chat_GLM5.2");
        assert_eq!(encode_path_seg("a/b c"), "a%2Fb%20c");
    }

    #[test]
    fn reward_text_prefers_interface_values() {
        assert_eq!(reward_text(Some(&json!({"reward": 10})), ""), "+10");
        assert_eq!(reward_text(Some(&json!({"reward": 1.5})), ""), "+1.5");
        assert_eq!(reward_text(Some(&json!({})), "oops"), "oops");
        assert_eq!(reward_text(Some(&json!({})), ""), "ok");
    }
}
