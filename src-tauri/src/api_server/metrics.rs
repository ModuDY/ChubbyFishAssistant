//! 网关观测层（自我进化 L1.5 观测 2.0）：按 `(pool, uid)` 维护分钟级指标。
//!
//! 定位：**只观测、不决策**。路由行为与现在完全一致，数据在内存里滚动 1 小时，
//! 每分钟落盘一次到 SQLite，重启后回填最近 60 分钟——先拿到真实分布，再谈
//! 自动调参（见 `docs/pending/自我进化方案.md` §5）。
//!
//! 指标口径：
//! - 每个序列（`{pool}:{uid}` 与池汇总 `{pool}:-`）一个 60 个 1 分钟桶的环；
//! - 首字 / 总耗时用对数分桶直方图近似 P50/P95（80 桶，50ms–64s）；
//! - `local_abort`（客户端断开 / 用户取消）**不计入**成功率；
//! - 对冲只记「触发 / 接管（胜出）」两个计数，不参与路由。
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use super::ErrKind;

/// 环形缓冲的分钟数：60 个 1 分钟桶 = 最近 1 小时
pub const WINDOW_BUCKETS: usize = 60;
/// 直方图桶数：50ms–64s 对数分桶，80 桶时相邻边界比约 1.094（半桶误差 < 5%）
const HIST_BUCKETS: usize = 80;
const HIST_MIN_MS: f64 = 50.0;
const HIST_MAX_MS: f64 = 64_000.0;
/// 落盘保留天数（决策数据，比 api_usage 的 90 天短）
pub const RETENTION_DAYS: i64 = 7;

fn now_minute() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64 / 60)
        .unwrap_or(0)
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 请求结果的观测归因（与 ErrKind 解耦：同一 ErrKind 在不同上下文可能不同）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Ok,
    /// 上游 5xx / 网关 502/503/504 等通用上游故障
    Upstream5xx,
    /// 首字超时 / 读超时
    Timeout,
    /// 限流：429 / SoftRate / PlanLimit / 积分耗尽
    RateLimited,
    /// 鉴权失败：401/403 / SessionDead / refresh_token 失效
    Auth,
    /// 本地中止：客户端断开、用户取消、网关停止——不计入成功率
    LocalAbort,
}

impl Outcome {
    pub const COUNT: usize = 6;

    fn index(self) -> usize {
        match self {
            Self::Ok => 0,
            Self::Upstream5xx => 1,
            Self::Timeout => 2,
            Self::RateLimited => 3,
            Self::Auth => 4,
            Self::LocalAbort => 5,
        }
    }

    pub fn is_success(self) -> bool {
        matches!(self, Self::Ok)
    }

    /// 从既有错误分类映射到观测归因（note_error 单一事实来源）
    pub fn from_err_kind(kind: ErrKind) -> Self {
        match kind {
            ErrKind::None => Self::Ok,
            ErrKind::Server => Self::Upstream5xx,
            ErrKind::PlanLimit | ErrKind::SoftRate | ErrKind::HardCredit => Self::RateLimited,
            // 404（模型/资源不存在）不是限流信号，归上游故障，避免虚高限流率
            ErrKind::NotFound => Self::Upstream5xx,
            ErrKind::SessionDead | ErrKind::Forbidden => Self::Auth,
            // 上游 4xx（非鉴权/非限流）：枚举无独立桶，归通用上游故障避免丢样本
            ErrKind::Client => Self::Upstream5xx,
        }
    }
}

/// 对数分桶直方图（单桶 4 字节 × 80 = 320B，合并/分位读取 O(桶数)）
#[derive(Clone, Copy)]
pub struct Histogram {
    buckets: [u32; HIST_BUCKETS],
}

impl Default for Histogram {
    fn default() -> Self {
        // 数组长度 80 无 `Default` 实现，显式全零初始化
        Self {
            buckets: [0; HIST_BUCKETS],
        }
    }
}

impl Histogram {
    fn bucket_index(ms: u64) -> usize {
        let v = (ms as f64).clamp(HIST_MIN_MS, HIST_MAX_MS);
        let ratio = (HIST_MAX_MS / HIST_MIN_MS).ln();
        let idx = ((v / HIST_MIN_MS).ln() / ratio * HIST_BUCKETS as f64).floor() as usize;
        idx.min(HIST_BUCKETS - 1)
    }

    /// 桶代表值（几何中点）：最大相对误差 ≈ (64s/50ms)^(1/160) - 1 ≈ 4.6%
    fn bucket_value(idx: usize) -> f64 {
        let ratio = (HIST_MAX_MS / HIST_MIN_MS).ln() / HIST_BUCKETS as f64;
        HIST_MIN_MS * ((idx as f64 + 0.5) * ratio).exp()
    }

    fn add(&mut self, ms: u64) {
        self.buckets[Self::bucket_index(ms)] += 1;
    }

    fn merge(&mut self, other: &Self) {
        for (a, b) in self.buckets.iter_mut().zip(other.buckets.iter()) {
            *a = a.saturating_add(*b);
        }
    }

    fn count(&self) -> u64 {
        self.buckets.iter().map(|&v| v as u64).sum()
    }

    /// 近似分位（最近邻上取整；无样本返回 None）
    fn percentile(&self, p: f64) -> Option<u64> {
        let total = self.count();
        if total == 0 {
            return None;
        }
        let target = (((p / 100.0) * total as f64).ceil() as u64).max(1);
        let mut seen = 0u64;
        for (idx, &n) in self.buckets.iter().enumerate() {
            seen += n as u64;
            if seen >= target {
                return Some(Self::bucket_value(idx).round() as u64);
            }
        }
        Some(HIST_MAX_MS as u64)
    }
}

/// 单个分钟桶
#[derive(Clone)]
pub struct MinuteBucket {
    pub minute_ts: i64,
    counts: [u32; Outcome::COUNT],
    ttft: Histogram,
    dur: Histogram,
    tokens: u64,
    hedge_fired: u32,
    hedge_won: u32,
}

impl Default for MinuteBucket {
    fn default() -> Self {
        Self {
            minute_ts: 0,
            counts: [0; Outcome::COUNT],
            ttft: Histogram::default(),
            dur: Histogram::default(),
            tokens: 0,
            hedge_fired: 0,
            hedge_won: 0,
        }
    }
}

impl MinuteBucket {
    fn bucket_mut(&mut self, minute: i64) -> &mut Self {
        if self.minute_ts != minute {
            *self = Self::default();
            self.minute_ts = minute;
        }
        self
    }

    fn record(&mut self, outcome: Outcome, ttft_ms: Option<u64>, duration_ms: u64, tokens: u64) {
        self.counts[outcome.index()] = self.counts[outcome.index()].saturating_add(1);
        if let Some(t) = ttft_ms {
            self.ttft.add(t);
        }
        self.dur.add(duration_ms);
        self.tokens = self.tokens.saturating_add(tokens);
    }

    fn to_row(&self, pool: &str, uid: &str) -> EvolutionMetricRow {
        EvolutionMetricRow {
            minute_ts: self.minute_ts * 60,
            pool: pool.to_string(),
            uid: uid.to_string(),
            ok: self.counts[Outcome::Ok.index()],
            upstream_5xx: self.counts[Outcome::Upstream5xx.index()],
            timeout: self.counts[Outcome::Timeout.index()],
            rate_limited: self.counts[Outcome::RateLimited.index()],
            auth: self.counts[Outcome::Auth.index()],
            local_abort: self.counts[Outcome::LocalAbort.index()],
            p50_ttft: self.ttft.percentile(50.0).unwrap_or(0),
            p95_ttft: self.ttft.percentile(95.0).unwrap_or(0),
            p50_dur: self.dur.percentile(50.0).unwrap_or(0),
            p95_dur: self.dur.percentile(95.0).unwrap_or(0),
            tokens: self.tokens,
            hedge_fired: self.hedge_fired,
            hedge_won: self.hedge_won,
        }
    }
}

/// 窗口聚合结果（多个分钟桶合并）
#[derive(Clone, Default)]
pub struct Agg {
    counts: [u32; Outcome::COUNT],
    ttft: Histogram,
    dur: Histogram,
    tokens: u64,
    hedge_fired: u32,
    hedge_won: u32,
}

impl Agg {
    fn add_bucket(&mut self, b: &MinuteBucket) {
        for (a, v) in self.counts.iter_mut().zip(b.counts.iter()) {
            *a = a.saturating_add(*v);
        }
        self.ttft.merge(&b.ttft);
        self.dur.merge(&b.dur);
        self.tokens = self.tokens.saturating_add(b.tokens);
        self.hedge_fired = self.hedge_fired.saturating_add(b.hedge_fired);
        self.hedge_won = self.hedge_won.saturating_add(b.hedge_won);
    }

    fn requests(&self) -> u64 {
        self.counts.iter().map(|&v| v as u64).sum()
    }

    fn success_rate(&self) -> f64 {
        let ok = self.counts[Outcome::Ok.index()] as f64;
        // local_abort 固定在最后一档：分母不含它
        let denom: u64 = self.counts.iter().take(Outcome::COUNT - 1).map(|&v| v as u64).sum();
        if denom == 0 {
            0.0
        } else {
            ok / denom as f64
        }
    }

    fn to_json(&self) -> serde_json::Value {
        let err = |o: Outcome| self.counts[o.index()];
        let p = |h: &Histogram, pct: f64| h.percentile(pct).map(|v| serde_json::json!(v)).unwrap_or(serde_json::Value::Null);
        // 分母为 0（窗口内只有 local_abort）时成功率无意义，返回 null 供面板显示「—」
        let denom: u64 = self
            .counts
            .iter()
            .take(Outcome::COUNT - 1)
            .map(|&v| v as u64)
            .sum();
        let success_rate = if denom == 0 {
            serde_json::Value::Null
        } else {
            serde_json::json!((self.success_rate() * 1000.0).round() / 1000.0)
        };
        serde_json::json!({
            "requests": self.requests(),
            "ok": self.counts[Outcome::Ok.index()],
            "successRate": success_rate,
            "errors": {
                "upstream5xx": err(Outcome::Upstream5xx),
                "timeout": err(Outcome::Timeout),
                "rateLimited": err(Outcome::RateLimited),
                "auth": err(Outcome::Auth),
                "localAbort": err(Outcome::LocalAbort),
            },
            "ttftP50": p(&self.ttft, 50.0),
            "ttftP95": p(&self.ttft, 95.0),
            "durP50": p(&self.dur, 50.0),
            "durP95": p(&self.dur, 95.0),
            "tokens": self.tokens,
            "hedgeFired": self.hedge_fired,
            "hedgeWon": self.hedge_won,
            "hedgeWinRate": if self.hedge_fired == 0 {
                serde_json::Value::Null
            } else {
                serde_json::json!(((self.hedge_won as f64 / self.hedge_fired as f64) * 1000.0).round() / 1000.0)
            },
        })
    }
}

/// 单序列的 1 小时环
pub struct Ring {
    buckets: Vec<MinuteBucket>,
}

impl Default for Ring {
    fn default() -> Self {
        Self {
            buckets: vec![MinuteBucket::default(); WINDOW_BUCKETS],
        }
    }
}

impl Ring {
    fn bucket_mut(&mut self, minute: i64) -> &mut MinuteBucket {
        let idx = (minute.rem_euclid(WINDOW_BUCKETS as i64)) as usize;
        self.buckets[idx].bucket_mut(minute)
    }

    fn aggregate(&self, now: i64, window_minutes: i64) -> Agg {
        let mut agg = Agg::default();
        for b in &self.buckets {
            if b.minute_ts > 0 && b.minute_ts > now - window_minutes {
                agg.add_bucket(b);
            }
        }
        agg
    }

    fn collect_rows(
        &self,
        after_minute: i64,
        before_minute: i64,
        pool: &str,
        uid: &str,
        out: &mut Vec<EvolutionMetricRow>,
    ) {
        for b in &self.buckets {
            if b.minute_ts > after_minute && b.minute_ts < before_minute {
                out.push(b.to_row(pool, uid));
            }
        }
    }

    fn merge_row(&mut self, row: &EvolutionMetricRow) {
        let minute = row.minute_ts / 60;
        let b = self.bucket_mut(minute);
        b.counts[Outcome::Ok.index()] = b.counts[Outcome::Ok.index()].saturating_add(row.ok);
        b.counts[Outcome::Upstream5xx.index()] =
            b.counts[Outcome::Upstream5xx.index()].saturating_add(row.upstream_5xx);
        b.counts[Outcome::Timeout.index()] =
            b.counts[Outcome::Timeout.index()].saturating_add(row.timeout);
        b.counts[Outcome::RateLimited.index()] =
            b.counts[Outcome::RateLimited.index()].saturating_add(row.rate_limited);
        b.counts[Outcome::Auth.index()] = b.counts[Outcome::Auth.index()].saturating_add(row.auth);
        b.counts[Outcome::LocalAbort.index()] =
            b.counts[Outcome::LocalAbort.index()].saturating_add(row.local_abort);
        b.tokens = b.tokens.saturating_add(row.tokens);
        b.hedge_fired = b.hedge_fired.saturating_add(row.hedge_fired);
        b.hedge_won = b.hedge_won.saturating_add(row.hedge_won);
        // 回填无法还原分布：分位代表值作单样本
        if row.p50_ttft > 0 {
            b.ttft.add(row.p50_ttft);
        }
        if row.p50_dur > 0 {
            b.dur.add(row.p50_dur);
        }
    }
}

/// 序列槽：池名 + uid + 环（取号后释放全局 map 锁）
struct SeriesSlot {
    pool: String,
    uid: String,
    ring: Mutex<Ring>,
}

/// 落盘行（与 SQLite 表列一一对应）
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EvolutionMetricRow {
    pub minute_ts: i64,
    pub pool: String,
    pub uid: String,
    pub ok: u32,
    pub upstream_5xx: u32,
    pub timeout: u32,
    pub rate_limited: u32,
    pub auth: u32,
    pub local_abort: u32,
    pub p50_ttft: u64,
    pub p95_ttft: u64,
    pub p50_dur: u64,
    pub p95_dur: u64,
    pub tokens: u64,
    pub hedge_fired: u32,
    pub hedge_won: u32,
}

/// 观测总表
#[derive(Default)]
pub struct Metrics {
    series: RwLock<HashMap<String, Arc<SeriesSlot>>>,
    /// 已落盘到的分钟（Unix 分钟）；启动回填后推进，避免重复写
    flushed_minute: AtomicI64,
}

fn series_key(pool: &str, uid: &str) -> String {
    format!("{pool}:{uid}")
}

impl Metrics {
    fn slot(&self, pool: &str, uid: &str) -> Arc<SeriesSlot> {
        let key = series_key(pool, uid);
        if let Some(s) = self
            .series
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key)
        {
            return s.clone();
        }
        let mut guard = self.series.write().unwrap_or_else(|e| e.into_inner());
        guard
            .entry(key)
            .or_insert_with(|| {
                Arc::new(SeriesSlot {
                    pool: pool.to_string(),
                    uid: uid.to_string(),
                    ring: Mutex::new(Ring::default()),
                })
            })
            .clone()
    }

    /// 记录一次请求结果：同时写入账号序列与池汇总序列（uid="-" 时只写汇总）
    pub fn record(
        &self,
        pool: &str,
        uid: &str,
        outcome: Outcome,
        ttft_ms: Option<u64>,
        duration_ms: u64,
        tokens: u64,
    ) {
        self.record_at(now_minute(), pool, uid, outcome, ttft_ms, duration_ms, tokens);
    }

    fn record_at(
        &self,
        minute: i64,
        pool: &str,
        uid: &str,
        outcome: Outcome,
        ttft_ms: Option<u64>,
        duration_ms: u64,
        tokens: u64,
    ) {
        let uid = if uid.is_empty() { "none" } else { uid };
        let targets: &[&str] = if uid == "-" { &["-"] } else { &[uid, "-"] };
        for u in targets {
            let slot = self.slot(pool, u);
            let mut ring = slot.ring.lock().unwrap_or_else(|e| e.into_inner());
            ring.bucket_mut(minute)
                .record(outcome, ttft_ms, duration_ms, tokens);
        }
    }

    /// 对冲结果（fired = 触发过对冲；won = 对冲侧接管并胜出）
    pub fn record_hedge(&self, pool: &str, uid: &str, fired: bool, won: bool) {
        if !fired {
            return;
        }
        let minute = now_minute();
        let uid = if uid.is_empty() { "none" } else { uid };
        let targets: &[&str] = if uid == "-" { &["-"] } else { &[uid, "-"] };
        for u in targets {
            let slot = self.slot(pool, u);
            let mut ring = slot.ring.lock().unwrap_or_else(|e| e.into_inner());
            let b = ring.bucket_mut(minute);
            b.hedge_fired = b.hedge_fired.saturating_add(1);
            if won {
                b.hedge_won = b.hedge_won.saturating_add(1);
            }
        }
    }

    /// 尝试级错误归因：由 `pool.note_error` 在拿到精确 ErrKind 时直接记账。
    ///
    /// 与 `record` 的分工：终局只记成功 / 客户端中断；失败一律以「尝试」为单位
    /// 在这里落一次错误（同一请求换号 N 次就有 N 条错误，成功那次再记一条 ok）。
    /// 这样既不会事后改写别人的计数，也不会与终局失败行双计。
    pub fn record_attempt(&self, pool: &str, uid: &str, outcome: Outcome) {
        if outcome.is_success() {
            return;
        }
        let minute = now_minute();
        let uid = if uid.is_empty() { "none" } else { uid };
        let targets: &[&str] = if uid == "-" { &["-"] } else { &[uid, "-"] };
        for u in targets {
            let slot = self.slot(pool, u);
            let mut ring = slot.ring.lock().unwrap_or_else(|e| e.into_inner());
            let b = ring.bucket_mut(minute);
            b.counts[outcome.index()] = b.counts[outcome.index()].saturating_add(1);
        }
    }

    /// 5 分钟 / 1 小时窗口聚合快照：
    /// `{ windows: { "5m": { pools: {pool: {..., accounts: {uid: {...}}}} }, "1h": {...} } }`
    pub fn snapshot_v2(&self) -> serde_json::Value {
        let now = now_minute();
        let mut windows = serde_json::Map::new();
        for (label, minutes) in [("5m", 5i64), ("1h", WINDOW_BUCKETS as i64)] {
            // 先聚合到 (pool, uid) → Agg，再组装 JSON，避免遍历顺序影响 accounts
            let mut aggs: HashMap<(String, String), Agg> = HashMap::new();
            {
                let guard = self.series.read().unwrap_or_else(|e| e.into_inner());
                for slot in guard.values() {
                    let agg = slot
                        .ring
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .aggregate(now, minutes);
                    if agg.requests() == 0 {
                        continue;
                    }
                    aggs.insert((slot.pool.clone(), slot.uid.clone()), agg);
                }
            }
            let mut pools: HashMap<String, serde_json::Map<String, serde_json::Value>> =
                HashMap::new();
            for ((pool, uid), agg) in &aggs {
                if uid == "-" {
                    pools.entry(pool.clone()).or_default().insert("-".into(), agg.to_json());
                }
            }
            for ((pool, uid), agg) in &aggs {
                if uid == "-" {
                    continue;
                }
                let pool_obj = pools
                    .entry(pool.clone())
                    .or_default()
                    .entry("-")
                    .or_insert_with(|| Agg::default().to_json());
                if let Some(obj) = pool_obj.as_object_mut() {
                    let accounts = obj
                        .entry("accounts")
                        .or_insert_with(|| serde_json::json!({}));
                    if let Some(a) = accounts.as_object_mut() {
                        a.insert(uid.clone(), agg.to_json());
                    }
                }
            }
            let mut out = serde_json::Map::new();
            for (pool, mut m) in pools {
                out.insert(pool, m.remove("-").unwrap_or_default());
            }
            windows.insert(
                label.into(),
                serde_json::json!({ "windowMinutes": minutes, "pools": out }),
            );
        }
        serde_json::json!({ "windows": windows })
    }

    /// 兼容旧面板：用 1 小时窗口给出累计口径（旧字段名保持不变）
    pub fn snapshot(&self) -> serde_json::Value {
        let now = now_minute();
        let guard = self.series.read().unwrap_or_else(|e| e.into_inner());
        let mut pools = serde_json::Map::new();
        let mut ok = 0u64;
        let mut fail = 0u64;
        let mut tokens = 0u64;
        for slot in guard.values() {
            if slot.uid != "-" {
                continue;
            }
            let agg = slot
                .ring
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .aggregate(now, WINDOW_BUCKETS as i64);
            let total = agg.requests();
            let ok_n = agg.counts[Outcome::Ok.index()] as u64;
            let fail_n = total.saturating_sub(ok_n).saturating_sub(agg.counts[Outcome::LocalAbort.index()] as u64);
            ok += ok_n;
            fail += fail_n;
            tokens += agg.tokens;
            pools.insert(
                slot.pool.clone(),
                serde_json::json!({
                    "ok": ok_n,
                    "fail": fail_n,
                    "total": total,
                    "successRate": (agg.success_rate() * 1000.0).round() / 1000.0,
                    "ewmaSuccess": (agg.success_rate() * 1000.0).round() / 1000.0,
                    "ewmaLatencyMs": agg.dur.percentile(50.0).unwrap_or(0),
                    "lastDurationMs": agg.dur.percentile(95.0).unwrap_or(0),
                    "tokens": agg.tokens,
                    "updatedAt": now_ms(),
                }),
            );
        }
        serde_json::json!({
            "pools": pools,
            "total": { "ok": ok, "fail": fail, "tokens": tokens },
        })
    }

    /// 收集待落盘行：`(after, before)` 开区间内、只取账号序列（池汇总可重建）
    pub fn rows_to_persist(&self, after_minute: i64, before_minute: i64) -> Vec<EvolutionMetricRow> {
        let guard = self.series.read().unwrap_or_else(|e| e.into_inner());
        let mut out = Vec::new();
        for slot in guard.values() {
            if slot.uid == "-" {
                continue;
            }
            slot.ring
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .collect_rows(after_minute, before_minute, &slot.pool, &slot.uid, &mut out);
        }
        out
    }

    /// 已落盘进度（Unix 分钟）；用于每分钟增量写，避免重复 upsert
    pub fn flushed_minute(&self) -> i64 {
        self.flushed_minute.load(Ordering::Relaxed)
    }

    pub fn mark_flushed(&self, minute: i64) {
        self.flushed_minute.store(minute, Ordering::Relaxed);
    }

    /// 启动回填：把最近 1 小时的持久化行合并回环形缓冲（账号 + 池汇总）
    pub fn restore_rows(&self, rows: &[EvolutionMetricRow]) {
        if rows.is_empty() {
            return;
        }
        let mut max_minute = self.flushed_minute();
        for row in rows {
            let minute = row.minute_ts / 60;
            max_minute = max_minute.max(minute);
            for uid in [row.uid.as_str(), "-"] {
                let slot = self.slot(&row.pool, uid);
                let mut ring = slot.ring.lock().unwrap_or_else(|e| e.into_inner());
                ring.merge_row(row);
            }
        }
        // 回填的最后一条可能仍在写入（重启发生在分钟中段）：进度退一分钟，
        // 让下一轮 flush 用「回填值 + 重启后新增」整体覆盖 upsert（主键幂等）
        self.mark_flushed(max_minute.saturating_sub(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sum_counts(m: &Metrics, pool: &str, uid: &str) -> [u32; Outcome::COUNT] {
        let slot = m.slot(pool, uid);
        let ring = slot.ring.lock().unwrap();
        let mut agg = Agg::default();
        for b in &ring.buckets {
            agg.add_bucket(b);
        }
        agg.counts
    }

    #[test]
    fn local_abort_excluded_from_success_rate() {
        let m = Metrics::default();
        let now = now_minute();
        m.record_at(now, "trae", "u1", Outcome::Ok, Some(100), 200, 10);
        m.record_at(now, "trae", "u1", Outcome::Upstream5xx, None, 300, 0);
        m.record_at(now, "trae", "u1", Outcome::LocalAbort, None, 50, 0);
        let slot = m.slot("trae", "u1");
        let agg = slot.ring.lock().unwrap().aggregate(now, 5);
        assert_eq!(agg.requests(), 3);
        assert_eq!(agg.success_rate(), 0.5, "local_abort 不进分母");
        // 池汇总同步累计
        let agg_all = m.slot("trae", "-").ring.lock().unwrap().aggregate(now, 5);
        assert_eq!(agg_all.requests(), 3);
    }

    #[test]
    fn minute_ring_expires_after_window() {
        let m = Metrics::default();
        let now = now_minute();
        m.record_at(now - 10, "wb", "u1", Outcome::Ok, None, 100, 0);
        m.record_at(now, "wb", "u1", Outcome::Ok, None, 100, 0);
        let slot = m.slot("wb", "u1");
        let ring = slot.ring.lock().unwrap();
        assert_eq!(ring.aggregate(now, 5).requests(), 1, "5 分钟窗口不含 10 分钟前");
        assert_eq!(ring.aggregate(now, 60).requests(), 2, "1 小时窗口包含");
    }

    #[test]
    fn histogram_percentile_error_within_5pct() {
        let mut h = Histogram::default();
        let samples = [80u64, 120, 200, 500, 1000, 2000, 4000, 8000, 16000];
        for s in samples {
            h.add(s);
        }
        let p50 = h.percentile(50.0).unwrap();
        let p95 = h.percentile(95.0).unwrap();
        let truth50 = 1000.0;
        let truth95 = 16000.0;
        assert!((p50 as f64 - truth50).abs() / truth50 < 0.05, "p50={p50}");
        assert!((p95 as f64 - truth95).abs() / truth95 < 0.05, "p95={p95}");
    }

    #[test]
    fn attempt_error_is_counted_without_terminal_record() {
        let m = Metrics::default();
        let now = now_minute();
        m.record_attempt("wb", "u1", Outcome::RateLimited);
        m.record_attempt("wb", "u1", Outcome::Ok); // 成功不走尝试级入口，不应计数
        let c = sum_counts(&m, "wb", "u1");
        assert_eq!(c[Outcome::RateLimited.index()], 1);
        assert_eq!(c[Outcome::Ok.index()], 0);
        // 池汇总同步累计
        let agg_all = m.slot("wb", "-").ring.lock().unwrap().aggregate(now, 5);
        assert_eq!(agg_all.requests(), 1);
        // 换号成功：1 条尝试错误 + 1 条终局成功 = 2 次尝试
        m.record_at(now, "wb", "u1", Outcome::Ok, Some(120), 300, 0);
        let agg = m.slot("wb", "u1").ring.lock().unwrap().aggregate(now, 5);
        assert_eq!(agg.requests(), 2);
        assert_eq!(agg.success_rate(), 0.5);
    }

    #[test]
    fn success_rate_is_null_when_only_local_aborts() {
        let m = Metrics::default();
        let now = now_minute();
        m.record_at(now, "trae", "u1", Outcome::LocalAbort, None, 50, 0);
        let slot = m.slot("trae", "u1");
        let agg = slot.ring.lock().unwrap().aggregate(now, 5);
        assert_eq!(agg.requests(), 1);
        assert_eq!(agg.to_json()["successRate"], serde_json::Value::Null);
    }

    #[test]
    fn hedge_counters_and_rows() {
        let m = Metrics::default();
        let now = now_minute();
        m.record_at(now, "trae", "u1", Outcome::Ok, Some(300), 900, 42);
        m.record_hedge("trae", "u1", true, true);
        m.record_hedge("trae", "u1", true, false);
        let rows = m.rows_to_persist(now - 1, now + 1);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].hedge_fired, 2);
        assert_eq!(rows[0].hedge_won, 1);
        assert_eq!(rows[0].tokens, 42);
        assert_eq!(rows[0].ok, 1);
    }

    #[test]
    fn restore_rebuilds_pool_aggregate() {
        let m = Metrics::default();
        let now = now_minute();
        let rows = vec![EvolutionMetricRow {
            minute_ts: now * 60,
            pool: "qoder".into(),
            uid: "u9".into(),
            ok: 3,
            tokens: 9,
            p50_dur: 120,
            ..Default::default()
        }];
        m.restore_rows(&rows);
        let agg = m.slot("qoder", "-").ring.lock().unwrap().aggregate(now, 5);
        assert_eq!(agg.requests(), 3);
        assert_eq!(agg.tokens, 9);
        assert!(m.flushed_minute() >= now - 1);
    }
}
