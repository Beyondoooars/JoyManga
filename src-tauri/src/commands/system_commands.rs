use std::path::{Path, PathBuf};

use anyhow::{anyhow, ensure, Context};
use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::{AppHandle, Manager};

use crate::config::ConfigStorage;
use crate::diagnostics;
use crate::errors::{CommandError, CommandResult};
use crate::extensions::AppHandleExt;
use crate::logger;
use crate::utils;

#[tauri::command(async)]
#[specta::specta]
pub async fn clear_logs(app: AppHandle) -> CommandResult<()> {
    tauri::async_runtime::spawn_blocking(move || clear_logs_blocking(&app))
        .await
        .map_err(|err| CommandError::from("清理日志任务失败", err))?
}

fn clear_logs_blocking(app: &AppHandle) -> CommandResult<()> {
    // 清理前先关闭当前滚动日志句柄，避免 Windows 下被占用文件无法删除或重载报拒绝访问，
    // 删除完后再按原等级重建新文件。
    let prev_level = logger::current_log_level();
    if prev_level != crate::types::LogLevel::Off {
        let _ = logger::reload_file_logger(crate::types::LogLevel::Off);
    }
    // 两处都要清：应用数据目录里的滚动日志，以及用户导出到书库 logs 的诊断日志。
    // 导出后诊断日志就搬到了书库，只清前者会让「清理日志」看起来没生效。
    let mut failures = 0;
    for logs_dir in all_logs_dirs(app) {
        if logs_dir.exists() {
            // 只删除本应用管理的日志，用户放进日志目录的其他文件保持原样
            failures += logger::clear_managed_logs(&logs_dir);
        }
    }
    logger::clear_recent_logs();
    if prev_level != crate::types::LogLevel::Off {
        if let Err(error) = logger::reload_file_logger(prev_level) {
            tracing::warn!(error = %error, "清理日志后重建文件日志层失败");
        }
    }
    if failures > 0 {
        return Err(CommandError::from(
            "清理日志失败",
            anyhow!("{failures} 个日志文件未能删除，其余日志已清理"),
        ));
    }
    tracing::info!("已清空应用日志");
    Ok(())
}

/// 应用日志可能落在三处：书库 `logs`（滚动日志、崩溃报告、导出的诊断日志都在这里），
/// 以及书库不可写时退回的应用数据目录。清理与大小统计必须覆盖同一组目录，
/// 否则「清理日志」会看起来没生效。
fn all_logs_dirs(app: &AppHandle) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let library_logs = diagnostic_logs_dir(&app.get_config().read().download_dir);
    dirs.push(library_logs);
    if let Ok(dir) = logger::logs_dir(app) {
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    // 书库可写时 logs_dir 就是书库 logs；退回配置目录时这里补上，两边都不漏。
    let fallback = app.state::<ConfigStorage>().logs_dir().to_path_buf();
    if !dirs.contains(&fallback) && fallback.exists() {
        dirs.push(fallback);
    }
    dirs
}

#[tauri::command(async)]
#[specta::specta]
pub async fn clear_cache(app: AppHandle) -> CommandResult<()> {
    tauri::async_runtime::spawn_blocking(move || clear_cache_blocking(&app))
        .await
        .map_err(|err| CommandError::from("清理缓存任务失败", err))?
}

fn clear_cache_blocking(app: &AppHandle) -> CommandResult<()> {
    let storage = app.state::<ConfigStorage>();
    let cache_dir = storage.webview_data_dir();
    clear_webview_caches(cache_dir).map_err(|err| CommandError::from("清理缓存失败", err))?;
    // 封面缩略图与页面尺寸缓存都是可重建的派生数据，一并清掉；下次扫描/阅读会按需重新生成
    if let Some(cover_dir) = crate::cover_cache::dir() {
        let _ = std::fs::remove_dir_all(&cover_dir);
    }
    if let Some(sizes_dir) = crate::page_size::sizes_cache::dir() {
        let _ = std::fs::remove_dir_all(&sizes_dir);
    }
    if let Some(cache) = app.try_state::<crate::library_catalog::DownloadedLibraryCache>() {
        cache.invalidate();
    }
    tracing::info!("已清空应用缓存");
    Ok(())
}

fn clear_webview_caches(root: &Path) -> anyhow::Result<()> {
    if !root.exists() {
        return Ok(());
    }
    let canonical_root = std::fs::canonicalize(root)?;
    // 只清理缓存目录，保留 Local Storage 中的阅读记录、书签和登录存储。
    let cache_roots: Vec<_> = walkdir::WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry.file_type().is_dir()
                && matches!(
                    entry.file_name().to_str(),
                    Some("Cache" | "Code Cache" | "GPUCache" | "DawnCache" | "CacheStorage")
                )
        })
        .map(|entry| entry.into_path())
        .collect();
    let mut failures = 0;
    for cache in cache_roots {
        for entry in walkdir::WalkDir::new(cache).follow_links(false) {
            let entry = entry?;
            if !entry.file_type().is_file() {
                continue;
            }
            let target = std::fs::canonicalize(entry.path())?;
            utils::validate_delete_target(&canonical_root, &target)?;
            if std::fs::remove_file(&target).is_err() {
                failures += 1;
            }
        }
    }
    ensure!(
        failures == 0,
        "{failures} 个缓存文件仍被占用，其余缓存已清理；阅读记录和书签未删除"
    );
    Ok(())
}

#[tauri::command(async)]
#[specta::specta]
pub async fn get_logs_dir_size(app: AppHandle) -> CommandResult<u64> {
    tauri::async_runtime::spawn_blocking(move || {
        // 与「清理日志」覆盖同一组目录，否则导出到书库的日志不会计入占用
        let logs_dir_size = all_logs_dirs(&app)
            .into_iter()
            .filter(|dir| dir.exists())
            .flat_map(|dir| {
                walkdir::WalkDir::new(dir)
                    .into_iter()
                    .filter_map(Result::ok)
                    .filter(|entry| entry.file_type().is_file())
                    .filter_map(|entry| entry.metadata().ok())
                    .map(|metadata| metadata.len())
                    .collect::<Vec<_>>()
            })
            .sum::<u64>();
        tracing::debug!("获取日志目录大小成功");
        Ok(logs_dir_size)
    })
    .await
    .map_err(|err| CommandError::from("获取日志目录大小任务失败", err))?
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ExportLogsResult {
    pub success: bool,
    pub file_path: String,
    pub message: String,
}

/// 诊断日志的固定位置：书库下的 `logs` 目录。
///
/// 书库默认是公共 `Download/JoyManga`，日志在文件管理器里直接可见，卸载应用也不会
/// 被系统清理；用户主动导出的诊断日志属于反馈材料，不该随应用数据一起消失。
/// 不提供修改入口，导出位置只有这一个。
pub(crate) fn diagnostic_logs_dir(library_dir: &Path) -> PathBuf {
    library_dir.join("logs")
}

#[tauri::command(async)]
#[specta::specta]
pub async fn export_logs(
    app: AppHandle,
    frontend_logs: Option<Vec<String>>,
) -> CommandResult<ExportLogsResult> {
    tauri::async_runtime::spawn_blocking(move || export_logs_blocking(&app, frontend_logs))
        .await
        .map_err(|err| CommandError::from("导出诊断包任务失败", err))?
}

fn export_logs_blocking(
    app: &AppHandle,
    frontend_logs: Option<Vec<String>>,
) -> CommandResult<ExportLogsResult> {
    let config = app.get_config().read().clone();

    let target_dir = diagnostic_logs_dir(&config.download_dir);
    std::fs::create_dir_all(&target_dir)
        .context(format!("创建日志目录`{}`失败", target_dir.display()))
        .map_err(|err| CommandError::from("导出日志失败", err))?;

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let log_file_name = format!("{}{timestamp}.json", crate::logger::DIAGNOSTIC_JSON_PREFIX);
    let target_file_path = target_dir.join(log_file_name);

    let report = diagnostics::build_report(app, frontend_logs)
        .context("组装诊断包失败")
        .map_err(|err| CommandError::from("导出诊断包失败", err))?;
    let bytes = serde_json::to_vec_pretty(&report)
        .context("序列化诊断包失败")
        .map_err(|err| CommandError::from("导出诊断包失败", err))?;

    std::fs::write(&target_file_path, &bytes)
        .context(format!("写入诊断包`{}`失败", target_file_path.display()))
        .map_err(|err| CommandError::from("导出诊断包失败", err))?;

    let file_path_str = target_file_path.to_string_lossy().to_string();

    tracing::info!(path = %file_path_str, bytes = %bytes.len(), "已导出诊断包");

    Ok(ExportLogsResult {
        success: true,
        file_path: file_path_str,
        message: format!("诊断包已成功导出至：{}", target_file_path.display()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_cleanup_preserves_reading_and_login_storage() {
        let root = std::env::temp_dir().join(format!("jm-cache-{}", uuid::Uuid::new_v4()));
        let cache = root.join("Default/Cache/Cache_Data");
        let storage = root.join("Default/Local Storage/leveldb");
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::create_dir_all(&storage).unwrap();
        std::fs::write(cache.join("cached-page"), b"cache").unwrap();
        std::fs::write(storage.join("reading-state"), b"keep").unwrap();
        clear_webview_caches(&root).unwrap();
        assert!(!cache.join("cached-page").exists());
        assert_eq!(
            std::fs::read(storage.join("reading-state")).unwrap(),
            b"keep"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn diagnostic_logs_live_under_the_library() {
        let library = Path::new("/tmp/JoyManga");
        assert_eq!(diagnostic_logs_dir(library), library.join("logs"));
    }

    #[test]
    fn test_check_directory_writable() {
        let temp_dir =
            std::env::temp_dir().join(format!("jm-writable-test-{}", uuid::Uuid::new_v4()));
        assert!(crate::utils::probe_directory_writable(&temp_dir));
        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
