use std::path::{Path, PathBuf};

use anyhow::{anyhow, ensure, Context};
use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::{AppHandle, Manager};
use tauri_plugin_opener::OpenerExt;

use crate::config::{Config, ConfigStorage};
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

/// 应用日志可能落在两处：应用数据目录（滚动日志、崩溃报告）与书库 `logs`
/// （用户主动导出的诊断日志）。清理与大小统计必须覆盖同一组目录。
fn all_logs_dirs(app: &AppHandle) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(dir) = logger::logs_dir(app) {
        dirs.push(dir);
    }
    let library_logs = diagnostic_logs_dir(&app.get_config().read().download_dir);
    if !dirs.contains(&library_logs) {
        dirs.push(library_logs);
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
    // 封面缩略图是可重建的派生数据，一并清掉；下次扫描会按需重新生成
    if let Some(cover_dir) = crate::cover_cache::dir() {
        let _ = std::fs::remove_dir_all(&cover_dir);
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

#[tauri::command(async)]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub fn clear_all_downloads(app: AppHandle) -> CommandResult<()> {
    let config_state = app.get_config();
    let (download_dir, export_dir) = {
        let lock = config_state.read();
        (lock.download_dir.clone(), lock.export_dir.clone())
    };

    let storage = app.state::<ConfigStorage>();
    let defaults = Config::default(storage.base_dir());
    app.get_download_manager()
        .with_idle_downloads(|| {
            let roots = [
                ("下载", &download_dir, &defaults.download_dir),
                ("导出", &export_dir, &defaults.export_dir),
            ];
            let mut existing_roots = Vec::new();
            for (name, configured, expected) in roots {
                if let Some(canonical) =
                    validate_clear_root(name, configured, expected, storage.base_dir())?
                {
                    existing_roots.push((name, canonical));
                }
            }

            for (name, root) in existing_roots {
                // 导出日志写在书库下的 logs 目录里；清空下载时整目录跳过，
                // 否则刚导出的诊断日志会被 remove_dir_all 一并删掉。
                let logs_dir = diagnostic_logs_dir(&root);
                for entry in std::fs::read_dir(&root).context(format!("读取{name}目录失败"))?
                {
                    let entry = entry?;
                    let path = entry.path();
                    // 备份 JSON 与诊断日志也放在书库目录里，清空下载时不能把它们一并删掉
                    if is_user_data_file(&entry.file_name().to_string_lossy()) {
                        continue;
                    }
                    let is_dir = entry.file_type()?.is_dir();
                    if is_dir && path == logs_dir {
                        continue;
                    }
                    if is_dir {
                        std::fs::remove_dir_all(&path)
                            .context(format!("删除{name}目录项`{}`失败", path.display()))?;
                    } else {
                        std::fs::remove_file(&path)
                            .context(format!("删除{name}文件`{}`失败", path.display()))?;
                    }
                }
            }
            Ok(())
        })
        .map_err(|err| CommandError::from("清空下载目录失败", err))?;

    if let Some(cache) = app.try_state::<utils::DownloadedLibraryCache>() {
        cache.invalidate();
    }
    tracing::info!("已清空全部已下载漫画及导出内容");
    Ok(())
}

/// 批量清空只允许两个默认位置：应用数据目录内的专用 library，或公共
/// `Download/JoyManga`；自定义路径一律拒绝，保护目录中的其他文件。
fn validate_clear_root(
    name: &str,
    configured: &Path,
    expected: &Path,
    app_data_root: &Path,
) -> anyhow::Result<Option<PathBuf>> {
    let is_default = configured == expected;
    // 与 initialize_android_storage 共用同一判定（任意 /storage/<卷>/Download/JoyManga），
    // 避免「初始化接受、清空拒绝」或反过来误删目录。
    let is_public_default = crate::config::is_public_library_dir(configured);
    ensure!(
        is_default || is_public_default,
        "{name}目录是自定义路径；为保护目录中的其他文件，请只在应用默认书库目录使用批量清空"
    );
    if !configured.exists() {
        return Ok(None);
    }
    ensure!(
        !is_link_or_reparse_point(&std::fs::symlink_metadata(configured)?),
        "{name}目录是链接，拒绝批量清空"
    );
    let canonical = std::fs::canonicalize(configured).context(format!("规范化{name}目录失败"))?;
    // 规范化后必须仍然指向同一语义的位置：否则祖先符号链接可能把清空引到别处。
    if is_public_default {
        ensure!(
            crate::config::is_public_library_dir(&canonical),
            "{name}目录解析后不是公共默认书库，拒绝清空"
        );
    } else {
        let canonical_root = std::fs::canonicalize(app_data_root)?;
        ensure!(
            canonical.starts_with(&canonical_root) && canonical != canonical_root,
            "{name}目录不在应用数据目录内，拒绝清空"
        );
    }
    for entry in walkdir::WalkDir::new(&canonical).follow_links(false) {
        let entry = entry.context(format!("检查{name}目录内容失败"))?;
        ensure!(
            !is_link_or_reparse_point(&std::fs::symlink_metadata(entry.path())?),
            "{name}目录包含链接，拒绝批量清空"
        );
    }
    Ok(Some(canonical))
}

/// 清空下载时要保留的用户数据：备份文件与导出的诊断日志与漫画无关。
fn is_user_data_file(file_name: &str) -> bool {
    let lower = file_name.to_ascii_lowercase();
    (lower.starts_with("joymanga_backup_") && lower.ends_with(".json"))
        || (lower.starts_with(crate::logger::DIAGNOSTIC_LOG_PREFIX) && lower.ends_with(".txt"))
}

fn is_link_or_reparse_point(metadata: &std::fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
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
/// 不提供修改入口，导出位置只有这一个。`clear_all_downloads` 会整目录跳过它。
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
    let log_file_name = format!("{}{timestamp}.txt", crate::logger::DIAGNOSTIC_LOG_PREFIX);
    let target_file_path = target_dir.join(log_file_name);

    let mut log_content = String::new();
    log_content.push_str("========================================\n");
    log_content.push_str("JoyManga 诊断日志导出\n");
    log_content.push_str("========================================\n");
    log_content.push_str(&format!(
        "系统平台: {} / {}\n",
        std::env::consts::OS,
        std::env::consts::ARCH
    ));
    log_content.push_str(&format!("导出时间戳(Unix): {}\n", timestamp));
    log_content.push_str(&format!("软件版本: {}\n", app.package_info().version));
    log_content.push_str(&format!("下载目录: {}\n", config.download_dir.display()));
    log_content.push_str(&format!("导出目录: {}\n", config.export_dir.display()));
    log_content.push_str(&format!("当前API域名: {}\n", config.get_api_domain()));
    log_content.push_str(&format!("当前图片域名: {}\n\n", config.get_image_domain()));

    if let Some(fe_logs) = frontend_logs {
        if !fe_logs.is_empty() {
            // 前端日志由 IPC 传入，限制条数与单条长度，避免超大载荷把内存打满
            const MAX_FRONTEND_LOG_LINES: usize = 5_000;
            const MAX_FRONTEND_LOG_LINE_CHARS: usize = 4_000;
            log_content.push_str("=== 运行内存日志（最近记录） ===\n");
            for line in fe_logs.into_iter().take(MAX_FRONTEND_LOG_LINES) {
                log_content.push_str(
                    &line
                        .chars()
                        .take(MAX_FRONTEND_LOG_LINE_CHARS)
                        .collect::<String>(),
                );
                log_content.push('\n');
            }
            log_content.push('\n');
        }
    }

    if let Ok(logs_dir) = logger::logs_dir(&app) {
        if logs_dir.exists() {
            log_content.push_str("=== 本地历史文件日志 ===\n");
            let mut log_files: Vec<_> = walkdir::WalkDir::new(&logs_dir)
                .into_iter()
                .filter_map(Result::ok)
                .filter(|e| {
                    e.file_type().is_file()
                        && e.path().extension().and_then(|s| s.to_str()) == Some("log")
                })
                .map(|e| e.into_path())
                .collect();
            log_files.sort();
            for file_path in log_files {
                let file_name = file_path.file_name().and_then(|s| s.to_str()).unwrap_or("");
                log_content.push_str(&format!("\n--- 文件: {file_name} ---\n"));
                if let Ok(content) = std::fs::read_to_string(&file_path) {
                    log_content.push_str(&content);
                }
                log_content.push('\n');
            }
        }
    }

    std::fs::write(&target_file_path, log_content.as_bytes())
        .context(format!("写入日志文件`{}`失败", target_file_path.display()))
        .map_err(|err| CommandError::from("导出日志失败", err))?;

    let file_path_str = target_file_path.to_string_lossy().to_string();

    tracing::info!(path = %file_path_str, "已导出应用运行日志");

    Ok(ExportLogsResult {
        success: true,
        file_path: file_path_str,
        message: format!("日志已成功导出至：{}", target_file_path.display()),
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
    fn bulk_clear_rejects_custom_and_parent_directories_and_preserves_files() {
        let root = std::env::temp_dir().join(format!("jm-clear-{}", uuid::Uuid::new_v4()));
        let expected = root.join("漫画下载");
        let custom = root.join("用户资料");
        std::fs::create_dir_all(&expected).unwrap();
        std::fs::create_dir_all(&custom).unwrap();
        let unrelated = custom.join("重要资料.txt");
        std::fs::write(&unrelated, b"keep").unwrap();
        assert!(validate_clear_root("下载", &custom, &expected, &root).is_err());
        assert!(validate_clear_root("下载", &root, &expected, &root).is_err());
        assert_eq!(std::fs::read(&unrelated).unwrap(), b"keep");
        assert!(validate_clear_root("下载", &expected, &expected, &root)
            .unwrap()
            .is_some());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn bulk_clear_rejects_public_like_paths_outside_storage() {
        let root = std::env::temp_dir().join(format!("jm-clear-pub-{}", uuid::Uuid::new_v4()));
        let expected = root.join("漫画下载");
        std::fs::create_dir_all(&expected).unwrap();
        // 形似公共默认目录但不在 /storage 下：拒绝（判定基于路径组件，不受大小写/尾随分隔符影响）
        let fake_public = root.join("download/joymanga");
        std::fs::create_dir_all(&fake_public).unwrap();
        let fake_public = std::fs::canonicalize(&fake_public).unwrap();
        assert!(
            validate_clear_root("下载", &fake_public, &expected, &root).is_err(),
            "不在 /storage 下的伪公共目录必须被拒绝"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn public_library_predicate_requires_the_exact_layout() {
        let public = Path::new(crate::config::ANDROID_PUBLIC_LIBRARY_DIR);
        assert!(crate::config::is_public_library_dir(public));
        assert!(crate::config::is_public_library_dir(Path::new(
            "/storage/1A2B-3C4D/Download/JoyManga"
        )));
        // 尾随分隔符不应影响判定
        assert!(crate::config::is_public_library_dir(Path::new(
            "/storage/emulated/0/Download/JoyManga/"
        )));
        // 字符串后缀匹配会误判的路径
        assert!(!crate::config::is_public_library_dir(Path::new(
            "/storage/emulated/0/NotDownload/JoyManga"
        )));
        assert!(!crate::config::is_public_library_dir(Path::new(
            "/data/media/0/Download/JoyManga"
        )));
        assert!(!crate::config::is_public_library_dir(Path::new(
            "/storage/emulated/0/Download"
        )));
    }

    #[test]
    fn diagnostic_logs_live_under_the_library_and_survive_bulk_clear() {
        let root = std::env::temp_dir().join(format!("jm-logs-{}", uuid::Uuid::new_v4()));
        let library = root.join("JoyManga");
        std::fs::create_dir_all(&library).unwrap();
        // 导出日志的位置必须与「清空下载」跳过的目录一致，否则日志会被一并删掉
        assert_eq!(diagnostic_logs_dir(&library), library.join("logs"));
        let logs_dir = diagnostic_logs_dir(&library);
        std::fs::create_dir_all(&logs_dir).unwrap();
        std::fs::write(logs_dir.join("joymanga-log-1.txt"), b"diagnosis").unwrap();
        assert!(is_user_data_file("joymanga-log-1.txt"));
        assert_eq!(
            std::fs::read(logs_dir.join("joymanga-log-1.txt")).unwrap(),
            b"diagnosis"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn clear_all_preserves_backups_and_diagnostic_logs() {
        assert!(is_user_data_file("joymanga_backup_1700000000.json"));
        assert!(is_user_data_file("joymanga-log-1700000000.txt"));
        assert!(!is_user_data_file("0001.jpg"));
        assert!(!is_user_data_file("some_comic [JM123].cbz"));
        assert!(!is_user_data_file("元数据.json"));
    }

    #[test]
    fn test_check_directory_writable() {
        let temp_dir =
            std::env::temp_dir().join(format!("jm-writable-test-{}", uuid::Uuid::new_v4()));
        assert!(crate::utils::probe_directory_writable(&temp_dir));
        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
