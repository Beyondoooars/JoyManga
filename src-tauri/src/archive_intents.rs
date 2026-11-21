use std::path::{Path, PathBuf};

use anyhow::{ensure, Context};
use indexmap::{IndexMap, IndexSet};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use crate::atomic_fs;
use crate::state_recovery;
use crate::types::CbzMode;

const MAX_INTENTS: usize = 2000;
pub const INTENTS_FILE_NAME: &str = "archive-intents.json";

/// 归档意图文件与配置同目录；启动、备份与事务恢复共用同一路径。
pub fn intents_path(config_path: &Path) -> PathBuf {
    config_path.with_file_name(INTENTS_FILE_NAME)
}

fn default_version() -> u32 {
    1
}

#[derive(Serialize, Deserialize)]
struct IntentFile {
    #[serde(default = "default_version")]
    version: u32,
    #[serde(default)]
    entries: IndexMap<i64, CbzMode>,
}

fn trim_ids(ids: &mut IndexSet<i64>) {
    while ids.len() > MAX_INTENTS {
        ids.shift_remove_index(0);
    }
}

fn trim_entries(entries: &mut IndexMap<i64, CbzMode>) {
    while entries.len() > MAX_INTENTS {
        entries.shift_remove_index(0);
    }
    for mode in entries.values_mut() {
        *mode = CbzMode::Merge;
    }
}

fn to_entries_map(ids: &IndexSet<i64>) -> IndexMap<i64, CbzMode> {
    ids.iter().map(|id| (*id, CbzMode::Merge)).collect()
}

fn parse_intent_value(value: serde_json::Value) -> anyhow::Result<IndexMap<i64, CbzMode>> {
    let mut file: IntentFile = serde_json::from_value(value).context("解析归档意图失败")?;
    ensure!(file.version == 1, "归档意图版本不受支持");
    trim_entries(&mut file.entries);
    Ok(file.entries)
}

fn write_intents_file(path: &Path, entries: &IndexMap<i64, CbzMode>) -> anyhow::Result<()> {
    let file = IntentFile {
        version: 1,
        entries: entries.clone(),
    };
    atomic_fs::write(path, serde_json::to_vec_pretty(&file)?).context("保存归档意图失败")
}

fn remove_intents_file(path: &Path) -> anyhow::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).context("删除归档意图失败"),
    }
}

pub(crate) fn read_file_snapshot(path: &Path) -> anyhow::Result<Option<serde_json::Value>> {
    let path = state_recovery::existing_path(path, "archive-intents");
    if path.exists() {
        Ok(Some(serde_json::from_slice(&std::fs::read(path)?)?))
    } else {
        Ok(None)
    }
}

/// 事务恢复使用：把备份中的意图快照写回磁盘；`None` 表示恢复为“没有意图文件”。
pub(crate) fn restore_intents_file(
    path: &Path,
    value: Option<&serde_json::Value>,
) -> anyhow::Result<()> {
    let path = &state_recovery::existing_path(path, "archive-intents");
    match value {
        Some(value) => {
            let entries = parse_intent_value(value.clone())?;
            write_intents_file(path, &entries)
        }
        None => remove_intents_file(path),
    }
}

/// 记录每本漫画期望的 CBZ 归档意图（下载完成后的自动整本合并）。
///
/// 与下载队列相互独立：队列负责任务级恢复，这里只负责「这本漫画下载完后是否需要整理」。
/// 整合成功后移除对应记录；移除保存失败时保留内存状态并通知后台错误。
pub struct ArchiveIntentStore {
    path: PathBuf,
    app: Option<AppHandle>,
    state: Mutex<IndexSet<i64>>,
}

impl ArchiveIntentStore {
    #[cfg(test)]
    pub fn load(path: PathBuf) -> anyhow::Result<Self> {
        Self::load_for_app(path, None).map(|(store, _)| store)
    }

    pub fn load_for_app(
        path: PathBuf,
        app: Option<AppHandle>,
    ) -> anyhow::Result<(Self, Vec<String>)> {
        let mut path = state_recovery::active_path(&path, "archive-intents");
        let mut notices = Vec::new();
        if let Some(notice) = state_recovery::recover_file_backup(&path)? {
            notices.push(notice);
        }
        let mut ids = IndexSet::new();
        if path.exists() {
            match std::fs::read(&path) {
                Ok(bytes) => {
                    let problem = match serde_json::from_slice::<IntentFile>(&bytes) {
                        Ok(file) if file.version == 1 => {
                            ids = file.entries.into_keys().collect();
                            trim_ids(&mut ids);
                            None
                        }
                        Ok(file) => Some(format!(
                            "归档意图版本 {} 不受支持，本次未载入",
                            file.version
                        )),
                        Err(error) => Some(format!("归档意图损坏，本次未载入：{error}")),
                    };
                    if let Some(problem) = problem {
                        let (active, notice) = state_recovery::preserve_or_use_fallback(
                            &path,
                            "archive-intents",
                            &problem,
                        );
                        path = active;
                        notices.push(notice);
                    }
                }
                Err(error) => {
                    path = path.with_file_name(".archive-intents.runtime.json");
                    tracing::error!(error = %error, "读取归档意图失败");
                    notices.push(format!("归档意图读取失败，本次使用独立运行文件：{error}"));
                }
            }
        }
        Ok((
            Self {
                path,
                app,
                state: Mutex::new(ids),
            },
            notices,
        ))
    }

    pub fn get(&self, comic_id: i64) -> Option<CbzMode> {
        self.state
            .lock()
            .contains(&comic_id)
            .then_some(CbzMode::Merge)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// 供备份读取当前意图的稳定快照。
    pub fn export_entries(&self) -> IndexMap<i64, CbzMode> {
        to_entries_map(&self.state.lock())
    }

    /// 整批替换：先成功落盘，再替换内存；失败时内存保持原样。
    pub fn replace_entries(&self, entries: IndexMap<i64, CbzMode>) -> anyhow::Result<()> {
        let mut ids: IndexSet<i64> = entries.into_keys().collect();
        trim_ids(&mut ids);
        write_intents_file(&self.path, &to_entries_map(&ids))?;
        *self.state.lock() = ids;
        Ok(())
    }

    /// 事务回滚：`Some` 写回旧快照，`None` 删除文件并清空内存。
    pub fn restore_snapshot(&self, value: Option<serde_json::Value>) -> anyhow::Result<()> {
        match value {
            Some(value) => {
                let entries = parse_intent_value(value)?;
                let mut ids: IndexSet<i64> = entries.into_keys().collect();
                trim_ids(&mut ids);
                write_intents_file(&self.path, &to_entries_map(&ids))?;
                *self.state.lock() = ids;
            }
            None => {
                remove_intents_file(&self.path)?;
                self.state.lock().clear();
            }
        }
        Ok(())
    }

    /// 解析备份中的 `archiveIntents` section；缺省/null 返回 `None`，空对象返回 `Some(空)`。
    pub fn parse_backup_section(
        value: Option<&serde_json::Value>,
    ) -> anyhow::Result<Option<IndexMap<i64, CbzMode>>> {
        let Some(value) = value.filter(|value| !value.is_null()) else {
            return Ok(None);
        };
        parse_intent_value(value.clone()).map(Some)
    }

    /// 供诊断包读取当前待执行的归档意图（只读快照）。
    pub fn entries(&self) -> Vec<(i64, CbzMode)> {
        self.state
            .lock()
            .iter()
            .map(|comic_id| (*comic_id, CbzMode::Merge))
            .collect()
    }

    /// 整合成功后清除意图；保存失败保留重试状态，不阻断已完成的文件整合。
    pub fn remove(&self, comic_id: i64) {
        let mut state = self.state.lock();
        let previous = state.clone();
        if state.shift_remove(&comic_id) {
            let _ = self.save_or_restore(state, previous);
        }
    }

    pub fn set(&self, comic_id: i64, _mode: CbzMode) -> anyhow::Result<()> {
        let mut state = self.state.lock();
        let previous = state.clone();
        state.shift_remove(&comic_id);
        state.insert(comic_id);
        trim_ids(&mut state);
        self.save_or_restore(state, previous)
    }

    /// 返回本次是否新增；已有意图保持不变。
    pub fn set_if_absent(&self, comic_id: i64, _mode: CbzMode) -> anyhow::Result<bool> {
        let mut state = self.state.lock();
        if state.contains(&comic_id) {
            return Ok(false);
        }
        let previous = state.clone();
        state.insert(comic_id);
        trim_ids(&mut state);
        self.save_or_restore(state, previous).map(|()| true)
    }

    fn save_or_restore(
        &self,
        mut state: parking_lot::MutexGuard<'_, IndexSet<i64>>,
        previous: IndexSet<i64>,
    ) -> anyhow::Result<()> {
        let result = self.save(&state);
        let app = self.app.clone();
        match result {
            Ok(()) => Ok(()),
            Err(error) => {
                *state = previous;
                drop(state);
                state_recovery::background_failure(
                    app.as_ref(),
                    "archive-intents",
                    "归档设置未保存，本次选择未生效",
                    &error,
                );
                Err(error)
            }
        }
    }

    fn save(&self, state: &IndexSet<i64>) -> anyhow::Result<()> {
        write_intents_file(&self.path, &to_entries_map(state))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intents_roundtrip_and_survive_restart() {
        let root = std::env::temp_dir().join(format!("jm-intents-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("archive-intents.json");
        {
            let store = ArchiveIntentStore::load(path.clone()).unwrap();
            store.set(101, CbzMode::Merge).unwrap();
            store.set(102, CbzMode::Merge).unwrap();
            store.set(101, CbzMode::Merge).unwrap();
        }
        let reloaded = ArchiveIntentStore::load(path.clone()).unwrap();
        assert_eq!(reloaded.get(101), Some(CbzMode::Merge));
        assert_eq!(reloaded.get(102), Some(CbzMode::Merge));
        reloaded.remove(101);
        assert_eq!(reloaded.get(101), None);
        let reloaded = ArchiveIntentStore::load(path).unwrap();
        assert_eq!(reloaded.get(101), None);
        assert_eq!(reloaded.get(102), Some(CbzMode::Merge));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn legacy_per_chapter_intent_loads_as_merge() {
        let root = std::env::temp_dir().join(format!("jm-intents-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("archive-intents.json");
        std::fs::write(
            &path,
            serde_json::to_vec(&serde_json::json!({
                "version": 1,
                "entries": {"102": "perChapter"},
            }))
            .unwrap(),
        )
        .unwrap();
        let store = ArchiveIntentStore::load(path).unwrap();
        assert_eq!(store.get(102), Some(CbzMode::Merge));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn backup_section_distinguishes_missing_from_empty() {
        let empty = ArchiveIntentStore::parse_backup_section(Some(&serde_json::json!({
            "version": 1,
            "entries": {},
        })))
        .unwrap();
        assert!(empty.unwrap().is_empty());
        let legacy = ArchiveIntentStore::parse_backup_section(Some(&serde_json::json!({
            "version": 1,
            "entries": {"102": "perChapter"},
        })))
        .unwrap()
        .unwrap();
        assert_eq!(legacy.get(&102), Some(&CbzMode::Merge));
        assert!(ArchiveIntentStore::parse_backup_section(None)
            .unwrap()
            .is_none());
        assert!(
            ArchiveIntentStore::parse_backup_section(Some(&serde_json::Value::Null))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn backup_section_rejects_unknown_version() {
        let error = ArchiveIntentStore::parse_backup_section(Some(&serde_json::json!({
            "version": 2,
            "entries": {},
        })))
        .unwrap_err();
        assert!(error.to_string().contains("版本"));
    }

    #[test]
    fn corrupt_intent_file_is_backed_up_and_ignored() {
        let root = std::env::temp_dir().join(format!("jm-intents-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("archive-intents.json");
        std::fs::write(&path, b"broken json").unwrap();
        let store = ArchiveIntentStore::load(path.clone()).unwrap();
        assert_eq!(store.get(1), None);
        assert!(!path.exists());
        let backup_count = std::fs::read_dir(&root).unwrap().count();
        assert_eq!(backup_count, 1);
        std::fs::remove_dir_all(root).unwrap();
    }
}
