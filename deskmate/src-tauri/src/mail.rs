//! QQ 邮箱 SMTP 提醒（lettre，SMTPS 465）。
//!
//! PRD 5.6：不在位且存在待处理行动项时，桌面弹窗与邮件双通道并行触发。
//! 发送失败按 PRD 要求重试 2 次，仍失败则返回错误由调用方记录。

use lettre::message::{header::ContentType, Mailbox};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{Message, SmtpTransport, Transport};

use crate::error::{AppError, AppResult};
use crate::secrets::{key, SecretsStore};

const SMTP_HOST: &str = "smtp.qq.com";
const RETRY_TIMES: usize = 2;

/// 发送一封邮件所需的全部配置。
pub struct MailConfig {
    pub user: String,
    pub auth_code: String,
    pub to: String,
}

impl MailConfig {
    /// 从密钥仓库读取；邮箱地址或授权码缺失时返回 None。
    pub fn from_store(store: &SecretsStore) -> Option<Self> {
        let user = store.get(key::SMTP_USER)?;
        let auth_code = store.get(key::SMTP_AUTH_CODE)?;
        let to = store
            .get(key::SMTP_TO)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| user.clone());
        Some(Self {
            user,
            auth_code,
            to,
        })
    }
}

/// 以文本方式发送邮件（阻塞调用，需在 blocking 上下文中执行）。
pub fn send(cfg: &MailConfig, subject: &str, body: &str) -> AppResult<()> {
    let from: Mailbox = cfg
        .user
        .parse()
        .map_err(|e| AppError::new(format!("发件人地址非法：{e}")))?;
    let to: Mailbox = cfg
        .to
        .parse()
        .map_err(|e| AppError::new(format!("收件人地址非法：{e}")))?;

    let email = Message::builder()
        .from(from)
        .to(to)
        .subject(subject)
        .header(ContentType::TEXT_PLAIN)
        .body(body.to_string())
        .map_err(|e| AppError::new(format!("邮件构造失败：{e}")))?;

    let transport = SmtpTransport::relay(SMTP_HOST)
        .map_err(|e| AppError::new(format!("SMTP 服务初始化失败：{e}")))?
        .credentials(Credentials::new(cfg.user.clone(), cfg.auth_code.clone()))
        .build();

    transport
        .send(&email)
        .map_err(|e| AppError::new(format!("邮件发送失败：{e}")))?;
    Ok(())
}

/// 带重试的发送：失败后重试 `RETRY_TIMES` 次。
pub fn send_with_retry(cfg: &MailConfig, subject: &str, body: &str) -> AppResult<()> {
    let mut last_err = None;
    for attempt in 0..=RETRY_TIMES {
        match send(cfg, subject, body) {
            Ok(()) => return Ok(()),
            Err(e) => {
                eprintln!("[mail] 第 {} 次发送失败：{}", attempt + 1, e.0);
                last_err = Some(e);
            }
        }
    }
    Err(last_err.unwrap_or_else(|| AppError::new("邮件发送失败")))
}
