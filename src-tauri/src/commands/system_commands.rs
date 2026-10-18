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

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct DirectoryListing {
    pub current_path: String,
    pub parent_path: Option<String>,
    pub subdirectories: Vec<String>,
    pub files: Vec<String>,
    pub exists: bool,
    pub writable: bool,
}

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

#[tauri::command(async)]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub fn show_logs_in_file_manager(app: AppHandle) -> CommandResult<()> {
    let logs_dir =
        logger::logs_dir(&app).map_err(|err| CommandError::from("获取日志目录失败", err))?;
    std::fs::create_dir_all(&logs_dir)
        .context(format!("创建日志目录`{}`失败", logs_dir.display()))
        .map_err(|err| CommandError::from("打开日志目录失败", err))?;
    reveal_path_in_file_manager(&app, &logs_dir, "打开日志目录失败")
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
    let logs_dir = logger::logs_dir(&app)
        .context("获取日志目录失败")
        .map_err(|err| CommandError::from("获取日志目录失败", err))?;
    if logs_dir.exists() {
        for entry in walkdir::WalkDir::new(&logs_dir)
            .into_iter()
            .filter_map(Result::ok)
        {
            if !entry.file_type().is_file() {
                continue;
            }
            // 只删除本应用管理的日志，用户放进日志目录的其他文件保持原样
            let managed = entry
                .file_name()
                .to_str()
                .is_some_and(logger::is_managed_log_file);
            if managed {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    tracing::info!("已清空应用日志");
    Ok(())
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
    let allowed_root = storage.base_dir().to_path_buf();
    app.get_download_manager()
        .with_idle_downloads(|| {
            let roots = [
                ("下载", &download_dir, &defaults.download_dir),
                ("导出", &export_dir, &defaults.export_dir),
            ];
            let mut existing_roots = Vec::new();
            for (name, configured, expected) in roots {
                if let Some(canonical) =
                    validate_clear_root(name, configured, expected, &allowed_root)?
                {
                    existing_roots.push((name, canonical));
                }
            }

            for (name, root) in existing_roots {
                for entry in std::fs::read_dir(&root).context(format!("读取{name}目录失败"))?
                {
                    let entry = entry?;
                    let path = entry.path();
                    if entry.file_type()?.is_dir() {
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

fn validate_clear_root(
    name: &str,
    configured: &Path,
    expected: &Path,
    allowed_root: &Path,
) -> anyhow::Result<Option<PathBuf>> {
    ensure!(
        configured == expected,
        "{name}目录是自定义路径；为保护目录中的其他文件，请只在应用默认专用目录使用批量清空"
    );
    if !configured.exists() {
        return Ok(None);
    }
    ensure!(
        !is_link_or_reparse_point(&std::fs::symlink_metadata(configured)?),
        "{name}目录是链接，拒绝批量清空"
    );
    let canonical = std::fs::canonicalize(configured).context(format!("规范化{name}目录失败"))?;
    // 即使配置指向默认目录，也必须落在应用数据根目录内，避免异常配置清空到别处。
    let canonical_root = std::fs::canonicalize(allowed_root)?;
    ensure!(
        canonical.starts_with(&canonical_root) && canonical != canonical_root,
        "{name}目录不在应用数据目录内，拒绝清空"
    );
    for entry in walkdir::WalkDir::new(&canonical).follow_links(false) {
        let entry = entry.context(format!("检查{name}目录内容失败"))?;
        ensure!(
            !is_link_or_reparse_point(&std::fs::symlink_metadata(entry.path())?),
            "{name}目录包含链接，拒绝批量清空"
        );
    }
    Ok(Some(canonical))
}

fn is_link_or_reparse_point(metadata: &std::fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

#[allow(clippy::needless_pass_by_value)]
#[tauri::command(async)]
#[specta::specta]
pub fn get_logs_dir_size(app: AppHandle) -> CommandResult<u64> {
    let logs_dir = logger::logs_dir(&app)
        .context("获取日志目录失败")
        .map_err(|err| CommandError::from("获取日志目录大小失败", err))?;
    let logs_dir_size = walkdir::WalkDir::new(&logs_dir)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .filter_map(|entry| entry.metadata().ok())
        .map(|metadata| metadata.len())
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

fn check_directory_writable(dir: &Path) -> bool {
    if !dir.exists() && std::fs::create_dir_all(dir).is_err() {
        return false;
    }
    let test_file = dir.join(format!(".write_test_{}", uuid::Uuid::new_v4()));
    if let Ok(mut f) = std::fs::File::create(&test_file) {
        use std::io::Write;
        let _ = f.write_all(b"test");
        drop(f);
        let _ = std::fs::remove_file(&test_file);
        true
    } else {
        false
    }
}

#[tauri::command(async)]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub fn export_logs(
    app: AppHandle,
    frontend_logs: Option<Vec<String>>,
) -> CommandResult<ExportLogsResult> {
    let config = app.get_config().read().clone();

    // 优先写入书库下的 logs 目录（用户可通过文件管理器访问）；不可写时退回应用数据目录。
    let target_dir = {
        let preferred = config.download_dir.join("logs");
        if check_directory_writable(&preferred) {
            preferred
        } else {
            let app_data = app
                .path()
                .app_data_dir()
                .unwrap_or_else(|_| PathBuf::from("."));
            app_data.join("logs")
        }
    };

    let _ = std::fs::create_dir_all(&target_dir);

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
    log_content.push_str(&format!("当前图片域名: {}\n", config.get_image_domain()));
    log_content.push_str(&format!("文件日志记录: {}\n\n", config.enable_file_logger));

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

#[allow(clippy::needless_pass_by_value)]
#[tauri::command(async)]
#[specta::specta]
pub fn list_directory_contents(
    app: AppHandle,
    path: Option<String>,
    include_files: Option<bool>,
) -> CommandResult<DirectoryListing> {
    let include_files = include_files.unwrap_or(false);
    let target_path_buf = match path.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(p) => PathBuf::from(p),
        None => app.get_config().read().download_dir.clone(),
    };

    let target_path_str = target_path_buf.to_string_lossy().to_string();
    let exists = target_path_buf.exists();
    let parent_path = target_path_buf
        .parent()
        .map(|p| p.to_string_lossy().to_string());

    let mut subdirectories = Vec::new();
    let mut files = Vec::new();
    let mut writable = false;

    if exists && target_path_buf.is_dir() {
        let entries = std::fs::read_dir(&target_path_buf).map_err(|err| {
            CommandError::from("无法读取目录，请使用系统文件选择器或检查授权", err)
        })?;
        {
            for entry in entries.flatten() {
                if let Ok(ft) = entry.file_type() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if !name.starts_with('.') {
                        if ft.is_dir() {
                            subdirectories.push(name);
                        } else if include_files && ft.is_file() {
                            files.push(name);
                        }
                    }
                }
            }
        }
        subdirectories.sort_by_key(|a| a.to_lowercase());
        files.sort_by_key(|a| a.to_lowercase());

        writable = std::fs::metadata(&target_path_buf)
            .is_ok_and(|metadata| !metadata.permissions().readonly());
    }

    Ok(DirectoryListing {
        current_path: target_path_str,
        parent_path,
        subdirectories,
        files,
        exists,
        writable,
    })
}

#[tauri::command(async)]
#[specta::specta]
pub fn validate_selected_path(path: String, mode: String) -> CommandResult<()> {
    let path = PathBuf::from(path);
    let result = match mode.as_str() {
        "file" => (|| -> anyhow::Result<()> {
            ensure!(path.is_file(), "所选路径不存在或不是文件");
            std::fs::File::open(&path)?;
            Ok(())
        })(),
        "directory" => (|| -> anyhow::Result<()> {
            ensure!(path.is_dir(), "所选路径不存在或不是目录");
            std::fs::read_dir(&path)?;
            Ok(())
        })(),
        _ => Err(anyhow!("选择模式无效")),
    };
    result.map_err(|err| CommandError::from("路径校验失败", err))
}

#[allow(clippy::needless_pass_by_value)]
#[tauri::command(async)]
#[specta::specta]
pub fn create_directory(_app: AppHandle, path: &str) -> CommandResult<()> {
    let p = Path::new(path.trim());
    if p.as_os_str().is_empty() {
        return Err(CommandError::from(
            "创建目录失败",
            anyhow!("目录路径不能为空"),
        ));
    }
    std::fs::create_dir_all(p)
        .context(format!("创建目录`{path}`失败，请检查存储权限"))
        .map_err(|err| CommandError::from("创建目录失败", err))?;
    Ok(())
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
    fn test_check_directory_writable() {
        let temp_dir =
            std::env::temp_dir().join(format!("jm-writable-test-{}", uuid::Uuid::new_v4()));
        assert!(check_directory_writable(&temp_dir));
        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
