use std::{collections::VecDeque, io::Write, sync::OnceLock, time::Duration};

use anyhow::Context;
use notify::{RecommendedWatcher, Watcher};
use tauri::{AppHandle, Manager};
use tracing::{Level, Subscriber};
use tracing_appender::{
    non_blocking::WorkerGuard,
    rolling::{RollingFileAppender, Rotation},
};
use tracing_subscriber::{
    filter::{LevelFilter, Targets},
    fmt::{layer, time::LocalTime, MakeWriter},
    layer::SubscriberExt,
    registry::LookupSpan,
    util::SubscriberInitExt,
    Layer, Registry,
};

use crate::{
    config::ConfigStorage,
    extensions::{AnyhowErrorToStringChain, AppHandleExt},
    types::LogLevel,
};

const RECENT_LOG_MAX_LINES: usize = 500;
const RECENT_LOG_MAX_BYTES: usize = 256 * 1024;
const RECENT_LOG_LINE_MAX_CHARS: usize = 4_000;

#[derive(Default)]
struct RecentLogsBuffer {
    lines: VecDeque<String>,
    total_bytes: usize,
}

impl RecentLogsBuffer {
    fn push(&mut self, raw: &str) {
        for line in raw.lines() {
            let trimmed = line.trim_end_matches('\r');
            if trimmed.is_empty() {
                continue;
            }
            let entry = truncate_chars(trimmed, RECENT_LOG_LINE_MAX_CHARS);
            self.total_bytes += entry.len();
            self.lines.push_back(entry);
            while self.lines.len() > RECENT_LOG_MAX_LINES
                || (self.total_bytes > RECENT_LOG_MAX_BYTES && self.lines.len() > 1)
            {
                if let Some(removed) = self.lines.pop_front() {
                    self.total_bytes = self.total_bytes.saturating_sub(removed.len());
                }
            }
        }
    }

    fn snapshot(&self) -> Vec<String> {
        self.lines.iter().cloned().collect()
    }

    fn clear(&mut self) {
        self.lines.clear();
        self.total_bytes = 0;
    }
}

fn truncate_chars(input: &str, max_chars: usize) -> String {
    match input.char_indices().nth(max_chars) {
        Some((idx, _)) => format!("{}...(truncated)", &input[..idx]),
        None => input.to_string(),
    }
}

static RECENT_LOGS: OnceLock<parking_lot::Mutex<RecentLogsBuffer>> = OnceLock::new();

fn recent_logs_cell() -> &'static parking_lot::Mutex<RecentLogsBuffer> {
    RECENT_LOGS.get_or_init(|| parking_lot::Mutex::new(RecentLogsBuffer::default()))
}

/// 读取最近写入内存环形缓冲的后端日志（已按行切分与截断）。
pub fn recent_logs() -> Vec<String> {
    RECENT_LOGS
        .get()
        .map(|cell| cell.lock().snapshot())
        .unwrap_or_default()
}

/// 清空内存近期日志缓冲（用户清理日志时一并清空）。
pub fn clear_recent_logs() {
    if let Some(cell) = RECENT_LOGS.get() {
        cell.lock().clear();
    }
}

#[derive(Clone, Copy)]
struct RecentLogMakeWriter;

struct RecentLogWriter {
    buf: Vec<u8>,
}

impl Write for RecentLogWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.buf.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if !self.buf.is_empty() {
            let text = String::from_utf8_lossy(&self.buf);
            recent_logs_cell().lock().push(&text);
            self.buf.clear();
        }
        Ok(())
    }
}

impl Drop for RecentLogWriter {
    fn drop(&mut self) {
        let _ = self.flush();
    }
}

impl<'a> MakeWriter<'a> for RecentLogMakeWriter {
    type Writer = RecentLogWriter;

    fn make_writer(&'a self) -> Self::Writer {
        RecentLogWriter {
            buf: Vec::with_capacity(256),
        }
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
/// 已解析的日志目录。
///
/// 解析过程会在目标目录写一个探针文件确认可写，而 watcher 就在监听的目录里；
/// 每次调用都重新探测会「写探针 → watcher 收到 Remove → 重载 → 再写探针」无限循环。
/// 因此只解析一次并缓存；书库位置变化时由 `reset_logs_dir` 显式失效。
static LOGS_DIR: OnceLock<parking_lot::RwLock<Option<std::path::PathBuf>>> = OnceLock::new();

/// 通知 `file_log_watcher` 日志目录缓存已重置，重新绑定到最新目录。
static LOG_DIR_NOTIFY: OnceLock<tokio::sync::watch::Sender<u64>> = OnceLock::new();

fn log_dir_sender() -> &'static tokio::sync::watch::Sender<u64> {
    LOG_DIR_NOTIFY.get_or_init(|| tokio::sync::watch::channel(0).0)
}

/// 当前文件日志等级（供配置变更后切目录时判断是否需要重建文件层）。
pub fn current_log_level() -> LogLevel {
    current_level()
}

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
    // 崩溃报告跟着日志目录走：此时配置已加载，书库路径可用。
    // 日志目录退回配置目录时崩溃报告同样留在那里，两者总在同一处，方便一起反馈。
    if let Ok(dir) = logs_dir(app) {
        crate::crash::set_dir(&dir);
    }
    // 输出到文件
    let (file_layer, guard) = create_file_layer(app, level)?;
    let (reloadable_file_layer, reload_handle) = tracing_subscriber::reload::Layer::new(file_layer);
    // 输出到控制台（显式限级，避免 Off/Info 下仍格式化 TRACE）
    let console_level = if cfg!(debug_assertions) {
        LevelFilter::DEBUG
    } else {
        LevelFilter::WARN
    };
    let console_layer = layer()
        .with_writer(std::io::stdout)
        .with_timer(LocalTime::rfc_3339())
        .with_file(true)
        .with_line_number(true)
        .with_filter(console_level);
    // 内存近期日志环形缓冲（供诊断包在文件日志关闭或日志目录不可写时兜底）
    let recent_layer = layer()
        .with_writer(RecentLogMakeWriter)
        .with_timer(LocalTime::rfc_3339())
        .with_ansi(false)
        .with_file(true)
        .with_line_number(true)
        .with_filter(LevelFilter::INFO);

    Registry::default()
        .with(target_filter)
        .with(reloadable_file_layer)
        .with(console_layer)
        .with(recent_layer)
        .init();

    GUARD.get_or_init(|| parking_lot::Mutex::new(guard));
    RELOAD_FN.get_or_init(move || {
        let app = app.clone();
        Box::new(move |level: LogLevel| {
            // 串行化「安装新层 + 替换 guard」：并发重载（设置保存与 watcher 事件）
            // 若交错执行，会把当前层的 guard 换成另一层的，丢弃后写入线程被关闭，
            // 文件日志会静默停止。
            let _reload_guard = RELOAD_LOCK.lock();
            if level != LogLevel::Off {
                // 先切到空层并释放旧句柄：Windows 上若旧文件刚被删除（DELETE_PENDING），
                // 旧 WorkerGuard 未释放前重开同名文件会报拒绝访问。
                if let Ok((off_layer, _)) = create_file_layer(&app, LogLevel::Off) {
                    let _ = reload_handle.reload(off_layer);
                }
                if let Some(guard_cell) = GUARD.get() {
                    *guard_cell.lock() = None;
                }
            }
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

    let mut dir_rx = log_dir_sender().subscribe();
    let _ = *dir_rx.borrow_and_update();
    let mut watched_dir: Option<std::path::PathBuf> = None;

    let bind_dir = |watcher: &mut RecommendedWatcher,
                    watched_dir: &mut Option<std::path::PathBuf>| {
        let next_dir = match logs_dir(&app) {
            Ok(dir) => dir,
            Err(err) => {
                let err_title = "日志文件watcher获取日志目录失败";
                let string_chain = err.to_string_chain();
                tracing::error!(err_title, message = string_chain);
                return false;
            }
        };
        if watched_dir.as_ref() == Some(&next_dir) {
            return true;
        }
        if let Some(old_dir) = watched_dir.take() {
            let _ = watcher.unwatch(&old_dir);
        }
        if let Err(err) = std::fs::create_dir_all(&next_dir) {
            let err_title = "创建日志目录失败";
            let string_chain = anyhow::Error::from(err).to_string_chain();
            tracing::error!(err_title, message = string_chain);
            return false;
        }
        if let Err(err) = watcher
            .watch(&next_dir, notify::RecursiveMode::NonRecursive)
            .map_err(anyhow::Error::from)
        {
            let err_title = "日志文件watcher监听日志目录失败";
            let string_chain = err.to_string_chain();
            tracing::error!(err_title, message = string_chain);
            return false;
        }
        *watched_dir = Some(next_dir);
        true
    };

    if !bind_dir(&mut watcher, &mut watched_dir) {
        return;
    }

    loop {
        tokio::select! {
            changed = dir_rx.changed() => {
                if changed.is_err() {
                    break;
                }
                let _ = bind_dir(&mut watcher, &mut watched_dir);
            }
            maybe_res = receiver.recv() => {
                let Some(res) = maybe_res else {
                    break;
                };
                match res.map_err(anyhow::Error::from) {
                    Ok(event) => {
                        // 只对受管的日志文件重载：书库目录里还有本子、备份与探针文件，
                        // 任何删除都重建文件层既无必要，也会让探针与 watcher 互相触发。
                        let managed = event
                            .paths
                            .iter()
                            .filter_map(|path| path.file_name()?.to_str())
                            .any(is_managed_log_file);
                        let removed = matches!(event.kind, notify::EventKind::Remove(_));
                        if managed && removed {
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
    }
}

pub const DEFAULT_LOG_RETENTION: Duration = Duration::from_secs(7 * 24 * 60 * 60); // 7天
pub const MAX_LOGS_TOTAL_SIZE: u64 = 20 * 1024 * 1024; // 20 MB 配额上限
pub const TARGET_LOGS_TOTAL_SIZE: u64 = 10 * 1024 * 1024; // 10 MB 清理目标大小

/// 滚动日志文件名前缀：`<前缀>.YYYY-MM-DD.log`
pub const LOG_FILE_PREFIX: &str = "joymanga";
/// 旧版诊断日志导出文件名前缀：`<前缀>-log-<时间戳>.txt`（仅用于清理与识别）
pub const DIAGNOSTIC_LOG_PREFIX: &str = "joymanga-log-";
/// 诊断包文件名前缀：`<前缀>-<时间戳>.json`
pub const DIAGNOSTIC_JSON_PREFIX: &str = "joymanga-diagnostics-";
/// 崩溃报告前缀：`crash-<unix 秒>.log`，由 panic hook 在日志系统未就绪时直接写盘
pub const CRASH_LOG_PREFIX: &str = "crash-";

pub fn is_managed_log_file(file_name: &str) -> bool {
    let lower = file_name.to_ascii_lowercase();
    // 诊断日志前缀（`joymanga-log-`）本身以滚动日志前缀（`joymanga`）开头，
    // 因此滚动日志必须先排除诊断前缀，否则 `joymanga-log-*.log` 会被误判。
    let is_diagnostic_log = lower.starts_with(DIAGNOSTIC_LOG_PREFIX) && lower.ends_with(".txt");
    let is_diagnostic_json = lower.starts_with(DIAGNOSTIC_JSON_PREFIX) && lower.ends_with(".json");
    let is_rolling_log = !is_diagnostic_log
        && !is_diagnostic_json
        && !lower.starts_with(DIAGNOSTIC_LOG_PREFIX)
        && !lower.starts_with(DIAGNOSTIC_JSON_PREFIX)
        && lower.starts_with(LOG_FILE_PREFIX)
        && lower.ends_with(".log");
    is_rolling_log
        || is_diagnostic_log
        || is_diagnostic_json
        || (lower.starts_with("panic") && lower.ends_with(".log"))
        || (lower.starts_with(CRASH_LOG_PREFIX) && lower.ends_with(".log"))
}

/// 删除日志目录下所有由本应用管理的日志文件（用户放入的其他文件保持原样）。
///
/// 返回删除失败的文件数。设置页的「清理日志」与维护外壳共用这一份规则，
/// 新增日志前缀时只需改 `is_managed_log_file`。
pub fn clear_managed_logs(logs_dir: &std::path::Path) -> usize {
    let mut failures = 0;
    for entry in walkdir::WalkDir::new(logs_dir)
        .into_iter()
        .filter_map(Result::ok)
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let managed = entry.file_name().to_str().is_some_and(is_managed_log_file);
        if managed && std::fs::remove_file(entry.path()).is_err() {
            failures += 1;
        }
    }
    failures
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

/// 滚动日志的落盘目录：书库下的 `logs`，与用户下载的本子放在一起。
///
/// 书库默认是公共 `Download/JoyManga`，日志在文件管理器里直接可见，卸载应用也不会
/// 被系统一并清掉；出问题时用户能自己把日志拷出来。书库不可写（未授予「所有文件
/// 访问」、书库被删除或只读）时退回应用配置目录，日志功能不能因此中断。
///
/// 结果会缓存，避免「写探针 → watcher 删除事件 → 重载 → 再写探针」的循环。
/// 书库变更后调用 `reset_logs_dir` 让下次重新解析。
pub fn logs_dir(app: &AppHandle) -> anyhow::Result<std::path::PathBuf> {
    let cell = LOGS_DIR.get_or_init(|| parking_lot::RwLock::new(None));
    if let Some(dir) = cell.read().clone() {
        return Ok(dir);
    }
    let resolved = resolve_logs_dir(app)?;
    *cell.write() = Some(resolved.clone());
    Ok(resolved)
}

/// 让已缓存的日志目录失效（书库位置变化后调用），下次 `logs_dir` 重新解析。
pub fn reset_logs_dir() {
    if let Some(cell) = LOGS_DIR.get() {
        *cell.write() = None;
    }
    if let Some(sender) = LOG_DIR_NOTIFY.get() {
        let next = sender.borrow().wrapping_add(1);
        sender.send_replace(next);
    }
}

fn resolve_logs_dir(app: &AppHandle) -> anyhow::Result<std::path::PathBuf> {
    let library_dir = app.get_config().read().download_dir.clone();
    let preferred = crate::commands::system_commands::diagnostic_logs_dir(&library_dir);
    if crate::utils::probe_directory_writable(&preferred) {
        return Ok(preferred);
    }
    let fallback = app.state::<ConfigStorage>().logs_dir().to_path_buf();
    if crate::utils::probe_directory_writable(&fallback) {
        return Ok(fallback);
    }
    Err(anyhow::anyhow!(
        "日志目录不可写：`{}` 与 `{}`",
        preferred.display(),
        fallback.display()
    ))
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
        assert!(is_managed_log_file("joymanga-diagnostics-1727610000.json"));
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

    #[test]
    fn recent_logs_buffer_bounds_lines_bytes_and_utf8() {
        let mut buffer = RecentLogsBuffer::default();
        for idx in 0..(RECENT_LOG_MAX_LINES + 50) {
            buffer.push(&format!("line-{idx}\n"));
        }
        assert_eq!(buffer.snapshot().len(), RECENT_LOG_MAX_LINES);
        assert_eq!(buffer.snapshot().first().unwrap(), "line-50");

        let huge = "汉".repeat(RECENT_LOG_LINE_MAX_CHARS + 20);
        buffer.push(&huge);
        let last = buffer.snapshot().pop().unwrap();
        assert!(last.ends_with("...(truncated)"));
        assert!(last.chars().count() <= RECENT_LOG_LINE_MAX_CHARS + 16);

        // 超过总字节上限时逐条淘汰旧日志，仅保留最新内容
        let wide_line = "a".repeat(3_500);
        for _ in 0..100 {
            buffer.push(&wide_line);
        }
        assert!(buffer.total_bytes <= RECENT_LOG_MAX_BYTES);

        buffer.clear();
        assert!(buffer.snapshot().is_empty());
        assert_eq!(buffer.total_bytes, 0);
    }
}
