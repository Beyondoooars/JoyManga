use serde::{Deserialize, Serialize};
use specta::Type;

/// 失败原因分类：用于界面提示、重试决策与诊断包聚合。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type, Default)]
#[serde(rename_all = "camelCase")]
pub enum FailureKind {
    /// 超时、DNS、TLS、连接被重置、代理不通
    Network,
    /// 5xx、网关页、限流、被墙、域名失效
    Upstream,
    /// JSON / HTML / 图片 / 归档内容解析失败
    Parse,
    /// 权限、磁盘满、IO、文件锁
    Storage,
    /// 401/404、章节不可用等业务语义失败
    Business,
    #[default]
    Unknown,
}

/// 任务当前正在执行的步骤。与 `DownloadTaskState`（生命周期）互补，不互相替代。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type, Default)]
#[serde(rename_all = "camelCase")]
pub enum TaskStage {
    #[default]
    Queued,
    FetchMeta,
    Downloading,
    Verifying,
    Archiving,
    Done,
}

/// 归档整合的子阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum ArchiveStage {
    Collecting,
    Sorting,
    Packing,
    Finalizing,
    Cleaning,
}

/// 一次可诊断的失败详情。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct TaskFailure {
    pub kind: FailureKind,
    /// 用户可读信息，最多 2048 字符。
    pub message: String,
    pub retryable: bool,
    pub stage: TaskStage,
    pub attempts: u32,
    /// 只记录线路主机名，不记录完整 URL（避免签名参数进入日志/诊断包）。
    pub line: Option<String>,
    /// 毫秒时间戳。
    pub at: i64,
}

/// 同一任务内按类型聚合的图片失败计数。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct FailureCount {
    pub kind: FailureKind,
    pub count: u32,
}

pub const MAX_FAILURE_MESSAGE_CHARS: usize = 2048;

pub fn truncate_failure_message(message: &str) -> String {
    message.chars().take(MAX_FAILURE_MESSAGE_CHARS).collect()
}

pub fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failure_message_is_truncated_by_chars_not_bytes() {
        let message = "错".repeat(MAX_FAILURE_MESSAGE_CHARS + 100);
        let truncated = truncate_failure_message(&message);
        assert_eq!(truncated.chars().count(), MAX_FAILURE_MESSAGE_CHARS);
    }

    #[test]
    fn default_stage_is_queued_and_kind_is_unknown() {
        assert_eq!(TaskStage::default(), TaskStage::Queued);
        assert_eq!(FailureKind::default(), FailureKind::Unknown);
    }
}
