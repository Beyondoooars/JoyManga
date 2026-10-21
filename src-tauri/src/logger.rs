use std::{io::Write, sync::OnceLock, time::Duration};

use anyhow::Context;
use notify::{RecommendedWatcher, Watcher};
use tauri::{AppHandle, Manager};
use tauri_specta::Event;
use tracing::{Level, Subscriber};
use tracing_appender::{
    non_blocking::WorkerGuard,
    rolling::{RollingFileAppender, Rotation},
};
use tracing_subscriber::{
    filter::{filter_fn, FilterExt, LevelFilter, Targets},
    fmt::{layer, time::LocalTime},
    layer::SubscriberExt,
    registry::LookupSpan,
    util::SubscriberInitExt,
    Layer, Registry,
};

use crate::{
    config::ConfigStorage,
    events::LogEvent,
    extensions::{AnyhowErrorToStringChain, AppHandleExt},
    types::LogLevel,
};

struct LogEventWriter {
    app: AppHandle,
}

impl Write for LogEventWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let log_string = String::from_utf8_lossy(buf);
        match serde_json::from_str::<LogEvent>(&log_string) {
            Ok(log_event) => {
                let _ = log_event.emit(&self.app);
            }
            Err(err) => {
                let log_string = log_string.to_string();
                let err_msg = err.to_string();
                tracing::error!(log_string, err_msg, "将日志字符串解析为LogEvent失败");
            }
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

static RELOAD_FN: OnceLock<Box<dyn Fn(LogLevel) -> anyhow::Result<()> + Send + Sync>> =
    OnceLock::new();
static GUARD: OnceLock<parking_lot::Mutex<Option<WorkerGuard>>> = OnceLock::new();
/// 串行化文件日志层的重载（设置保存与 watcher 事件可能并发）。
static RELOAD_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());
/// 文件日志 watcher 是否已启动（Off 启动、之后切回其他等级时需要补启动）。
static WATCHER_STARTED: OnceLock<std::sync::atomic::AtomicBool> = OnceLock::new();
/// 启动时保存的 AppHandle，供运行期补启动 watcher 使用。
static APP_HANDLE: OnceLock<AppHandle> = OnceLock::new();
/// 当前文件日志收集等级：供 watcher 重建文件层与定时清理任务读取，
/// 随设置页切换更新，而不是停留在启动时的等级。
static CURRENT_LEVEL: OnceLock<parking_lot::RwLock<LogLevel>> = OnceLock::new();

fn current_level() -> LogLevel {
    CURRENT_LEVEL
        .get()
        .map(|cell| *cell.read())
        .unwrap_or(LogLevel::Off)
}

pub fn init(app: &AppHandle) -> anyhow::Result<()> {
    let lib_module_path = module_path!();
    let lib_target = lib_module_path.split("::").next().context(format!(
        "解析lib_target失败: lib_module_path={lib_module_path}"
    ))?;
    // 过滤掉来自其他库的日志
    let target_filter = Targets::new().with_target(lib_target, Level::TRACE);
    let level = app.get_config().read().log_level;
    CURRENT_LEVEL.get_or_init(|| parking_lot::RwLock::new(level));
    let _ = APP_HANDLE.set(app.clone());
    // 输出到文件
    let (file_layer, guard) = create_file_layer(app, level)?;
    let (reloadable_file_layer, reload_handle) = tracing_subscriber::reload::Layer::new(file_layer);
    // 输出到控制台
    let console_layer = layer()
        .with_writer(std::io::stdout)
        .with_timer(LocalTime::rfc_3339())
        .with_file(true)
        .with_line_number(true);
    // 发送到前端
    let log_event_writer = std::sync::Mutex::new(LogEventWriter { app: app.clone() });
    let log_event_layer = layer()
        .with_writer(log_event_writer)
        .with_timer(LocalTime::rfc_3339())
        .with_file(true)
        .with_line_number(true)
        .json()
        // 过滤掉来自这个文件的日志(LogEvent解析失败的日志)，避免无限递归
        .with_filter(target_filter.clone().and(filter_fn(|metadata| {
            metadata.module_path() != Some(lib_module_path)
        })));

    Registry::default()
        .with(target_filter)
        .with(reloadable_file_layer)
        .with(console_layer)
        .with(log_event_layer)
        .init();

    GUARD.get_or_init(|| parking_lot::Mutex::new(guard));
    RELOAD_FN.get_or_init(move || {
        let app = app.clone();
        Box::new(move |level: LogLevel| {
            // 串行化「安装新层 + 替换 guard」：并发重载（设置保存与 watcher 事件）
            // 若交错执行，会把当前层的 guard 换成另一层的，丢弃后写入线程被关闭，
            // 文件日志会静默停止。
            let _reload_guard = RELOAD_LOCK.lock();
            let (file_layer, guard) = create_file_layer(&app, level)?;
            reload_handle.reload(file_layer).context("reload失败")?;
            *GUARD.get().context("GUARD未初始化")?.lock() = guard;
            Ok(())
        })
    });
    // watcher 只在需要写文件日志时启动：Off 时连 inotify 监听都不建立。
    // 定时清理常驻，但每轮动作前按当前等级跳过，Off 时不产生任何 I/O。
    if level != LogLevel::Off {
        ensure_file_log_watcher(app);
    }
    tauri::async_runtime::spawn(auto_cleanup_logs_task(app.clone()));
    // 启动时清理一次过期日志，避免 Off 期间堆积的崩溃报告长期占位。
    // 这是启动阶段唯一的清理入口（create_file_layer 已不再顺带清理）。
    if let Ok(dir) = logs_dir(app) {
        cleanup_managed_logs(
            &dir,
            DEFAULT_LOG_RETENTION,
            MAX_LOGS_TOTAL_SIZE,
            TARGET_LOGS_TOTAL_SIZE,
        );
    }

    Ok(())
}

/// 切换文件日志收集等级：设置页保存后立即生效，无需重启。
pub fn apply_log_level(level: LogLevel) {
    // 先重载再更新状态：重载失败时 CURRENT_LEVEL 仍指向实际生效的等级，
    // watcher/清理任务不会按一个并不存在的等级工作。
    match reload_file_logger(level) {
        Ok(()) => {
            if let Some(cell) = CURRENT_LEVEL.get() {
                *cell.write() = level;
            }
            if level != LogLevel::Off {
                if let Some(app) = APP_HANDLE.get() {
                    ensure_file_log_watcher(app);
                }
            }
        }
        Err(err) => {
            tracing::error!(error = %err, level = ?level, "切换文件日志等级失败");
        }
    }
}

pub fn reload_file_logger(level: LogLevel) -> anyhow::Result<()> {
    RELOAD_FN.get().context("RELOAD_FN未初始化")?(level)
}

/// 启动文件日志 watcher（只启动一次）。从 Off 切回其他等级时补启动。
fn ensure_file_log_watcher(app: &AppHandle) {
    if WATCHER_STARTED
        .get_or_init(|| std::sync::atomic::AtomicBool::new(false))
        .swap(true, std::sync::atomic::Ordering::SeqCst)
    {
        return;
    }
    tauri::async_runtime::spawn(file_log_watcher(app.clone()));
}

/// 文件日志收集等级 → tracing `LevelFilter`；`Off` 对应无文件输出。
fn level_filter(level: LogLevel) -> LevelFilter {
    match level {
        LogLevel::Off => LevelFilter::OFF,
        LogLevel::Trace => LevelFilter::TRACE,
        LogLevel::Debug => LevelFilter::DEBUG,
        LogLevel::Info => LevelFilter::INFO,
        LogLevel::Warn => LevelFilter::WARN,
        LogLevel::Error => LevelFilter::ERROR,
    }
}

fn create_file_layer<S>(
    app: &AppHandle,
    level: LogLevel,
) -> anyhow::Result<(Box<dyn Layer<S> + Send + Sync>, Option<WorkerGuard>)>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    // 关闭收集时返回占位用的sink layer，不创建也不输出日志文件。
    // 必须带上 OFF 过滤：全局 target 过滤放行 TRACE，否则每条 trace/debug 仍会
    // 完整格式化（含时区转换）后再丢进 io::sink，关闭日志并不能省下 CPU。
    if level == LogLevel::Off {
        let sink_layer = layer()
            .with_writer(std::io::sink)
            .with_timer(LocalTime::rfc_3339())
            .with_ansi(false)
            .with_file(true)
            .with_line_number(true)
            .with_filter(LevelFilter::OFF);
        return Ok((Box::new(sink_layer), None));
    }
    // 注意：这里不做日志清理。清理只发生在启动（init）、定时任务与用户手动清理时；
    // 若放在这里，watcher 每次收到文件删除事件都会触发一次全量扫描并顺带删掉
    // crash 报告等旧日志，用户在文件管理器删一个日志就会误伤其他诊断文件。
    let logs_dir = logs_dir(app).context("获取日志目录失败")?;
    let file_appender = RollingFileAppender::builder()
        .filename_prefix(LOG_FILE_PREFIX)
        .filename_suffix("log")
        .rotation(Rotation::DAILY)
        .build(&logs_dir)
        .context("创建RollingFileAppender失败")?;
    let (non_blocking_appender, guard) = tracing_appender::non_blocking(file_appender);
    let file_layer = layer()
        .with_writer(non_blocking_appender)
        .with_timer(LocalTime::rfc_3339())
        .with_ansi(false)
        .with_file(true)
        .with_line_number(true)
        .with_filter(level_filter(level));
    Ok((Box::new(file_layer), Some(guard)))
}

async fn file_log_watcher(app: AppHandle) {
    let (sender, mut receiver) = tokio::sync::mpsc::channel(64);

    let event_handler = move |res| {
        // notify的回调线程不能等待异步通道；队列满时丢弃普通文件变更事件即可。
        let _ = sender.try_send(res);
    };

    let mut watcher = match RecommendedWatcher::new(event_handler, notify::Config::default())
        .map_err(anyhow::Error::from)
    {
        Ok(watcher) => watcher,
        Err(err) => {
            let err_title = "创建日志文件watcher失败";
            let string_chain = err.to_string_chain();
            tracing::error!(err_title, message = string_chain);
            return;
        }
    };

    let logs_dir = match logs_dir(&app) {
        Ok(logs_dir) => logs_dir,
        Err(err) => {
            let err_title = "日志文件watcher获取日志目录失败";
            let string_chain = err.to_string_chain();
            tracing::error!(err_title, message = string_chain);
            return;
        }
    };

    if let Err(err) = std::fs::create_dir_all(&logs_dir) {
        let err_title = "创建日志目录失败";
        let string_chain = anyhow::Error::from(err).to_string_chain();
        tracing::error!(err_title, message = string_chain);
        return;
    }

    if let Err(err) = watcher
        .watch(&logs_dir, notify::RecursiveMode::NonRecursive)
        .map_err(anyhow::Error::from)
    {
        let err_title = "日志文件watcher监听日志目录失败";
        let string_chain = err.to_string_chain();
        tracing::error!(err_title, message = string_chain);
        return;
    }

    while let Some(res) = receiver.recv().await {
        match res.map_err(anyhow::Error::from) {
            Ok(event) => {
                if let notify::EventKind::Remove(_) = event.kind {
                    // 当前等级为 Off 时文件层是 sink，重建无意义，跳过。
                    let level = current_level();
                    if level != LogLevel::Off {
                        if let Err(err) = reload_file_logger(level) {
                            let err_title = "重置日志文件失败";
                            let string_chain = err.to_string_chain();
                            tracing::error!(err_title, message = string_chain);
                        }
                    }
                }
            }
            Err(err) => {
                let err_title = "接收日志文件watcher事件失败";
                let string_chain = err.to_string_chain();
                tracing::error!(err_title, message = string_chain);
            }
        }
    }
}

pub const DEFAULT_LOG_RETENTION: Duration = Duration::from_secs(7 * 24 * 60 * 60); // 7天
pub const MAX_LOGS_TOTAL_SIZE: u64 = 20 * 1024 * 1024; // 20 MB 配额上限
pub const TARGET_LOGS_TOTAL_SIZE: u64 = 10 * 1024 * 1024; // 10 MB 清理目标大小

/// 滚动日志文件名前缀：`<前缀>.YYYY-MM-DD.log`
pub const LOG_FILE_PREFIX: &str = "joymanga";
/// 诊断日志导出文件名前缀：`<前缀>-log-<时间戳>.txt`
pub const DIAGNOSTIC_LOG_PREFIX: &str = "joymanga-log-";
/// 崩溃报告前缀：`crash-<unix 秒>.log`，由 panic hook 在日志系统未就绪时直接写盘
pub const CRASH_LOG_PREFIX: &str = "crash-";

pub fn is_managed_log_file(file_name: &str) -> bool {
    let lower = file_name.to_ascii_lowercase();
    // 诊断日志前缀（`joymanga-log-`）本身以滚动日志前缀（`joymanga`）开头，
    // 因此滚动日志必须先排除诊断前缀，否则 `joymanga-log-*.log` 会被误判。
    let is_diagnostic_log = lower.starts_with(DIAGNOSTIC_LOG_PREFIX) && lower.ends_with(".txt");
    let is_rolling_log = !is_diagnostic_log
        && !lower.starts_with(DIAGNOSTIC_LOG_PREFIX)
        && lower.starts_with(LOG_FILE_PREFIX)
        && lower.ends_with(".log");
    is_rolling_log
        || is_diagnostic_log
        || (lower.starts_with("panic") && lower.ends_with(".log"))
        || (lower.starts_with(CRASH_LOG_PREFIX) && lower.ends_with(".log"))
}

pub fn cleanup_managed_logs(
    logs_dir: &std::path::Path,
    retention: Duration,
    max_total_size: u64,
    target_total_size: u64,
) {
    let Ok(entries) = std::fs::read_dir(logs_dir) else {
        return;
    };
    let now = std::time::SystemTime::now();
    let mut remaining_logs = Vec::new();
    let mut total_size = 0u64;

    // 第一阶段：清理超过过期时间的日志
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        let file_name = match path.file_name().and_then(|name| name.to_str()) {
            Some(name) => name,
            None => continue,
        };
        if !is_managed_log_file(file_name) {
            continue;
        }

        let metadata = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        let modified = metadata.modified().unwrap_or(std::time::UNIX_EPOCH);
        let age = now.duration_since(modified).unwrap_or_default();
        let size = metadata.len();

        if age > retention {
            let _ = std::fs::remove_file(&path);
        } else {
            total_size += size;
            remaining_logs.push((path, size, modified));
        }
    }

    // 第二阶段：如果剩余日志总大小超过限制，按修改时间从最旧到最新依次删除，直到降到目标大小以下
    if total_size > max_total_size && remaining_logs.len() > 1 {
        // 按修改时间升序排列（最旧的在前面）
        remaining_logs.sort_by_key(|(_, _, modified)| *modified);

        // 保留最新的一个日志文件不删除（通常是当前正在写入的日志）
        let can_delete_count = remaining_logs.len().saturating_sub(1);
        for (path, size, _) in &remaining_logs[..can_delete_count] {
            if total_size <= target_total_size {
                break;
            }
            if std::fs::remove_file(path).is_ok() {
                total_size = total_size.saturating_sub(*size);
            }
        }
    }
}

async fn auto_cleanup_logs_task(app: AppHandle) {
    let mut interval = tokio::time::interval(Duration::from_secs(30 * 60)); // 每30分钟自动检查清理一次
    loop {
        interval.tick().await;
        // 等级为 Off 时不产生任何清理 I/O（崩溃报告由启动时的那次清理兜底）。
        if current_level() == LogLevel::Off {
            continue;
        }
        if let Ok(dir) = logs_dir(&app) {
            cleanup_managed_logs(
                &dir,
                DEFAULT_LOG_RETENTION,
                MAX_LOGS_TOTAL_SIZE,
                TARGET_LOGS_TOTAL_SIZE,
            );
        }
    }
}

pub fn logs_dir(app: &AppHandle) -> anyhow::Result<std::path::PathBuf> {
    Ok(app.state::<ConfigStorage>().logs_dir().to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;

    #[test]
    fn test_is_managed_log_file() {
        assert!(is_managed_log_file("joymanga.2026-09-29.log"));
        assert!(is_managed_log_file("joymanga-log-1727610000.txt"));
        assert!(is_managed_log_file("panic.log"));
        // panic hook 写出的崩溃报告同样纳入清理，避免长期堆积
        assert!(is_managed_log_file("crash-1727610000.log"));
        assert!(!is_managed_log_file("config.json"));
        assert!(!is_managed_log_file("notes.txt"));
        // 导出的诊断日志是 .txt，不应与滚动日志混为一类
        assert!(!is_managed_log_file("joymanga-log-1727610000.log"));
    }

    #[test]
    fn test_cleanup_managed_logs_by_size_and_age() {
        let temp_dir = std::env::temp_dir().join(format!(
            "joymanga-test-log-cleanup-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&temp_dir).unwrap();

        let log1 = temp_dir.join("joymanga.2026-01-01.log");
        let log2 = temp_dir.join("joymanga.2026-01-02.log");
        let log3 = temp_dir.join("joymanga.2026-01-03.log");
        let other = temp_dir.join("important-unrelated.txt");

        let chunk = vec![b'a'; 1024]; // 1KB
        File::create(&log1).unwrap().write_all(&chunk).unwrap();
        File::create(&log2).unwrap().write_all(&chunk).unwrap();
        File::create(&log3).unwrap().write_all(&chunk).unwrap();
        File::create(&other).unwrap().write_all(&chunk).unwrap();

        // 限制总量不超过2.5KB，目标为1.5KB。log1, log2, log3 共3KB，应删除最旧的直到 <= 1.5KB，并保留最新的 log3
        cleanup_managed_logs(
            &temp_dir,
            Duration::from_secs(365 * 24 * 60 * 60), // 不过期
            2500,
            1500,
        );

        assert!(other.exists(), "非日志文件不应被删除");
        assert!(log3.exists(), "最新日志文件应被保留");

        let _ = std::fs::remove_dir_all(temp_dir);
    }
}
