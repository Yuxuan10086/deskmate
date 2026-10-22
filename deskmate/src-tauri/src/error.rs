//! 统一错误类型：对前端序列化为纯字符串消息。

use serde::{Serialize, Serializer};

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct AppError(pub String);

impl Serialize for AppError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl AppError {
    pub fn new(msg: impl Into<String>) -> Self {
        Self(msg.into())
    }
}

impl From<sqlx::Error> for AppError {
    fn from(e: sqlx::Error) -> Self {
        Self(format!("数据库错误：{e}"))
    }
}

impl From<serde_json::Error> for AppError {
    fn from(e: serde_json::Error) -> Self {
        Self(format!("数据序列化错误：{e}"))
    }
}

impl From<chrono::ParseError> for AppError {
    fn from(e: chrono::ParseError) -> Self {
        Self(format!("日期解析错误：{e}"))
    }
}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        Self(format!("文件系统错误：{e}"))
    }
}

impl From<zip::result::ZipError> for AppError {
    fn from(e: zip::result::ZipError) -> Self {
        Self(format!("ZIP 错误：{e}"))
    }
}

impl From<quick_xml::Error> for AppError {
    fn from(e: quick_xml::Error) -> Self {
        Self(format!("XML 解析错误：{e}"))
    }
}

impl From<quick_xml::escape::EscapeError> for AppError {
    fn from(e: quick_xml::escape::EscapeError) -> Self {
        Self(format!("XML 转义错误：{e}"))
    }
}

impl From<base64::DecodeError> for AppError {
    fn from(e: base64::DecodeError) -> Self {
        Self(format!("base64 解码错误：{e}"))
    }
}

impl From<cpal::BuildStreamError> for AppError {
    fn from(e: cpal::BuildStreamError) -> Self {
        Self(format!("音频流创建失败：{e}"))
    }
}

impl From<cpal::PlayStreamError> for AppError {
    fn from(e: cpal::PlayStreamError) -> Self {
        Self(format!("音频流启动失败：{e}"))
    }
}

impl From<cpal::DefaultStreamConfigError> for AppError {
    fn from(e: cpal::DefaultStreamConfigError) -> Self {
        Self(format!("默认音频配置获取失败：{e}"))
    }
}

pub type AppResult<T> = Result<T, AppError>;
