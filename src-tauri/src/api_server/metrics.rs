//! 网关观测层（自我进化 P1）：按池累计请求结果，供后续自适应调参使用。
//!
//! 定位：**只观测、不决策**。路由行为与现在完全一致，数据在内存里滚动，
//! 重启清零——先拿到真实分布，再谈自动调参（见 `docs/pending/自我进化方案.md`）。
//!
//! 指标口径：
//! - `ok/fail/total`：进程内累计，用于粗粒度成功率；
//! - `ewmaSuccess`：指数滑动成功率（α=0.05，约等于最近 20 次），抗尖峰；
//! - `ewmaLatencyMs`：指数滑动平均耗时；
//! - `tokens`：累计 token（与活动事件里的里程碑同源）。
use std::collections::HashMap;
use std::sync::Mutex;

/// EWMA 平滑系数：越小越平滑（0.05 ≈ 最近 20 次请求的加权平均）
const ALPHA: f64 = 0.05;

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[derive(Default, Clone)]
pub struct Series {
    pub ok: u64,
    pub fail: u64,
    pub tokens: u64,
    pub last_duration_ms: u64,
    /// 滑动成功率（0..1）
    pub ewma_success: f64,
    /// 滑动平均耗时（ms）
    pub ewma_latency_ms: f64,
    pub updated_at_ms: i64,
}

impl Series {
    fn record(&mut self, ok: bool, duration_ms: u64, tokens: u64) {
        let first = self.ok + self.fail == 0;
        if ok {
            self.ok += 1;
        } else {
            self.fail += 1;
        }
        self.tokens += tokens;
        self.last_duration_ms = duration_ms;

        let sample = if ok { 1.0 } else { 0.0 };
        let latency = duration_ms as f64;
        // 首个样本直接取初值，避免从 0 慢慢爬（会让面板一开始显示 0ms）
        if first {
            self.ewma_success = sample;
            self.ewma_latency_ms = latency;
        } else {
            self.ewma_success = self.ewma_success * (1.0 - ALPHA) + sample * ALPHA;
            self.ewma_latency_ms = self.ewma_latency_ms * (1.0 - ALPHA) + latency * ALPHA;
        }
        self.updated_at_ms = now_ms();
    }

    fn to_json(&self) -> serde_json::Value {
        let total = self.ok + self.fail;
        let rate = if total == 0 {
            0.0
        } else {
            self.ok as f64 / total as f64
        };
        serde_json::json!({
            "ok": self.ok,
            "fail": self.fail,
            "total": total,
            // 保留三位小数，前端直接乘 100 显示百分比
            "successRate": (rate * 1000.0).round() / 1000.0,
            "ewmaSuccess": (self.ewma_success * 1000.0).round() / 1000.0,
            "ewmaLatencyMs": self.ewma_latency_ms.round(),
            "lastDurationMs": self.last_duration_ms,
            "tokens": self.tokens,
            "updatedAt": self.updated_at_ms,
        })
    }
}

/// 按池聚合的观测表（trae / wb / qoder / doubao / custom）
#[derive(Default)]
pub struct Metrics {
    inner: Mutex<HashMap<String, Series>>,
}

impl Metrics {
    pub fn record(&self, pool: &str, ok: bool, duration_ms: u64, tokens: u64) {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard
            .entry(pool.to_string())
            .or_default()
            .record(ok, duration_ms, tokens);
    }

    /// 快照：给设置页/调试命令读取（只读，不会清除计数）
    pub fn snapshot(&self) -> serde_json::Value {
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let mut pools = serde_json::Map::new();
        let mut ok = 0u64;
        let mut fail = 0u64;
        let mut tokens = 0u64;
        for (name, series) in guard.iter() {
            ok += series.ok;
            fail += series.fail;
            tokens += series.tokens;
            pools.insert(name.clone(), series.to_json());
        }
        serde_json::json!({
            "pools": pools,
            "total": { "ok": ok, "fail": fail, "tokens": tokens },
        })
    }

    pub fn reset(&self) {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }
}
