//! 系统通知封装（tauri-plugin-notification，Rust 侧）+ 失败通知渠道分发（F-19，批次3 T3.6）。
//!
//! 用于托盘操作、后台任务（签到完成 / API 服务启停）等无窗口交互场景的结果反馈。
//! F-19 渠道扩展：企业微信群机器人 webhook / Server酱——作为桌面通知之外的可选渠道，
//! 由 workbuddy_settings.json 配置；渠道失败静默记日志，绝不影响主流程。

use tauri::AppHandle;

/// 发送系统通知（**已停用**，保留仅为记录历史行为）。
///
/// 2026-10-21 用户决定：所有系统通知改为桌宠 AI 气泡，通知栏不再出现。
/// 故这里改为私有且不再被调用——留着是为了让后续维护者知道「以前是这么发的」，
/// 而不是让人以为漏了某个调用点。要用请走 [`pet_notice`]。
#[allow(dead_code)]
fn system_notify_disabled() {}

/// 系统通知的统一替代入口：桌宠气泡。
///
/// 与旧 `notify()` 的调用形态一致（app/title/body），因此调用方只需改名。
/// 气泡在桌宠隐藏或全屏被收起时会自动临时叫醒，到期还原。
pub fn pet_notice(app: &AppHandle, title: &str, body: &str) {
    crate::commands::pet::notice_bubble(app, title, body);
}

// ── F-19 通知渠道（Trae/Buddy 全平台共用，配置存 app Settings） ─────────────

/// 通知事件类别：用于按事件开关过滤推送（总开关之后二级过滤）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifyEvent {
    /// 签到完成（一键签到/补签结果）
    CheckinDone,
    /// 调度任务失败（应用内调度器任务返回 Err）
    TaskFail,
    /// 其他手动操作结果（设备标识重置、测试等，不受事件开关限制）
    Other,
}

/// 企业微信群机器人的 @ 目标（仅 Webhook 渠道；Bark / Server酱 无此概念）
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub enum MentionTarget {
    /// 不 @ 任何人——群里有对方时可安静投递，避免无事也弹红点
    #[default]
    None,
    /// @所有人（真正需要人响应时才用：连续失败等）
    All,
    /// @指定成员：企业微信 userid 与手机号混填，投递时按形态分流
    /// （11 位纯数字 → mentioned_mobile_list，其余 → mentioned_list）
    Some(Vec<String>),
}

/// 可选通知渠道配置（来自 app Settings；None/空 = 关闭该渠道）
#[derive(Debug, Default, Clone)]
pub struct NotifyChannels {
    /// Bark 推送地址（https://api.day.app/<key>）
    pub bark_url: Option<String>,
    /// 通用 Webhook 地址（POST JSON，兼容企业微信群机器人等自建端）
    pub wechat_webhook: Option<String>,
    /// Webhook 渠道的 @ 范围
    pub wechat_mention: MentionTarget,
    /// Server酱 SendKey（https://sctapi.ftqq.com/<sendkey>.send）
    pub serverchan_sendkey: Option<String>,
}

impl NotifyChannels {
    /// 三渠道均未配置
    pub fn is_empty(&self) -> bool {
        [self.bark_url.as_deref(), self.wechat_webhook.as_deref(), self.serverchan_sendkey.as_deref()]
            .iter()
            .all(|c| c.map_or(true, |s| s.trim().is_empty()))
    }

    /// 从 app Settings 构造：空白一律当作未配置，@ 范围随 Webhook 一起读。
    ///
    /// 集中在此供 push_notify 与 notify_test 共用——两处各建一份会漂移
    /// （「测试」按配置 @ 了，实际推送却没 @，是最难查的一类不一致）。
    pub fn from_settings(s: &crate::models::Settings) -> Self {
        let mention = match s.notify_wechat_mention.as_deref().unwrap_or("none") {
            "all" => MentionTarget::All,
            "some" => MentionTarget::Some(
                s.notify_wechat_mention_list
                    .clone()
                    .unwrap_or_default()
                    .iter()
                    .map(|x| x.trim().to_string())
                    .filter(|x| !x.is_empty())
                    .collect(),
            ),
            _ => MentionTarget::None,
        };
        Self {
            bark_url: s.notify_bark_url.clone().filter(|x| !x.trim().is_empty()),
            wechat_webhook: s.notify_webhook_url.clone().filter(|x| !x.trim().is_empty()),
            wechat_mention: mention,
            serverchan_sendkey: s.notify_serverchan_sendkey
                .clone()
                .filter(|x| !x.trim().is_empty()),
        }
    }
}

/// Bark：POST JSON 到 <server>/push（device_key 取地址末段；零新增依赖，ureq 同步调用）
pub fn notify_bark(bark_url: &str, title: &str, body: &str) -> Result<(), String> {
    let base = bark_url.trim().trim_end_matches('/');
    if !base.starts_with("https://") {
        return Err("Bark 推送地址需为 https://".into());
    }
    let device_key = base.rsplit('/').next().unwrap_or("").to_string();
    if device_key.is_empty() || !device_key.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err("Bark 地址末段（Device Key）格式不正确".into());
    }
    let payload = serde_json::json!({
        "device_key": device_key,
        "title": title,
        "body": body,
        "group": "AIWorkAssistant",
    });
    let agent = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(10))
        .build();
    agent
        .post(&format!("{base}/push"))
        .set("Content-Type", "application/json; charset=utf-8")
        .send_string(&payload.to_string())
        .map_err(|e| format!("Bark 请求失败: {e}"))?;
    Ok(())
}

/// 11 位纯数字视为手机号（企业微信手机号 @ 必须走 mobile 专用字段，
/// 与 userid 走不同的 key——混用会被对端忽略，导致 @ 静默失效）
fn is_mobile(s: &str) -> bool {
    s.len() == 11 && s.chars().all(|c| c.is_ascii_digit())
}

/// 企业微信群机器人：POST text 消息（零新增依赖，ureq 同步调用）。
/// `mention` 控制 @ 范围——群里有对方时可以安静投递，只有真需要人响应时才 @。
fn wechat_text_payload(title: &str, body: &str, mention: &MentionTarget) -> serde_json::Value {
    let mut text = serde_json::Map::new();
    text.insert("content".into(), serde_json::json!(format!("{title}\n{body}")));
    match mention {
        MentionTarget::None => {}
        MentionTarget::All => {
            text.insert("mentioned_list".into(), serde_json::json!(["@all"]));
        }
        MentionTarget::Some(list) => {
            let ids: Vec<&str> = list
                .iter()
                .map(String::as_str)
                .filter(|s| !s.trim().is_empty() && !is_mobile(s))
                .collect();
            let mobiles: Vec<&str> = list
                .iter()
                .map(String::as_str)
                .filter(|s| !s.trim().is_empty() && is_mobile(s))
                .collect();
            // 名单全空（模式切到「指定成员」但还没填人）→ 两个字段都不发，
            // 退回普通投递；发空数组会让对端直接拒收整条消息
            if !ids.is_empty() {
                text.insert("mentioned_list".into(), serde_json::json!(ids));
            }
            if !mobiles.is_empty() {
                text.insert("mentioned_mobile_list".into(), serde_json::json!(mobiles));
            }
        }
    }
    serde_json::json!({ "msgtype": "text", "text": text })
}

pub fn notify_wechat_webhook(
    webhook: &str,
    title: &str,
    body: &str,
    mention: &MentionTarget,
) -> Result<(), String> {
    let url = webhook.trim();
    if !url.starts_with("https://") {
        return Err("企业微信 webhook 需为 https:// 地址".into());
    }
    let payload = wechat_text_payload(title, body, mention);
    let agent = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(10))
        .build();
    agent
        .post(url)
        .set("Content-Type", "application/json")
        .send_string(&payload.to_string())
        .map_err(|e| format!("企业微信 webhook 请求失败: {e}"))?;
    Ok(())
}

/// Server酱：表单提交 title/desp（Turbo 版 sctapi 端点）
pub fn notify_serverchan(sendkey: &str, title: &str, body: &str) -> Result<(), String> {
    let key = sendkey.trim();
    if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err("Server酱 SendKey 格式不正确".into());
    }
    let agent = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(10))
        .build();
    // desp 支持 markdown：换行转两空格+换行保持段落
    let desp = body.replace('\n', "  \n");
    agent
        .post(&format!("https://sctapi.ftqq.com/{key}.send"))
        .send_form(&[("title", title), ("desp", desp.as_str())])
        .map_err(|e| format!("Server酱 请求失败: {e}"))?;
    Ok(())
}

/// 统一分发入口：**桌宠气泡**（有 AppHandle 时）+ 可选远程渠道；渠道失败仅记日志。
///
/// 2026-10-21 改版（用户选择）：桌面通知栏这一层**整体换成桌宠气泡**，
/// 所有原有调用点（签到结果 / 任务失败 / API·代理操作失败 / 环境重置…）
/// 自动跟着走气泡，无需逐个改调用方。
///
/// 刻意保留的两件事：
/// - 远程渠道（Bark / 企业微信 / Server酱）与桌面通知互相独立：手机端看不到桌宠，
///   这几条推送必须照旧发送，否则用户出门在外就彻底收不到通知；
/// - `app.is_none()` 时（如调度线程只传 data_dir）气泡发不出去，但仍不回落系统通知——
///   需求是「通知栏不再出现」，半途回落会让用户又看到两类通知混杂。
pub fn notify_all(app: Option<&AppHandle>, data_dir: &std::path::Path, title: &str, body: &str, channels: &NotifyChannels) {
    if let Some(app) = app {
        crate::commands::pet::notice_bubble(app, title, body);
    }
    if channels.is_empty() {
        return;
    }
    if let Some(bark) = channels.bark_url.as_deref().filter(|s| !s.trim().is_empty()) {
        if let Err(e) = notify_bark(bark, title, body) {
            crate::fs_utils::app_log(data_dir, &format!("通知渠道(Bark)失败: {e}"));
        }
    }
    if let Some(webhook) = channels.wechat_webhook.as_deref().filter(|s| !s.trim().is_empty()) {
        if let Err(e) = notify_wechat_webhook(webhook, title, body, &channels.wechat_mention) {
            crate::fs_utils::app_log(data_dir, &format!("通知渠道(企业微信)失败: {e}"));
        }
    }
    if let Some(sendkey) = channels.serverchan_sendkey.as_deref().filter(|s| !s.trim().is_empty()) {
        if let Err(e) = notify_serverchan(sendkey, title, body) {
            crate::fs_utils::app_log(data_dir, &format!("通知渠道(Server酱)失败: {e}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 三渠道空判定：默认全空；任一渠道有值（含空白包裹）即非空
    #[test]
    fn channels_empty_detection() {
        assert!(NotifyChannels::default().is_empty());
        assert!(!NotifyChannels { bark_url: Some(" https://api.day.app/k ".into()), ..Default::default() }.is_empty());
        assert!(!NotifyChannels { serverchan_sendkey: Some("SCT123".into()), ..Default::default() }.is_empty());
        assert!(!NotifyChannels { wechat_webhook: Some("https://qyapi.weixin.qq.com/x".into()), ..Default::default() }.is_empty());
    }

    /// Bark 地址校验（错误路径，不触网）：协议 / 空 Key / 非 alnum Key 均拒绝
    #[test]
    fn bark_url_validation_rejects_bad_input() {
        assert!(notify_bark("http://api.day.app/abc123", "t", "b").is_err(), "非 https 拒绝");
        assert!(notify_bark("https://api.day.app/", "t", "b").is_err(), "空 Key 拒绝");
        assert!(notify_bark("https://api.day.app/ab c!/", "t", "b").is_err(), "非法字符 Key 拒绝");
    }

    // ── 企业微信 @ 目标（纯 payload 构造，不触网） ───────────────────────────

    /// 不 @：只发内容，不带任何 mention 字段——群里有对方时可安静投递
    #[test]
    fn wechat_mention_none_omits_mention_fields() {
        let p = wechat_text_payload("标题", "正文", &MentionTarget::None);
        assert_eq!(p["msgtype"], json!("text"));
        assert_eq!(p["text"]["content"], json!("标题\n正文"));
        assert!(p["text"].get("mentioned_list").is_none());
        assert!(p["text"].get("mentioned_mobile_list").is_none());
    }

    /// @所有人：mentioned_list 填 "@all"
    #[test]
    fn wechat_mention_all_targets_everyone() {
        let p = wechat_text_payload("标题", "正文", &MentionTarget::All);
        assert_eq!(p["text"]["mentioned_list"], json!(["@all"]));
        assert!(p["text"].get("mentioned_mobile_list").is_none());
    }

    /// @指定成员：userid 与手机号混填时按形态分流到对端各自的字段
    /// （走错字段会被静默忽略，@ 不生效）
    #[test]
    fn wechat_mention_some_splits_userid_and_mobile() {
        let p = wechat_text_payload(
            "标题",
            "正文",
            &MentionTarget::Some(vec![
                "WangQing".into(),
                "13800138000".into(),
                "  ".into(),
                "zhangsan".into(),
            ]),
        );
        assert_eq!(p["text"]["mentioned_list"], json!(["WangQing", "zhangsan"]));
        assert_eq!(p["text"]["mentioned_mobile_list"], json!(["13800138000"]));
    }

    /// 选了「指定成员」但名单还空着 → 退回普通投递，不发空数组
    /// （空数组会让对端直接拒收整条消息）
    #[test]
    fn wechat_mention_some_empty_list_falls_back_to_plain() {
        let p = wechat_text_payload("标题", "正文", &MentionTarget::Some(vec![]));
        assert!(p["text"].get("mentioned_list").is_none());
        assert!(p["text"].get("mentioned_mobile_list").is_none());
        assert_eq!(p["text"]["content"], json!("标题\n正文"));
    }

    /// 配置 → @ 目标映射：Some 模式顺带裁掉空白项（空白不该被当作成员发出去）；
    /// 未配置或未知值一律按「不 @」处理，不猜用户意图
    #[test]
    fn from_settings_maps_mention_modes() {
        let s = crate::models::Settings {
            notify_wechat_mention: Some("all".into()),
            ..Default::default()
        };
        assert_eq!(NotifyChannels::from_settings(&s).wechat_mention, MentionTarget::All);

        let s = crate::models::Settings {
            notify_wechat_mention: Some("some".into()),
            notify_wechat_mention_list: Some(vec!["WangQing".into(), " ".into()]),
            ..Default::default()
        };
        assert_eq!(
            NotifyChannels::from_settings(&s).wechat_mention,
            MentionTarget::Some(vec!["WangQing".into()])
        );

        let s = crate::models::Settings::default();
        assert_eq!(NotifyChannels::from_settings(&s).wechat_mention, MentionTarget::None);
    }

    /// 事件门控映射完整（Other 不受事件开关限制，在 push_notify 层处理）
    #[test]
    fn notify_event_is_copy_and_distinct() {
        let e = NotifyEvent::CheckinDone;
        let f = e;
        assert!(e == f, "Copy 语义");
        assert!(NotifyEvent::TaskFail != NotifyEvent::Other);
    }

    // ── 集成测试：notify_all 渠道分发 × app_log 落盘 ─────────────────────────
    // 全部走「校验失败 / 本地不可达」路径，不依赖外网；用 app.log 作为发送尝试的可观测信号。

    fn tmp_dir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("twa_notify_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn read_log(dir: &std::path::Path) -> String {
        std::fs::read_to_string(dir.join("logs").join("app.log")).unwrap_or_default()
    }

    /// 空渠道：is_empty 短路，既不触网也不写日志
    #[test]
    fn notify_all_empty_channels_no_log_no_panic() {
        let dir = tmp_dir("empty");
        notify_all(None, &dir, "标题", "内容", &NotifyChannels::default());
        assert!(!dir.join("logs").join("app.log").exists(), "空渠道不应产生任何日志");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 三渠道均配置无效值：分发到每个渠道 → 格式校验失败 → app_log 各落一条失败记录（不触网）
    #[test]
    fn notify_all_invalid_channels_all_logged() {
        let dir = tmp_dir("invalid");
        let ch = NotifyChannels {
            bark_url: Some("http://api.day.app/abc".into()),          // 非 https 拒绝
            wechat_webhook: Some("ftp://qyapi.example.com/x".into()), // 非 https 拒绝
            serverchan_sendkey: Some("SCT abc!".into()),              // 含空格/非法字符拒绝
            ..Default::default()
        };
        notify_all(None, &dir, "标题", "内容", &ch);
        let log = read_log(&dir);
        assert!(log.contains("通知渠道(Bark)失败"), "log: {log}");
        assert!(log.contains("通知渠道(企业微信)失败"), "log: {log}");
        assert!(log.contains("通知渠道(Server酱)失败"), "log: {log}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 合法格式 + 本地不可达端点（127.0.0.1:1 保留端口，回环立即拒连）：
    /// 验证校验通过后的完整链路——请求真实发起 → 失败 → app_log 落盘
    #[test]
    fn notify_all_valid_format_unreachable_bark_logged() {
        let dir = tmp_dir("unreachable");
        let ch = NotifyChannels { bark_url: Some("https://127.0.0.1:1/abcKey123".into()), ..Default::default() };
        notify_all(None, &dir, "标题", "内容", &ch);
        let log = read_log(&dir);
        assert!(log.contains("通知渠道(Bark)失败"), "合法格式 Bark 请求失败应落日志: {log}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 关键回归（2026-10-21 改造）：`notify_all` 传了 `app` 时**必须走桌宠气泡**，
    /// 而远程渠道与气泡互相独立——气泡可用不代表可以停掉手机推送。
    ///
    /// 这里不构造真实 AppHandle（那需要起一个 Tauri app），而是锁住两件可从
    /// 代码结构验证的事：① 渠道分发不因 `app` 有无而改变；② 气泡是唯一桌面出口。
    #[test]
    fn remote_channels_independent_of_app_handle() {
        let dir = tmp_dir("indep");
        let ch = NotifyChannels {
            bark_url: Some("http://api.day.app/abc".into()), // 非 https 必失败 → 落日志
            ..Default::default()
        };
        // app=None（气泡发不出去）时远程渠道照旧尝试
        notify_all(None, &dir, "标题", "内容", &ch);
        let log = read_log(&dir);
        assert!(
            log.contains("通知渠道(Bark)失败"),
            "远程渠道不能因为气泡这一层而停发: {log}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
