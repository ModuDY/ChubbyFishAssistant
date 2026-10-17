//! 只读网关接口：存活探测、账号池诊断和模型目录。
use super::{dispatch, unified_catalog, ApiSharedState};
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};
use std::sync::Arc;

pub async fn health(State(state): State<Arc<ApiSharedState>>) -> impl IntoResponse {
    // 收敏（2026-10-12 审查 P2）：/health 在 auth.rs 免鉴权放行（探活豁免），
    // 原实现透出 total_credits/active_uid/last_error/池账号数——非环回监听时
    // 局域网内无 Key 即可抓取账号与积分画像，与「非环回强制鉴权」防线矛盾。
    // 探活只需存活布尔；完整诊断由需鉴权的 GET /status 承载（字段超集）
    let _ = &state;
    Json(json!({
        "status": "ok",
        "running": true,
    }))
}

/// /healthz（T2.3/F-32）：三池均无可用账号 → 503，供探活/看门狗。
/// H1 修复（审查）：旧口径仅 Trae ∨ WB 且不查 hard_credit/积分过期/零积分——
/// 仅部署 Qoder 账号或全部账号 hard_credit 冷却时探活恒 503（可能触发误重启）。
/// 改用 has_selectable()（= pool::selectable 口径：健康 + 未禁用 + 未冷却 +
/// 积分未过期且非零），三池 × enabled 开关，与 /v1/models 徽章判定同源
pub async fn healthz(State(state): State<Arc<ApiSharedState>>) -> Response {
    let trae_enabled = state
        .trae_enabled
        .load(std::sync::atomic::Ordering::Relaxed);
    let solo_ok = trae_enabled && state.pool.has_selectable();
    let wb_enabled = state.wb_enabled.load(std::sync::atomic::Ordering::Relaxed);
    let wb_ok = wb_enabled && state.wb_pool.has_selectable();
    let qoder_enabled = state
        .qoder_enabled
        .load(std::sync::atomic::Ordering::Relaxed);
    let qoder_ok = qoder_enabled && state.qoder_pool.has_selectable();
    if solo_ok || wb_ok || qoder_ok {
        (
            StatusCode::OK,
            axum::Json(json!({
                "status": "ok",
                "solo_available": solo_ok,
                "wb_available": wb_ok,
                "qoder_available": qoder_ok,
            })),
        )
            .into_response()
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            axum::Json(json!({ "status": "unavailable", "reason": "no healthy account" })),
        )
            .into_response()
    }
}

pub async fn status(State(state): State<Arc<ApiSharedState>>) -> impl IntoResponse {
    let pool = state.pool.status_list();
    let now: i64 = chrono::Utc::now().timestamp();
    let total = state
        .total_requests
        .load(std::sync::atomic::Ordering::Relaxed);
    let active = state
        .active_uid
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let last_err = state
        .last_error
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();

    // 汇总统计
    let total_accounts = pool.len();
    let available = pool.iter().filter(|p| !p.disabled && !p.cooling).count();
    let cooling = pool.iter().filter(|p| p.cooling).count();
    let disabled = pool.iter().filter(|p| p.disabled).count();
    let total_credits: f64 = pool.iter().filter_map(|p| p.credits).sum();
    let total_credits = (total_credits * 100.0).round() / 100.0;

    // 账号明细（inflight 为 F-77 账号级实时并发）
    let accounts: Vec<Value> = pool
        .iter()
        .map(|p| {
            let status = if p.disabled {
                "disabled"
            } else if p.cooling {
                "cooling"
            } else if p.credits_expire_at.map_or(false, |exp| exp < now) {
                "expired"
            } else if p.credits.map_or(false, |c| c <= 0.0) {
                "no_credits"
            } else {
                "available"
            };
            json!({
                "uid": p.uid,
                "name": p.name,
                "status": status,
                "credits": p.credits,
                "credits_expire_at": p.credits_expire_at,
                "cooling": p.cooling,
                "cooldown_until": p.cooldown_until,
                "cooldown_reason": p.cooldown_reason,
                "disabled": p.disabled,
                "err_count": p.err_count,
                "state": p.state,
                "inflight": p.inflight,
            })
        })
        .collect();

    // WB 池画像（T2.3/F-32；inflight 为 F-77 账号级实时并发）
    let wb_pool = state.wb_pool.status_list();
    let wb_enabled = state.wb_enabled.load(std::sync::atomic::Ordering::Relaxed);
    let wb_accounts: Vec<Value> = wb_pool
        .iter()
        .map(|p| {
            json!({
                "uid": p.uid, "name": p.name, "credits": p.credits,
                "cooling": p.cooling, "cooldown_until": p.cooldown_until,
                "cooldown_reason": p.cooldown_reason, "disabled": p.disabled,
                "err_count": p.err_count, "state": p.state, "inflight": p.inflight,
            })
        })
        .collect();
    // F-77：per-account 并发列表（活跃账号 [{uid, name, inflight}]），
    // 供前端池状态页实时展示 busy/idle 分布（active_uid 单值的超集）
    let active_accounts: Vec<Value> = pool
        .iter()
        .chain(wb_pool.iter())
        .filter(|p| p.inflight > 0)
        .map(|p| json!({ "uid": p.uid, "name": p.name, "inflight": p.inflight }))
        .collect();
    let model_cooldowns: Vec<Value> = {
        let map = state
            .model_cooldowns
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        map.iter()
            .filter(|(_, (until, _))| *until > now)
            .map(|(m, (until, fails))| {
                json!({
                    "model": m, "until": until, "remaining_s": until - now, "fails": fails,
                })
            })
            .collect()
    };

    Json(json!({
        "running": true,
        "total_requests": total,
        // 当前并发数（统一网关 §4.5）：InflightGuard RAII 维护，覆盖 6 业务端点
        "inflight": state.inflight.load(std::sync::atomic::Ordering::Relaxed),
        "active_uid": active,
        // F-77：per-account 并发列表（busy 账号实时画像）
        "active_accounts": active_accounts,
        "last_error": last_err,
        "summary": {
            "total_accounts": total_accounts,
            "available": available,
            "cooling": cooling,
            "disabled": disabled,
            "total_credits": total_credits,
        },
        "accounts": accounts,
        "wb": {
            "enabled": wb_enabled,
            "total_accounts": wb_pool.len(),
            "accounts": wb_accounts,
            "model_cooldowns": model_cooldowns,
            "sticky_sessions": state.wb_sticky.len(),
            // 上游健康探针（F-34 ④/§2.2）：-1 未探测 / 0 不可达 / 1 在线
            "probe_ok": state.wb_probe_ok.load(std::sync::atomic::Ordering::Relaxed),
            "probe_ts_ms": state.wb_probe_ts_ms.load(std::sync::atomic::Ordering::Relaxed),
        },
    }))
}

pub async fn models(State(state): State<Arc<ApiSharedState>>) -> impl IntoResponse {
    // 统一模型目录（§3.4）：实时聚合 data/api_models.json（Trae，元数据四层链）
    // 与 data/wb_model_catalog.json（Buddy），纯派生不落盘。官网/目录同步后
    // 无需重启 API 服务即可通过 /v1/models 看到最新列表
    let wb_enabled = state.wb_enabled.load(std::sync::atomic::Ordering::Relaxed);
    // 可用性标记运行时派生（§3.3 #5）：HTTP 端点用实时池健康；
    // Trae 开关（trae_enabled，默认开）参与徽章判定——关闭时 Trae 源徽章置灰
    let trae_enabled = state
        .trae_enabled
        .load(std::sync::atomic::Ordering::Relaxed);
    let trae_ok = trae_enabled && state.pool.has_selectable();
    let buddy_ok = state.wb_pool.has_selectable();
    // Qoder 源旗标（p3-3）：条目始终入表（开关关闭时徽章置灰）
    let qoder_enabled = state
        .qoder_enabled
        .load(std::sync::atomic::Ordering::Relaxed);
    let qoder_ok = state.qoder_pool.has_selectable();
    // 豆包源旗标（E-01）：条目始终入表（开关关闭/占位模型时徽章置灰）
    let doubao_enabled = state
        .doubao_enabled
        .load(std::sync::atomic::Ordering::Relaxed);
    let doubao_ok = state.doubao_pool.has_selectable();
    let data_dir = state.data_dir.clone();
    // issue #90 修复③a：来源标识开关（dispatch_policy.model_source_labels）——
    // 开启后为多源同名模型附带池前缀别名条目（trae/xxx / buddy/xxx / qoder/xxx），
    // 客户端（如 CC Switch）可在同名模型间区分来源池（三家倍率与效果不同）
    let source_labels = dispatch::load_policy(&data_dir).model_source_labels;
    let list = tokio::task::spawn_blocking(move || {
        // issue #26：对外目录经全局白名单过滤（管理端 api_unified_models 不过滤）
        unified_catalog::unified_models_whitelisted_ex(
            &data_dir,
            wb_enabled,
            trae_ok,
            buddy_ok,
            Some((qoder_enabled, qoder_ok)),
            Some((doubao_enabled, doubao_ok)),
        )
    })
    .await
    .unwrap_or_default();
    // wb_enabled=false：仅 Buddy 源的模型过滤，双源模型保留（仍可由 Trae 源服务 §3.4）
    let mut data: Vec<Value> = list
        .iter()
        .filter(|m| wb_enabled || !m.sources.iter().all(|s| s.pool == "buddy"))
        .map(|m| {
            json!({
                "id": m.id,
                "object": "model",
                "created": 1753600000,
                "owned_by": "unified",
                "display": m.display,
                "rate": m.rate,
                "context_length": m.context_length,
                "max_tokens": m.max_tokens,
                "supports_image": m.supports_image,
                "supported_efforts": m.efforts,
                // Max Mode 支持（issue #38-1：UnifiedModel 已按支持表计算，
                // 此前手工序列化遗漏导致 release notes/手册声明的字段从未出现在响应中）
                "max_mode": m.max_mode,
                // 来源池集合：[{pool: "trae"|"buddy", rate, enabled}]（徽章/降级判定
                // 由客户端按元数据自决，勿硬编码 §3.4）
                "sources": m.sources,
                "manual": m.manual,
            })
        })
        .collect();
    // issue #90 修复③a：多源同名模型的池前缀别名条目（开关开启时追加，不改写
    // 原有 id——已有客户端零影响）。带前缀 id 请求经 resolve_target 锁定该池，
    // 白名单按裸名放行（whitelist_check 同步剥离）；别名条目随基础条目白名单
    // 过滤结果派生，不重复过滤
    if source_labels {
        for m in list.iter().filter(|m| m.sources.len() >= 2) {
            for s in m.sources.iter().filter(|s| s.pool != "custom") {
                data.push(json!({
                    "id": format!("{}/{}", s.pool, m.id),
                    "object": "model",
                    "created": 1753600000,
                    "owned_by": "unified",
                    "display": format!("[{}] {}", s.pool, m.display),
                    "rate": s.rate,
                    "context_length": m.context_length,
                    "max_tokens": m.max_tokens,
                    "supports_image": m.supports_image,
                    "supported_efforts": m.efforts,
                    "max_mode": m.max_mode,
                    "sources": m.sources,
                    // 别名标记：客户端可据此折叠展示；source_pool = 该 id 实际锁定的池
                    "alias_of": m.id,
                    "source_pool": s.pool,
                    "manual": m.manual,
                }));
            }
        }
    }
    Json(json!({ "object": "list", "data": data }))
}
