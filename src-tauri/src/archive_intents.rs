use std::path::PathBuf;

use anyhow::Context;
use indexmap::IndexMap;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::atomic_fs;
use crate::types::CbzMode;

const MAX_INTENTS: usize = 200;

#[derive(Serialize, Deserialize)]
struct IntentFile {
    version: u32,
    entries: IndexMap<i64, CbzMode>,
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

    /// 出厂重置等场景下清空全部记录。
    pub fn clear(&self) {
        let mut state = self.state.lock();
        if state.is_empty() {
            return;
        }
        state.clear();
        if let Err(error) = self.save(&state) {
            tracing::error!(error = %error, "清空归档意图失败");
        }
    }

    fn save(&self, state: &IndexMap<i64, CbzMode>) -> anyhow::Result<()> {
        let file = IntentFile {
            version: 1,
            entries: state.clone(),
        };
        atomic_fs::write(&self.path, serde_json::to_vec_pretty(&file)?).context("保存归档意图失败")
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
