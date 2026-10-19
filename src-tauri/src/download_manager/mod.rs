use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::{AppHandle, Manager};
use tauri_specta::Event;

use crate::config::ConfigStorage;
use crate::download_queue::{
    DownloadQueue, DownloadSettings, DownloadTaskSnapshot, StoredDownload,
};
use crate::download_scheduler::DownloadLimiter;
use crate::events::DownloadTaskEvent;
use crate::extensions::AppHandleExt;
use crate::types::Comic;
use crate::DownloadSpeedEvent;

pub(crate) mod download_task;
pub(crate) mod image_download;
pub(crate) mod package;
pub(crate) mod scramble;

pub(crate) use download_task::DownloadTask;

/// 用于管理下载任务
///
/// 克隆 `DownloadManager` 的开销极小，性能开销几乎可以忽略不计。
/// 可以放心地在多个线程中传递和使用它的克隆副本。
///
/// 具体来说：
/// - `client`和`app`的克隆开销很小。
/// - 其他字段都被 `Arc` 包裹，这些字段的克隆操作仅仅是增加引用计数。
#[derive(Clone)]
pub struct DownloadManager {
    app: AppHandle,
    pub(crate) chapter_sem: Arc<DownloadLimiter>,
    pub(crate) img_sem: Arc<DownloadLimiter>,
    pub(crate) byte_per_sec: Arc<AtomicU64>,
    download_tasks: Arc<RwLock<HashMap<i64, DownloadTask>>>,
    pub(crate) queue: Arc<DownloadQueue>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
pub enum DownloadTaskState {
    Pending,
    Downloading,
    Paused,
    Cancelled,
    Completed,
    Failed,
}

impl DownloadManager {
    pub fn new(app: AppHandle) -> anyhow::Result<Self> {
        let (chapter_concurrency, img_concurrency) = {
            let config = app.get_config();
            let config = config.read();
            (config.chapter_concurrency, config.img_concurrency)
        };

        let queue_path = app
            .state::<ConfigStorage>()
            .config_path()
            .with_file_name("download-queue.json");
        let queue = Arc::new(DownloadQueue::load(queue_path)?);
        let manager = DownloadManager {
            app,
            chapter_sem: DownloadLimiter::new(chapter_concurrency),
            img_sem: DownloadLimiter::new(img_concurrency),
            byte_per_sec: Arc::new(AtomicU64::new(0)),
            download_tasks: Arc::new(RwLock::new(HashMap::new())),
            queue,
        };

        tauri::async_runtime::spawn(manager.clone().emit_download_speed_loop());

        Ok(manager)
    }

    pub fn snapshots(&self) -> Vec<DownloadTaskSnapshot> {
        self.queue
            .entries()
            .into_iter()
            .map(|entry| entry.snapshot)
            .collect()
    }

    /// 该漫画是否仍有活跃的下载任务。
    ///
    /// 只有排队中/下载中的任务算“活跃”；已暂停、失败、取消或完成的任务不阻塞归档整合，
    /// 否则暂停一章就会让整本永远无法自动合并，手动“整理为整本”也会被误拦。
    pub fn has_active_tasks_for_comic(&self, comic_id: i64) -> bool {
        self.download_tasks.read().values().any(|task| {
            task.comic.id == comic_id
                && matches!(
                    *task.state_sender.borrow(),
                    DownloadTaskState::Pending | DownloadTaskState::Downloading
                )
        })
    }

    /// 在“没有任何下载任务且没有归档整合进行中”时执行清理类操作
    /// （切换书库目录、清空任务等），避免与整合的合并/删除文件交叉。
    pub fn with_idle_downloads<T>(
        &self,
        action: impl FnOnce() -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let tasks = self.download_tasks.write();
        anyhow::ensure!(tasks.is_empty(), "仍有下载任务运行，请等待完成或取消后重试");
        if let Some(state) = self
            .app
            .try_state::<crate::archive_finalize::ArchiveFinalizeState>()
        {
            anyhow::ensure!(!state.is_any_running(), "正在整理 CBZ，请稍候再试");
        }
        action()
    }

    pub fn restore_pending(&self) {
        let mut tasks = self.download_tasks.write();
        let settings = DownloadSettings::from(&*self.app.get_config().read());
        for entry in self.queue.entries() {
            if entry.snapshot.state == DownloadTaskState::Failed
                || tasks.contains_key(&entry.snapshot.chapter_info.chapter_id)
            {
                continue;
            }
            let restore = (|| -> anyhow::Result<()> {
                let resumed_state = entry.resumed_state(&settings)?;
                let task = DownloadTask::new(
                    self.app.clone(),
                    *entry.snapshot.comic.clone(),
                    entry.snapshot.chapter_info.chapter_id,
                )?;
                task.state_sender.send_replace(resumed_state);
                task.downloaded_img_count
                    .store(entry.snapshot.downloaded_img_count, Ordering::Relaxed);
                task.total_img_count
                    .store(entry.snapshot.total_img_count, Ordering::Relaxed);
                self.queue.insert(StoredDownload {
                    snapshot: task.snapshot(),
                    settings: task.settings.clone(),
                })?;
                tasks.insert(task.chapter_info.chapter_id, task.clone());
                tauri::async_runtime::spawn(task.process());
                Ok(())
            })();
            if let Err(error) = restore {
                let mut snapshot = entry.snapshot;
                snapshot.state = DownloadTaskState::Failed;
                snapshot.error_message = Some(format!("恢复任务失败：{error:#}"));
                self.queue.update(snapshot);
            }
        }
    }

    pub fn create_download_task(&self, comic: Comic, chapter_id: i64) -> anyhow::Result<()> {
        use DownloadTaskState::{Downloading, Paused, Pending};
        // 整合进行中不要创建新任务：整合会按快照合并并删除来源章节文件，
        // 与下载写入交叉会导致漏章。请用户等整理完成后再下载。
        if let Some(state) = self
            .app
            .try_state::<crate::archive_finalize::ArchiveFinalizeState>()
        {
            if state.is_running(comic.id) {
                return Err(anyhow!("该漫画正在整理 CBZ，请稍后再下载"));
            }
        }
        let mut tasks = self.download_tasks.write();
        if let Some(task) = tasks.get(&chapter_id) {
            let state = *task.state_sender.borrow();
            if matches!(state, Pending | Downloading | Paused) {
                return Err(anyhow!("章节ID为`{chapter_id}`的下载任务已存在"));
            }
        }
        let task = DownloadTask::new(self.app.clone(), comic, chapter_id)
            .context("DownloadTask创建失败")?;
        self.queue.insert(StoredDownload {
            snapshot: task.snapshot(),
            settings: task.settings.clone(),
        })?;
        tasks.insert(chapter_id, task.clone());
        drop(tasks);
        tauri::async_runtime::spawn(task.process());
        Ok(())
    }

    pub fn reload_concurrency(&self) {
        let config = self.app.get_config();
        let config = config.read();
        self.chapter_sem.set_limit(config.chapter_concurrency);
        self.img_sem.set_limit(config.img_concurrency);
    }

    pub(crate) fn remove_finished_task(&self, chapter_id: i64, task_id: uuid::Uuid) {
        let mut tasks = self.download_tasks.write();
        let should_remove = tasks
            .get(&chapter_id)
            .is_some_and(|task| task.task_id == task_id);
        if should_remove {
            tasks.remove(&chapter_id);
        }
    }

    pub fn pause_download_task(&self, chapter_id: i64) -> anyhow::Result<()> {
        let tasks = self.download_tasks.read();
        let Some(task) = tasks.get(&chapter_id) else {
            return Err(anyhow!("未找到章节ID为`{chapter_id}`的下载任务"));
        };
        task.change_state(
            DownloadTaskState::Paused,
            &[DownloadTaskState::Pending, DownloadTaskState::Downloading],
        );
        Ok(())
    }

    pub fn resume_download_task(&self, chapter_id: i64) -> anyhow::Result<()> {
        let tasks = self.download_tasks.read();
        let Some(task) = tasks.get(&chapter_id) else {
            return Err(anyhow!("未找到章节ID为`{chapter_id}`的下载任务"));
        };
        task.change_state(DownloadTaskState::Pending, &[DownloadTaskState::Paused]);
        Ok(())
    }

    pub fn cancel_download_task(&self, chapter_id: i64) -> anyhow::Result<()> {
        let tasks = self.download_tasks.read();
        if let Some(task) = tasks.get(&chapter_id) {
            task.change_state(
                DownloadTaskState::Cancelled,
                &[
                    DownloadTaskState::Pending,
                    DownloadTaskState::Downloading,
                    DownloadTaskState::Paused,
                    DownloadTaskState::Failed,
                ],
            );
        } else {
            let mut snapshot = self
                .snapshots()
                .into_iter()
                .find(|snapshot| snapshot.chapter_info.chapter_id == chapter_id)
                .context("未找到要移除的下载任务")?;
            // 只允许从可取消状态迁移：已完成的任务不能被“移除”改写成取消并从历史中消失
            anyhow::ensure!(
                matches!(
                    snapshot.state,
                    DownloadTaskState::Pending
                        | DownloadTaskState::Paused
                        | DownloadTaskState::Failed
                ),
                "该任务已结束，无需取消"
            );
            snapshot.state = DownloadTaskState::Cancelled;
            if let Some(snapshot) = self.queue.update(snapshot) {
                let _ = DownloadTaskEvent::Create(snapshot).emit(&self.app);
            }
        }
        Ok(())
    }

    #[allow(clippy::cast_precision_loss)]
    async fn emit_download_speed_loop(self) {
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        let mut was_zero = true;

        loop {
            interval.tick().await;
            let byte_per_sec = self.byte_per_sec.swap(0, Ordering::Relaxed);
            if byte_per_sec == 0 {
                if was_zero {
                    continue;
                }
                was_zero = true;
            } else {
                was_zero = false;
            }
            let mega_byte_per_sec = byte_per_sec as f64 / 1024.0 / 1024.0;
            let speed = format!("{mega_byte_per_sec:.2}MB/s");
            // 发送总进度条下载速度事件
            let _ = DownloadSpeedEvent { speed }.emit(&self.app);
        }
    }
}

#[cfg(test)]
mod regression_tests {
    use super::download_task::TaskFileWrites;
    use super::image_download::{is_valid_saved_image, release_inactive_image_permit, save_img};
    use super::scramble::stitch_img;
    use super::*;
    use crate::atomic_fs;
    use crate::download_scheduler::DownloadLimiter;
    use crate::types::{ChapterInfo, Comic};
    use bytes::Bytes;
    use image::{ImageFormat, RgbImage};
    use std::io::Cursor;
    use std::path::{Path, PathBuf};
    use tokio::sync::watch;

    #[test]
    fn flat_download_does_not_overwrite_another_comics_root_metadata() {
        let root = std::env::temp_dir().join(format!("jm-flat-meta-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("元数据.json");
        let original = serde_json::to_vec(&Comic {
            id: 1,
            ..Comic::default()
        })
        .unwrap();
        std::fs::write(&path, &original).unwrap();
        let comic = Comic {
            id: 2,
            comic_download_dir: Some(root.clone()),
            ..Comic::default()
        };
        let settings = DownloadSettings {
            download_dir: root.clone(),
        };
        super::download_task::save_download_metadata(&comic, &settings).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), original);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn coalesced_pause_resume_releases_image_slot_for_other_chapter() {
        let chapters = DownloadLimiter::new(1);
        let images = DownloadLimiter::new(1);
        let first_chapter = chapters.acquire().await;
        let mut image_permit = Some(images.acquire().await);
        let (state, receiver) = watch::channel(DownloadTaskState::Downloading);

        state.send_replace(DownloadTaskState::Paused);
        drop(first_chapter);
        let second_chapter = chapters.acquire().await;
        // 图片任务尚未被调度，直接观察到 Pending，跳过了 Paused。
        state.send_replace(DownloadTaskState::Pending);
        release_inactive_image_permit(*receiver.borrow(), &mut image_permit);

        let second_image = tokio::time::timeout(Duration::from_secs(1), images.acquire())
            .await
            .expect("排队章节不能占住图片名额");
        drop(second_image);
        drop(second_chapter);
        let _resumed = tokio::time::timeout(Duration::from_secs(1), chapters.acquire())
            .await
            .expect("恢复的章节可以继续执行");
    }

    #[test]
    fn cancelled_worker_cannot_overwrite_retry_or_recreate_old_format() {
        let root = std::env::temp_dir().join(format!("jm-cancel-{}", uuid::Uuid::new_v4()));
        let old_writes = Arc::new(TaskFileWrites::default());
        let worker_writes = old_writes.clone();
        let worker_root = root.clone();
        let (release, wait) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            wait.recv().unwrap();
            worker_writes.run(|| atomic_fs::write(&worker_root.join("0001.png"), b"old"))
        });
        old_writes.cancel();
        let retry = TaskFileWrites::default();
        retry
            .run(|| atomic_fs::write(&root.join("0001.jpg"), b"new"))
            .unwrap();
        release.send(()).unwrap();
        assert!(worker.join().unwrap().is_err());
        assert!(!root.join("0001.png").exists());
        assert_eq!(std::fs::read(root.join("0001.jpg")).unwrap(), b"new");
        assert!(old_writes
            .run(|| atomic_fs::write(&root.join("0001.jpg"), b"old"))
            .is_err());
        assert_eq!(std::fs::read(root.join("0001.jpg")).unwrap(), b"new");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cancel_waits_for_an_in_progress_commit_and_blocks_later_commits() {
        let writes = Arc::new(TaskFileWrites::default());
        let worker_writes = writes.clone();
        let (entered, started) = std::sync::mpsc::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let writer = std::thread::spawn(move || {
            worker_writes.run(|| {
                entered.send(()).unwrap();
                wait.recv().unwrap();
                Ok(())
            })
        });
        started.recv().unwrap();
        assert!(writes.cancelled.try_lock().is_none());
        let cancelling_writes = writes.clone();
        let (cancel_started, cancelling) = std::sync::mpsc::channel();
        let canceller = std::thread::spawn(move || {
            cancel_started.send(()).unwrap();
            cancelling_writes.cancel();
        });
        cancelling.recv().unwrap();
        release.send(()).unwrap();
        writer.join().unwrap().unwrap();
        canceller.join().unwrap();
        assert!(writes.run(|| Ok(())).is_err());
    }

    #[tokio::test]
    async fn rayon_image_save_checks_cancellation_before_committing() {
        let root = std::env::temp_dir().join(format!("jm-encode-{}", uuid::Uuid::new_v4()));
        let path = root.join("0001.png");
        let mut source = Cursor::new(Vec::new());
        RgbImage::new(2, 2)
            .write_to(&mut source, ImageFormat::Png)
            .unwrap();
        let writes = Arc::new(TaskFileWrites::default());
        writes.cancel();
        let source = Bytes::from(source.into_inner());
        let result = save_img(&path, 0, source.clone(), ImageFormat::Png, writes).await;
        assert!(result.unwrap_err().to_string().contains("保存图片"));
        assert!(!root.exists());
        save_img(&path, 0, source, ImageFormat::Png, Arc::default())
        .await
        .unwrap();
        assert!(is_valid_saved_image(&path));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn flat_layout_is_applied_and_missing_chapters_are_rejected() {
        let settings = DownloadSettings {
            download_dir: PathBuf::from("downloads"),
        };
        // 没有章节信息时拒绝更新，避免写入一个指向空目录的库路径
        let mut empty = Comic::default();
        assert!(empty.apply_download_dir(&settings).is_err());
        assert!(empty.comic_download_dir.is_none());

        // 固定平铺布局：书库目录 + 本子名 [JM本子号-章节号].cbz
        let mut comic = Comic {
            id: 42,
            name: "测试本子".into(),
            chapter_infos: vec![ChapterInfo {
                chapter_id: 1,
                chapter_title: "第一章".into(),
                order: 1,
                is_downloaded: None,
                chapter_download_dir: None,
            }],
            ..Comic::default()
        };
        comic.apply_download_dir(&settings).unwrap();
        assert_eq!(comic.comic_download_dir.as_deref(), Some(Path::new("downloads")));
        let chapter_dir = comic.chapter_infos[0].chapter_download_dir.as_ref().unwrap();
        assert_eq!(chapter_dir.parent(), Some(Path::new("downloads")));
        assert_eq!(
            chapter_dir.file_name().and_then(|name| name.to_str()),
            Some("测试本子 [JM42-1].cbz")
        );
    }

    #[test]
    fn stitch_img_produces_identical_output_to_reference() {
        fn reference_stitch(src: &image::RgbImage, block_num: u32) -> image::RgbImage {
            let (width, height) = src.dimensions();
            let mut stitched = image::ImageBuffer::new(width, height);
            let remainder_height = height % block_num;
            for i in 0..block_num {
                let mut block_height = height / block_num;
                let src_img_y_start = height - (block_height * (i + 1)) - remainder_height;
                let mut dst_img_y_start = block_height * i;
                if i == 0 {
                    block_height += remainder_height;
                } else {
                    dst_img_y_start += remainder_height;
                }
                for y in 0..block_height {
                    let src_y = src_img_y_start + y;
                    let dst_y = dst_img_y_start + y;
                    for x in 0..width {
                        stitched.put_pixel(x, dst_y, *src.get_pixel(x, src_y));
                    }
                }
            }
            stitched
        }

        let width = 37u32;
        let height = 53u32;
        let mut test_img = image::RgbImage::new(width, height);
        for y in 0..height {
            for x in 0..width {
                test_img.put_pixel(
                    x,
                    y,
                    image::Rgb([(x % 256) as u8, (y % 256) as u8, ((x + y) % 256) as u8]),
                );
            }
        }

        for blocks in [1, 2, 3, 5, 8, 10] {
            let mut img_copy = test_img.clone();
            let optimized = stitch_img(&mut img_copy, blocks);
            let reference = reference_stitch(&test_img, blocks);
            assert_eq!(
                optimized.as_raw(),
                reference.as_raw(),
                "mismatch for block_num={blocks}"
            );
        }

        // 边界条件测试
        let mut img_copy = test_img.clone();
        assert_eq!(stitch_img(&mut img_copy, 0).as_raw(), test_img.as_raw());
        assert_eq!(stitch_img(&mut img_copy, 999).as_raw(), test_img.as_raw());
    }
}
