use std::io::{Cursor, Read};
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{ensure, Context};
use bytes::Bytes;
use image::codecs::png::{self, PngEncoder};
use image::ImageFormat;
use tauri::AppHandle;
use tokio::sync::watch;
use tokio::time::sleep;

use crate::atomic_fs;
use crate::download_scheduler::DownloadPermit;
use crate::extensions::{AnyhowErrorToStringChain, AppHandleExt};
use crate::types::DownloadFormat;

use super::download_task::{DownloadTask, TaskFileWrites};
use super::scramble::stitch_img;
use super::{DownloadManager, DownloadTaskState};

const MAX_PAGE_BYTES: u64 = 32 * 1024 * 1024;
const MAX_PAGES: usize = 10_000;

#[derive(Clone)]
pub(crate) struct DownloadImgTask {
    app: AppHandle,
    download_manager: DownloadManager,
    download_task: DownloadTask,
    url: String,
    index: usize,
    temp_download_path: PathBuf,
    block_num: u32,
}

impl DownloadImgTask {
    pub(crate) fn new(
        download_task: &DownloadTask,
        url: String,
        index: usize,
        temp_download_path: PathBuf,
        block_num: u32,
    ) -> Self {
        Self {
            app: download_task.app.clone(),
            download_manager: download_task.download_manager.clone(),
            download_task: download_task.clone(),
            url,
            index,
            temp_download_path,
            block_num,
        }
    }

    pub(crate) async fn process(self) {
        let download_img_task = self.download_img();
        tokio::pin!(download_img_task);

        let mut state_receiver = self.download_task.state_sender.subscribe();
        state_receiver.mark_changed();
        let mut permit = None;

        loop {
            let state_is_downloading = *state_receiver.borrow() == DownloadTaskState::Downloading;
            release_inactive_image_permit(*state_receiver.borrow(), &mut permit);
            tokio::select! {
                () = &mut download_img_task, if state_is_downloading && permit.is_some() => break,
                control_flow = self.acquire_img_permit(&mut permit), if state_is_downloading && permit.is_none() => {
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
    }

    async fn download_img(&self) {
        let url = &self.url;
        let comic_title = &self.download_task.comic.name;
        let chapter_title = &self.download_task.chapter_info.chapter_title;
        let temp_download_path = &self.temp_download_path;

        let index_filename = format!("{:04}", self.index + 1);
        let download_format = self.download_task.settings.download_format;
        let ext = download_format.extension();

        let user_format_path = temp_download_path.join(format!("{index_filename}.{ext}"));
        let gif_path = temp_download_path.join(format!("{index_filename}.gif"));

        let already_saved = if download_format == DownloadFormat::Original {
            // 原始模式下扩展名随源格式而定，任一图片格式的有效文件都算已保存
            find_existing_page(temp_download_path, self.index + 1)
                .is_some_and(|path| is_valid_saved_image(&path))
        } else {
            is_valid_saved_image(&user_format_path)
                || (download_format != DownloadFormat::Png && is_valid_saved_image(&gif_path))
        };
        if already_saved {
            // 如果图片已经存在，直接返回
            self.download_task
                .downloaded_img_count
                .fetch_add(1, Ordering::Relaxed);

            self.download_task.emit_download_task_update_event();

            tracing::trace!(url, comic_title, chapter_title, "图片已存在，跳过下载");
            return;
        }

        tracing::trace!(url, comic_title, chapter_title, "开始下载图片");

        let (img_data, format) = match self.app.get_jm_client().get_img_data_and_format(url).await {
            Ok(data) => data,
            Err(err) => {
                let err_title = format!("下载图片`{url}`失败");
                let string_chain = err.to_string_chain();
                tracing::error!(err_title, message = string_chain);
                self.download_task
                    .record_image_error(format!("第 {} 张图片下载失败：{err:#}", self.index + 1));
                return;
            }
        };
        let img_data_len = img_data.len() as u64;

        tracing::trace!(url, comic_title, chapter_title, "图片成功下载到内存");

        let save_ext = if download_format == DownloadFormat::Original {
            // 未混淆的图片保持源格式；需要反混淆重组的图片编码为无损 WebP
            if self.block_num == 0 {
                source_extension(format)
            } else {
                "webp"
            }
        } else if format == ImageFormat::Gif && download_format != DownloadFormat::Png {
            "gif"
        } else {
            download_format.extension()
        };
        let save_path = temp_download_path.join(format!("{index_filename}.{save_ext}"));

        let block_num = self.block_num;
        // 保存图片
        if let Err(err) = save_img(
            &save_path,
            download_format,
            block_num,
            img_data,
            format,
            self.download_task.file_writes.clone(),
        )
        .await
        {
            let err_title = format!("保存图片`{url}`失败");
            let string_chain = err.to_string_chain();
            tracing::error!(err_title, message = string_chain);
            self.download_task.record_image_error(format!(
                "第 {} 张图片处理或保存失败：{err:#}",
                self.index + 1
            ));
            return;
        }

        tracing::trace!(
            url,
            comic_title,
            chapter_title,
            "图片成功保存到`{}`",
            save_path.display()
        );

        // 记录下载字节数
        self.download_manager
            .byte_per_sec
            .fetch_add(img_data_len, Ordering::Relaxed);

        self.download_task
            .downloaded_img_count
            .fetch_add(1, Ordering::Relaxed);

        self.download_task.emit_download_task_update_event();

        let img_download_interval_sec = self.app.get_config().read().img_download_interval_sec;
        sleep(Duration::from_secs(img_download_interval_sec)).await;
    }

    async fn acquire_img_permit(&self, permit: &mut Option<DownloadPermit>) -> ControlFlow<()> {
        let url = &self.url;
        let comic_title = &self.download_task.comic.name;
        let chapter_title = &self.download_task.chapter_info.chapter_title;

        tracing::trace!(comic_title, chapter_title, url, "图片开始排队");
        if permit.is_none() {
            *permit = Some(self.download_manager.img_sem.acquire().await);
        }
        ControlFlow::Continue(())
    }

    fn handle_state_change(
        &self,
        permit: &mut Option<DownloadPermit>,
        state_receiver: &mut watch::Receiver<DownloadTaskState>,
    ) -> ControlFlow<()> {
        let url = &self.url;
        let comic_title = &self.download_task.comic.name;
        let chapter_title = &self.download_task.chapter_info.chapter_title;

        let state = *state_receiver.borrow();
        match state {
            DownloadTaskState::Paused => {
                tracing::trace!(comic_title, chapter_title, url, "图片暂停下载");
                if let Some(permit) = permit.take() {
                    drop(permit);
                }
                ControlFlow::Continue(())
            }
            DownloadTaskState::Cancelled => {
                tracing::trace!(comic_title, chapter_title, url, "图片取消下载");
                ControlFlow::Break(())
            }
            _ => ControlFlow::Continue(()),
        }
    }
}

pub(crate) fn release_inactive_image_permit(
    state: DownloadTaskState,
    permit: &mut Option<DownloadPermit>,
) {
    // watch 可能合并 Paused -> Pending 通知，不能只在观察到 Paused 时释放名额。
    if state != DownloadTaskState::Downloading {
        *permit = None;
    }
}

pub(crate) async fn save_img(
    save_path: &Path,
    download_format: DownloadFormat,
    block_num: u32,
    src_img_data: Bytes,
    src_format: ImageFormat,
    file_writes: Arc<TaskFileWrites>,
) -> anyhow::Result<()> {
    if src_format == ImageFormat::Gif && download_format != DownloadFormat::Png {
        // 如果是GIF格式，直接保存
        file_writes
            .run(|| atomic_fs::write(save_path, src_img_data))
            .context(format!("保存图片`{}`失败", save_path.display()))?;
        return Ok(());
    }

    let block_num = if src_format == ImageFormat::Gif {
        0
    } else {
        block_num
    };
    let matches_user_format = download_format == DownloadFormat::Original
        || matches!(
            (download_format, src_format),
            (DownloadFormat::Jpeg, ImageFormat::Jpeg)
                | (DownloadFormat::Png, ImageFormat::Png)
                | (DownloadFormat::Webp, ImageFormat::WebP)
        );

    if block_num == 0 && matches_user_format {
        // 未切片混淆且无需转换格式时，直接写入网络原始字节
        // 避免二次编解码的严重CPU开销与JPEG二次有损压缩导致的画质损失
        file_writes
            .run(|| atomic_fs::write(save_path, src_img_data))
            .context(format!("保存图片`{}`失败", save_path.display()))?;
        return Ok(());
    }

    // 图像处理的闭包
    let save_path = save_path.to_path_buf();
    let process_img = move || -> anyhow::Result<()> {
        let mut src_img = image::load_from_memory(&src_img_data)
            .context("解码图片失败")?
            .to_rgb8();
        // 如果无需拼接，直接根据格式保存图片
        // 否则拼接图片
        let dst_img = if block_num == 0 {
            src_img
        } else {
            stitch_img(&mut src_img, block_num)
        };
        // 用来存图片编码后的数据
        let mut dst_img_data = Vec::new();
        match download_format {
            DownloadFormat::Jpeg => {
                dst_img.write_to(&mut Cursor::new(&mut dst_img_data), ImageFormat::Jpeg)?;
            }
            DownloadFormat::Png => {
                let encoder = PngEncoder::new_with_quality(
                    Cursor::new(&mut dst_img_data),
                    png::CompressionType::Default,
                    png::FilterType::default(),
                );
                dst_img.write_with_encoder(encoder)?;
            }
            DownloadFormat::Webp | DownloadFormat::Original => {
                // 无损 WebP：体积明显小于 PNG，且不引入有损压缩
                dst_img.write_to(&mut Cursor::new(&mut dst_img_data), ImageFormat::WebP)?;
            }
        }
        // 保存编码后的图片数据
        file_writes
            .run(|| atomic_fs::write(&save_path, dst_img_data))
            .context(format!("保存图片`{}`失败", save_path.display()))?;
        Ok(())
    };
    // 因为图像处理是CPU密集型操作，所以使用rayon并发处理
    let (sender, receiver) = tokio::sync::oneshot::channel::<anyhow::Result<()>>();
    rayon::spawn(move || {
        let _ = sender.send(process_img());
    });
    // 在tokio任务中等待rayon任务的完成，避免阻塞worker threads
    receiver.await?
}

pub(crate) fn is_valid_saved_image(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() || metadata.len() < 16 || metadata.len() > MAX_PAGE_BYTES {
        return false;
    }

    let Ok(data) = std::fs::read(path) else {
        return false;
    };
    valid_page(&data)
}

/// 图片格式对应的落盘扩展名（原始模式按源格式保存时使用）。
fn source_extension(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::Jpeg => "jpg",
        ImageFormat::Png => "png",
        ImageFormat::WebP => "webp",
        ImageFormat::Gif => "gif",
        _ => "webp",
    }
}

/// 原始模式：查找指定页码已存在的任意**有效**图片格式文件。
///
/// 同一页可能同时存在多个扩展名（旧格式残留、上次下载中断），必须返回能解码的那个；
/// 直接取 `read_dir` 的第一个可能命中断点残留的坏文件，打包时就会混入坏页。
fn find_existing_page(dir: &Path, page: usize) -> Option<PathBuf> {
    let stem = format!("{page:04}");
    let entries = std::fs::read_dir(dir).ok()?;
    let mut candidates: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path.file_stem().and_then(|s| s.to_str()) == Some(stem.as_str())
                && path
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(str::to_lowercase)
                    .is_some_and(|ext| {
                        matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "webp" | "gif")
                    })
        })
        .collect();
    // 优先最近写入的文件，再逐个校验可解码
    candidates
        .sort_by_key(|path| std::cmp::Reverse(path.metadata().and_then(|m| m.modified()).ok()));
    candidates
        .into_iter()
        .find(|path| is_valid_saved_image(path))
}

fn valid_page(bytes: &[u8]) -> bool {
    let reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format();
    reader.is_ok_and(|mut reader| {
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(30_000);
        limits.max_image_height = Some(30_000);
        limits.max_alloc = Some(256 * 1024 * 1024);
        reader.limits(limits);
        reader.decode().is_ok()
    })
}

fn read_page(reader: impl Read) -> anyhow::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.take(MAX_PAGE_BYTES + 1).read_to_end(&mut bytes)?;
    ensure!(
        u64::try_from(bytes.len())? <= MAX_PAGE_BYTES,
        "图片超过 32 MB"
    );
    Ok(bytes)
}

/// 从最终章节中只复用可完整解码的页面；页面编号保持与服务器一致。
pub(crate) fn seed_valid_pages(
    source: &Path,
    temp: &Path,
    download_format: DownloadFormat,
    expected: usize,
) -> anyhow::Result<()> {
    if !source.is_file() {
        return Ok(());
    }
    let Ok(mut zip) = zip::ZipArchive::new(std::fs::File::open(source)?) else {
        return Ok(());
    };
    for index in 0..zip.len().min(MAX_PAGES) {
        let Ok(entry) = zip.by_index(index) else {
            continue;
        };
        let name = entry.name().to_owned();
        let path = Path::new(&name);
        let Some(ext) = path.extension().and_then(|ext| ext.to_str()) else {
            continue;
        };
        let ext_matches = match download_format {
            // 原始模式复用任意图片格式
            DownloadFormat::Original => matches!(ext, "jpg" | "jpeg" | "png" | "webp" | "gif"),
            DownloadFormat::Png => ext == "png" || matches!(ext, "jpg" | "jpeg" | "webp"),
            format => ext == format.extension() || ext == "gif",
        };
        if !ext_matches {
            continue;
        }
        let Ok(page) = path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .parse::<usize>()
        else {
            continue;
        };
        if page == 0 || page > expected {
            continue;
        }
        let target_extension = if download_format == DownloadFormat::Png {
            "png"
        } else {
            ext
        };
        let target = temp.join(format!("{page:04}.{target_extension}"));
        if is_valid_saved_image(&target) {
            continue;
        }
        if let Ok(bytes) = read_page(entry) {
            if valid_page(&bytes) {
                if target_extension == "png" && image::guess_format(&bytes)? != ImageFormat::Png {
                    crate::image_data::validate_image(&bytes)?;
                    let image = image::load_from_memory(&bytes)?;
                    let mut png = Cursor::new(Vec::new());
                    image.write_to(&mut png, ImageFormat::Png)?;
                    atomic_fs::write(&target, png.into_inner())?;
                } else {
                    atomic_fs::write(&target, bytes)?;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    fn png() -> Vec<u8> {
        let mut bytes = Cursor::new(Vec::new());
        image::RgbImage::new(2, 2)
            .write_to(&mut bytes, ImageFormat::Png)
            .unwrap();
        bytes.into_inner()
    }

    #[test]
    fn retry_reuses_valid_pages_without_copying_corruption() {
        let root = std::env::temp_dir().join(format!("jm-download-reuse-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("chapter.cbz");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&source).unwrap());
        for (name, bytes) in [("0001.png", png()), ("0003.png", b"broken".to_vec())] {
            zip.start_file(name, SimpleFileOptions::default()).unwrap();
            zip.write_all(&bytes).unwrap();
        }
        zip.finish().unwrap();
        seed_valid_pages(&source, &root.join("temp"), DownloadFormat::Png, 4).unwrap();
        assert!(root.join("temp/0001.png").exists());
        assert!(!root.join("temp/0003.png").exists());
        assert!(source.exists());
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
mod png_reuse_tests {
    use super::*;
    use std::io::Write;
    #[test]
    fn old_jpeg_archive_is_preserved_and_reused_page_is_real_png() {
        let root = std::env::temp_dir().join(format!("jm-png-migrate-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("old.cbz");
        let mut jpeg = Cursor::new(Vec::new());
        image::RgbImage::new(2, 2)
            .write_to(&mut jpeg, ImageFormat::Jpeg)
            .unwrap();
        let mut archive = zip::ZipWriter::new(std::fs::File::create(&source).unwrap());
        archive
            .start_file("0001.jpg", zip::write::SimpleFileOptions::default())
            .unwrap();
        archive.write_all(&jpeg.into_inner()).unwrap();
        archive.finish().unwrap();
        let original = std::fs::read(&source).unwrap();
        seed_valid_pages(&source, &root.join("stage"), DownloadFormat::Png, 1).unwrap();
        let page = std::fs::read(root.join("stage/0001.png")).unwrap();
        assert_eq!(image::guess_format(&page).unwrap(), ImageFormat::Png);
        assert_eq!(std::fs::read(source).unwrap(), original);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
mod gif_png_tests {
    use super::*;
    #[tokio::test]
    async fn gif_source_uses_a_static_png_first_frame_under_png_policy() {
        let path = std::env::temp_dir().join(format!("jm-gif-png-{}.png", uuid::Uuid::new_v4()));
        let mut gif = Vec::new();
        image::codecs::gif::GifEncoder::new(&mut gif)
            .encode_frame(image::Frame::new(image::RgbaImage::new(2, 2)))
            .unwrap();
        save_img(
            &path,
            DownloadFormat::Png,
            20,
            Bytes::from(gif),
            ImageFormat::Gif,
            Arc::new(TaskFileWrites::default()),
        )
        .await
        .unwrap();
        assert_eq!(
            image::guess_format(&std::fs::read(&path).unwrap()).unwrap(),
            ImageFormat::Png
        );
        std::fs::remove_file(path).unwrap();
    }
}
