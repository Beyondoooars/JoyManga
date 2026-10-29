use std::path::PathBuf;

use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::archive_intents::ArchiveIntentStore;
use crate::config::Config;
use crate::download_manager::DownloadTaskState;
use crate::download_queue::DownloadTaskSnapshot;
use crate::extensions::AppHandleExt;
use crate::types::{
    now_millis, CbzMode, FailureCount, LogLevel, ProxyMode, ReadingDirection, ReadingMode,
    TaskFailure, TaskStage,
};

const MAX_REPORT_BYTES: usize = 5 * 1024 * 1024;
const MAX_LOG_BYTES: usize = 4 * 1024 * 1024;
const MAX_LOG_FILE_BYTES: usize = 512 * 1024;
const MAX_CRASH_FILE_BYTES: usize = 256 * 1024;
const MAX_FRONTEND_LOG_BYTES: usize = 1024 * 1024;
const MAX_FRONTEND_LOG_LINES: usize = 5_000;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsReport {
    pub generated_at: i64,
    pub app: AppInfo,
    pub device: DeviceInfo,
    pub network: NetworkInfo,
    pub config: ConfigInfo,
    pub queue: Vec<QueueTaskInfo>,
    pub archive_intents: Vec<ArchiveIntentInfo>,
    pub logs: Vec<LogFileInfo>,
    pub frontend_logs: Vec<String>,
    pub crashes: Vec<CrashInfo>,
    pub notes: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub version: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceInfo {
    pub os: &'static str,
    pub arch: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkInfo {
    pub api_domain: String,
    pub image_domain: String,
    /// 系统是否报告为计费网络；`known == false` 表示还没收到 Android 侧事件。
    pub metered: bool,
    pub metered_known: bool,
    pub vpn: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigInfo {
    /// 是否开启了自动登录；只报告配置意图，不暴露账号密码。
    pub auto_login: bool,
    /// 账号 / 密码是否已保存；排查自动登录是否因缺少凭据而未触发。
    pub username_saved: bool,
    pub password_saved: bool,
    pub download_dir: PathBuf,
    pub export_dir: PathBuf,
    pub proxy_mode: ProxyMode,
    pub proxy_host: String,
    pub proxy_port: u16,
    pub chapter_concurrency: usize,
    pub img_concurrency: usize,
    pub api_domain_mode: crate::config::ApiDomainMode,
    pub custom_api_domain: String,
    pub image_domain_mode: crate::config::ImageDomainMode,
    pub custom_image_domain: String,
    pub default_cbz_mode: CbzMode,
    pub reading_direction: ReadingDirection,
    pub reading_mode: ReadingMode,
    pub keep_screen_on: bool,
    pub volume_key_paging: bool,
    pub download_only_wifi: bool,
    pub log_level: LogLevel,
}

impl From<&Config> for ConfigInfo {
    fn from(config: &Config) -> Self {
        Self {
            auto_login: config.auto_login,
            username_saved: !config.username.is_empty(),
            password_saved: !config.password.is_empty(),
            download_dir: config.download_dir.clone(),
            export_dir: config.export_dir.clone(),
            proxy_mode: config.proxy_mode.clone(),
            proxy_host: config.proxy_host.clone(),
            proxy_port: config.proxy_port,
            chapter_concurrency: config.chapter_concurrency,
            img_concurrency: config.img_concurrency,
            api_domain_mode: config.api_domain_mode,
            custom_api_domain: config.custom_api_domain.clone(),
            image_domain_mode: config.image_domain_mode,
            custom_image_domain: config.custom_image_domain.clone(),
            default_cbz_mode: config.default_cbz_mode,
            reading_direction: config.reading_direction,
            reading_mode: config.reading_mode,
            keep_screen_on: config.keep_screen_on,
            volume_key_paging: config.volume_key_paging,
            download_only_wifi: config.download_only_wifi,
            log_level: config.log_level,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueTaskInfo {
    pub task_id: String,
    pub comic_id: i64,
    pub comic_name: String,
    pub chapter_id: i64,
    pub chapter_title: String,
    pub state: DownloadTaskState,
    pub stage: TaskStage,
    pub downloaded_img_count: u32,
    pub total_img_count: u32,
    pub attempts: u32,
    pub failure: Option<TaskFailure>,
    pub image_failures: Vec<FailureCount>,
    pub error_message: Option<String>,
}

impl From<DownloadTaskSnapshot> for QueueTaskInfo {
    fn from(snapshot: DownloadTaskSnapshot) -> Self {
        Self {
            task_id: snapshot.task_id,
            comic_id: snapshot.comic.id,
            comic_name: snapshot.comic.name.clone(),
            chapter_id: snapshot.chapter_info.chapter_id,
            chapter_title: snapshot.chapter_info.chapter_title.clone(),
            state: snapshot.state,
            stage: snapshot.stage,
            downloaded_img_count: snapshot.downloaded_img_count,
            total_img_count: snapshot.total_img_count,
            attempts: snapshot.attempts,
            failure: snapshot.failure,
            image_failures: snapshot.image_failures,
            error_message: snapshot.error_message,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveIntentInfo {
    pub comic_id: i64,
    pub mode: CbzMode,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogFileInfo {
    pub file: String,
    pub content: String,
    pub truncated: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CrashInfo {
    pub file: String,
    pub size: u64,
    pub content: String,
}

/// 组装诊断包。只收集定位问题所需的字段：完整配置里的账号、密码、cookie
/// 一律不进入报告；日志和 URL 在写入前再做一次脱敏。
pub fn build_report(
    app: &AppHandle,
    frontend_logs: Option<Vec<String>>,
) -> anyhow::Result<DiagnosticsReport> {
    let config = app.get_config().read().clone();
    let queue = app
        .get_download_manager()
        .snapshots()
        .into_iter()
        .map(QueueTaskInfo::from)
        .collect();
    let archive_intents = app
        .try_state::<ArchiveIntentStore>()
        .map(|store| {
            store
                .entries()
                .into_iter()
                .map(|(comic_id, mode)| ArchiveIntentInfo { comic_id, mode })
                .collect()
        })
        .unwrap_or_default();
    let mut notes = Vec::new();
    let logs = collect_logs(app, &mut notes);
    let crashes = collect_crashes(app, &mut notes);
    let frontend_logs = collect_frontend_logs(frontend_logs, &mut notes);

    let mut report = DiagnosticsReport {
        generated_at: now_millis(),
        app: AppInfo {
            version: app.package_info().version.to_string(),
        },
        device: DeviceInfo {
            os: std::env::consts::OS,
            arch: std::env::consts::ARCH,
        },
        network: NetworkInfo {
            api_domain: config.get_api_domain(),
            image_domain: config.get_image_domain(),
            metered: crate::network_state::is_metered(),
            metered_known: crate::network_state::is_known(),
            vpn: crate::network_state::is_vpn_active(),
        },
        config: ConfigInfo::from(&config),
        queue,
        archive_intents,
        logs,
        frontend_logs,
        crashes,
        notes,
    };

    // 极端情况下（日志很多、崩溃报告带大 backtrace）可能超过 5MB；先去掉正文，
    // 保留任务与失败详情，保证诊断包仍然可解析、可发送。
    let bytes = serde_json::to_vec_pretty(&report)?;
    if bytes.len() > MAX_REPORT_BYTES {
        report.logs.clear();
        report.frontend_logs.clear();
        for crash in &mut report.crashes {
            crash.content.clear();
        }
        report
            .notes
            .push("报告超过 5MB，已省略日志与崩溃正文".to_string());
    }
    Ok(report)
}

fn collect_logs(app: &AppHandle, notes: &mut Vec<String>) -> Vec<LogFileInfo> {
    let Ok(logs_dir) = crate::logger::logs_dir(app) else {
        notes.push("日志目录不可用".to_string());
        return Vec::new();
    };
    let retention = std::time::Duration::from_secs(7 * 24 * 60 * 60);
    let now = std::time::SystemTime::now();
    let mut files: Vec<(String, PathBuf, std::time::SystemTime)> = std::fs::read_dir(&logs_dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_name()?.to_str()?.to_string();
            let is_rolling = name.starts_with(crate::logger::LOG_FILE_PREFIX)
                && name.ends_with(".log")
                && !name.starts_with(crate::logger::DIAGNOSTIC_LOG_PREFIX);
            if !is_rolling || !entry.file_type().ok()?.is_file() {
                return None;
            }
            let modified = entry.metadata().ok()?.modified().ok()?;
            if now.duration_since(modified).unwrap_or_default() > retention {
                return None;
            }
            Some((name, path, modified))
        })
        .collect();
    files.sort_by_key(|(_, _, modified)| std::cmp::Reverse(*modified));

    let mut logs = Vec::new();
    let mut total_bytes = 0usize;
    for (file, path, _) in files {
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        let mut content = redact_text(&content);
        let mut truncated = false;
        if content.len() > MAX_LOG_FILE_BYTES {
            content.truncate(floor_char_boundary(&content, MAX_LOG_FILE_BYTES));
            truncated = true;
        }
        if total_bytes + content.len() > MAX_LOG_BYTES {
            notes.push(format!("日志总量超过 4MB，已省略较早文件（从 {file} 起）"));
            break;
        }
        total_bytes += content.len();
        logs.push(LogFileInfo {
            file,
            content,
            truncated,
        });
    }
    logs
}

fn collect_crashes(app: &AppHandle, notes: &mut Vec<String>) -> Vec<CrashInfo> {
    let Ok(logs_dir) = crate::logger::logs_dir(app) else {
        return Vec::new();
    };
    let mut crashes = Vec::new();
    let entries = match std::fs::read_dir(&logs_dir) {
        Ok(entries) => entries,
        Err(_) => return crashes,
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let is_crash = name.starts_with(crate::logger::CRASH_LOG_PREFIX)
            || (name.starts_with("panic") && name.ends_with(".log"));
        if !is_crash {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let mut content = std::fs::read_to_string(&path)
            .map(|content| redact_text(&content))
            .unwrap_or_default();
        let truncated = content.len() > MAX_CRASH_FILE_BYTES;
        if truncated {
            content.truncate(floor_char_boundary(&content, MAX_CRASH_FILE_BYTES));
            notes.push(format!("崩溃报告 {name} 超过 256KB，已截断"));
        }
        crashes.push(CrashInfo {
            file: name.to_string(),
            size: metadata.len(),
            content,
        });
    }
    crashes.sort_by(|left, right| left.file.cmp(&right.file));
    crashes
}

fn collect_frontend_logs(
    frontend_logs: Option<Vec<String>>,
    notes: &mut Vec<String>,
) -> Vec<String> {
    let Some(frontend_logs) = frontend_logs else {
        return Vec::new();
    };
    let mut total_bytes = 0usize;
    let mut logs = Vec::new();
    for line in frontend_logs.into_iter().take(MAX_FRONTEND_LOG_LINES) {
        let line = redact_text(&line.chars().take(4_000).collect::<String>());
        if total_bytes + line.len() > MAX_FRONTEND_LOG_BYTES {
            notes.push("前端日志超过 1MB，已省略较早记录".to_string());
            break;
        }
        total_bytes += line.len();
        logs.push(line);
    }
    logs
}

/// 日志与诊断包共用的脱敏：去掉 URL 查询串，替换敏感键的值。
pub fn redact_text(text: &str) -> String {
    text.lines()
        .map(|line| redact_secrets(&strip_url_queries(line)))
        .collect::<Vec<_>>()
        .join("\n")
}

const SENSITIVE_KEYS: [&str; 8] = [
    "password",
    "passwd",
    "pwd",
    "cookie",
    "authorization",
    "token",
    "avs",
    "username",
];

fn strip_url_queries(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(index) = find_url_start(rest) {
        let (before, after) = rest.split_at(index);
        out.push_str(before);
        let end = after
            .find(|character: char| {
                character.is_whitespace() || matches!(character, '"' | '\'' | ')' | ']' | ',' | ';')
            })
            .unwrap_or(after.len());
        let token = &after[..end];
        match token.find('?') {
            Some(query) => {
                out.push_str(&token[..query]);
                out.push_str("?<redacted>");
            }
            None => out.push_str(token),
        }
        rest = &after[end..];
    }
    out.push_str(rest);
    out
}

fn find_url_start(text: &str) -> Option<usize> {
    match (text.find("http://"), text.find("https://")) {
        (Some(http), Some(https)) => Some(http.min(https)),
        (http, https) => http.or(https),
    }
}

fn redact_secrets(line: &str) -> String {
    let lower = line.to_ascii_lowercase();
    let lower_bytes = lower.as_bytes();
    let bytes = line.as_bytes();
    let mut out = String::with_capacity(line.len());
    let mut cursor = 0usize;
    let mut index = 0usize;

    while index < lower_bytes.len() {
        let Some(key) = SENSITIVE_KEYS
            .iter()
            .find(|key| lower_bytes[index..].starts_with(key.as_bytes()))
        else {
            index += 1;
            continue;
        };
        let mut pos = index + key.len();
        while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
            pos += 1;
        }
        // JSON / JS 对象里的键通常带引号："password": "secret"。
        if pos < bytes.len() && matches!(bytes[pos], b'"' | b'\'') {
            let mut after_quote = pos + 1;
            while after_quote < bytes.len() && bytes[after_quote].is_ascii_whitespace() {
                after_quote += 1;
            }
            if after_quote < bytes.len() && matches!(bytes[after_quote], b'=' | b':') {
                pos = after_quote;
            } else {
                index += key.len();
                continue;
            }
        }
        if pos < bytes.len() && matches!(bytes[pos], b'=' | b':') {
            pos += 1;
            while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
                pos += 1;
            }
            let quote = if pos < bytes.len() && matches!(bytes[pos], b'"' | b'\'') {
                let quote = bytes[pos];
                pos += 1;
                Some(quote)
            } else {
                None
            };
            let value_start = pos;
            match quote {
                Some(quote) => {
                    while pos < bytes.len() && bytes[pos] != quote {
                        // 跳过反斜杠转义，避免把 \" 当成值的结束。
                        pos += if bytes[pos] == b'\\' && pos + 1 < bytes.len() {
                            2
                        } else {
                            1
                        };
                    }
                }
                None => {
                    while pos < bytes.len() && !is_secret_value_delimiter(bytes[pos]) {
                        pos += 1;
                    }
                }
            }
            if value_start < pos {
                out.push_str(&line[cursor..value_start]);
                out.push_str("***");
                cursor = pos;
                index = pos;
                continue;
            }
        }
        index += key.len();
    }
    out.push_str(&line[cursor..]);
    out
}

fn is_secret_value_delimiter(byte: u8) -> bool {
    byte.is_ascii_whitespace() || matches!(byte, b'&' | b',' | b';' | b'"' | b'\'' | b'}' | b']')
}

fn floor_char_boundary(text: &str, max_bytes: usize) -> usize {
    if max_bytes >= text.len() {
        return text.len();
    }
    let mut index = max_bytes;
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_queries_are_removed_but_paths_and_hosts_are_kept() {
        let line = "GET https://cdn.example/media/1.jpg?token=abc&sig=1 failed";
        let redacted = redact_text(line);
        assert_eq!(
            redacted,
            "GET https://cdn.example/media/1.jpg?<redacted> failed"
        );
        assert!(!redacted.contains("token=abc"));
    }

    #[test]
    fn multiple_urls_and_bracketed_urls_are_redacted() {
        let line = "a https://x/1?x=1 b http://y/2?y=2) c";
        let redacted = redact_text(line);
        assert_eq!(
            redacted,
            "a https://x/1?<redacted> b http://y/2?<redacted>) c"
        );
    }

    #[test]
    fn sensitive_values_are_replaced() {
        let line = "login username=alice password=secret cookie=abc123 token: xyz";
        let redacted = redact_text(line);
        assert_eq!(
            redacted,
            "login username=*** password=*** cookie=*** token: ***"
        );
    }

    #[test]
    fn json_style_sensitive_values_are_replaced() {
        let line = r#"{"username":"alice","password":"s3cret","token": "abc","ok":true}"#;
        assert_eq!(
            redact_text(line),
            r#"{"username":"***","password":"***","token": "***","ok":true}"#
        );
    }

    #[test]
    fn quoted_single_values_and_escaped_quotes_are_replaced() {
        let line = r#"password='s3cr\'et' username: 'bob'"#;
        assert_eq!(redact_text(line), "password='***' username: '***'");
    }

    #[test]
    fn config_info_reports_credential_presence_without_values() {
        let mut config = Config::default(std::path::Path::new("test-app-data"));
        config.auto_login = true;
        config.username = "alice".to_string();
        config.password = "secret".to_string();

        let info = ConfigInfo::from(&config);
        assert!(info.auto_login);
        assert!(info.username_saved);
        assert!(info.password_saved);

        let json = serde_json::to_string(&info).unwrap();
        assert!(!json.contains("alice"));
        assert!(!json.contains("secret"));
    }

    #[test]
    fn plain_text_is_unchanged() {
        let line = "第 3 张图片读取超时，请重试";
        assert_eq!(redact_text(line), line);
    }

    #[test]
    fn truncation_respects_utf8_boundaries() {
        let text = "错".repeat(10);
        let index = floor_char_boundary(&text, 4);
        assert!(text.is_char_boundary(index));
        assert_eq!(&text[..index], "错");
    }
}
