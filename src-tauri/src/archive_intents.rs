use std::path::{Path, PathBuf};

use anyhow::{ensure, Context};
use indexmap::IndexMap;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::atomic_fs;
use crate::types::CbzMode;

const MAX_INTENTS: usize = 200;
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

fn trim_entries(entries: &mut IndexMap<i64, CbzMode>) {
    while entries.len() > MAX_INTENTS {
        entries.shift_remove_index(0);
    }
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

/// 事务恢复使用：把备份中的意图快照写回磁盘；`None` 表示恢复为“没有意图文件”。
pub(crate) fn restore_intents_file(
    path: &Path,
    value: Option<&serde_json::Value>,
) -> anyhow::Result<()> {
    match value {
        Some(value) => {
            let entries = parse_intent_value(value.clone())?;
            write_intents_file(path, &entries)
        }
        None => remove_intents_file(path),
    }
}

/// 记录每本漫画期望的 CBZ 归档方式（下载完成后的自动整合行为）。
///
/// 与下载队列相互独立：队列负责任务级恢复，这里只负责「这本漫画下载完后怎么整理」。
/// 整合成功后移除对应记录；写入失败只记录日志，不影响下载本身。
pub struct ArchiveIntentStore {
    path: PathBuf,
    state: Mutex<IndexMap<i64, CbzMode>>,
}

impl ArchiveIntentStore {
    pub fn load(path: PathBuf) -> anyhow::Result<Self> {
        atomic_fs::recover_file_backup(&path)?;
        let mut entries = IndexMap::new();
        if path.exists() {
            let parsed = std::fs::read(&path)
                .map_err(anyhow::Error::from)
                .and_then(|bytes| {
                    serde_json::from_slice::<IntentFile>(&bytes).map_err(anyhow::Error::from)
                });
            match parsed {
                Ok(file) if file.version == 1 => entries = file.entries,
                Ok(_) => {
                    tracing::warn!("归档意图文件版本不受支持，已忽略");
                }
                Err(error) => {
                    let backup = path.with_file_name(format!(
                        "archive-intents.corrupt-{}.json",
                        uuid::Uuid::new_v4()
                    ));
                    let _ = std::fs::rename(&path, &backup);
                    tracing::error!(
                        error = %error,
                        backup = %backup.display(),
                        "归档意图文件损坏，已保留备份"
                    );
                }
            }
        }
        Ok(Self {
            path,
            state: Mutex::new(entries),
        })
    }

    pub fn get(&self, comic_id: i64) -> Option<CbzMode> {
        self.state.lock().get(&comic_id).copied()
    }

    /// 供备份读取当前意图的稳定快照。
    pub fn export_entries(&self) -> IndexMap<i64, CbzMode> {
        self.state.lock().clone()
    }

    /// 整批替换：先成功落盘，再替换内存；失败时内存保持原样。
    pub fn replace_entries(&self, mut entries: IndexMap<i64, CbzMode>) -> anyhow::Result<()> {
        trim_entries(&mut entries);
        write_intents_file(&self.path, &entries)?;
        *self.state.lock() = entries;
        Ok(())
    }

    /// 事务回滚：`Some` 写回旧快照，`None` 删除文件并清空内存。
    pub fn restore_snapshot(&self, value: Option<serde_json::Value>) -> anyhow::Result<()> {
        match value {
            Some(value) => {
                let entries = parse_intent_value(value)?;
                write_intents_file(&self.path, &entries)?;
                *self.state.lock() = entries;
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
            .map(|(comic_id, mode)| (*comic_id, *mode))
            .collect()
    }

    pub fn remove(&self, comic_id: i64) {
        let mut state = self.state.lock();
        if state.shift_remove(&comic_id).is_some() {
            if let Err(error) = self.save(&state) {
                tracing::error!(error = %error, "保存归档意图失败");
            }
        }
    }

    pub fn set(&self, comic_id: i64, mode: CbzMode) {
        let mut state = self.state.lock();
        state.shift_remove(&comic_id);
        state.insert(comic_id, mode);
        while state.len() > MAX_INTENTS {
            state.shift_remove_index(0);
        }
        if let Err(error) = self.save(&state) {
            tracing::error!(error = %error, "保存归档意图失败");
        }
    }

    fn save(&self, state: &IndexMap<i64, CbzMode>) -> anyhow::Result<()> {
        write_intents_file(&self.path, state)
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
            store.set(101, CbzMode::Merge);
            store.set(102, CbzMode::PerChapter);
            store.set(101, CbzMode::PerChapter);
        }
        let reloaded = ArchiveIntentStore::load(path.clone()).unwrap();
        assert_eq!(reloaded.get(101), Some(CbzMode::PerChapter));
        assert_eq!(reloaded.get(102), Some(CbzMode::PerChapter));
        reloaded.remove(101);
        assert_eq!(reloaded.get(101), None);
        let reloaded = ArchiveIntentStore::load(path).unwrap();
        assert_eq!(reloaded.get(101), None);
        assert_eq!(reloaded.get(102), Some(CbzMode::PerChapter));
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
