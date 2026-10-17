use super::config::{Config, DEFAULT_PASSWORD};
use lettre::{
    message::{header::ContentType, Attachment, MultiPart, SinglePart},
    transport::smtp::authentication::Credentials,
    Message, SmtpTransport, Transport,
};
use std::time::Duration;

pub struct Failure {
    pub message: &'static str,
    pub retry: bool,
    pub uncertain: bool,
    pub auth: bool,
}

pub fn send(
    config: &Config,
    code: &str,
    id: &str,
    attachment: Option<Vec<u8>>,
    total: usize,
) -> Result<(), Failure> {
    let invalid = || Failure {
        message: "邮件配置或内容构建失败",
        retry: false,
        uncertain: false,
        auth: false,
    };
    let body = if attachment.is_some() {
        // 邮件正文附上附件密码：恢复时无需再回忆（邮箱本身即访问边界）。
        format!(
            "AI Work 助手加密账号备份\n备份编号：{id}\n账号数：{total}\n附件密码：{DEFAULT_PASSWORD}\n\
             恢复方式：应用左上角品牌图标 → 输入入口密码 → 选择附件并预览恢复。\n\
             服务器接受不代表邮件已投递，请检查收件箱及垃圾箱。"
        )
    } else {
        "AI Work 助手 SMTP 测试邮件。此邮件不含账号或凭据。".into()
    };
    let mut multipart = MultiPart::mixed().singlepart(SinglePart::plain(body));
    if let Some(bytes) = attachment {
        multipart = multipart.singlepart(Attachment::new(format!("aiwork-{id}.aiwbak")).body(
            bytes,
            ContentType::parse("application/octet-stream").map_err(|_| invalid())?,
        ));
    }
    let msg = Message::builder()
        .from(config.sender.parse().map_err(|_| invalid())?)
        .to(config.recipient.parse().map_err(|_| invalid())?)
        .subject("AI Work 助手 · 加密备份 / 邮件测试")
        .message_id(Some(format!("<{id}@backup.aiwork.local>")))
        .multipart(multipart)
        .map_err(|_| invalid())?;
    let builder = if config.smtp_port == 465 {
        SmtpTransport::relay(&config.smtp_host)
    } else {
        SmtpTransport::starttls_relay(&config.smtp_host)
    }
    .map_err(|_| invalid())?;
    let transport = builder
        .port(config.smtp_port)
        .timeout(Some(Duration::from_secs(30)))
        .credentials(Credentials::new(config.sender.clone(), code.to_owned()))
        .build();
    transport.send(&msg).map(|_| ()).map_err(|err| {
        // Never expose server text: it may echo SMTP auth or message content.
        match err.status().map(|s| s.to_string()) {
            Some(status)
                if status.starts_with("535")
                    || status.starts_with("534")
                    || status.starts_with("530") =>
            {
                Failure {
                    message: "SMTP 身份验证失败，请检查邮箱授权码并重新保存配置",
                    retry: false,
                    uncertain: false,
                    auth: true,
                }
            }
            Some(status) if status.starts_with('4') => Failure {
                message: "SMTP 暂时拒绝，等待重试",
                retry: true,
                uncertain: false,
                auth: false,
            },
            Some(_) => Failure {
                message: "SMTP 拒绝邮件，请检查邮箱配置或附件限制",
                retry: false,
                uncertain: false,
                auth: false,
            },
            None => Failure {
                message:
                    "连接失败或确认中断，发送状态未知；为避免重复投递已停止自动重发，可手动重试",
                retry: false,
                uncertain: true,
                auth: false,
            },
        }
    })
}
