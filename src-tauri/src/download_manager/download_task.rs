use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use anyhow::{anyhow, Context};
use parking_lot::{Mutex, RwLock};
use tauri::{AppHandle, Manager};
use tauri_specta::Event;
use tokio::sync::watch;

use crate::download_queue::{DownloadSettings, DownloadTaskSnapshot};
use crate::download_scheduler::{run_bounded, DownloadPermit};
use crate::events::DownloadTaskEvent;
use crate::extensions::{AnyhowErrorToStringChain, AppHandleExt};
use crate::types::{ChapterInfo, Comic};
use crate::utils;

use super::image_download::DownloadImgTask;
use super::package::package_chapter_to_cbz;
use super::scramble::calculate_block_num;
use super::{DownloadManager, DownloadTaskState};

#[derive(Default)]
pub(crate) struct TaskFileWrites {
    pub(crate) cancelled: Mutex<bool>,
}

impl TaskFileWrites {
    pub(crate) fn cancel(&self) {
        *self.cancelled.lock() = true;
    }

    pub(crate) fn run<T>(&self, write: impl FnOnce() -> anyhow::Result<T>) -> anyhow::Result<T> {
        let cancelled = self.cancelled.lock();
        anyhow::ensure!(!*cancelled, "下载任务已取消，跳过文件写入");
        write()
    }
}

pub(crate) fn save_download_metadata(
    comic: &Comic,
    settings: &DownloadSettings,
) -> anyhow::Result<()> {
    // 平铺下载的元数据随各章节写入CBZ，根目录不能共用一个漫画的元数据文件。
    if comic.comic_download_dir.as_deref() == Some(settings.download_dir.as_path()) {
        return Ok(());
    }
    comic.save_comic_metadata()
}

#[derive(Clone)]
pub(crate) struct DownloadTask {
    pub(crate) task_id: uuid::Uuid,
    pub(crate) app: AppHandle,
    pub(crate) download_manager: DownloadManager,
    pub(crate) comic: Arc<Comic>,
    pub(crate) chapter_info: Arc<ChapterInfo>,
    pub(crate) state_sender: watch::Sender<DownloadTaskState>,
    pub(crate) downloaded_img_count: Arc<AtomicU32>,
    pub(crate) total_img_count: Arc<AtomicU32>,
    pub(crate) error_message: Arc<RwLock<Option<String>>>,
    pub(crate) event_lock: Arc<Mutex<()>>,
    pub(crate) file_writes: Arc<TaskFileWrites>,
    pub(crate) settings: DownloadSettings,
}

impl DownloadTask {
    pub(crate) fn new(app: AppHandle, mut comic: Comic, chapter_id: i64) -> anyhow::Result<Self> {
        let settings = DownloadSettings::from(&*app.get_config().read());
        comic
            .apply_download_dir(&settings)
            .context(format!("漫画`{}`更新`download_dir`字段失败", comic.name))?;

        let chapter_info = comic
            .chapter_infos
            .iter()
            .find(|chapter| chapter.chapter_id == chapter_id)
            .cloned()
            .context(format!("未找到章节ID为`{chapter_id}`的章节信息"))?;

        let download_manager = app.get_download_manager().inner().clone();
        let (state_sender, _) = watch::channel(DownloadTaskState::Pending);

        let task = Self {
            task_id: uuid::Uuid::new_v4(),
            app,
            download_manager,
            comic: Arc::new(comic),
            chapter_info: Arc::new(chapter_info),
            state_sender,
            downloaded_img_count: Arc::new(AtomicU32::new(0)),
            total_img_count: Arc::new(AtomicU32::new(0)),
            error_message: Arc::new(RwLock::new(None)),
            event_lock: Arc::new(Mutex::new(())),
            file_writes: Arc::default(),
            settings,
        };

        Ok(task)
    }

    pub(crate) async fn process(self) {
        self.emit_download_task_create_event();

        let download_comic_task = self.download_chapter();
        tokio::pin!(download_comic_task);

        let mut state_receiver = self.state_sender.subscribe();
        state_receiver.mark_changed();
        let mut permit = None;
        loop {
            let state_is_downloading = *state_receiver.borrow() == DownloadTaskState::Downloading;
            let state_is_pending = *state_receiver.borrow() == DownloadTaskState::Pending;
            tokio::select! {
                () = &mut download_comic_task, if state_is_downloading && permit.is_some() => break,
                control_flow = self.acquire_chapter_permit(&mut permit), if state_is_pending => {
                    match control_flow {
                        ControlFlow::Continue(()) => continue,
                        ControlFlow::Break(()) => break,
                    }
                },
                _ = state_receiver.changed() => {
                    match self.handle_state_change(&mut permit, &mut state_receiver) {
                        ControlFlow::Continue(()) => continue,
                        ControlFlow::Break(()) => break,
                    }
                }
            }
        }

        let completed = *self.state_sender.borrow() == DownloadTaskState::Completed;
        self.download_manager
            .remove_finished_task(self.chapter_info.chapter_id, self.task_id);
        if completed {
            // 该章节完成后，若这本漫画已没有待处理的下载任务，则触发归档最终整合
            // （合并为整本 CBZ / 逐话命名整理），由后台单飞任务执行。
            crate::archive_finalize::spawn_finalize_if_idle(&self.app, self.comic.id);
        }
    }

    async fn download_chapter(&self) {
        // 恢复时先显示上次进度，开始执行后按实际有效图片重新计数。
        self.downloaded_img_count.store(0, Ordering::Relaxed);
        self.total_img_count.store(0, Ordering::Relaxed);
        let comic_title = &self.comic.name;
        let chapter_title = &self.chapter_info.chapter_title;
        let chapter_id = self.chapter_info.chapter_id;

        if let Err(err) = self
            .file_writes
            .run(|| save_download_metadata(&self.comic, &self.settings))
        {
            let err_title = format!("`{comic_title}`保存元数据失败");
            let string_chain = err.to_string_chain();
            tracing::error!(err_title, message = string_chain);

            self.fail(format!("{err_title}：{err:#}"));

            return;
        }

        if let Some(dir) = &self.comic.comic_download_dir {
            if let Some(cache) = self.app.try_state::<utils::DownloadedLibraryCache>() {
                cache.insert(self.comic.id, dir.clone());
            }
        }

        // 获取此章节每张图片的下载链接以及对应的block_num
        let Some(urls_with_block_num) = self.get_urls_with_block_num(chapter_id).await else {
            return;
        };
        // 记录总共需要下载的图片数量
        #[allow(clippy::cast_possible_truncation)]
        self.total_img_count
            .store(urls_with_block_num.len() as u32, Ordering::Relaxed);
        // 创建临时下载目录
        let Some(temp_download_dir) = self.create_temp_download_dir() else {
            return;
        };
        // 清理临时下载目录中不属于当前页码或不是图片的文件
        self.clean_temp_download_dir(&temp_download_dir);

        // 重新下载时保留原CBZ，复用可读取的页面，只请求缺失或损坏页面。
        if let Some(source) = &self.chapter_info.chapter_download_dir {
            if let Err(error) = self.file_writes.run(|| {
                if source.is_file() {
                    super::package::ensure_archive_identity(
                        source,
                        &self.comic,
                        &self.chapter_info,
                    )?;
                }
                super::image_download::seed_valid_pages(
                    source,
                    &temp_download_dir,
                    urls_with_block_num.len(),
                )
            }) {
                self.fail(format!("复用现有章节图片失败：{error:#}"));
                return;
            }
        }

        let window_size = self.app.get_config().read().img_concurrency.max(1);
        if let Err(error) = run_bounded(
            urls_with_block_num.into_iter().enumerate(),
            window_size,
            |(index, (url, block_num))| {
                DownloadImgTask::new(self, url, index, temp_download_dir.clone(), block_num)
                    .process()
            },
        )
        .await
        {
            self.fail(format!("图片任务异常退出：{error:#}"));
            return;
        }
        tracing::trace!(comic_title, chapter_title, "所有图片下载任务完成");
        // 检查此章节的图片是否全部下载成功
        let downloaded_img_count = self.downloaded_img_count.load(Ordering::Relaxed);
        let total_img_count = self.total_img_count.load(Ordering::Relaxed);
        if downloaded_img_count != total_img_count {
            // 此章节的图片未全部下载成功
            let err_title = format!("`{comic_title} - {chapter_title}`下载不完整");
            let err_msg =
                format!("总共有`{total_img_count}`张图片，但只下载了`{downloaded_img_count}`张");
            tracing::error!(err_title, message = err_msg);

            let detail = self.error_message.read().clone().unwrap_or_default();
            self.fail(format!("{err_msg}。{detail}"));

            return;
        }

        // 图片全部下载并反混淆校验完成，打包为CBZ并清理临时散图目录
        if let Err(err) = self.file_writes.run(|| {
            package_chapter_to_cbz(
                &self.comic,
                &self.chapter_info,
                &self.app,
                &temp_download_dir,
                total_img_count as usize,
            )
        }) {
            let err_title = format!("`{comic_title} - {chapter_title}`打包CBZ失败");
            let string_chain = err.to_string_chain();
            tracing::error!(err_title, message = string_chain);

            self.fail(format!("{err_title}：{err:#}"));

            return;
        }

        tracing::info!(comic_title, chapter_title, "章节下载与打包成功");

        // 完成只允许从“下载中”迁移：用户恰好在收尾阶段点了暂停时，
        // 不能被这里覆盖成 Completed（否则前端状态与按钮语义错乱）。
        self.change_state(
            DownloadTaskState::Completed,
            &[DownloadTaskState::Downloading],
        );
        self.emit_download_task_update_event();
    }

    fn create_temp_download_dir(&self) -> Option<PathBuf> {
        let comic_title = &self.comic.name;
        let chapter_title = &self.chapter_info.chapter_title;

        let temp_download_dir = match self.chapter_info.get_temp_download_dir() {
            Ok(temp_download_dir) => temp_download_dir,
            Err(err) => {
                let err_title = format!("`{comic_title} - {chapter_title}`获取临时下载目录失败");
                let string_chain = err.to_string_chain();
                tracing::error!(err_title, message = string_chain);

                self.fail(format!("{err_title}：{err:#}"));

                return None;
            }
        };

        if let Err(err) = self
            .file_writes
            .run(|| std::fs::create_dir_all(&temp_download_dir).map_err(anyhow::Error::from))
        {
            let err_title = format!(
                "`{comic_title} - {chapter_title}`创建临时下载目录`{}`失败",
                temp_download_dir.display()
            );
            let string_chain = err.to_string_chain();
            tracing::error!(err_title, message = string_chain);

            self.fail(format!("{err_title}：{err:#}"));

            return None;
        }

        tracing::trace!(
            comic_title,
            chapter_title,
            "创建临时下载目录`{}`成功",
            temp_download_dir.display()
        );

        Some(temp_download_dir)
    }

    async fn get_urls_with_block_num(&self, chapter_id: i64) -> Option<Vec<(String, u32)>> {
        let comic_title = &self.comic.name;
        let chapter_title = &self.chapter_info.chapter_title;
        let jm_client = self.app.get_jm_client();

        let res = tokio::try_join!(
            jm_client.get_scramble_id(chapter_id),
            jm_client.get_chapter(chapter_id)
        );

        let (scramble_id, chapter_resp_data) = match res {
            Ok(data) => data,
            Err(err) => {
                let err_title = format!("`{comic_title} - {chapter_title}`获取图片下载链接失败");
                let string_chain = err.to_string_chain();
                tracing::error!(err_title, message = string_chain);

                self.fail(format!("{err_title}：{err:#}"));

                return None;
            }
        };
        // 构造图片下载链接
        let image_domain = self.app.get_config().read().get_image_domain();
        let urls_with_block_num = chapter_resp_data
            .images
            .into_iter()
            .map(|filename| -> anyhow::Result<(String, u32)> {
                let file_path = Path::new(&filename);
                let ext = file_path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .map(str::to_lowercase)
                    .context(format!("图片文件名`{filename}`没有有效扩展名"))?;
                let url = format!("https://{image_domain}/media/photos/{chapter_id}/{filename}");
                if ext == "gif" {
                    return Ok((url, 0));
                }
                if !matches!(ext.as_str(), "webp" | "jpg" | "jpeg" | "png") {
                    return Err(anyhow!("不支持的图片扩展名`{ext}`：{filename}"));
                }

                let filename_without_ext = file_path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .context(format!("图片文件名`{filename}`没有有效主文件名"))?;
                let block_num = calculate_block_num(scramble_id, chapter_id, filename_without_ext);
                Ok((url, block_num))
            })
            .collect::<anyhow::Result<Vec<_>>>();

        let urls_with_block_num = match urls_with_block_num {
            Ok(urls) if !urls.is_empty() => urls,
            Ok(_) => {
                tracing::error!(comic_title, chapter_title, "接口没有返回任何图片");
                self.fail("接口没有返回任何图片，请稍后重试".to_string());
                return None;
            }
            Err(err) => {
                tracing::error!(comic_title, chapter_title, error = %err, "解析图片列表失败");
                self.fail(format!("解析图片列表失败：{err:#}"));
                return None;
            }
        };

        tracing::trace!(comic_title, chapter_title, "获取图片链接成功");

        Some(urls_with_block_num)
    }

    /// 删除临时下载目录中不属于当前页码或不是图片的文件
    fn clean_temp_download_dir(&self, temp_download_dir: &Path) {
        let comic_title = &self.comic.name;
        let chapter_title = &self.chapter_info.chapter_title;

        let entries = match std::fs::read_dir(temp_download_dir).map_err(anyhow::Error::from) {
            Ok(entries) => entries,
            Err(err) => {
                let err_title = format!(
                    "`{comic_title}`读取临时下载目录`{}`失败",
                    temp_download_dir.display()
                );
                let string_chain = err.to_string_chain();
                tracing::error!(err_title, message = string_chain);
                return;
            }
        };

        for path in entries.filter_map(Result::ok).map(|entry| entry.path()) {
            // 只保留合法页码的图片文件（扩展名随源格式，不做格式转换）
            let should_keep = path
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| matches!(ext, "jpg" | "jpeg" | "png" | "webp" | "gif"))
                && path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .and_then(|stem| stem.parse::<u32>().ok())
                    .is_some_and(|page| {
                        page > 0 && page <= self.total_img_count.load(Ordering::Relaxed)
                    });
            if should_keep {
                continue;
            }
            // 否则删除文件
            if let Err(err) = self
                .file_writes
                .run(|| std::fs::remove_file(&path).map_err(anyhow::Error::from))
            {
                let err_title =
                    format!("`{comic_title}`删除临时下载目录的`{}`失败", path.display());
                let string_chain = err.to_string_chain();
                tracing::error!(err_title, message = string_chain);
            }
        }

        tracing::trace!(
            comic_title,
            chapter_title,
            "清理临时下载目录`{}`成功",
            temp_download_dir.display()
        );
    }

    async fn acquire_chapter_permit(&self, permit: &mut Option<DownloadPermit>) -> ControlFlow<()> {
        let comic_title = &self.comic.name;
        let chapter_title = &self.chapter_info.chapter_title;

        tracing::debug!(comic_title, chapter_title, "章节开始排队");

        self.emit_download_task_update_event();
        if permit.is_none() {
            *permit = Some(self.download_manager.chapter_sem.acquire().await);
        }
        self.change_state(
            DownloadTaskState::Downloading,
            &[DownloadTaskState::Pending],
        );
        ControlFlow::Continue(())
    }

    fn handle_state_change(
        &self,
        permit: &mut Option<DownloadPermit>,
        state_receiver: &mut watch::Receiver<DownloadTaskState>,
    ) -> ControlFlow<()> {
        let comic_title = &self.comic.name;
        let chapter_title = &self.chapter_info.chapter_title;

        self.emit_download_task_update_event();
        let state = *state_receiver.borrow();
        match state {
            DownloadTaskState::Paused => {
                tracing::debug!(comic_title, chapter_title, "章节暂停中");
                if let Some(permit) = permit.take() {
                    drop(permit);
                }
                ControlFlow::Continue(())
            }
            DownloadTaskState::Cancelled => {
                tracing::debug!(comic_title, chapter_title, "章节取消下载");
                ControlFlow::Break(())
            }
            _ => ControlFlow::Continue(()),
        }
    }

    pub(crate) fn change_state(&self, state: DownloadTaskState, allowed: &[DownloadTaskState]) {
        let _guard = self.event_lock.lock();
        let current = *self.state_sender.borrow();
        if allowed.contains(&current) {
            if state == DownloadTaskState::Cancelled {
                // 等待已开始的写入完成，并禁止后台编码任务提交迟到结果。
                self.file_writes.cancel();
            }
            self.state_sender.send_replace(state);
            self.publish_update();
        }
    }

    pub(crate) fn fail(&self, message: String) {
        let _guard = self.event_lock.lock();
        if *self.state_sender.borrow() == DownloadTaskState::Cancelled {
            return;
        }
        self.file_writes.cancel();
        *self.error_message.write() = Some(message.chars().take(2048).collect());
        self.state_sender.send_replace(DownloadTaskState::Failed);
        self.publish_update();
    }

    pub(crate) fn record_image_error(&self, message: String) {
        let _guard = self.event_lock.lock();
        let mut error = self.error_message.write();
        if error.is_none() {
            *error = Some(message.chars().take(2048).collect());
        }
    }

    pub(crate) fn snapshot(&self) -> DownloadTaskSnapshot {
        DownloadTaskSnapshot {
            task_id: self.task_id.to_string(),
            revision: 0,
            state: *self.state_sender.borrow(),
            comic: Box::new(self.comic.as_ref().clone()),
            chapter_info: Box::new(self.chapter_info.as_ref().clone()),
            downloaded_img_count: self.downloaded_img_count.load(Ordering::Relaxed),
            total_img_count: self.total_img_count.load(Ordering::Relaxed),
            error_message: self.error_message.read().clone(),
        }
    }

    pub(crate) fn publish_update(&self) {
        if let Some(snapshot) = self.download_manager.queue.update(self.snapshot()) {
            let _ = DownloadTaskEvent::Update {
                chapter_id: snapshot.chapter_info.chapter_id,
                revision: snapshot.revision,
                state: snapshot.state,
                downloaded_img_count: snapshot.downloaded_img_count,
                total_img_count: snapshot.total_img_count,
                error_message: snapshot.error_message,
            }
            .emit(&self.app);
        }
    }

    pub(crate) fn emit_download_task_update_event(&self) {
        let _guard = self.event_lock.lock();
        self.publish_update();
    }

    pub(crate) fn emit_download_task_create_event(&self) {
        let _guard = self.event_lock.lock();
        if let Some(snapshot) = self.download_manager.queue.update(self.snapshot()) {
            let _ = DownloadTaskEvent::Create(snapshot).emit(&self.app);
        }
    }
}

impl Comic {
    /// 根据下载设置更新 `comic_download_dir` 与 `chapter_infos.chapter_download_dir`。
    ///
    /// 目录布局已固定为「书库目录 / 本子名 [JM本子号-章节号].cbz」的平铺结构，
    /// 不再支持用户自定义目录模板。
    pub(crate) fn apply_download_dir(
        &mut self,
        settings: &DownloadSettings,
    ) -> anyhow::Result<()> {
        if self.chapter_infos.is_empty() {
            return Err(anyhow!("没有章节信息，无法更新下载目录字段"));
        }

        let comic_dir = settings.download_dir.clone();
        for chapter_info in &mut self.chapter_infos {
            // 保留身份匹配的旧包，不迁移用户已有文件；新文件始终使用唯一 ID。
            let keep_existing = chapter_info
                .chapter_download_dir
                .as_ref()
                .is_some_and(|path| {
                    path.is_file()
                        && std::fs::canonicalize(path).is_ok_and(|path| {
                            std::fs::canonicalize(&comic_dir)
                                .is_ok_and(|root| path.starts_with(root))
                        })
                        && super::package::ensure_archive_identity(
                            path,
                            &Comic {
                                id: self.id,
                                ..Default::default()
                            },
                            chapter_info,
                        )
                        .is_ok()
                });
            if !keep_existing {
                chapter_info.chapter_download_dir = Some(comic_dir.join(
                    utils::storage_cbz_filename(&self.name, self.id, chapter_info.chapter_id),
                ));
            }
        }

        self.comic_download_dir = Some(comic_dir);

        Ok(())
    }
}

impl ChapterInfo {
    fn get_temp_download_dir(&self) -> anyhow::Result<PathBuf> {
        let chapter_download_dir = self
            .chapter_download_dir
            .as_ref()
            .context("`chapter_download_dir`字段为`None`")?;

        let chapter_download_dir_name = self
            .get_chapter_download_dir_name()
            .context("获取章节下载目录名失败")?;

        let parent = chapter_download_dir.parent().context(format!(
            "`{}`的父目录不存在",
            chapter_download_dir.display()
        ))?;

        let safe_dir_name = utils::truncate_utf8_by_bytes(&chapter_download_dir_name, 120);
        let safe_dir_name = if safe_dir_name.is_empty() {
            "chapter"
        } else {
            safe_dir_name
        };
        let temp_download_dir =
            parent.join(format!(".downloading-{safe_dir_name}-{}", self.chapter_id));
        Ok(temp_download_dir)
    }
}
