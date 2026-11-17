use std::path::{Path, PathBuf};

use anyhow::{ensure, Context};
use indexmap::IndexMap;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use specta::Type;

#[derive(Default)]
pub(crate) struct StartupNotices {
    messages: Mutex<Vec<String>>,
}

impl StartupNotices {
    pub fn extend(&self, messages: impl IntoIterator<Item = String>) {
        self.messages.lock().extend(messages);
    }

    pub fn messages(&self) -> Vec<String> {
        self.messages.lock().clone()
    }
}

pub fn background_failure(
    app: Option<&tauri::AppHandle>,
    domain: &'static str,
    title: &'static str,
    error: &anyhow::Error,
) {
    state_recovery::background_failure(app, domain, title, error);
}

use crate::{
    atomic_fs,
    config::Config,
    download_manager::DownloadTaskState,
    state_recovery,
    types::{ChapterInfo, Comic, FailureCount, TaskFailure, TaskStage},
};

pub const QUEUE_FORMAT_VERSION: u32 = 1;
const MAX_FINISHED_HISTORY: usize = 500;

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
    /// 当前步骤；旧队列没有该字段时按 `Queued` 处理。
    #[serde(default)]
    pub stage: TaskStage,
    /// 首次失败详情；成功/重试后的新任务会重新置空。
    #[serde(default)]
    pub failure: Option<TaskFailure>,
    /// 已发生的尝试次数（图片重试、恢复尝试）。
    #[serde(default)]
    pub attempts: u32,
    /// 按类型聚合的图片失败数，仅在任务失败或阶段切换时落盘。
    #[serde(default)]
    pub image_failures: Vec<FailureCount>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DownloadSettings {
    pub download_dir: PathBuf,
}

impl From<&Config> for DownloadSettings {
    fn from(config: &Config) -> Self {
        Self {
            download_dir: config.download_dir.clone(),
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
            self.settings.download_dir == settings.download_dir,
            "下载目录已改变，请恢复原配置后重试，或按当前配置重新下载"
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
    app: Option<tauri::AppHandle>,
    state: Mutex<QueueState>,
    #[cfg(test)]
    save_count: std::sync::atomic::AtomicUsize,
}

fn should_restore(state: DownloadTaskState) -> bool {
    !matches!(
        state,
        DownloadTaskState::Completed | DownloadTaskState::Cancelled
    )
}

fn compact_finished(mut entry: StoredDownload) -> StoredDownload {
    if entry.snapshot.state == DownloadTaskState::Completed
        && entry.snapshot.stage == TaskStage::Done
    {
        entry.snapshot.chapter_info.chapter_download_dir = None;
        // 完成历史只需要漫画身份、显示名称、所在位置和当前章节。
        // 保留目录供完成列表“打开所在位置”使用，其他网络元数据不必反复写盘。
        let comic = std::mem::take(&mut *entry.snapshot.comic);
        entry.snapshot.comic = Box::new(crate::types::Comic {
            id: comic.id,
            name: comic.name,
            chapter_infos: vec![(*entry.snapshot.chapter_info).clone()],
            is_downloaded: Some(true),
            comic_download_dir: comic.comic_download_dir,
            ..crate::types::Comic::default()
        });
    }
    entry
}

fn should_persist(snapshot: &DownloadTaskSnapshot) -> bool {
    should_restore(snapshot.state)
        || (snapshot.state == DownloadTaskState::Completed && snapshot.stage == TaskStage::Done)
}

/// 只有已经开始的任务能凭 Done 认回已提交 CBZ；取消/失败不被改写成完成。
fn normalize_restored_entry(mut entry: StoredDownload) -> StoredDownload {
    if entry.snapshot.stage == TaskStage::Done
        && matches!(
            entry.snapshot.state,
            DownloadTaskState::Pending | DownloadTaskState::Downloading | DownloadTaskState::Paused
        )
    {
        entry.snapshot.state = DownloadTaskState::Completed;
        entry.snapshot.error_message = None;
        entry.snapshot.failure = None;
    }
    entry
}

pub(crate) fn queue_path(config_path: &Path) -> PathBuf {
    config_path.with_file_name("download-queue.json")
}

pub(crate) fn restore_queue_file(
    config_path: &Path,
    value: Option<&serde_json::Value>,
) -> anyhow::Result<PathBuf> {
    let path = state_recovery::existing_path(&queue_path(config_path), "download-queue");
    state_recovery::restore_file(&path, "download-queue", value)?;
    Ok(path)
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
        ensure!(
            file.version == QUEUE_FORMAT_VERSION,
            "备份下载队列版本不受支持"
        );
        let mut ids = std::collections::HashSet::new();
        let mut entries = Vec::new();
        for entry in file.tasks {
            let entry = normalize_restored_entry(entry);
            if !should_persist(&entry.snapshot) {
                continue;
            }
            ensure!(
                ids.insert(entry.snapshot.chapter_info.chapter_id),
                "备份包含重复的章节任务"
            );
            if entry.snapshot.stage != TaskStage::Done {
                entry
                    .resumed_state(settings)
                    .context("备份队列与备份配置不匹配")?;
            }
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

    #[cfg(test)]
    pub fn load(path: PathBuf) -> anyhow::Result<Self> {
        Self::load_for_app(path, None).map(|(queue, _)| queue)
    }

    pub fn load_for_app(
        path: PathBuf,
        app: Option<tauri::AppHandle>,
    ) -> anyhow::Result<(Self, Vec<String>)> {
        let mut notices = Vec::new();
        let mut path = state_recovery::active_path(&path, "download-queue");
        if let Some(notice) = state_recovery::recover_file_backup(&path)? {
            notices.push(notice);
        }
        let mut state = QueueState::default();
        if path.exists() {
            match std::fs::read(&path) {
                Ok(bytes) => match serde_json::from_slice::<QueueFile>(&bytes) {
                    Ok(file) if file.version == QUEUE_FORMAT_VERSION => {
                        for entry in file.tasks {
                            let mut entry = normalize_restored_entry(entry);
                            if !should_persist(&entry.snapshot) {
                                continue;
                            }
                            let id = entry.snapshot.chapter_info.chapter_id;
                            if state.entries.contains_key(&id) {
                                let (active, notice) = state_recovery::preserve_or_use_fallback(
                                    &path,
                                    "download-queue",
                                    "下载队列含重复章节，本次未载入",
                                );
                                path = active;
                                notices.push(notice);
                                state = QueueState::default();
                                break;
                            }
                            state.revision += 1;
                            entry.snapshot.revision = state.revision;
                            state.entries.insert(id, entry);
                        }
                    }
                    Ok(file) => {
                        let (active, notice) = state_recovery::preserve_or_use_fallback(
                            &path,
                            "download-queue",
                            &format!("下载队列版本 {} 不受支持，本次未载入", file.version),
                        );
                        path = active;
                        notices.push(notice);
                    }
                    Err(error) => {
                        let (active, notice) = state_recovery::preserve_or_use_fallback(
                            &path,
                            "download-queue",
                            &format!("下载队列损坏，本次未载入：{error}"),
                        );
                        path = active;
                        notices.push(notice);
                    }
                },
                Err(error) => {
                    // 无法读取时不假定目录可写；独立运行文件避免覆盖可能仍存在的原件。
                    path = path.with_file_name(".download-queue.runtime.json");
                    let message = format!("下载队列读取失败，本次使用独立运行文件：{error}");
                    tracing::error!(error = %error, "读取下载队列失败");
                    notices.push(message);
                }
            }
        }
        Ok((
            Self {
                path,
                app,
                state: Mutex::new(state),
                #[cfg(test)]
                save_count: std::sync::atomic::AtomicUsize::new(0),
            },
            notices,
        ))
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub fn entries(&self) -> Vec<StoredDownload> {
        self.state.lock().entries.values().cloned().collect()
    }

    /// 备份包含完成历史，使恢复后仍可按归档意图补跑。
    pub fn backup_snapshot(&self) -> serde_json::Value {
        let tasks: Vec<_> = self
            .state
            .lock()
            .entries
            .values()
            .filter(|entry| should_persist(&entry.snapshot))
            .cloned()
            .map(compact_finished)
            .collect();
        serde_json::json!({ "version": QUEUE_FORMAT_VERSION, "tasks": tasks })
    }

    /// 同一批次只落盘一次。失败时整批恢复，不留下半批任务。
    pub fn insert_many(
        &self,
        entries: Vec<StoredDownload>,
        replace_existing: bool,
    ) -> anyhow::Result<Vec<DownloadTaskSnapshot>> {
        if entries.is_empty() {
            return Ok(Vec::new());
        }
        let mut state = self.state.lock();
        let mut candidate = QueueState {
            revision: state.revision,
            entries: state.entries.clone(),
            dirty: state.dirty,
        };
        let mut snapshots = Vec::with_capacity(entries.len());
        for mut entry in entries {
            let id = entry.snapshot.chapter_info.chapter_id;
            if !replace_existing && candidate.entries.contains_key(&id) {
                anyhow::bail!("章节ID为`{id}`的下载任务已存在");
            }
            candidate.revision += 1;
            entry.snapshot.revision = candidate.revision;
            candidate.dirty = true;
            snapshots.push(entry.snapshot.clone());
            candidate.entries.insert(id, entry);
        }
        self.trim_finished(&mut candidate);
        self.save(&candidate)?;
        *state = candidate;
        Ok(snapshots)
    }

    /// 撤销同一批刚创建、尚未启动的任务；写盘失败时保留内存并报告错误。
    pub fn remove_many(&self, entries: &[StoredDownload]) -> anyhow::Result<()> {
        if entries.is_empty() {
            return Ok(());
        }
        let mut state = self.state.lock();
        let mut candidate = QueueState {
            revision: state.revision,
            entries: state.entries.clone(),
            dirty: state.dirty,
        };
        for entry in entries {
            let id = entry.snapshot.chapter_info.chapter_id;
            if candidate
                .entries
                .get(&id)
                .is_some_and(|current| current.snapshot.task_id == entry.snapshot.task_id)
            {
                candidate.entries.shift_remove(&id);
            }
        }
        self.save(&candidate)?;
        *state = candidate;
        Ok(())
    }

    #[cfg(test)]
    pub fn insert(&self, entry: StoredDownload) -> anyhow::Result<DownloadTaskSnapshot> {
        Ok(self.insert_many(vec![entry], true)?.remove(0))
    }

    fn update_locked(
        &self,
        state: &mut QueueState,
        mut snapshot: DownloadTaskSnapshot,
    ) -> Option<DownloadTaskSnapshot> {
        let entry = state.entries.get(&snapshot.chapter_info.chapter_id)?;
        // 重试会创建新的 task_id，丢弃旧任务迟到的事件。
        if entry.snapshot.task_id != snapshot.task_id {
            return None;
        }
        // 迟到的暂停、打包或重试事件不能把已完成/已取消的任务拉回运行态。
        if matches!(
            entry.snapshot.state,
            DownloadTaskState::Completed | DownloadTaskState::Cancelled
        ) && entry.snapshot.state != snapshot.state
        {
            return None;
        }
        let persist = entry.snapshot.state != snapshot.state
            || entry.snapshot.error_message != snapshot.error_message
            || entry.snapshot.stage != snapshot.stage
            || entry.snapshot.failure != snapshot.failure
            || entry.snapshot.attempts != snapshot.attempts
            || entry.snapshot.image_failures != snapshot.image_failures;
        state.revision += 1;
        snapshot.revision = state.revision;
        state
            .entries
            .get_mut(&snapshot.chapter_info.chapter_id)?
            .snapshot = snapshot.clone();
        state.dirty |= persist || !should_persist(&snapshot);
        Some(snapshot)
    }

    fn save_dirty(&self, mut state: parking_lot::MutexGuard<'_, QueueState>) {
        if !state.dirty {
            return;
        }
        let result = self.save(&state);
        let app = self.app.clone();
        match result {
            Ok(()) => state.dirty = false,
            Err(error) => {
                // 先提交可恢复的内存终态，再在锁外发布通知；监听器可安全查询队列。
                drop(state);
                background_failure(
                    app.as_ref(),
                    "download-queue",
                    "下载状态未保存，重启后可能恢复为较早状态",
                    &error,
                );
            }
        }
    }

    fn trim_finished(&self, state: &mut QueueState) {
        // 未完成和失败任务始终保留；限制本次运行的完成历史大小。
        let finished: Vec<_> = state
            .entries
            .iter()
            .filter(|(_, entry)| !should_restore(entry.snapshot.state))
            .map(|(id, _)| *id)
            .collect();
        for id in finished
            .iter()
            .take(finished.len().saturating_sub(MAX_FINISHED_HISTORY))
        {
            state.dirty |= state.entries.shift_remove(id).is_some();
        }
    }

    pub fn update_many(&self, snapshots: Vec<DownloadTaskSnapshot>) -> Vec<DownloadTaskSnapshot> {
        if snapshots.is_empty() {
            return Vec::new();
        }
        let mut state = self.state.lock();
        let updated: Vec<_> = snapshots
            .into_iter()
            .filter_map(|snapshot| self.update_locked(&mut state, snapshot))
            .collect();
        self.trim_finished(&mut state);
        self.save_dirty(state);
        updated
    }

    pub fn update(&self, snapshot: DownloadTaskSnapshot) -> Option<DownloadTaskSnapshot> {
        let mut state = self.state.lock();
        let updated = self.update_locked(&mut state, snapshot);
        self.trim_finished(&mut state);
        self.save_dirty(state);
        updated
    }

    fn save(&self, state: &QueueState) -> anyhow::Result<()> {
        #[cfg(test)]
        self.save_count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let file = QueueFile {
            version: QUEUE_FORMAT_VERSION,
            tasks: state
                .entries
                .values()
                .filter(|entry| should_persist(&entry.snapshot))
                .cloned()
                .map(compact_finished)
                .collect(),
        };
        atomic_fs::write(&self.path, serde_json::to_vec_pretty(&file)?)
            .context("保存下载队列失败")?;
        Ok(())
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
                stage: TaskStage::Queued,
                failure: None,
                attempts: 0,
                image_failures: Vec::new(),
            },
            settings: DownloadSettings {
                download_dir: PathBuf::from("downloads"),
            },
        }
    }

    #[test]
    fn batch_updates_persist_the_queue_only_once() {
        let root = std::env::temp_dir().join(format!("jm-queue-{}", uuid::Uuid::new_v4()));
        let path = root.join("queue.json");
        let queue = DownloadQueue::load(path.clone()).unwrap();
        let mut first = queue
            .insert(entry(1, DownloadTaskState::Downloading))
            .unwrap();
        let mut second = queue.insert(entry(2, DownloadTaskState::Pending)).unwrap();
        let before = queue.save_count.load(std::sync::atomic::Ordering::Relaxed);
        first.state = DownloadTaskState::Paused;
        second.state = DownloadTaskState::Paused;
        assert_eq!(queue.update_many(vec![first, second]).len(), 2);
        assert_eq!(
            queue.save_count.load(std::sync::atomic::Ordering::Relaxed),
            before + 1
        );
        assert_eq!(queue.update_many(Vec::new()).len(), 0);
        let loaded = DownloadQueue::load(path).unwrap().entries();
        assert!(loaded
            .iter()
            .all(|entry| entry.snapshot.state == DownloadTaskState::Paused));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn restart_keeps_unfinished_failed_and_completed_but_drops_cancelled() {
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
            let mut entry = entry(id, state);
            if state == DownloadTaskState::Completed {
                entry.snapshot.stage = TaskStage::Done;
            }
            queue.insert(entry).unwrap();
        }
        let restored = DownloadQueue::load(path).unwrap().entries();
        assert_eq!(restored.len(), 4);
        assert_eq!(restored[1].snapshot.state, DownloadTaskState::Paused);
        assert_eq!(restored[2].snapshot.state, DownloadTaskState::Failed);
        assert_eq!(restored[3].snapshot.state, DownloadTaskState::Completed);
        assert_eq!(restored[3].snapshot.stage, TaskStage::Done);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn backup_snapshot_contains_restorable_tasks_and_done_history() {
        let root = std::env::temp_dir().join(format!("jm-queue-{}", uuid::Uuid::new_v4()));
        let queue = DownloadQueue::load(root.join("queue.json")).unwrap();
        queue.insert(entry(1, DownloadTaskState::Pending)).unwrap();
        let mut completed = entry(2, DownloadTaskState::Completed);
        completed.snapshot.stage = TaskStage::Done;
        queue.insert(completed).unwrap();
        queue
            .insert(entry(3, DownloadTaskState::Cancelled))
            .unwrap();

        let backup: QueueFile = serde_json::from_value(queue.backup_snapshot()).unwrap();
        assert_eq!(backup.version, 1);
        assert_eq!(backup.tasks.len(), 2);
        assert_eq!(backup.tasks[0].snapshot.chapter_info.chapter_id, 1);
        assert_eq!(backup.tasks[1].snapshot.chapter_info.chapter_id, 2);
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
            app: None,
            state: Mutex::default(),
            #[cfg(test)]
            save_count: std::sync::atomic::AtomicUsize::new(0),
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
    fn old_queue_without_observability_fields_loads_with_defaults() {
        let root = std::env::temp_dir().join(format!("jm-queue-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("queue.json");
        let mut value = serde_json::to_value(QueueFile {
            version: 1,
            tasks: vec![entry(1, DownloadTaskState::Paused)],
        })
        .unwrap();
        for task in value["tasks"].as_array_mut().unwrap() {
            let snapshot = task["snapshot"].as_object_mut().unwrap();
            snapshot.remove("stage");
            snapshot.remove("failure");
            snapshot.remove("attempts");
            snapshot.remove("imageFailures");
        }
        std::fs::write(&path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();

        let loaded = DownloadQueue::load(path).unwrap().entries();
        let snapshot = &loaded[0].snapshot;
        assert_eq!(snapshot.state, DownloadTaskState::Paused);
        assert_eq!(snapshot.stage, TaskStage::Queued);
        assert!(snapshot.failure.is_none());
        assert_eq!(snapshot.attempts, 0);
        assert!(snapshot.image_failures.is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failure_details_survive_restart() {
        let root = std::env::temp_dir().join(format!("jm-queue-{}", uuid::Uuid::new_v4()));
        let path = root.join("queue.json");
        let queue = DownloadQueue::load(path.clone()).unwrap();
        let mut snapshot = queue.insert(entry(1, DownloadTaskState::Pending)).unwrap();
        snapshot.state = DownloadTaskState::Failed;
        snapshot.stage = TaskStage::Downloading;
        snapshot.attempts = 2;
        snapshot.error_message = Some("第 3 张图片读取超时".into());
        snapshot.failure = Some(TaskFailure {
            kind: crate::types::FailureKind::Network,
            message: "第 3 张图片读取超时".into(),
            retryable: true,
            stage: TaskStage::Downloading,
            attempts: 2,
            line: Some("cdn.example".into()),
            at: 1_700_000_000_000,
        });
        snapshot.image_failures = vec![FailureCount {
            kind: crate::types::FailureKind::Network,
            count: 2,
        }];
        queue.update(snapshot).unwrap();

        let loaded = DownloadQueue::load(path).unwrap().entries();
        let restored = &loaded[0].snapshot;
        assert_eq!(restored.stage, TaskStage::Downloading);
        assert_eq!(restored.attempts, 2);
        assert_eq!(
            restored
                .failure
                .as_ref()
                .map(|failure| failure.line.clone()),
            Some(Some("cdn.example".into()))
        );
        assert_eq!(restored.image_failures.len(), 1);
        assert_eq!(restored.image_failures[0].count, 2);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn finished_history_limit_is_saved_and_runtime_fallback_stays_active() {
        let root = std::env::temp_dir().join(format!("jm-queue-{}", uuid::Uuid::new_v4()));
        let path = root.join("download-queue.json");
        let queue = DownloadQueue::load(path.clone()).unwrap();
        let mut first = queue.insert(entry(1, DownloadTaskState::Pending)).unwrap();
        for id in 2..=(MAX_FINISHED_HISTORY as i64 + 2) {
            let mut entry = entry(id, DownloadTaskState::Completed);
            entry.snapshot.stage = TaskStage::Done;
            queue.insert(entry).unwrap();
        }
        first.state = DownloadTaskState::Paused;
        queue.update(first).unwrap();
        assert_eq!(queue.entries().len(), MAX_FINISHED_HISTORY + 1);
        assert!(!queue
            .entries()
            .iter()
            .any(|entry| entry.snapshot.chapter_info.chapter_id == 2));

        // 原件不可读时，备用文件承载故障期间的活动状态。
        std::fs::rename(&path, path.with_file_name(".download-queue.runtime.json")).unwrap();
        let restored = DownloadQueue::load(path.clone()).unwrap();
        assert!(restored.path().file_name().unwrap() == ".download-queue.runtime.json");
        assert_eq!(restored.entries().len(), MAX_FINISHED_HISTORY + 1);

        // 原件恢复可读后必须重新使用原件；未确认可清理前保留备用副本。
        let primary_file = QueueFile {
            version: QUEUE_FORMAT_VERSION,
            tasks: vec![entry(1, DownloadTaskState::Paused)],
        };
        std::fs::write(&path, serde_json::to_vec_pretty(&primary_file).unwrap()).unwrap();
        let primary = DownloadQueue::load(path.clone()).unwrap();
        assert_eq!(primary.path(), path);
        assert!(path.with_file_name(".download-queue.runtime.json").exists());
        assert_eq!(primary.entries().len(), 1);
        assert_eq!(primary.entries()[0].snapshot.chapter_info.chapter_id, 1);
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
mod resumed_state_tests {
    use super::*;
    #[test]
    fn old_queue_resumes_under_current_settings_without_accepting_a_changed_root() {
        let current = DownloadSettings::from(&Config::default(std::path::Path::new("library")));
        let old = current.clone();
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
                stage: TaskStage::Queued,
                failure: None,
                attempts: 0,
                image_failures: Vec::new(),
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
