#[cfg(target_os = "android")]
use crate::extensions::AppHandleExt;
use crate::{config::Config, events::ConfigChangedEvent};
use anyhow::{ensure, Context};
use tauri::AppHandle;
use tauri_specta::Event;

pub fn merge_config_changes(
    current: &Config,
    base: &Config,
    desired: &Config,
) -> anyhow::Result<Config> {
    ensure!(
        base.revision <= current.revision,
        "设置版本无效，请重新载入"
    );
    let mut merged = serde_json::to_value(current)?;
    let base = serde_json::to_value(base)?;
    let desired = serde_json::to_value(desired)?;
    for (key, value) in desired.as_object().context("设置格式无效")? {
        if matches!(
            key.as_str(),
            "revision" | "schemaVersion" | "androidStorageInitialized" | "exportDir"
        ) || base.get(key) == Some(value)
        {
            continue;
        }
        ensure!(
            merged.get(key) == base.get(key) || merged.get(key) == Some(value),
            "设置 {key} 已被另一操作更新，请重新确认后保存"
        );
        merged[key] = value.clone();
    }
    let mut config: Config = serde_json::from_value(merged)?;
    config.apply_storage_policy();
    config.validate()?;
    Ok(config)
}

pub fn commit_locked(
    app: &AppHandle,
    current: &mut Config,
    mut next: Config,
) -> anyhow::Result<Config> {
    next.revision = current.revision.checked_add(1).context("设置版本耗尽")?;
    ensure!(next.revision < (1_u64 << 53), "设置版本超出客户端范围");
    next.schema_version = 1;
    next.apply_download_policy();
    next.apply_storage_policy();
    next.validate()?;
    std::fs::create_dir_all(&next.download_dir).context("创建书库目录失败")?;
    std::fs::create_dir_all(&next.export_dir).context("创建导出目录失败")?;
    next.save(app)?;
    *current = next.clone();
    Ok(next)
}

pub fn publish(app: &AppHandle, config: &Config) {
    let _ = ConfigChangedEvent {
        config: config.clone(),
    }
    .emit(app);
}

#[cfg(target_os = "android")]
pub fn update(
    app: &AppHandle,
    change: impl FnOnce(&Config) -> anyhow::Result<Config>,
) -> anyhow::Result<Config> {
    let state = app.get_config();
    let next = {
        let mut current = state.write();
        let next = change(&current)?;
        commit_locked(app, &mut current, next)?
    };
    publish(app, &next);
    Ok(next)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stale_snapshot_preserves_new_route_but_merges_unrelated_changes() {
        let base = Config::default(std::path::Path::new("app"));
        let mut current = base.clone();
        current.revision = 1;
        current.api_domain_mode = crate::config::ApiDomainMode::Domain3;
        let mut desired = base.clone();
        desired.img_concurrency = 7;
        let merged = merge_config_changes(&current, &base, &desired).unwrap();
        assert_eq!(merged.api_domain_mode, current.api_domain_mode);
        assert_eq!(merged.img_concurrency, 7);
        desired.api_domain_mode = crate::config::ApiDomainMode::Domain4;
        assert!(merge_config_changes(&current, &base, &desired).is_err());
    }
    #[test]
    fn old_export_field_cannot_diverge_from_the_changed_library() {
        let base = Config::default(std::path::Path::new("app"));
        let mut desired = base.clone();
        desired.download_dir = std::path::PathBuf::from("new-library");
        desired.export_dir = std::path::PathBuf::from("old-export-choice");
        let merged = merge_config_changes(&base, &base, &desired).unwrap();
        assert_eq!(merged.download_dir, desired.download_dir);
        assert_eq!(merged.export_dir, desired.download_dir);
    }
}
