use serde::Serialize;
use specta::Type;

use crate::extensions::AnyhowErrorToStringChain;

pub type CommandResult<T> = Result<T, CommandError>;

/// 错误类别：前端据此决定给用户什么操作，而不是解析中文文案。
///
/// 只做**决策相关**的粗分类；更细的定位仍看 `err_message`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type, Default)]
pub enum CommandErrorKind {
    /// 网络不通、连接被重置、DNS/代理问题：建议换线路或检查网络
    Network,
    /// 请求超时：建议重试
    Timeout,
    /// 需要登录 / 登录已失效：建议重新登录
    Auth,
    /// 资源不存在（漫画被下架、章节被删）：重试无意义
    NotFound,
    /// 磁盘写入失败、空间不足、权限问题：建议清理空间或换目录
    Storage,
    /// 其他未归类
    #[default]
    Unknown,
}

#[derive(Debug, Type, Serialize)]
pub struct CommandError {
    pub err_title: String,
    pub err_message: String,
    /// 错误类别（旧前端忽略该字段即可，保持向后兼容）
    pub kind: CommandErrorKind,
}

impl CommandError {
    pub fn from<E>(err_title: &str, err: E) -> Self
    where
        E: Into<anyhow::Error>,
    {
        let error = err.into();
        let kind = classify(&error);
        let string_chain = error.to_string_chain();
        tracing::error!(err_title, message = string_chain, ?kind);
        Self {
            err_title: err_title.to_string(),
            err_message: string_chain,
            kind,
        }
    }

    /// 显式指定类别（用于调用方比自动分类更清楚的情况，例如业务层已知 404 语义）。
    pub fn with_kind<E>(err_title: &str, err: E, kind: CommandErrorKind) -> Self
    where
        E: Into<anyhow::Error>,
    {
        let error = err.into();
        let string_chain = error.to_string_chain();
        tracing::error!(err_title, message = string_chain, ?kind);
        Self {
            err_title: err_title.to_string(),
            err_message: string_chain,
            kind,
        }
    }
}

/// 按错误链里的类型与文案做粗分类。
///
/// reqwest 的错误类型最可靠（有 `is_timeout` / `is_connect` / `status`），
/// 其余（文件系统、业务错误）只能看错误链文本；宁可归到 Unknown 也不要误判，
/// 因为前端会按类别改变主操作按钮。
pub fn classify(error: &anyhow::Error) -> CommandErrorKind {
    for cause in error.chain() {
        if let Some(err) = cause.downcast_ref::<reqwest::Error>() {
            if err.is_timeout() {
                return CommandErrorKind::Timeout;
            }
            if err.is_connect() || err.is_request() {
                return CommandErrorKind::Network;
            }
            if let Some(status) = err.status() {
                return kind_for_status(status);
            }
        }
        if let Some(reqwest_middleware::Error::Reqwest(err)) =
            cause.downcast_ref::<reqwest_middleware::Error>()
        {
            if err.is_timeout() {
                return CommandErrorKind::Timeout;
            }
            if err.is_connect() {
                return CommandErrorKind::Network;
            }
            if let Some(status) = err.status() {
                return kind_for_status(status);
            }
        }
        if let Some(io) = cause.downcast_ref::<std::io::Error>() {
            return kind_for_io(io.kind());
        }
    }

    let message = error.to_string();
    let lower = message.to_lowercase();
    if lower.contains("timeout") || lower.contains("超时") {
        return CommandErrorKind::Timeout;
    }
    if lower.contains("登录") || lower.contains("未登录") || lower.contains("avs") {
        return CommandErrorKind::Auth;
    }
    if lower.contains("磁盘") || lower.contains("空间") || lower.contains("no space") {
        return CommandErrorKind::Storage;
    }
    if lower.contains("network") || lower.contains("连接") || lower.contains("dns") {
        return CommandErrorKind::Network;
    }
    if lower.contains("不存在") || lower.contains("not found") || lower.contains("已下架") {
        return CommandErrorKind::NotFound;
    }
    CommandErrorKind::Unknown
}

fn kind_for_status(status: reqwest::StatusCode) -> CommandErrorKind {
    match status.as_u16() {
        401 | 403 => CommandErrorKind::Auth,
        404 | 410 => CommandErrorKind::NotFound,
        408 | 504 => CommandErrorKind::Timeout,
        500..=599 => CommandErrorKind::Network,
        _ => CommandErrorKind::Unknown,
    }
}

fn kind_for_io(kind: std::io::ErrorKind) -> CommandErrorKind {
    match kind {
        std::io::ErrorKind::TimedOut => CommandErrorKind::Timeout,
        std::io::ErrorKind::NotFound => CommandErrorKind::NotFound,
        std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::StorageFull => {
            CommandErrorKind::Storage
        }
        std::io::ErrorKind::ConnectionRefused
        | std::io::ErrorKind::ConnectionReset
        | std::io::ErrorKind::ConnectionAborted
        | std::io::ErrorKind::NotConnected => CommandErrorKind::Network,
        _ => CommandErrorKind::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn io_errors_map_to_actionable_kinds() {
        let disk = anyhow::Error::from(std::io::Error::from(std::io::ErrorKind::StorageFull));
        assert_eq!(classify(&disk), CommandErrorKind::Storage);

        let missing = anyhow::Error::from(std::io::Error::from(std::io::ErrorKind::NotFound));
        assert_eq!(classify(&missing), CommandErrorKind::NotFound);

        let reset = anyhow::Error::from(std::io::Error::from(std::io::ErrorKind::ConnectionReset));
        assert_eq!(classify(&reset), CommandErrorKind::Network);
    }

    #[test]
    fn chinese_messages_are_classified_conservatively() {
        let auth = anyhow::anyhow!("尚未登录，请先登录");
        assert_eq!(classify(&auth), CommandErrorKind::Auth);

        let timeout = anyhow::anyhow!("请求超时，请重试");
        assert_eq!(classify(&timeout), CommandErrorKind::Timeout);

        let unknown = anyhow::anyhow!("序列化失败的奇怪问题");
        assert_eq!(classify(&unknown), CommandErrorKind::Unknown);
    }

    #[test]
    fn explicit_kind_wins_over_inference() {
        let error = CommandError::with_kind(
            "读取失败",
            anyhow::anyhow!("whatever"),
            CommandErrorKind::NotFound,
        );
        assert_eq!(error.kind, CommandErrorKind::NotFound);
    }
}
