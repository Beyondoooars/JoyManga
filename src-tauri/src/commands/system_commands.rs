use std::path::{Path, PathBuf};

use anyhow::{anyhow, ensure, Context};
use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::{AppHandle, Manager};
use tauri_plugin_opener::OpenerExt;

use crate::config::ConfigStorage;
use crate::diagnostics;
use crate::errors::{CommandError, CommandResult};
use crate::extensions::AppHandleExt;
use crate::logger;
use crate::utils;

#[allow(clippy::needless_pass_by_value)]
fn reveal_path_in_file_manager(app: &AppHandle, path: &Path, err_title: &str) -> CommandResult<()> {
    let path_str = path.to_string_lossy();
    let _ = app.opener().open_path(path_str.as_ref(), None::<&str>);
    let _ = err_title;
    Ok(())
}

fn validate_reveal_path(app: &AppHandle, path: &Path) -> anyhow::Result<()> {
    // 先校验再打开：不能对前端传入的任意路径执行 create_dir_all，
    // 否则可以借该命令在任意可写位置创建目录树。
    // 目录尚不存在（例如首次使用、导出目录未创建）时，校验最近的已存在祖先。
    let target = std::fs::canonicalize(path).or_else(|_| {
        path.ancestors()
            .skip(1)
            .find_map(|ancestor| std::fs::canonicalize(ancestor).ok())
            .context(format!("要打开的路径`{}`不存在", path.display()))
    })?;
    let config = app.get_config();
    let config = config.read();
    let mut allowed_roots = vec![app.path().app_data_dir()?];
    allowed_roots.push(config.download_dir.clone());
    allowed_roots.push(config.export_dir.clone());
    drop(config);
    if let Some(storage) = app.try_state::<ConfigStorage>() {
        allowed_roots.push(storage.base_dir().to_path_buf());
    }

    for root in &allowed_roots {
        let _ = std::fs::create_dir_all(root);
    }

    let allowed = allowed_roots.into_iter().any(|root| {
        std::fs::canonicalize(root).is_ok_and(|canonical_root| target.starts_with(canonical_root))
    });
    if !allowed {
        return Err(anyhow!(
            "路径`{}`不在应用数据、下载或导出目录内",
            target.display()
        ));
    }
    // 校验通过后再创建目录（仅当目标不存在时）
    if !path.exists() {
        std::fs::create_dir_all(path).context(format!("创建目录`{}`失败", path.display()))?;
    }
    Ok(())
}

#[allow(clippy::needless_pass_by_value)]
#[tauri::command(async)]
#[specta::specta]
pub fn show_path_in_file_manager(app: AppHandle, path: &str) -> CommandResult<()> {
    validate_reveal_path(&app, Path::new(path))
        .map_err(|err| CommandError::from("拒绝打开不受管理的路径", err))?;
    reveal_path_in_file_manager(&app, Path::new(path), "在文件管理器中打开失败")
}

#[tauri::command(async)]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub fn clear_logs(app: AppHandle) -> CommandResult<()> {
    // 两处都要清：应用数据目录里的滚动日志，以及用户导出到书库 logs 的诊断日志。
    // 导出后诊断日志就搬到了书库，只清前者会让「清理日志」看起来没生效。
    let mut failures = 0;
    for logs_dir in all_logs_dirs(&app) {
        if logs_dir.exists() {
            // 只删除本应用管理的日志，用户放进日志目录的其他文件保持原样
            failures += logger::clear_managed_logs(&logs_dir);
        }
    }
    if failures > 0 {
        tracing::warn!(failures, "部分日志文件删除失败");
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
#[allow(clippy::needless_pass_by_value)]
pub fn clear_cache(app: AppHandle) -> CommandResult<()> {
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
    if let Some(cache) = app.try_state::<utils::DownloadedLibraryCache>() {
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

#[allow(clippy::needless_pass_by_value)]
#[tauri::command(async)]
#[specta::specta]
pub fn get_logs_dir_size(app: AppHandle) -> CommandResult<u64> {
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
#[allow(clippy::needless_pass_by_value)]
pub fn export_logs(
    app: AppHandle,
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

    let report = diagnostics::build_report(&app, frontend_logs)
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
