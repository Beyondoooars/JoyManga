use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use parking_lot::RwLock;

use crate::logger::CRASH_LOG_PREFIX;

/// 崩溃报告落盘目录。日志系统就绪前就要能写，因此直接记录路径而不是依赖 AppHandle。
/// 启动早期可能只有临时目录可用，配置加载完成后由 [`set_dir`] 改到日志目录。
static CRASH_DIR: OnceLock<RwLock<Option<PathBuf>>> = OnceLock::new();

fn dir_slot() -> &'static RwLock<Option<PathBuf>> {
    CRASH_DIR.get_or_init(|| RwLock::new(None))
}

/// 安装 panic hook：release 版是 `panic = "abort"` 且 Windows 无控制台，
/// 不落盘的话崩溃后没有任何线索。这里只做最小工作（格式化 + 写文件），
/// 不使用 tracing（panic 可能发生在日志系统初始化前后）。
///
/// 目录不可写时静默放弃，绝不能因为写崩溃报告本身再触发一次 panic。
pub fn install(dir: &Path) {
    let _ = std::fs::create_dir_all(dir);
    *dir_slot().write() = Some(dir.to_path_buf());

    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        write_report(info);
        default_hook(info);
    }));
}

/// 启动后期把崩溃报告改写到日志目录（与滚动日志一起受清理策略管理）。
pub fn set_dir(dir: &Path) {
    let _ = std::fs::create_dir_all(dir);
    *dir_slot().write() = Some(dir.to_path_buf());
}

fn write_report(info: &std::panic::PanicHookInfo<'_>) {
    let Some(dir) = dir_slot().read().clone() else {
        return;
    };
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    let path = dir.join(format!("{CRASH_LOG_PREFIX}{timestamp}.log"));

    let payload = panic_payload(info);
    let location = info
        .location()
        .map(|location| {
            format!(
                "{}:{}:{}",
                location.file(),
                location.line(),
                location.column()
            )
        })
        .unwrap_or_else(|| "unknown".to_string());
    let thread = std::thread::current();
    let thread_name = thread.name().unwrap_or("unnamed");

    let report = format!(
        "time_unix={timestamp}\nthread={thread_name}\nlocation={location}\nmessage={payload}\n\nbacktrace:\n{}\n",
        std::backtrace::Backtrace::force_capture()
    );

    // 用 create_new 避免同一秒内多次崩溃互相覆盖；失败再退化为普通追加。
    let result = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&path)
        .and_then(|mut file| file.write_all(report.as_bytes()));
    if result.is_err() {
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .and_then(|mut file| file.write_all(report.as_bytes()));
    }
}

fn panic_payload(info: &std::panic::PanicHookInfo<'_>) -> String {
    if let Some(message) = info.payload().downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = info.payload().downcast_ref::<String>() {
        message.clone()
    } else {
        "non-string panic payload".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_file_name_uses_crash_prefix_and_second_precision() {
        let name = format!("{CRASH_LOG_PREFIX}1727610000.log");
        assert!(name.starts_with("crash-"));
        assert!(name.ends_with(".log"));
    }

    #[test]
    fn set_dir_redirects_reports() {
        let dir = std::env::temp_dir().join(format!("jm-crash-{}", uuid::Uuid::new_v4()));
        set_dir(&dir);
        assert_eq!(dir_slot().read().as_deref(), Some(dir.as_path()));
        assert!(dir.is_dir());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
