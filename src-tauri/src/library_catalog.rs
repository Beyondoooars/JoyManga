use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{ensure, Context};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::{atomic_fs, config::ConfigStorage};

pub(crate) static CATALOG_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct RegisteredCbz {
    root: String,
    pub id: i64,
    pub path: PathBuf,
}

#[derive(Clone, Serialize, Deserialize, PartialEq)]
struct RemovedRecord {
    root: String,
    id: i64,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct LibraryCatalog {
    version: u8,
    imports: Vec<RegisteredCbz>,
    removed: Vec<RemovedRecord>,
}

impl Default for LibraryCatalog {
    fn default() -> Self {
        Self {
            version: 1,
            imports: Vec::new(),
            removed: Vec::new(),
        }
    }
}

fn root_key(root: &Path) -> String {
    let path = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    path.to_string_lossy().into_owned()
}

pub(crate) fn catalog_path(app: &AppHandle) -> PathBuf {
    app.state::<ConfigStorage>()
        .config_path()
        .with_file_name(".library.json")
}

impl LibraryCatalog {
    /// 读取书库记录。文件损坏时先把原文件改名备份（便于恢复与排查），
    /// 再以空记录继续，避免一个坏文件让整个书库“消失”且无法自愈。
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        atomic_fs::recover_file_backup(path)?;
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default())
            }
            Err(error) => return Err(error).context("读取书库记录失败"),
        };
        match Self::parse(&bytes) {
            Ok(catalog) => Ok(catalog),
            Err(error) => {
                let backup = path.with_file_name(format!(
                    "{}.corrupt-{}",
                    path.file_name().unwrap_or_default().to_string_lossy(),
                    uuid::Uuid::new_v4()
                ));
                match std::fs::rename(path, &backup) {
                    Ok(()) => tracing::error!(
                        error = %error,
                        backup = %backup.display(),
                        "书库记录损坏，已备份原文件并以空记录继续"
                    ),
                    Err(rename_error) => tracing::error!(
                        error = %error,
                        rename_error = %rename_error,
                        "书库记录损坏且无法备份，本次以空记录继续"
                    ),
                }
                Ok(Self::default())
            }
        }
    }

    fn parse(bytes: &[u8]) -> anyhow::Result<Self> {
        let catalog: Self = serde_json::from_slice(bytes).context("书库记录文件损坏")?;
        ensure!(catalog.version == 1, "不支持的书库记录版本");
        Ok(catalog)
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        atomic_fs::write(path, serde_json::to_vec_pretty(self)?)
    }

    /// 删除事务回滚/恢复使用：这些路径已在“隔离改名失败”分支被直接删除，
    /// 不能再把它们的导入记录写回书库。
    ///
    /// 只丢弃当前确实不存在的路径：若崩溃发生在删除之前，文件仍在，记录必须保留。
    pub fn forget_missing_paths(&mut self, paths: &[PathBuf]) {
        let missing: Vec<&PathBuf> = paths.iter().filter(|path| !path.exists()).collect();
        if missing.is_empty() {
            return;
        }
        self.imports.retain(|record| {
            let record_path =
                std::fs::canonicalize(&record.path).unwrap_or_else(|_| record.path.clone());
            !missing
                .iter()
                .any(|path| record_path == **path || record_path.starts_with(*path))
        });
    }

    pub fn removed_ids(&self, root: &Path) -> HashSet<i64> {
        let key = root_key(root);
        self.removed
            .iter()
            .filter(|record| record.root == key)
            .map(|record| record.id)
            .collect()
    }

    pub fn visible_imports(&self, root: &Path) -> Vec<RegisteredCbz> {
        let key = root_key(root);
        let removed = self.removed_ids(root);
        self.imports
            .iter()
            .filter(|record| record.root == key && !removed.contains(&record.id))
            .cloned()
            .collect()
    }

    pub fn restore(&mut self, root: &Path, id: i64) -> bool {
        let key = root_key(root);
        let before = self.removed.len();
        self.removed
            .retain(|record| record.root != key || record.id != id);
        before != self.removed.len()
    }

    pub fn remove(&mut self, root: &Path, id: i64) {
        let record = RemovedRecord {
            root: root_key(root),
            id,
        };
        if !self.removed.contains(&record) {
            self.removed.push(record);
        }
    }

    pub fn register(&mut self, root: &Path, path: &Path) -> anyhow::Result<(i64, bool)> {
        let path = std::fs::canonicalize(path)?;
        let key = root_key(root);
        if let Some(id) = self
            .imports
            .iter()
            .find(|record| record.root == key && root_key(&record.path) == root_key(&path))
            .map(|record| record.id)
        {
            return Ok((id, self.restore(root, id)));
        }
        let digest = format!("{:x}", md5::compute(root_key(&path)));
        let id = -i64::from_str_radix(&digest[..13], 16)?.max(1);
        ensure!(
            !self
                .imports
                .iter()
                .any(|record| record.root == key && record.id == id),
            "导入文件的编号发生冲突"
        );
        self.imports.push(RegisteredCbz {
            root: key,
            id,
            path,
        });
        self.restore(root, id);
        Ok((id, true))
    }
}

pub(crate) fn registered_cbz_comic(record: &RegisteredCbz) -> anyhow::Result<crate::types::Comic> {
    use crate::types::{ChapterInfo, Comic};
    ensure!(record.path.is_file(), "已登记的 CBZ 文件不存在");
    let original = Comic::from_cbz(&record.path).ok();
    let title = record
        .path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    Ok(Comic {
        id: record.id,
        name: original.as_ref().map_or(title, |comic| comic.name.clone()),
        author: original
            .as_ref()
            .map_or_else(Vec::new, |comic| comic.author.clone()),
        tags: original
            .as_ref()
            .map_or_else(Vec::new, |comic| comic.tags.clone()),
        chapter_infos: vec![ChapterInfo {
            chapter_id: record.id - 1,
            chapter_title: "第1话".into(),
            order: 1,
            is_downloaded: Some(true),
            chapter_download_dir: Some(record.path.clone()),
        }],
        comic_download_dir: Some(record.path.clone()),
        is_downloaded: Some(true),
        local_cover_data: crate::comic_scanner::load_local_cover_data(&record.path),
        ..Comic::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removed_records_survive_restart_preserve_files_and_can_be_reimported() {
        let root = std::env::temp_dir().join(format!("jm-catalog-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("漫画.cbz");
        std::fs::write(&source, b"original data").unwrap();
        let path = root.join("catalog.json");
        let mut catalog = LibraryCatalog::default();
        let (id, added) = catalog.register(&root, &source).unwrap();
        assert!(added);
        catalog.remove(&root, id);
        catalog.save(&path).unwrap();
        let mut restored = LibraryCatalog::load(&path).unwrap();
        assert!(restored.visible_imports(&root).is_empty());
        assert!(!restored
            .removed_ids(&root.join("other-library"))
            .contains(&id));
        assert_eq!(std::fs::read(&source).unwrap(), b"original data");
        assert_eq!(restored.register(&root, &source).unwrap(), (id, true));
        assert_eq!(restored.visible_imports(&root).len(), 1);
        assert_eq!(restored.register(&root, &source).unwrap(), (id, false));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn load_backs_up_corrupt_catalog_and_starts_empty() {
        let root = std::env::temp_dir().join(format!("jm-catalog-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join(".library.json");
        std::fs::write(&path, b"{ not json").unwrap();
        let catalog = LibraryCatalog::load(&path).unwrap();
        assert!(catalog.visible_imports(&root).is_empty());
        assert!(!path.exists(), "损坏文件应被改名而不是留在原位");
        let backups = std::fs::read_dir(&root)
            .unwrap()
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().contains(".corrupt-"))
            .count();
        assert_eq!(backups, 1);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn forget_missing_paths_keeps_records_for_files_that_still_exist() {
        let root = std::env::temp_dir().join(format!("jm-catalog-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let kept = root.join("kept.cbz");
        let deleted = root.join("deleted.cbz");
        std::fs::write(&kept, b"kept").unwrap();
        std::fs::write(&deleted, b"deleted").unwrap();
        let kept = std::fs::canonicalize(kept).unwrap();
        let deleted = std::fs::canonicalize(deleted).unwrap();
        let mut catalog = LibraryCatalog::default();
        let (kept_id, _) = catalog.register(&root, &kept).unwrap();
        let (deleted_id, _) = catalog.register(&root, &deleted).unwrap();
        std::fs::remove_file(&deleted).unwrap();

        catalog.forget_missing_paths(&[deleted]);
        // 文件仍存在的路径不能误删记录（崩溃可能发生在真正删除之前）
        catalog.forget_missing_paths(&[kept]);

        let ids: Vec<i64> = catalog
            .visible_imports(&root)
            .iter()
            .map(|record| record.id)
            .collect();
        assert_eq!(ids, vec![kept_id]);
        assert!(!ids.contains(&deleted_id));
        std::fs::remove_dir_all(root).unwrap();
    }
}
