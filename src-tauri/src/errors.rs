use serde::Serialize;
use specta::Type;

use crate::extensions::AnyhowErrorToStringChain;
use crate::types::{now_millis, truncate_failure_message, FailureKind, TaskFailure, TaskStage};

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
    if lower.contains("登录")
        || lower.contains("未登录")
        || lower.contains("avs")
        || lower.contains("密码错误")
        || lower.contains("密碼錯誤")
        || lower.contains("账号或密码")
        || lower.contains("帳號或密碼")
        || lower.contains("账号不存在")
        || lower.contains("帳號不存在")
    {
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

/// 将下载 / 归档链路上的错误归类为可诊断的 `TaskFailure`。
///
/// 与 `classify` 的区别：这里保留「上游 5xx」与「网络」的区分，并附带阶段、
/// 可重试性与线路，供任务快照和诊断包直接使用。
pub fn classify_failure(error: &anyhow::Error, stage: TaskStage) -> TaskFailure {
    let kind = failure_kind(error);
    TaskFailure {
        kind,
        message: truncate_failure_message(&format!("{error:#}")),
        retryable: is_retryable(kind, stage),
        stage,
        attempts: 1,
        line: None,
        at: now_millis(),
    }
}

/// 网络 / 上游错误值得重试；解析错误只在接口阶段（换线路或站点恢复后）可重试，
/// 图片解码和归档解析失败重试通常只会得到同样结果。
pub fn is_retryable(kind: FailureKind, stage: TaskStage) -> bool {
    match kind {
        FailureKind::Network | FailureKind::Upstream => true,
        FailureKind::Parse => stage == TaskStage::FetchMeta,
        FailureKind::Business | FailureKind::Storage | FailureKind::Unknown => false,
    }
}

fn failure_kind(error: &anyhow::Error) -> FailureKind {
    for cause in error.chain() {
        if let Some(err) = cause.downcast_ref::<reqwest::Error>() {
            return failure_kind_for_reqwest(err);
        }
        if let Some(err) = cause.downcast_ref::<reqwest_middleware::Error>() {
            return match err {
                reqwest_middleware::Error::Reqwest(err) => failure_kind_for_reqwest(err),
                reqwest_middleware::Error::Middleware(_) => FailureKind::Unknown,
            };
        }
        if let Some(err) = cause.downcast_ref::<std::io::Error>() {
            return failure_kind_for_io(err.kind());
        }
        if let Some(err) = cause.downcast_ref::<zip::result::ZipError>() {
            return match err {
                zip::result::ZipError::Io(_) => FailureKind::Storage,
                _ => FailureKind::Parse,
            };
        }
        if cause.downcast_ref::<image::ImageError>().is_some()
            || cause.downcast_ref::<serde_json::Error>().is_some()
        {
            return FailureKind::Parse;
        }
    }
    failure_kind_from_message(&format!("{error:#}"))
}

fn failure_kind_for_reqwest(err: &reqwest::Error) -> FailureKind {
    if err.is_timeout() || err.is_connect() || err.is_request() || err.is_body() {
        return FailureKind::Network;
    }
    if err.is_decode() {
        return FailureKind::Parse;
    }
    err.status()
        .map_or(FailureKind::Unknown, failure_kind_for_status)
}

fn failure_kind_for_status(status: reqwest::StatusCode) -> FailureKind {
    match status.as_u16() {
        401 | 403 | 404 | 410 => FailureKind::Business,
        408 | 429 | 500..=599 => FailureKind::Upstream,
        _ => FailureKind::Unknown,
    }
}

fn failure_kind_for_io(kind: std::io::ErrorKind) -> FailureKind {
    match kind {
        std::io::ErrorKind::TimedOut
        | std::io::ErrorKind::ConnectionRefused
        | std::io::ErrorKind::ConnectionReset
        | std::io::ErrorKind::ConnectionAborted
        | std::io::ErrorKind::NotConnected
        | std::io::ErrorKind::UnexpectedEof => FailureKind::Network,
        _ => FailureKind::Storage,
    }
}

/// 类型匹配不到时的兜底：宁可 Unknown，也不能把存储失败说成网络失败。
fn failure_kind_from_message(message: &str) -> FailureKind {
    let lower = message.to_lowercase();
    if lower.contains("timeout") || lower.contains("超时") {
        return FailureKind::Network;
    }
    if lower.contains("登录")
        || lower.contains("未登录")
        || lower.contains("avs")
        || lower.contains("401")
        || lower.contains("403")
        || lower.contains("404")
        || lower.contains("不存在")
        || lower.contains("已下架")
        || lower.contains("not found")
    {
        return FailureKind::Business;
    }
    if lower.contains("gateway")
        || lower.contains("cloudflare")
        || lower.contains("状态码`5")
        || lower.contains("status 5")
        || lower.contains("http 5")
        || lower.contains("429")
        || lower.contains("限流")
        || lower.contains("网关")
    {
        return FailureKind::Upstream;
    }
    if lower.contains("json")
        || lower.contains("html")
        || lower.contains("解析")
        || lower.contains("解码")
        || lower.contains("格式")
        || lower.contains("parse")
        || lower.contains("decode")
    {
        return FailureKind::Parse;
    }
    if lower.contains("磁盘")
        || lower.contains("空间")
        || lower.contains("权限")
        || lower.contains("存储")
        || lower.contains("写入")
        || lower.contains("no space")
        || lower.contains("permission")
        || lower.contains("read-only")
    {
        return FailureKind::Storage;
    }
    if lower.contains("network")
        || lower.contains("连接")
        || lower.contains("dns")
        || lower.contains("代理")
        || lower.contains("断流")
        || lower.contains("reset")
    {
        return FailureKind::Network;
    }
    FailureKind::Unknown
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

        // 站点登录接口的业务错误不带「登录」二字，仍应视为账号问题而不是可重试的网络错误
        let wrong_password = anyhow::anyhow!("帳號或密碼錯誤");
        assert_eq!(classify(&wrong_password), CommandErrorKind::Auth);

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

    #[test]
    fn failure_classification_prefers_error_types() {
        let disk = anyhow::Error::from(std::io::Error::from(std::io::ErrorKind::StorageFull));
        let failure = classify_failure(&disk, TaskStage::Downloading);
        assert_eq!(failure.kind, FailureKind::Storage);
        assert!(!failure.retryable);

        let reset = anyhow::Error::from(std::io::Error::from(std::io::ErrorKind::ConnectionReset));
        let failure = classify_failure(&reset, TaskStage::Downloading);
        assert_eq!(failure.kind, FailureKind::Network);
        assert!(failure.retryable);
    }

    #[test]
    fn failure_parse_retryability_depends_on_stage() {
        let error = anyhow::anyhow!("解析图片列表失败：expected value at line 1");
        let fetching = classify_failure(&error, TaskStage::FetchMeta);
        assert_eq!(fetching.kind, FailureKind::Parse);
        assert!(fetching.retryable);

        let downloading = classify_failure(&error, TaskStage::Downloading);
        assert_eq!(downloading.kind, FailureKind::Parse);
        assert!(!downloading.retryable);
    }

    #[test]
    fn failure_message_fallback_keeps_storage_and_upstream_apart() {
        let no_space = anyhow::anyhow!("no space left on device");
        assert_eq!(
            classify_failure(&no_space, TaskStage::Archiving).kind,
            FailureKind::Storage
        );

        let upstream = anyhow::anyhow!("下载图片失败，预料之外的状态码`503`: gateway");
        assert_eq!(
            classify_failure(&upstream, TaskStage::Downloading).kind,
            FailureKind::Upstream
        );

        let business = anyhow::anyhow!("章节不存在或已下架");
        let failure = classify_failure(&business, TaskStage::FetchMeta);
        assert_eq!(failure.kind, FailureKind::Business);
        assert!(!failure.retryable);
    }

    #[test]
    fn failure_carries_stage_and_timestamp() {
        let failure =
            classify_failure(&anyhow::anyhow!("请求超时，请重试"), TaskStage::Downloading);
        assert_eq!(failure.stage, TaskStage::Downloading);
        assert!(failure.at > 0);
        assert_eq!(failure.attempts, 1);
    }
}
