use std::sync::OnceLock;

use crate::extensions::AppHandleExt;
use crate::{config::Config, events::ConfigChangedEvent};
use anyhow::{ensure, Context};
use parking_lot::{Mutex, MutexGuard};
use tauri::AppHandle;
use tauri_specta::Event;

static CONFIG_WRITE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

/// 序列化配置落盘提交的写锁令牌；持锁期间仅保护「检查修订号 → 写盘 → 交换内存配置」顺序，
/// 磁盘 I/O 不再阻塞 `Config` 读锁访问。
pub struct ConfigWriteGuard(#[allow(dead_code)] MutexGuard<'static, ()>);

pub fn write_guard() -> ConfigWriteGuard {
    ConfigWriteGuard(CONFIG_WRITE_LOCK.get_or_init(|| Mutex::new(())).lock())
}

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
            "revision" | "schemaVersion" | "androidStorageInitialized"
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

pub fn commit(
    app: &AppHandle,
    _guard: &ConfigWriteGuard,
    mut next: Config,
) -> anyhow::Result<Config> {
    let current_revision = app.get_config().read().revision;
    next.revision = current_revision.checked_add(1).context("设置版本耗尽")?;
    ensure!(next.revision < (1_u64 << 53), "设置版本超出客户端范围");
    next.schema_version = crate::config::CONFIG_SCHEMA_VERSION;
    next.apply_storage_policy();
    next.validate()?;
    std::fs::create_dir_all(&next.download_dir).context("创建书库目录失败")?;
    next.save(app)?;
    *app.get_config().write() = next.clone();
    Ok(next)
}

pub fn publish(app: &AppHandle, config: &Config) {
    let _ = ConfigChangedEvent {
        config: config.clone(),
    }
    .emit(app);
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
    fn library_dir_change_merges_cleanly() {
        let base = Config::default(std::path::Path::new("app"));
        let mut desired = base.clone();
        desired.download_dir = std::path::PathBuf::from("new-library");
        let merged = merge_config_changes(&base, &base, &desired).unwrap();
        assert_eq!(merged.download_dir, desired.download_dir);
    }

    #[test]
    fn config_write_guard_and_download_policy_lock_order_never_deadlocks() {
        let _state_lock = crate::network_state::STATE_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let cfg = std::sync::Arc::new(parking_lot::RwLock::new(Config::default(
            std::path::Path::new("app"),
        )));
        let tasks = std::sync::Arc::new(parking_lot::RwLock::new(vec![1_i64]));

        let cfg_a = std::sync::Arc::clone(&cfg);
        let tasks_a = std::sync::Arc::clone(&tasks);
        let thread_a = std::thread::spawn(move || {
            for _ in 0..40 {
                crate::network_state::with_download_policy(|| {
                    let guard = write_guard();
                    let next = cfg_a.read().clone();
                    *cfg_a.write() = next;
                    drop(guard);
                    let _ = tasks_a.read().len();
                });
            }
        });

        let cfg_b = std::sync::Arc::clone(&cfg);
        let tasks_b = std::sync::Arc::clone(&tasks);
        let thread_b = std::thread::spawn(move || {
            for _ in 0..40 {
                let mut t = tasks_b.write();
                t[0] += 1;
                let _guard = write_guard();
                let next = cfg_b.read().clone();
                *cfg_b.write() = next;
            }
        });

        thread_a.join().unwrap();
        thread_b.join().unwrap();
    }
}
