use std::path::PathBuf;
#[cfg(target_os = "android")]
use std::path::{Component, Path};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, ensure, Context};
use indexmap::IndexMap;
use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::archive_intents::ArchiveIntentStore;
use crate::atomic_fs;
use crate::config::{Config, ConfigStorage};
use crate::download_queue::{DownloadQueue, DownloadSettings, StoredDownload};
use crate::errors::{CommandError, CommandResult};
use crate::extensions::AppHandleExt;
use crate::types::CbzMode;
use crate::utils;

const BACKUP_FORMAT_VERSION: u32 = 2;
const MAX_BACKUP_BYTES: usize = 8 * 1024 * 1024;
const MAX_READING_STATE_BYTES: usize = 2 * 1024 * 1024;

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
    replace_config_and_queue(
        &app,
        default_config.clone(),
        Vec::new(),
        Some(IndexMap::new()),
    )?;

    if let Some(cache) = app.try_state::<utils::DownloadedLibraryCache>() {
        cache.invalidate();
    }

    tracing::info!("应用已恢复出厂设置并重置任务队列");
    Ok(get_config(app))
}

#[derive(Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct RestoreOutcome {
    pub config: Config,
    pub reading_state: Option<serde_json::Value>,
    pub archive_intents_restored: Option<u32>,
    pub backup_version: u32,
}

struct ParsedBackup {
    config: Config,
    entries: Vec<StoredDownload>,
    reading_state: Option<serde_json::Value>,
    intents: Option<IndexMap<i64, CbzMode>>,
    backup_version: u32,
}

fn validate_reading_state_section(value: &Option<serde_json::Value>) -> anyhow::Result<()> {
    let Some(value) = value else {
        return Ok(());
    };
    let object = value.as_object().context("阅读记录格式无效")?;
    ensure!(
        object.get("version").and_then(serde_json::Value::as_u64) == Some(1),
        "阅读记录版本不受支持"
    );
    ensure!(
        object
            .get("progress")
            .is_some_and(serde_json::Value::is_object),
        "阅读记录进度格式无效"
    );
    ensure!(
        object
            .get("bookmarks")
            .is_some_and(serde_json::Value::is_array),
        "阅读记录书签格式无效"
    );
    let size = serde_json::to_vec(value)
        .context("序列化阅读记录失败")?
        .len();
    ensure!(size <= MAX_READING_STATE_BYTES, "阅读记录超过 2MB");
    Ok(())
}

fn parse_backup_document(backup_value: &serde_json::Value) -> anyhow::Result<ParsedBackup> {
    let backup_version = match backup_value.get("backupVersion") {
        None => 1,
        Some(serde_json::Value::Number(number)) => {
            let version = number.as_u64().context("备份格式版本无效")?;
            ensure!((1..=2).contains(&version), "备份格式版本不受支持");
            version as u32
        }
        Some(_) => return Err(anyhow!("备份格式版本无效")),
    };
    let app_name = backup_value.get("app").and_then(serde_json::Value::as_str);
    if backup_version >= 2 {
        ensure!(app_name == Some("joymanga"), "备份应用标记不匹配");
    } else if app_name != Some("joymanga") {
        tracing::warn!(app = ?app_name, "旧备份缺少应用标记，仍按兼容格式尝试恢复");
    }

    let config_value = backup_value
        .get("config")
        .context("备份文件中缺少 config 字段")?;
    let mut config = crate::config::deserialize_config_value(config_value.clone())
        .context("解析备份配置失败")?;
    config.apply_storage_policy();
    config.validate().context("备份配置校验失败")?;

    let settings = DownloadSettings::from(&config);
    let entries = DownloadQueue::parse_backup_entries(backup_value.get("queue"), &settings)
        .context("备份队列校验失败")?;
    let reading_state = backup_value
        .get("readingState")
        .filter(|value| !value.is_null())
        .cloned();
    validate_reading_state_section(&reading_state).context("备份阅读记录校验失败")?;
    let intents = ArchiveIntentStore::parse_backup_section(backup_value.get("archiveIntents"))
        .context("备份归档意图校验失败")?;

    Ok(ParsedBackup {
        config,
        entries,
        reading_state,
        intents,
        backup_version,
    })
}

fn replace_config_and_queue(
    app: &AppHandle,
    config: Config,
    entries: Vec<StoredDownload>,
    intents: Option<IndexMap<i64, CbzMode>>,
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
                Some(serde_json::from_slice::<serde_json::Value>(
                    &std::fs::read(&queue_path)?,
                )?)
            } else {
                None
            };
            let intents_path = crate::archive_intents::intents_path(storage.config_path());
            let intents_managed = intents.is_some();
            let previous_intents = if intents_managed && intents_path.exists() {
                Some(serde_json::from_slice::<serde_json::Value>(
                    &std::fs::read(&intents_path)?,
                )?)
            } else {
                None
            };
            let journal_path = storage.config_path().with_file_name(".state-replace.json");
            let mut journal = crate::state_transactions::StateJournal {
                previous_config: previous.clone(),
                previous_queue,
                previous_intents: previous_intents.clone(),
                intents_managed,
                committed: false,
            };
            journal.save(&journal_path)?;
            let result = (|| -> anyhow::Result<Config> {
                let next = crate::config_updates::commit_locked(app, &mut current, config)?;
                manager.queue.replace_entries(entries)?;
                if let Some(intents) = intents {
                    app.state::<ArchiveIntentStore>().replace_entries(intents)?;
                }
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
                    let mut rollback_errors = Vec::new();
                    if intents_managed {
                        if let Err(rollback_error) = app
                            .state::<ArchiveIntentStore>()
                            .restore_snapshot(previous_intents.clone())
                        {
                            rollback_errors.push(format!("回退归档意图失败：{rollback_error:#}"));
                        }
                    }
                    if let Err(rollback_error) = manager.queue.replace_entries(previous_entries) {
                        rollback_errors.push(format!("回退下载队列失败：{rollback_error:#}"));
                    }
                    if let Err(rollback_error) =
                        crate::config_updates::commit_locked(app, &mut current, previous)
                    {
                        rollback_errors.push(format!("回退配置失败：{rollback_error:#}"));
                    }
                    if rollback_errors.is_empty() {
                        if let Err(remove_error) = std::fs::remove_file(&journal_path) {
                            tracing::warn!(
                                error = %remove_error,
                                "回退完成但提交记录删除失败，留待下次清理"
                            );
                        }
                        Err(error)
                    } else {
                        Err(error.context(format!(
                            "回退尚未完成，已保留恢复记录：{}",
                            rollback_errors.join("；")
                        )))
                    }
                }
            }
        })
        .map_err(|err| CommandError::from("替换应用状态失败", err))?;
    if let Some((previous, next)) = changes {
        apply_config_effects(app, &previous, &next);
        crate::config_updates::publish(app, &next);
    }
    Ok(())
}

#[tauri::command(async)]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub fn backup_app_data(
    app: AppHandle,
    reading_state: Option<serde_json::Value>,
) -> CommandResult<String> {
    validate_reading_state_section(&reading_state)
        .context("阅读记录格式无效")
        .map_err(|err| CommandError::from("导出备份失败", err))?;
    let config_state = app.get_config();
    let config = {
        let lock = config_state.read();
        lock.clone()
    };
    let queue_content = app.get_download_manager().queue.backup_snapshot();
    let intents = app.state::<ArchiveIntentStore>().export_entries();

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());

    let backup_data = build_backup_data(
        &config,
        queue_content,
        intents,
        reading_state,
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
    if content_str.len() > MAX_BACKUP_BYTES {
        return Err(CommandError::from(
            "导出备份失败",
            anyhow!("备份数据超过 8MB，无法写入"),
        ));
    }
    atomic_fs::write(&target_path, content_str)
        .context("写入备份文件失败")
        .map_err(|err| CommandError::from("导出备份失败", err))?;

    tracing::info!(path = %target_path.display(), "已创建应用数据备份");
    Ok(target_path.to_string_lossy().to_string())
}

fn build_backup_data(
    config: &Config,
    queue_content: serde_json::Value,
    intents: IndexMap<i64, CbzMode>,
    reading_state: Option<serde_json::Value>,
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
        "backupVersion": BACKUP_FORMAT_VERSION,
        "version": version,
        "timestamp": timestamp,
        "config": backup_config,
        "queue": queue_content,
        "archiveIntents": {
            "version": 1,
            "entries": intents,
        },
        "readingState": reading_state,
    })
}

#[tauri::command(async)]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub fn restore_app_data(app: AppHandle, backup_path: String) -> CommandResult<RestoreOutcome> {
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
        MAX_BACKUP_BYTES as u64,
    )
    .map_err(|err| CommandError::from("恢复备份失败", err))?;
    let content = String::from_utf8(bytes)
        .context("读取备份文件失败")
        .map_err(|err| CommandError::from("恢复备份失败", err))?;

    let backup_value: serde_json::Value = serde_json::from_str(&content)
        .context("备份文件格式不正确（无法解析 JSON）")
        .map_err(|err| CommandError::from("恢复备份失败", err))?;

    // 备份里的账号密码若已是 enc:v1 密文，必须和正常启动一样先解密；
    // 否则密文会被当成明文继续保存、并用于登录。
    let ParsedBackup {
        config,
        entries,
        reading_state,
        intents,
        backup_version,
    } = parse_backup_document(&backup_value)
        .map_err(|err| CommandError::from("恢复备份失败", err))?;
    let archive_intents_restored = intents
        .as_ref()
        .map(|entries| u32::try_from(entries.len()).unwrap_or(u32::MAX));
    replace_config_and_queue(&app, config, entries, intents)?;
    app.get_download_manager().restore_pending();

    if let Some(cache) = app.try_state::<utils::DownloadedLibraryCache>() {
        cache.invalidate();
    }

    tracing::info!(
        path = %backup_path,
        backup_version,
        intents = archive_intents_restored.unwrap_or(0),
        "已从备份恢复应用数据"
    );
    Ok(RestoreOutcome {
        config: get_config(app),
        reading_state,
        archive_intents_restored,
        backup_version,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_queue() -> serde_json::Value {
        serde_json::json!({ "version": 1, "tasks": [] })
    }

    fn reading_state() -> serde_json::Value {
        serde_json::json!({ "version": 1, "progress": {}, "bookmarks": [] })
    }

    #[test]
    fn exported_backup_excludes_credentials_without_changing_live_config() {
        let mut config = Config::default(std::path::Path::new("app-data"));
        config.username = "private-user".into();
        config.password = "private-password".into();
        let backup = build_backup_data(&config, empty_queue(), IndexMap::new(), None, "test", 1);
        let serialized = serde_json::to_string(&backup).unwrap();
        assert!(!serialized.contains("private-user"));
        assert!(!serialized.contains("private-password"));
        assert_eq!(backup["config"]["username"], "");
        assert_eq!(backup["config"]["password"], "");
        assert_eq!(config.username, "private-user");
        assert_eq!(config.password, "private-password");
    }

    #[test]
    fn v2_backup_marks_format_and_includes_new_sections() {
        let config = Config::default(std::path::Path::new("app-data"));
        let mut intents = IndexMap::new();
        intents.insert(42, CbzMode::Merge);
        let backup = build_backup_data(
            &config,
            empty_queue(),
            intents,
            Some(reading_state()),
            "test",
            1,
        );
        assert_eq!(backup["backupVersion"], 2);
        assert_eq!(backup["archiveIntents"]["version"], 1);
        assert_eq!(backup["archiveIntents"]["entries"]["42"], "merge");
        assert_eq!(backup["readingState"], reading_state());

        let parsed = parse_backup_document(&backup).unwrap();
        assert_eq!(parsed.backup_version, 2);
        assert_eq!(parsed.reading_state, Some(reading_state()));
        assert_eq!(parsed.intents.unwrap().get(&42), Some(&CbzMode::Merge));
    }

    #[test]
    fn v1_backup_without_new_sections_remains_restorable() {
        let config = Config::default(std::path::Path::new("app-data"));
        let backup = serde_json::json!({
            "config": config,
            "queue": empty_queue(),
        });
        let parsed = parse_backup_document(&backup).unwrap();
        assert_eq!(parsed.backup_version, 1);
        assert!(parsed.reading_state.is_none());
        assert!(parsed.intents.is_none());
        assert!(parsed.entries.is_empty());
    }

    #[test]
    fn v2_backup_requires_matching_app_marker() {
        let config = Config::default(std::path::Path::new("app-data"));
        let mut backup =
            build_backup_data(&config, empty_queue(), IndexMap::new(), None, "test", 1);
        backup["app"] = serde_json::json!("other-app");
        assert!(parse_backup_document(&backup).is_err());
    }

    #[test]
    fn unsupported_backup_version_is_rejected() {
        let config = Config::default(std::path::Path::new("app-data"));
        let mut backup =
            build_backup_data(&config, empty_queue(), IndexMap::new(), None, "test", 1);
        backup["backupVersion"] = serde_json::json!(3);
        assert!(parse_backup_document(&backup).is_err());
    }

    #[test]
    fn malformed_reading_state_is_rejected() {
        let config = Config::default(std::path::Path::new("app-data"));
        let invalid = serde_json::json!({
            "version": 1,
            "progress": [],
            "bookmarks": [],
        });
        let backup = build_backup_data(
            &config,
            empty_queue(),
            IndexMap::new(),
            Some(invalid),
            "test",
            1,
        );
        assert!(parse_backup_document(&backup).is_err());
    }
}
