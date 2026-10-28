use std::path::PathBuf;
#[cfg(target_os = "android")]
use std::path::{Component, Path};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Context};
use tauri::{AppHandle, Manager};

use crate::atomic_fs;
use crate::config::{Config, ConfigStorage};
use crate::download_queue::{DownloadQueue, DownloadSettings, StoredDownload};
use crate::errors::{CommandError, CommandResult};
use crate::extensions::AppHandleExt;
use crate::utils;

/// 纯词法折叠 `..`/`.`（不解析符号链接），用于尚未创建的路径。
#[cfg(target_os = "android")]
fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                if !normalized.pop() {
                    normalized.push(component);
                }
            }
            Component::CurDir => {}
            other => normalized.push(other),
        }
    }
    normalized
}

#[tauri::command]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub fn get_config(app: AppHandle) -> Config {
    app.get_config().read().clone()
}

#[tauri::command(async)]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub fn save_config(app: AppHandle, config: Config, base_config: Config) -> CommandResult<Config> {
    let state = app.get_config();
    let (previous, next) = {
        let mut current = state.write();
        let previous = current.clone();
        let candidate =
            crate::config_updates::merge_config_changes(&current, &base_config, &config)
                .map_err(|err| CommandError::from("设置存在冲突", err))?;
        let next = crate::config_updates::commit_locked(&app, &mut current, candidate)
            .map_err(|err| CommandError::from("保存配置失败", err))?;
        if previous.get_api_domain() != next.get_api_domain() {
            app.get_jm_client().invalidate_session();
        }
        (previous, next)
    };
    apply_config_effects(&app, &previous, &next);
    crate::config_updates::publish(&app, &next);
    Ok(next)
}

fn apply_config_effects(app: &AppHandle, previous: &Config, next: &Config) {
    if previous.download_dir != next.download_dir {
        if let Some(cache) = app.try_state::<utils::DownloadedLibraryCache>() {
            cache.invalidate();
        }
        // 日志与崩溃报告跟着书库走：位置变了要让日志目录重新解析并切到新书库。
        crate::logger::reset_logs_dir();
        let level = crate::logger::current_log_level();
        if level != crate::types::LogLevel::Off {
            if let Err(error) = crate::logger::reload_file_logger(level) {
                tracing::warn!(error = %error, "书库变更后切换日志目录失败");
            }
            if let Ok(dir) = crate::logger::logs_dir(app) {
                crate::crash::set_dir(&dir);
            }
        }
    }
    if previous.log_level != next.log_level {
        crate::logger::apply_log_level(next.log_level);
    }
    if previous.proxy_mode != next.proxy_mode
        || previous.proxy_host != next.proxy_host
        || previous.proxy_port != next.proxy_port
    {
        app.get_jm_client().cancel_login();
        app.get_jm_client().reload_client();
    }
    if previous.chapter_concurrency != next.chapter_concurrency
        || previous.img_concurrency != next.img_concurrency
    {
        app.get_download_manager().reload_concurrency();
    }
}

#[tauri::command(async)]
#[specta::specta]
pub fn initialize_android_storage(app: AppHandle, library_dir: String) -> CommandResult<Config> {
    #[cfg(target_os = "android")]
    {
        // 先做词法规范化再判定：`/storage/…/Download/../Documents/Download/JoyManga`
        // 这类路径不能因为字符串后缀通过校验。目录可能尚不存在，不能用 canonicalize
        // （它会解析到挂载命名空间路径，导致存储下来的书库路径与桥接层不一致）。
        let raw = PathBuf::from(&library_dir);
        if !raw.is_absolute() {
            return Err(CommandError::from(
                "初始化书库失败",
                anyhow!("应用书库路径无效"),
            ));
        }
        let candidate = normalize_path(&raw);
        let internal = normalize_path(&app.state::<ConfigStorage>().base_dir().join("library"));
        let suffix = PathBuf::from("Android")
            .join("data")
            .join(&app.config().identifier)
            .join("files")
            .join("library");
        if candidate != internal
            && !candidate.ends_with(&suffix)
            && !crate::config::is_public_library_dir(&candidate)
        {
            return Err(CommandError::from(
                "初始化书库失败",
                anyhow!("应用书库路径无效"),
            ));
        }
        return crate::config_updates::update(&app, |current| {
            let mut next = current.clone();
            if !current.android_storage_initialized {
                next.export_dir = candidate.clone();
                next.download_dir = candidate;
                next.android_storage_initialized = true;
            }
            Ok(next)
        })
        .map_err(|err| CommandError::from("初始化书库失败", err));
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = library_dir;
        Ok(get_config(app))
    }
}

#[tauri::command]
#[specta::specta]
pub fn get_api_domain_presets() -> Vec<crate::config::ApiDomainPreset> {
    crate::config::api_domain_presets()
}

#[tauri::command(async)]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub fn reset_app_state(app: AppHandle) -> CommandResult<Config> {
    let storage = app.state::<ConfigStorage>();
    let default_config = Config::default(storage.base_dir());
    replace_config_and_queue(&app, default_config.clone(), Vec::new())?;

    if let Some(cache) = app.try_state::<utils::DownloadedLibraryCache>() {
        cache.invalidate();
    }
    app.state::<crate::archive_intents::ArchiveIntentStore>()
        .clear();

    tracing::info!("应用已恢复出厂设置并重置任务队列");
    Ok(get_config(app))
}

fn replace_config_and_queue(
    app: &AppHandle,
    config: Config,
    entries: Vec<StoredDownload>,
) -> CommandResult<()> {
    let manager = app.get_download_manager();
    let mut changes = None;
    manager
        .with_idle_downloads(|| {
            let config_state = app.get_config();
            let mut current = config_state.write();
            let previous = current.clone();
            let previous_entries = manager.queue.entries();
            let storage = app.state::<ConfigStorage>();
            let queue_path = storage.config_path().with_file_name("download-queue.json");
            let previous_queue = if queue_path.exists() {
                Some(serde_json::from_slice(&std::fs::read(&queue_path)?)?)
            } else {
                None
            };
            let journal_path = storage.config_path().with_file_name(".state-replace.json");
            let mut journal = crate::state_transactions::StateJournal {
                previous_config: previous.clone(),
                previous_queue,
                committed: false,
            };
            journal.save(&journal_path)?;
            let result = (|| -> anyhow::Result<Config> {
                let next = crate::config_updates::commit_locked(app, &mut current, config)?;
                manager.queue.replace_entries(entries)?;
                journal.committed = true;
                journal.save(&journal_path)?;
                Ok(next)
            })();
            match result {
                Ok(next) => {
                    app.get_jm_client().invalidate_session();
                    changes = Some((previous, next));
                    if let Err(error) = std::fs::remove_file(&journal_path) {
                        tracing::warn!(error = %error, "状态恢复已完成，提交记录留待下次清理");
                    }
                    Ok(())
                }
                Err(error) => {
                    manager
                        .queue
                        .replace_entries(previous_entries)
                        .context("回退下载队列失败，已保留恢复记录")?;
                    crate::config_updates::commit_locked(app, &mut current, previous)
                        .context("回退配置失败，已保留恢复记录")?;
                    std::fs::remove_file(&journal_path)?;
                    Err(error)
                }
            }
        })
        .map_err(|err| CommandError::from("替换配置与下载队列失败", err))?;
    if let Some((previous, next)) = changes {
        apply_config_effects(app, &previous, &next);
        crate::config_updates::publish(app, &next);
    }
    Ok(())
}

#[tauri::command(async)]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub fn backup_app_data(app: AppHandle) -> CommandResult<String> {
    let storage = app.state::<ConfigStorage>();
    let config_state = app.get_config();
    let config = {
        let lock = config_state.read();
        lock.clone()
    };
    let queue_path = storage.config_path().with_file_name("download-queue.json");
    let queue_content: Option<serde_json::Value> = if queue_path.exists() {
        std::fs::read_to_string(&queue_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
    } else {
        None
    };

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());

    let backup_data = build_backup_data(
        &config,
        queue_content,
        &app.package_info().version.to_string(),
        timestamp,
    );

    let filename = format!("joymanga_backup_{timestamp}.json");
    let backup_dir = config.download_dir.clone();
    let _ = std::fs::create_dir_all(&backup_dir);
    let target_path = backup_dir.join(&filename);

    let content_str = serde_json::to_string_pretty(&backup_data)
        .context("序列化备份数据失败")
        .map_err(|err| CommandError::from("导出备份失败", err))?;
    atomic_fs::write(&target_path, content_str)
        .context("写入备份文件失败")
        .map_err(|err| CommandError::from("导出备份失败", err))?;

    tracing::info!(path = %target_path.display(), "已创建应用数据备份");
    Ok(target_path.to_string_lossy().to_string())
}

fn build_backup_data(
    config: &Config,
    queue_content: Option<serde_json::Value>,
    version: &str,
    timestamp: u64,
) -> serde_json::Value {
    // 备份可能位于用户指定的共享目录，不能把内存中的登录凭据写入其中。
    let mut backup_config = config.clone();
    backup_config.username.clear();
    backup_config.password.clear();
    backup_config.auto_login = false;
    serde_json::json!({
        "app": "joymanga",
        "version": version,
        "timestamp": timestamp,
        "config": backup_config,
        "queue": queue_content,
    })
}

#[tauri::command(async)]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub fn restore_app_data(app: AppHandle, backup_path: String) -> CommandResult<Config> {
    let path = PathBuf::from(&backup_path);
    if !path.is_file() {
        return Err(CommandError::from(
            "恢复备份失败",
            anyhow!("备份文件不存在"),
        ));
    }

    let file = std::fs::File::open(&path).map_err(|err| CommandError::from("恢复备份失败", err))?;
    let bytes = crate::image_data::read_limited(
        &file,
        file.metadata()
            .map_err(|err| CommandError::from("恢复备份失败", err))?
            .len(),
        8 * 1024 * 1024,
    )
    .map_err(|err| CommandError::from("恢复备份失败", err))?;
    let content = String::from_utf8(bytes)
        .context("读取备份文件失败")
        .map_err(|err| CommandError::from("恢复备份失败", err))?;

    let backup_value: serde_json::Value = serde_json::from_str(&content)
        .context("备份文件格式不正确（无法解析 JSON）")
        .map_err(|err| CommandError::from("恢复备份失败", err))?;

    let config_value = backup_value
        .get("config")
        .ok_or_else(|| CommandError::from("恢复备份失败", anyhow!("备份文件中缺少 config 字段")))?;

    let mut restored_config: Config = serde_json::from_value(config_value.clone())
        .context("解析备份配置失败")
        .map_err(|err| CommandError::from("恢复备份失败", err))?;
    restored_config.apply_storage_policy();
    restored_config
        .validate()
        .map_err(|err| CommandError::from("备份配置校验失败", err))?;
    let entries = DownloadQueue::parse_backup_entries(
        backup_value.get("queue"),
        &DownloadSettings::from(&restored_config),
    )
    .map_err(|err| CommandError::from("备份队列校验失败", err))?;
    replace_config_and_queue(&app, restored_config.clone(), entries)?;
    app.get_download_manager().restore_pending();

    if let Some(cache) = app.try_state::<utils::DownloadedLibraryCache>() {
        cache.invalidate();
    }

    tracing::info!(path = %backup_path, "已从备份恢复配置");
    Ok(get_config(app))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exported_backup_excludes_credentials_without_changing_live_config() {
        let mut config = Config::default(std::path::Path::new("app-data"));
        config.username = "private-user".into();
        config.password = "private-password".into();
        let backup = build_backup_data(&config, None, "test", 1);
        let serialized = serde_json::to_string(&backup).unwrap();
        assert!(!serialized.contains("private-user"));
        assert!(!serialized.contains("private-password"));
        assert_eq!(backup["config"]["username"], "");
        assert_eq!(backup["config"]["password"], "");
        assert_eq!(config.username, "private-user");
        assert_eq!(config.password, "private-password");
    }
}
