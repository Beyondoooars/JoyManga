use std::path::PathBuf;

use anyhow::{ensure, Context};
use indexmap::IndexMap;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use specta::Type;

use crate::{
    atomic_fs,
    config::Config,
    download_manager::DownloadTaskState,
    types::{ChapterInfo, Comic, DownloadFormat},
};

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct DownloadTaskSnapshot {
    pub task_id: String,
    pub revision: u64,
    pub state: DownloadTaskState,
    pub comic: Box<Comic>,
    pub chapter_info: Box<ChapterInfo>,
    pub downloaded_img_count: u32,
    pub total_img_count: u32,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DownloadSettings {
    pub download_dir: PathBuf,
    pub dir_fmt: String,
    pub download_format: DownloadFormat,
}

impl From<&Config> for DownloadSettings {
    fn from(config: &Config) -> Self {
        Self {
            download_dir: config.download_dir.clone(),
            dir_fmt: config.dir_fmt.clone(),
            download_format: config.download_format,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredDownload {
    pub snapshot: DownloadTaskSnapshot,
    pub settings: DownloadSettings,
}

impl StoredDownload {
    pub fn resumed_state(&self, settings: &DownloadSettings) -> anyhow::Result<DownloadTaskState> {
        ensure!(
            self.settings.download_dir == settings.download_dir
                && self.settings.dir_fmt == settings.dir_fmt
                && (self.settings.download_format == settings.download_format
                    // 升级到新保存策略（PNG / 原始格式）后，旧格式的队列可以按新策略续传
                    || settings.download_format == DownloadFormat::Png
                    || settings.download_format == DownloadFormat::Original),
            "下载目录或命名规则已改变，请恢复原配置后重试，或按当前配置重新下载"
        );
        Ok(match self.snapshot.state {
            DownloadTaskState::Downloading => DownloadTaskState::Pending,
            state => state,
        })
    }
}

#[derive(Serialize, Deserialize)]
struct QueueFile {
    version: u32,
    tasks: Vec<StoredDownload>,
}

#[derive(Default)]
struct QueueState {
    revision: u64,
    entries: IndexMap<i64, StoredDownload>,
    dirty: bool,
}

pub struct DownloadQueue {
    path: PathBuf,
    state: Mutex<QueueState>,
}

fn should_restore(state: DownloadTaskState) -> bool {
    !matches!(
        state,
        DownloadTaskState::Completed | DownloadTaskState::Cancelled
    )
}

impl DownloadQueue {
    pub fn parse_backup_entries(
        value: Option<&serde_json::Value>,
        settings: &DownloadSettings,
    ) -> anyhow::Result<Vec<StoredDownload>> {
        let Some(value) = value.filter(|value| !value.is_null()) else {
            return Ok(Vec::new());
        };
        let file: QueueFile =
            serde_json::from_value(value.clone()).context("解析备份下载队列失败")?;
        ensure!(file.version == 1, "备份下载队列版本不受支持");
        let mut ids = std::collections::HashSet::new();
        let mut entries = Vec::new();
        for entry in file.tasks {
            if !should_restore(entry.snapshot.state) {
                continue;
            }
            ensure!(
                ids.insert(entry.snapshot.chapter_info.chapter_id),
                "备份包含重复的章节任务"
            );
            entry
                .resumed_state(settings)
                .context("备份队列与备份配置不匹配")?;
            entries.push(entry);
        }
        Ok(entries)
    }

    /// 先完整写入候选队列，再替换内存状态，写入失败时保留原队列。
    pub fn replace_entries(&self, entries: Vec<StoredDownload>) -> anyhow::Result<()> {
        let mut state = self.state.lock();
        let mut candidate = QueueState {
            revision: state.revision,
            ..QueueState::default()
        };
        for mut entry in entries {
            candidate.revision += 1;
            entry.snapshot.revision = candidate.revision;
            candidate
                .entries
                .insert(entry.snapshot.chapter_info.chapter_id, entry);
        }
        self.save(&candidate)?;
        *state = candidate;
        Ok(())
    }

    pub fn load(path: PathBuf) -> anyhow::Result<Self> {
        atomic_fs::recover_file_backup(&path)?;
        let mut state = QueueState::default();
        if path.exists() {
            let bytes = std::fs::read(&path).context("读取下载队列失败")?;
            let parsed = serde_json::from_slice::<QueueFile>(&bytes);
            match parsed {
                Ok(file) => {
                    ensure!(
                        file.version == 1,
                        "下载队列版本不受支持，请使用兼容版本打开"
                    );
                    for mut entry in file.tasks {
                        if !should_restore(entry.snapshot.state) {
                            continue;
                        }
                        state.revision += 1;
                        entry.snapshot.revision = state.revision;
                        state
                            .entries
                            .insert(entry.snapshot.chapter_info.chapter_id, entry);
                    }
                }
                Err(error) => {
                    let backup = path.with_file_name(format!(
                        "download-queue.corrupt-{}.json",
                        uuid::Uuid::new_v4()
                    ));
                    // 只读目录/文件被占用时重命名可能失败：降级为空队列继续启动，
                    // 不能因为一个损坏的队列文件让整个应用打不开。
                    match std::fs::rename(&path, &backup) {
                        Ok(()) => tracing::error!(
                            error = %error,
                            backup = %backup.display(),
                            "下载队列损坏，已保留备份；可重新添加任务续用临时图片"
                        ),
                        Err(rename_error) => tracing::error!(
                            error = %error,
                            rename_error = %rename_error,
                            "下载队列损坏且无法备份，本次以空队列继续"
                        ),
                    }
                }
            }
        }
        Ok(Self {
            path,
            state: Mutex::new(state),
        })
    }

    pub fn entries(&self) -> Vec<StoredDownload> {
        self.state.lock().entries.values().cloned().collect()
    }

    /// 首次入队必须先成功落盘，再允许下载开始。
    pub fn insert(&self, mut entry: StoredDownload) -> anyhow::Result<DownloadTaskSnapshot> {
        let mut state = self.state.lock();
        state.revision += 1;
        entry.snapshot.revision = state.revision;
        let id = entry.snapshot.chapter_info.chapter_id;
        let snapshot = entry.snapshot.clone();
        let previous = state.entries.insert(id, entry);
        if let Err(error) = self.save(&state) {
            if let Some(previous) = previous {
                state.entries.insert(id, previous);
            } else {
                state.entries.shift_remove(&id);
            }
            return Err(error);
        }
        state.dirty = false;
        Ok(snapshot)
    }

    pub fn update(&self, mut snapshot: DownloadTaskSnapshot) -> Option<DownloadTaskSnapshot> {
        let mut state = self.state.lock();
        let entry = state.entries.get(&snapshot.chapter_info.chapter_id)?;
        // 重试会创建新的 task_id，丢弃旧任务迟到的事件。
        if entry.snapshot.task_id != snapshot.task_id {
            return None;
        }
        let persist = entry.snapshot.state != snapshot.state
            || entry.snapshot.error_message != snapshot.error_message;
        state.revision += 1;
        snapshot.revision = state.revision;
        state
            .entries
            .get_mut(&snapshot.chapter_info.chapter_id)?
            .snapshot = snapshot.clone();
        state.dirty |= persist;
        if state.dirty {
            match self.save(&state) {
                Ok(()) => state.dirty = false,
                Err(error) => {
                    tracing::error!(error = %error, "保存下载队列失败，重启后的任务状态可能过期");
                }
            }
        }
        // 未完成和失败任务始终保留；限制本次运行的完成历史大小。
        let finished: Vec<_> = state
            .entries
            .iter()
            .filter(|(_, entry)| !should_restore(entry.snapshot.state))
            .map(|(id, _)| *id)
            .collect();
        for id in finished.iter().take(finished.len().saturating_sub(500)) {
            state.entries.shift_remove(id);
        }
        Some(snapshot)
    }

    fn save(&self, state: &QueueState) -> anyhow::Result<()> {
        let file = QueueFile {
            version: 1,
            tasks: state
                .entries
                .values()
                .filter(|entry| should_restore(entry.snapshot.state))
                .cloned()
                .collect(),
        };
        atomic_fs::write(&self.path, serde_json::to_vec_pretty(&file)?).context("保存下载队列失败")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: i64, state: DownloadTaskState) -> StoredDownload {
        StoredDownload {
            snapshot: DownloadTaskSnapshot {
                task_id: uuid::Uuid::new_v4().to_string(),
                revision: 0,
                state,
                comic: Box::default(),
                chapter_info: Box::new(ChapterInfo {
                    chapter_id: id,
                    chapter_title: "测试章节".into(),
                    order: 1,
                    is_downloaded: None,
                    chapter_download_dir: None,
                }),
                downloaded_img_count: 2,
                total_img_count: 5,
                error_message: None,
            },
            settings: DownloadSettings {
                download_dir: PathBuf::from("downloads"),
                dir_fmt: "{comic_id}/{chapter_id}".into(),
                download_format: DownloadFormat::Jpeg,
            },
        }
    }

    #[test]
    fn restart_keeps_unfinished_and_failed_but_drops_completed_and_cancelled() {
        let root = std::env::temp_dir().join(format!("jm-queue-{}", uuid::Uuid::new_v4()));
        let path = root.join("download-queue.json");
        let queue = DownloadQueue::load(path.clone()).unwrap();
        for (id, state) in [
            (1, DownloadTaskState::Downloading),
            (2, DownloadTaskState::Paused),
            (3, DownloadTaskState::Failed),
            (4, DownloadTaskState::Completed),
            (5, DownloadTaskState::Cancelled),
        ] {
            queue.insert(entry(id, state)).unwrap();
        }
        let restored = DownloadQueue::load(path).unwrap().entries();
        assert_eq!(restored.len(), 3);
        assert_eq!(restored[1].snapshot.state, DownloadTaskState::Paused);
        assert_eq!(restored[2].snapshot.state, DownloadTaskState::Failed);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn restart_recovers_queue_left_between_atomic_renames() {
        let root = std::env::temp_dir().join(format!("jm-queue-{}", uuid::Uuid::new_v4()));
        let path = root.join("download-queue.json");
        let queue = DownloadQueue::load(path.clone()).unwrap();
        let snapshot = queue.insert(entry(1, DownloadTaskState::Paused)).unwrap();
        let backup = root.join(format!(
            ".download-queue.json.backup-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::rename(&path, &backup).unwrap();
        let restored = DownloadQueue::load(path).unwrap().entries();
        assert_eq!(restored[0].snapshot.task_id, snapshot.task_id);
        assert_eq!(restored[0].snapshot.state, DownloadTaskState::Paused);
        assert!(!backup.exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn stale_task_cannot_overwrite_retry_and_failed_write_rolls_back() {
        let root = std::env::temp_dir().join(format!("jm-queue-{}", uuid::Uuid::new_v4()));
        let queue = DownloadQueue::load(root.join("queue.json")).unwrap();
        let old = queue.insert(entry(1, DownloadTaskState::Failed)).unwrap();
        let fresh = queue.insert(entry(1, DownloadTaskState::Pending)).unwrap();
        assert!(queue.update(old).is_none());
        assert_eq!(queue.entries()[0].snapshot.task_id, fresh.task_id);
        let blocked_path = root.join("file-as-parent");
        std::fs::write(&blocked_path, b"not a directory").unwrap();
        let blocked = DownloadQueue {
            path: blocked_path.join("queue.json"),
            state: Mutex::default(),
        };
        assert!(blocked
            .insert(entry(2, DownloadTaskState::Pending))
            .is_err());
        assert!(blocked.entries().is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn corrupt_queue_is_preserved_in_backup() {
        let root = std::env::temp_dir().join(format!("jm-queue-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("queue.json");
        std::fs::write(&path, b"broken json").unwrap();
        assert!(DownloadQueue::load(path).unwrap().entries().is_empty());
        let backup = std::fs::read_dir(&root)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert_eq!(std::fs::read(backup).unwrap(), b"broken json");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn state_changes_and_failure_details_survive_restart_and_cancel_clears_queue() {
        let root = std::env::temp_dir().join(format!("jm-queue-{}", uuid::Uuid::new_v4()));
        let path = root.join("queue.json");
        let queue = DownloadQueue::load(path.clone()).unwrap();
        let mut snapshot = queue.insert(entry(1, DownloadTaskState::Pending)).unwrap();
        snapshot.state = DownloadTaskState::Failed;
        snapshot.error_message = Some("第 3 张图片读取超时".into());
        queue.update(snapshot.clone()).unwrap();
        let loaded = DownloadQueue::load(path.clone()).unwrap().entries();
        assert_eq!(loaded[0].snapshot.state, DownloadTaskState::Failed);
        assert_eq!(loaded[0].snapshot.error_message, snapshot.error_message);
        snapshot.state = DownloadTaskState::Cancelled;
        queue.update(snapshot).unwrap();
        assert!(DownloadQueue::load(path).unwrap().entries().is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recovery_preserves_pause_and_rejects_changed_storage_settings() {
        let mut download = entry(1, DownloadTaskState::Downloading);
        assert_eq!(
            download.resumed_state(&download.settings).unwrap(),
            DownloadTaskState::Pending
        );
        download.snapshot.state = DownloadTaskState::Paused;
        assert_eq!(
            download.resumed_state(&download.settings).unwrap(),
            DownloadTaskState::Paused
        );
        let mut changed = download.settings.clone();
        changed.download_dir = PathBuf::from("different");
        assert!(download.resumed_state(&changed).is_err());
        changed = download.settings.clone();
        changed.download_format = DownloadFormat::Webp;
        assert!(download.resumed_state(&changed).is_err());
        changed = download.settings.clone();
        changed.dir_fmt = "{comic_id}/new/{chapter_id}".into();
        assert!(download.resumed_state(&changed).is_err());
    }

    #[test]
    fn failed_state_write_is_retried_on_next_update() {
        let root = std::env::temp_dir().join(format!("jm-queue-{}", uuid::Uuid::new_v4()));
        let good_path = root.join("queue.json");
        let mut queue = DownloadQueue::load(good_path.clone()).unwrap();
        let mut snapshot = queue.insert(entry(1, DownloadTaskState::Pending)).unwrap();
        let blocked = root.join("blocked");
        std::fs::write(&blocked, b"not a directory").unwrap();
        queue.path = blocked.join("queue.json");
        snapshot.state = DownloadTaskState::Paused;
        queue.update(snapshot.clone()).unwrap();
        assert!(queue.state.lock().dirty);
        queue.path = good_path.clone();
        queue.update(snapshot).unwrap();
        assert!(!queue.state.lock().dirty);
        assert_eq!(
            DownloadQueue::load(good_path).unwrap().entries()[0]
                .snapshot
                .state,
            DownloadTaskState::Paused
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn replacement_updates_memory_and_disk_and_rolls_back_on_write_failure() {
        let root = std::env::temp_dir().join(format!("jm-queue-{}", uuid::Uuid::new_v4()));
        let path = root.join("queue.json");
        let mut queue = DownloadQueue::load(path.clone()).unwrap();
        queue.insert(entry(1, DownloadTaskState::Failed)).unwrap();
        queue
            .replace_entries(vec![entry(2, DownloadTaskState::Paused)])
            .unwrap();
        assert_eq!(queue.entries()[0].snapshot.chapter_info.chapter_id, 2);
        assert_eq!(
            DownloadQueue::load(path.clone()).unwrap().entries()[0]
                .snapshot
                .chapter_info
                .chapter_id,
            2
        );
        let blocked = root.join("blocked");
        std::fs::write(&blocked, b"not a directory").unwrap();
        queue.path = blocked.join("queue.json");
        assert!(queue.replace_entries(Vec::new()).is_err());
        assert_eq!(queue.entries()[0].snapshot.chapter_info.chapter_id, 2);
        queue.path = path.clone();
        queue.replace_entries(Vec::new()).unwrap();
        assert!(queue.entries().is_empty());
        assert!(DownloadQueue::load(path).unwrap().entries().is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn backup_queue_validation_rejects_duplicates_and_changed_settings() {
        let download = entry(1, DownloadTaskState::Paused);
        let settings = download.settings.clone();
        let duplicate = serde_json::to_value(QueueFile {
            version: 1,
            tasks: vec![download.clone(), download.clone()],
        })
        .unwrap();
        assert!(DownloadQueue::parse_backup_entries(Some(&duplicate), &settings).is_err());
        let valid = serde_json::to_value(QueueFile {
            version: 1,
            tasks: vec![download],
        })
        .unwrap();
        assert_eq!(
            DownloadQueue::parse_backup_entries(Some(&valid), &settings)
                .unwrap()
                .len(),
            1
        );
        let mut changed = settings;
        changed.download_dir = PathBuf::from("other");
        assert!(DownloadQueue::parse_backup_entries(Some(&valid), &changed).is_err());
    }
}

#[cfg(test)]
mod png_upgrade_tests {
    use super::*;
    #[test]
    fn old_jpeg_queue_can_resume_with_png_without_accepting_a_changed_root() {
        let current = DownloadSettings::from(&Config::default(std::path::Path::new("library")));
        let mut old = current.clone();
        old.download_format = DownloadFormat::Jpeg;
        let stored = StoredDownload {
            settings: old,
            snapshot: DownloadTaskSnapshot {
                task_id: "old".into(),
                revision: 0,
                state: DownloadTaskState::Paused,
                comic: Box::new(Comic::default()),
                chapter_info: Box::new(ChapterInfo {
                    chapter_id: 1,
                    chapter_title: "第1话".into(),
                    order: 1,
                    is_downloaded: None,
                    chapter_download_dir: None,
                }),
                downloaded_img_count: 0,
                total_img_count: 1,
                error_message: None,
            },
        };
        assert_eq!(
            stored.resumed_state(&current).unwrap(),
            DownloadTaskState::Paused
        );
        let mut changed = current;
        changed.download_dir = "different".into();
        assert!(stored.resumed_state(&changed).is_err());
    }
}
