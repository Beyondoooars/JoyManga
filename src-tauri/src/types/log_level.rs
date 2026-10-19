use serde::{Deserialize, Serialize};
use specta::Type;

/// 日志等级。
///
/// `Trace`~`Error` 的 serde 字符串与 tracing JSON 输出的 `level` 字段一致，
/// 既用于 `LogEvent` 的等级反序列化，也用于设置页的文件日志收集等级；
/// `Off` 只在收集等级场景出现（表示不落盘），默认收集 `Info` 及以上。
#[derive(Debug, Default, Copy, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub enum LogLevel {
    #[serde(rename = "TRACE")]
    Trace,
    #[serde(rename = "DEBUG")]
    Debug,
    #[serde(rename = "INFO")]
    #[default]
    Info,
    #[serde(rename = "WARN")]
    Warn,
    #[serde(rename = "ERROR")]
    Error,
    /// 不写入文件日志（仍输出到前端事件层）。仅用于设置页的收集等级。
    #[serde(rename = "OFF")]
    Off,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serde_roundtrip_matches_tracing_level_strings() {
        for (value, expected) in [
            (LogLevel::Trace, "TRACE"),
            (LogLevel::Debug, "DEBUG"),
            (LogLevel::Info, "INFO"),
            (LogLevel::Warn, "WARN"),
            (LogLevel::Error, "ERROR"),
            (LogLevel::Off, "OFF"),
        ] {
            assert_eq!(
                serde_json::to_string(&value).unwrap(),
                format!("\"{expected}\"")
            );
            assert_eq!(
                serde_json::from_str::<LogLevel>(&format!("\"{expected}\"")).unwrap(),
                value
            );
        }
    }

    #[test]
    fn default_collects_info_level() {
        assert_eq!(LogLevel::default(), LogLevel::Info);
        // 旧配置文件缺 logLevel 字段时反序列化为默认值
        assert_eq!(
            serde_json::from_str::<LogLevel>("null").unwrap_or_default(),
            LogLevel::Info
        );
    }
}
