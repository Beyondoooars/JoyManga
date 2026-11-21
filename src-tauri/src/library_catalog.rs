use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{ensure, Context};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::{
    atomic_fs,
    comic_scanner::{read_comic_id_from_cbz, walk_comic_metadata_files},
    config::ConfigStorage,
    extensions::{AppHandleExt, PathIsCbz},
    types::Comic,
};

pub(crate) static CATALOG_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

type ComicIndex = (PathBuf, HashMap<i64, Comic>);
type PathMapIndex = (PathBuf, HashMap<i64, PathBuf>);

/// 最近一次书库扫描的摘要，供 `diagnose_library` 复用，避免刷新后立刻再扫一遍整库。
pub struct LibraryScanSnapshot {
    pub root: PathBuf,
    pub comic_count: usize,
    pub skipped: Vec<String>,
}

#[derive(Default)]
pub struct DownloadedLibraryCache {
    map: RwLock<Option<PathMapIndex>>,
    comics: RwLock<Option<ComicIndex>>,
    scan: RwLock<Option<LibraryScanSnapshot>>,
}

impl DownloadedLibraryCache {
    pub fn new() -> Self {
        Self {
            map: RwLock::new(None),
            comics: RwLock::new(None),
            scan: RwLock::new(None),
        }
    }

    pub fn set_scan(&self, root: &Path, comic_count: usize, skipped: Vec<String>) {
        *self.scan.write() = Some(LibraryScanSnapshot {
            root: root.to_path_buf(),
            comic_count,
            skipped,
        });
    }

    /// 只在书库根目录没变时复用；换过目录后旧摘要说明不了新目录的情况。
    pub fn scan_summary(&self, root: &Path) -> Option<(usize, Vec<String>)> {
        let guard = self.scan.read();
        let snapshot = guard.as_ref()?;
        (snapshot.root == root).then(|| (snapshot.comic_count, snapshot.skipped.clone()))
    }

    pub fn get(&self, root: &Path) -> Option<HashMap<i64, PathBuf>> {
        let guard = self.map.read();
        let (stored_root, map) = guard.as_ref()?;
        (stored_root == root).then(|| map.clone())
    }

    pub fn set(&self, root: &Path, map: HashMap<i64, PathBuf>) {
        // 路径映射变了，基于旧映射建立的漫画快照同样失效；否则归档整合会拿到
        // 章节没有下载路径的旧快照，整本合并直接失败。
        *self.comics.write() = None;
        *self.scan.write() = None;
        *self.map.write() = Some((root.to_path_buf(), map));
    }

    #[cfg(test)]
    pub fn insert(&self, root: &Path, id: i64, path: PathBuf) {
        *self.comics.write() = None;
        let mut guard = self.map.write();
        if let Some((stored_root, map)) = guard.as_mut() {
            if stored_root == root {
                map.insert(id, path);
            }
        }
    }

    pub fn remove(&self, id: i64) {
        *self.comics.write() = None;
        let mut guard = self.map.write();
        if let Some((_, map)) = guard.as_mut() {
            map.remove(&id);
        }
    }

    pub fn invalidate(&self) {
        *self.map.write() = None;
        *self.comics.write() = None;
        // 扫描摘要同样过期：下载完成/整合后书库内容已经变了。
        *self.scan.write() = None;
    }

    pub fn set_comics(&self, root: &Path, comics: Vec<Comic>) {
        *self.comics.write() = Some((
            root.to_path_buf(),
            comics
                .into_iter()
                .map(|mut comic| {
                    comic.local_cover_data = None;
                    (comic.id, comic)
                })
                .collect(),
        ));
    }

    pub fn comic(&self, root: &Path, id: i64) -> Option<Comic> {
        let index = self.comics.read();
        let (stored_root, comics) = index.as_ref()?;
        if stored_root != root {
            return None;
        }
        comics.get(&id).cloned()
    }
}

pub fn create_id_to_dir_map(app: &AppHandle) -> anyhow::Result<HashMap<i64, PathBuf>> {
    let download_dir = app.get_config().read().download_dir.clone();
    let catalog = {
        let _guard = CATALOG_LOCK.lock();
        LibraryCatalog::load(&catalog_path(app))?
    };
    let removed = catalog.removed_ids(&download_dir);
    if let Some(cache) = app.try_state::<DownloadedLibraryCache>() {
        if let Some(mut cached_map) = cache.get(&download_dir) {
            cached_map.retain(|id, _| !removed.contains(id));
            return Ok(cached_map);
        }
    }

    let mut id_to_dir_map: HashMap<i64, PathBuf> = HashMap::new();
    if !download_dir.exists() {
        if let Some(cache) = app.try_state::<DownloadedLibraryCache>() {
            cache.set(&download_dir, id_to_dir_map.clone());
        }
        return Ok(id_to_dir_map);
    }

    // 1. 扫描下载根目录下的平铺 .cbz 文件
    if let Ok(entries) = std::fs::read_dir(&download_dir) {
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.is_file() && path.is_cbz() {
                if let Ok(id) = read_comic_id_from_cbz(&path) {
                    id_to_dir_map.entry(id).or_insert(path);
                }
            }
        }
    }

    // 2. 扫描历史子目录中的 元数据.json
    for entry in walk_comic_metadata_files(&download_dir) {
        let path = entry.path();
        let id = match read_comic_id(path) {
            Ok(id) => id,
            Err(err) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %err,
                    "跳过损坏的漫画元数据"
                );
                continue;
            }
        };

        let parent = path
            .parent()
            .context(format!("`{}`没有父目录", path.display()))?;

        id_to_dir_map.entry(id).or_insert(parent.to_path_buf());
    }

    for record in catalog.visible_imports(&download_dir) {
        if record.path.is_file() {
            id_to_dir_map.insert(record.id, record.path);
        }
    }
    id_to_dir_map.retain(|id, _| !removed.contains(id));
    if let Some(cache) = app.try_state::<DownloadedLibraryCache>() {
        cache.set(&download_dir, id_to_dir_map.clone());
    }

    Ok(id_to_dir_map)
}

fn read_comic_id(path: &Path) -> anyhow::Result<i64> {
    let metadata_str =
        std::fs::read_to_string(path).context(format!("读取`{}`失败", path.display()))?;
    let comic_json: serde_json::Value = serde_json::from_str(&metadata_str).context(format!(
        "将`{}`反序列化为serde_json::Value失败",
        path.display()
    ))?;
    comic_json
        .get("id")
        .and_then(serde_json::Value::as_i64)
        .context(format!("`{}`没有有效的`id`字段", path.display()))
}

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

/// 重新下载章节成功落盘后，若该漫画曾被「移除出书库」，自动清除移除黑名单使其恢复可见与可合并。
pub(crate) fn restore_comic_in_catalog(app: &AppHandle, root: &Path, comic_id: i64) {
    let _guard = CATALOG_LOCK.lock();
    let path = catalog_path(app);
    let mut catalog = match LibraryCatalog::load(&path) {
        Ok(catalog) => catalog,
        Err(error) => {
            tracing::warn!(comic_id, error = %error, "加载书库记录以恢复已移除漫画失败");
            return;
        }
    };
    if catalog.restore(root, comic_id) {
        if let Err(error) = catalog.save(&path) {
            tracing::warn!(comic_id, error = %error, "保存解除移除标记的书库记录失败");
        }
    }
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
    let local_cover_data = original
        .as_ref()
        .and_then(|comic| comic.local_cover_data.clone())
        .or_else(|| crate::comic_scanner::load_local_cover_data(&record.path));
    let title = record
        .path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let inspected = crate::comic_scanner::inspect_cbz_chapters(&record.path).unwrap_or_default();
    let chapter_infos = if inspected.is_empty() {
        vec![ChapterInfo {
            chapter_id: record.id - 1,
            chapter_title: "第1话".into(),
            order: 1,
            is_downloaded: Some(true),
            chapter_download_dir: Some(record.path.clone()),
        }]
    } else {
        inspected
            .into_iter()
            .enumerate()
            .map(|(idx, (order, chapter_title))| ChapterInfo {
                chapter_id: record
                    .id
                    .saturating_sub(i64::try_from(idx + 1).unwrap_or(i64::MAX)),
                chapter_title,
                order,
                is_downloaded: Some(true),
                chapter_download_dir: Some(record.path.clone()),
            })
            .collect()
    };
    Ok(Comic {
        id: record.id,
        name: original.as_ref().map_or(title, |comic| comic.name.clone()),
        author: original
            .as_ref()
            .map_or_else(Vec::new, |comic| comic.author.clone()),
        tags: original
            .as_ref()
            .map_or_else(Vec::new, |comic| comic.tags.clone()),
        chapter_infos,
        comic_download_dir: Some(record.path.clone()),
        is_downloaded: Some(true),
        local_cover_data,
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

    #[test]
    fn downloaded_library_cache_operations() {
        let cache = DownloadedLibraryCache::new();
        let root = Path::new("downloads");
        assert!(cache.get(root).is_none());

        let mut map = HashMap::new();
        map.insert(123, PathBuf::from("dir1"));
        cache.set(root, map);
        assert_eq!(
            cache.get(root).unwrap().get(&123),
            Some(&PathBuf::from("dir1"))
        );
        assert!(cache.get(Path::new("other")).is_none());

        cache.insert(root, 456, PathBuf::from("dir2"));
        assert_eq!(
            cache.get(root).unwrap().get(&456),
            Some(&PathBuf::from("dir2"))
        );

        cache.remove(123);
        assert_eq!(cache.get(root).unwrap().get(&123), None);

        cache.invalidate();
        assert!(cache.get(root).is_none());
    }

    #[test]
    fn chapter_index_is_bound_to_root_and_invalidated_when_library_changes() {
        let cache = DownloadedLibraryCache::new();
        let root = Path::new("downloads");
        let comic = Comic {
            id: 1,
            local_cover_data: Some("large-cover".into()),
            ..Comic::default()
        };
        cache.set_comics(root, vec![comic.clone()]);
        assert!(cache.comic(root, 1).unwrap().local_cover_data.is_none());
        assert!(cache.comic(Path::new("other"), 1).is_none());
        cache.insert(root, 1, root.join("chapter"));
        assert!(cache.comic(root, 1).is_none());
        cache.set_comics(root, vec![comic]);
        cache.invalidate();
        assert!(cache.comic(root, 1).is_none());
    }

    #[test]
    fn replacing_the_path_map_invalidates_cached_comic_snapshots() {
        let cache = DownloadedLibraryCache::new();
        let root = Path::new("downloads");
        let comic = Comic {
            id: 1,
            ..Comic::default()
        };
        cache.set_comics(root, vec![comic]);
        assert!(cache.comic(root, 1).is_some());

        let mut map = HashMap::new();
        map.insert(1, root.join("chapter.cbz"));
        cache.set(root, map);
        // 重新扫描书库后，旧快照必须失效，否则归档整合会拿到没有下载路径的漫画。
        assert!(cache.comic(root, 1).is_none());
    }

    #[test]
    fn restore_clears_removed_id_for_matching_root() {
        let root = std::env::temp_dir().join(format!("jm-restore-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let other = root.join("other");
        std::fs::create_dir_all(&other).unwrap();
        let mut catalog = LibraryCatalog::default();
        catalog.remove(&root, 42);
        catalog.remove(&other, 42);

        assert!(catalog.removed_ids(&root).contains(&42));
        assert!(catalog.restore(&root, 42));
        assert!(!catalog.restore(&root, 42));
        assert!(!catalog.removed_ids(&root).contains(&42));
        assert!(catalog.removed_ids(&other).contains(&42));
        std::fs::remove_dir_all(root).unwrap();
    }
}
