//! 内容拦截降级状态机（P3-1 第一步）。
//!
//! 背景：上游内容审核按 system 提示词里的身份模板句**逐字匹配**误杀，
//! 命中即返回 400 审核文案、整条请求被打死。逐句指纹改写覆盖不全时，
//! 换成一段极简中性提示词即可绕开（误报处理，不对抗）。
//!
//! 状态语义：
//! - 触发后进入降级期，直到**次日 00:00（+08:00）**自动恢复；
//! - 降级期内参数照常，调用方直接发中性提示词，不必先撞一次 400；
//! - 进程内存保存，重启清零（重启少见，且开关默认关闭，不进该路径）；
//! - 提供手动清除入口，用户不必等零点的"模型变松散"一整天。
//!
//! 刻意不做持久化：降级是应急态，跨重启保留反而会把一次偶发误杀
//! 拖成长期生效。

use std::sync::Mutex;

/// 降级截止时间（Unix 秒）；0 = 未激活
static UNTIL: Mutex<i64> = Mutex::new(0);

fn now_ts() -> i64 {
    chrono::Utc::now().timestamp()
}

/// 当前是否处于降级期
pub fn active() -> bool {
    let until = *UNTIL.lock().unwrap_or_else(|e| e.into_inner());
    until > now_ts()
}

/// 降级截止时间（0 = 未激活；供 UI 展示）
pub fn until_ts() -> i64 {
    let until = *UNTIL.lock().unwrap_or_else(|e| e.into_inner());
    if until > now_ts() {
        until
    } else {
        0
    }
}

/// 触发降级到次日 00:00（+08:00）。
///
/// 已在降级期内**不续期**：保持最早触发点的零点重置语义，
/// 避免连续误杀把降级期无限往后推。
pub fn trigger() {
    let mut until = UNTIL.lock().unwrap_or_else(|e| e.into_inner());
    if *until <= now_ts() {
        *until = next_midnight_cst(chrono::Utc::now());
    }
}

/// 立即恢复（用户手动关闭降级）
pub fn clear() {
    let mut until = UNTIL.lock().unwrap_or_else(|e| e.into_inner());
    *until = 0;
}

/// 降级期内短路：把出站体直接换成中性提示词，省掉「先撞一次 400」。
///
/// 此前 `active()` **没有任何调用方**——降级期只被记录、不被使用，于是降级期内
/// 每个请求都仍带着触发拦截的那段 system 指纹先撞一次 400，拿到审核文案才降级
/// 重试。上游是逐字匹配，这段指纹在降级期内必然还在，那一次往返与一次账号级
/// 错误计次纯属白付（降级期的意义因此折损一半）。
///
/// 纯逻辑（不读全局态），便于测试直接覆盖三种组合。
pub fn pre_apply(enabled: bool, degraded: bool, body: &[u8]) -> Option<Vec<u8>> {
    if enabled && degraded {
        Some(super::prompt::apply_degraded_system(body))
    } else {
        None
    }
}

/// `pre_apply` 的热路径包装：读开关原子量与当前降级状态。
///
/// 返回 true 表示本请求已按降级发首包——调用方据此把「每请求一次降级重试」
/// 标记为已用：内容已是中性，命中拦截后再换成中性重发一次不会有任何变化。
pub fn pre_apply_if_active(enabled: &std::sync::atomic::AtomicBool, body: &mut Vec<u8>) -> bool {
    match pre_apply(
        enabled.load(std::sync::atomic::Ordering::Relaxed),
        active(),
        body,
    ) {
        Some(rewritten) => {
            *body = rewritten;
            true
        }
        None => false,
    }
}

/// `now` 之后最近的一次 Asia/Shanghai 00:00（Unix 秒）。
///
/// 用固定 +08:00 偏移计算，不依赖系统时区配置（容器/宿主机时区不确定）。
/// 边界：23:59 → 次日 00:00；恰好 00:00 → 次日 00:00。
pub fn next_midnight_cst(now: chrono::DateTime<chrono::Utc>) -> i64 {
    use chrono::TimeZone;
    let cst = chrono::FixedOffset::east_opt(8 * 3600).expect("+08:00 合法偏移");
    let local_now = now.with_timezone(&cst);
    let today = local_now.date_naive();
    let mut midnight = cst
        .from_local_datetime(&today.and_hms_opt(0, 0, 0).expect("00:00:00 合法"))
        .single()
        .expect("固定偏移无歧义");
    while midnight <= local_now {
        midnight += chrono::Duration::days(1);
    }
    midnight.timestamp()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn utc(y: i32, m: u32, d: u32, h: u32, min: u32) -> chrono::DateTime<chrono::Utc> {
        chrono::Utc
            .with_ymd_and_hms(y, m, d, h, min, 0)
            .single()
            .unwrap()
    }

    /// 23:59（CST）→ 几秒后的次日 00:00
    #[test]
    fn midnight_from_late_night() {
        // 2026-10-17 23:59 CST = 15:59 UTC
        let now = utc(2026, 10, 17, 15, 59);
        let next = next_midnight_cst(now);
        // 2026-10-18 00:00 CST = 2026-10-17 16:00 UTC
        assert_eq!(next, utc(2026, 10, 17, 16, 0).timestamp());
        assert_eq!(next - now.timestamp(), 60);
    }

    /// 恰好 00:00（CST）→ 次日零点，不是当天
    #[test]
    fn midnight_from_exact_midnight() {
        // 2026-10-17 00:00 CST = 2026-10-16 16:00 UTC
        let now = utc(2026, 10, 16, 16, 0);
        let next = next_midnight_cst(now);
        assert_eq!(next, utc(2026, 10, 17, 16, 0).timestamp());
    }

    /// 白天（CST）→ 当天过后的零点
    #[test]
    fn midnight_from_noon() {
        // 2026-10-17 12:00 CST = 04:00 UTC
        let now = utc(2026, 10, 17, 4, 0);
        let next = next_midnight_cst(now);
        assert_eq!(next, utc(2026, 10, 17, 16, 0).timestamp());
    }

    /// 触发/清除状态机：清除后立即失效
    #[test]
    fn trigger_and_clear() {
        clear();
        assert!(!active());
        assert_eq!(until_ts(), 0);
        trigger();
        assert!(active(), "触发后应处于降级期");
        let first = until_ts();
        assert!(first > now_ts());
        // 重复触发不续期
        trigger();
        assert_eq!(until_ts(), first, "降级期内不得续期");
        clear();
        assert!(!active());
        assert_eq!(until_ts(), 0);
    }
}
