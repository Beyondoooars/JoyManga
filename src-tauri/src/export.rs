use std::{
    collections::{BTreeMap, HashSet},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{atomic::AtomicU32, Arc},
};

use anyhow::{anyhow, ensure, Context};

use lopdf::{
    content::{Content, Operation},
    dictionary, Bookmark, Document, Object, Stream,
};
use parking_lot::Mutex;
use rayon::iter::{IntoParallelIterator, ParallelIterator};
use tauri::AppHandle;
use tauri_specta::Event;
use zip::{write::SimpleFileOptions, ZipWriter};

use crate::{
    atomic_fs,
    events::{ExportCbzEvent, ExportPdfEvent},
    extensions::{AnyhowErrorToStringChain, AppHandleExt, PathIsImg},
    types::{ChapterInfo, Comic, ComicInfo},
    utils,
};

pub enum ExportArchive {
    Cbz,
    Pdf,
}

struct TempFileCleanup(PathBuf);

impl Drop for TempFileCleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

impl ExportArchive {
    pub fn extension(&self) -> &str {
        match self {
            ExportArchive::Cbz => "cbz",
            ExportArchive::Pdf => "pdf",
        }
    }
}

fn cbz_filename(comic: &Comic, chapter: &ChapterInfo, chapter_count: usize) -> String {
    utils::cbz_filename(
        &comic.name,
        comic.id,
        &chapter.chapter_title,
        chapter.chapter_id,
        chapter.order,
        chapter_count,
    )
}

fn merged_cbz_filename(comic: &Comic) -> String {
    let suffix = format!(" [JM{}]（合并）.cbz", comic.id);
    let title = utils::filename_filter(&comic.name);
    let title = utils::truncate_utf8_by_bytes(&title, 180_usize.saturating_sub(suffix.len()));
    format!("{}{suffix}", if title.is_empty() { "comic" } else { title })
}

fn validate_merge_sources(comic: &Comic, chapters: &[&ChapterInfo]) -> anyhow::Result<()> {
    let mut ids = HashSet::new();
    let mut single_chapter_paths = HashSet::new();
    for chapter in chapters {
        ensure!(
            comic
                .chapter_infos
                .iter()
                .any(|known| known.chapter_id == chapter.chapter_id),
            "合并章节不属于这本漫画"
        );
        ensure!(ids.insert(chapter.chapter_id), "章节记录重复，不能重复合并");
        let path = chapter
            .chapter_download_dir
            .as_ref()
            .context("章节没有本地路径")?;
        let canonical = std::fs::canonicalize(path).context("章节文件不存在")?;
        if path.is_file() {
            match crate::comic_scanner::read_chapter_identity(path)? {
                // 独立章节文件：身份必须匹配，且同一文件不能被多话共用
                Some(identity) if identity == (comic.id, chapter.chapter_id) => {
                    ensure!(
                        single_chapter_paths.insert(canonical),
                        "章节已在同一个文件中，不需要再次合并"
                    );
                }
                Some(_) => return Err(anyhow!("章节文件的漫画或话数身份不匹配")),
                // 无 chapter.json 的整本归档：允许同漫画的多个话按序切片引用同一文件
                None => {}
            }
        } else {
            ensure!(
                single_chapter_paths.insert(canonical),
                "章节已在同一个文件中，不需要再次合并"
            );
        }
    }
    Ok(())
}

fn validate_merge_target(
    comic: &Comic,
    chapters: &[&ChapterInfo],
    path: &Path,
) -> anyhow::Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let target = std::fs::canonicalize(path)?;
    for chapter in chapters {
        let source = chapter
            .chapter_download_dir
            .as_ref()
            .context("章节没有本地路径")?;
        if std::fs::canonicalize(source)? == target {
            // 允许整本归档原位重写（更新后再合并）与单话书的最终命名重写；
            // 拒绝把多话合并写进某个独立章节文件。
            let override_ok = if chapters.len() == 1 {
                crate::comic_scanner::read_chapter_identity(path)?
                    == Some((comic.id, chapter.chapter_id))
            } else {
                crate::comic_scanner::read_chapter_identity(path)?.is_none()
            };
            ensure!(override_ok, "合并文件不能覆盖原章节");
        }
    }
    let mut archive = zip::ZipArchive::new(std::fs::File::open(path)?)?;
    let mut entry = archive
        .by_name("元数据.json")
        .context("目标文件没有可确认的漫画身份，保留原文件")?;
    let size = entry.size();
    let bytes =
        crate::image_data::read_limited(&mut entry, size, crate::image_data::MAX_METADATA_BYTES)?;
    let metadata: serde_json::Value = serde_json::from_slice(&bytes)?;
    ensure!(
        metadata["id"].as_i64() == Some(comic.id),
        "目标文件属于其他漫画，保留原文件"
    );
    Ok(())
}

fn copy_cbz_if_needed(source: &Path, target: &Path) -> anyhow::Result<()> {
    if target.exists() && std::fs::canonicalize(source)? == std::fs::canonicalize(target)? {
        return Ok(());
    }
    atomic_fs::copy_file(source, target)
}

fn validate_download_paths(
    app: &AppHandle,
    comic: &Comic,
    chapters: &[&ChapterInfo],
) -> anyhow::Result<()> {
    let download_dir = app.get_config().read().download_dir.clone();
    let canonical_download_dir = std::fs::canonicalize(&download_dir)
        .context(format!("无法解析下载根目录`{}`", download_dir.display()))?;

    let comic_download_dir = comic
        .comic_download_dir
        .as_ref()
        .context("漫画的comic_download_dir字段为None")?;
    let canonical_comic_dir = std::fs::canonicalize(comic_download_dir).context(format!(
        "无法解析漫画目录`{}`",
        comic_download_dir.display()
    ))?;
    let catalog = {
        let _guard = crate::library_catalog::CATALOG_LOCK.lock();
        crate::library_catalog::LibraryCatalog::load(&crate::library_catalog::catalog_path(app))?
    };
    let imported_cbz = comic.id < 0
        && catalog.visible_imports(&download_dir).iter().any(|record| {
            record.id == comic.id
                && std::fs::canonicalize(&record.path).is_ok_and(|path| path == canonical_comic_dir)
        });
    ensure!(
        canonical_comic_dir.starts_with(&canonical_download_dir) || imported_cbz,
        "漫画目录`{}`不在下载根目录`{}`内",
        canonical_comic_dir.display(),
        canonical_download_dir.display()
    );

    for chapter in chapters {
        let chapter_dir = chapter
            .chapter_download_dir
            .as_ref()
            .context(format!("章节`{}`的下载目录为空", chapter.chapter_title))?;
        let canonical_chapter_dir = std::fs::canonicalize(chapter_dir)
            .context(format!("无法解析章节目录`{}`", chapter_dir.display()))?;
        ensure!(
            canonical_chapter_dir.starts_with(&canonical_download_dir)
                || (imported_cbz && canonical_chapter_dir == canonical_comic_dir),
            "章节目录`{}`不在下载根目录`{}`内",
            canonical_chapter_dir.display(),
            canonical_download_dir.display()
        );
    }
    Ok(())
}

struct CbzErrorEventGuard {
    uuid: String,
    app: AppHandle,
    success: bool,
}

impl Drop for CbzErrorEventGuard {
    fn drop(&mut self) {
        if self.success {
            return;
        }

        let uuid = self.uuid.clone();
        let _ = ExportCbzEvent::Error { uuid }.emit(&self.app);
    }
}

#[allow(clippy::cast_possible_wrap)]
#[allow(clippy::cast_possible_truncation)]
#[allow(clippy::too_many_lines)]
pub fn cbz(app: &AppHandle, comic: &Comic) -> anyhow::Result<()> {
    let downloaded_chapter_infos = comic
        .chapter_infos
        .iter()
        .filter(|chapter_info| chapter_info.is_downloaded.unwrap_or(false))
        .collect::<Vec<_>>();
    if downloaded_chapter_infos.is_empty() {
        return Err(anyhow!("漫画`{}`没有可导出的已下载章节", comic.name));
    }
    validate_download_paths(app, comic, &downloaded_chapter_infos)?;
    let downloaded_chapter_count = downloaded_chapter_infos.len();
    // 生成格式化的xml
    let cfg = yaserde::ser::Config {
        perform_indent: true,
        ..Default::default()
    };
    let event_uuid = uuid::Uuid::new_v4().to_string();
    // 发送开始导出cbz事件
    let _ = ExportCbzEvent::Start {
        uuid: event_uuid.clone(),
        comic_title: comic.name.clone(),
        total: downloaded_chapter_infos.len() as u32,
    }
    .emit(app);
    // 如果success为false，drop时发送Error事件
    let mut error_event_guard = CbzErrorEventGuard {
        uuid: event_uuid.clone(),
        app: app.clone(),
        success: false,
    };
    // 用来记录导出进度
    let current = Arc::new(AtomicU32::new(0));

    let extension = ExportArchive::Cbz.extension();
    let comic_export_dir = comic
        .get_comic_export_dir(app)
        .context("获取导出目录失败")?;
    let chapter_export_dir = utils::create_dir_all_beneath(&comic_export_dir, Path::new(extension))
        .context(format!(
            "在漫画导出目录`{}`下安全创建`{extension}`目录失败",
            comic_export_dir.display()
        ))?;
    // 先把封面拷贝到导出目录(如果有)
    if let Err(err) = copy_cover(comic, &chapter_export_dir) {
        let comic_title = &comic.name;
        let err_title = format!("`{comic_title}`导出cbz时，将封面拷贝到导出目录失败");
        let string_chain = err.to_string_chain();
        tracing::error!(err_title, message = string_chain);
    }
    // 并发处理
    let downloaded_chapter_infos = downloaded_chapter_infos.into_par_iter();
    downloaded_chapter_infos.try_for_each(|chapter_info| -> anyhow::Result<()> {
        let chapter_title = chapter_info.chapter_title.clone();
        let filename = cbz_filename(comic, chapter_info, downloaded_chapter_count);
        let save_path = chapter_export_dir.join(filename);

        let chapter_download_dir = chapter_info.chapter_download_dir.as_ref().context(format!(
            "章节`{chapter_title}`的`chapter_download_dir`字段为`None`"
        ))?;

        if chapter_download_dir.is_file() {
            // 身份属于本章的独立章节文件直接复制；整本合并归档则切片重新打包本话，
            // 否则每一话都会导出整本书（内容重复、占用成倍）。
            let mut archive =
                zip::ZipArchive::new(std::fs::File::open(chapter_download_dir).context(
                    format!("打开CBZ文件`{}`失败", chapter_download_dir.display()),
                )?)
                .context(format!(
                    "解析CBZ文件`{}`失败",
                    chapter_download_dir.display()
                ))?;
            let is_own_chapter_file =
                crate::comic_scanner::read_chapter_identity_from_archive(&mut archive)?.is_some();
            if is_own_chapter_file {
                copy_cbz_if_needed(chapter_download_dir, &save_path).context(format!(
                    "章节`{chapter_title}`复制CBZ到`{}`失败",
                    save_path.display()
                ))?;
                let current = current.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                let _ = ExportCbzEvent::Progress {
                    uuid: event_uuid.clone(),
                    current,
                }
                .emit(app);
                return Ok(());
            }

            let mut img_names: Vec<String> = (0..archive.len())
                .filter_map(|index| {
                    let entry = archive.by_index(index).ok()?;
                    let name = entry.name().to_string();
                    (Path::new(&name).is_img() && !name.starts_with('.') && !name.starts_with('_'))
                        .then_some(name)
                })
                .collect();
            img_names = crate::comic_scanner::select_chapter_pages(&img_names, chapter_info.order)
                .with_context(|| {
                    format!(
                        "整本归档`{}`中找不到章节`{}`的页面",
                        chapter_download_dir.display(),
                        chapter_title
                    )
                })?;
            img_names.sort_by(|a, b| utils::natural_cmp(a, b));

            let mut comic_info = ComicInfo::from(comic, chapter_info);
            comic_info.page_count = i64::try_from(img_names.len()).unwrap_or(0);
            let comic_info_xml =
                yaserde::ser::to_string_with_config(&comic_info, &cfg).map_err(|err_msg| {
                    anyhow!("章节`{chapter_title}`序列化`ComicInfo.xml`失败: {err_msg}")
                })?;

            let temp_path = atomic_fs::temp_path(&save_path)?;
            let _temp_file_cleanup = TempFileCleanup(temp_path.clone());
            let zip_file = std::fs::File::create(&temp_path).context(format!(
                "章节`{chapter_title}`创建文件`{}`失败",
                temp_path.display()
            ))?;
            let mut zip_writer = ZipWriter::new(zip_file);
            zip_writer
                .start_file("ComicInfo.xml", SimpleFileOptions::default())
                .context("在分部CBZ创建`ComicInfo.xml`条目失败")?;
            zip_writer
                .write_all(comic_info_xml.as_bytes())
                .context("写入`ComicInfo.xml`失败")?;
            for (page_idx, entry_name) in img_names.iter().enumerate() {
                let extension = Path::new(entry_name)
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .unwrap_or("jpg")
                    .to_lowercase();
                let target_name = format!("{:04}.{extension}", page_idx + 1);
                zip_writer
                    .start_file(&target_name, SimpleFileOptions::default())
                    .context(format!("在分部CBZ创建条目`{target_name}`失败"))?;
                let mut entry = archive
                    .by_name(entry_name)
                    .context(format!("读取原章节图片`{entry_name}`失败"))?;
                // 从已有归档复制时同样要保证单条目不超过 ZIP32 上限
                crate::zip_limits::ensure_archive_entry_size(&entry, entry_name)?;
                std::io::copy(&mut entry, &mut zip_writer)
                    .context(format!("将图片写入分部CBZ条目`{target_name}`失败"))?;
            }
            let zip_file = zip_writer.finish().context(format!(
                "章节`{chapter_title}`关闭`{}`失败",
                temp_path.display()
            ))?;
            zip_file
                .sync_all()
                .context(format!("同步临时CBZ`{}`失败", temp_path.display()))?;
            if let Err(err) = atomic_fs::commit_file(&temp_path, &save_path) {
                let _ = std::fs::remove_file(&temp_path);
                return Err(err).context(format!("提交CBZ`{}`失败", save_path.display()));
            }
            let current = current.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            let _ = ExportCbzEvent::Progress {
                uuid: event_uuid.clone(),
                current,
            }
            .emit(app);
            return Ok(());
        }

        // 遍历下载目录，将文件写入cbz
        let mut image_paths = std::fs::read_dir(chapter_download_dir)
            .context(format!(
                "章节`{chapter_title}`读取目录`{}`失败",
                chapter_download_dir.display()
            ))?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.is_img())
            .collect::<Vec<_>>();
        image_paths.sort_by(|a, b| a.file_name().cmp(&b.file_name()));

        let mut comic_info = ComicInfo::from(comic, chapter_info);
        comic_info.page_count = i64::try_from(image_paths.len()).unwrap_or(0);
        let comic_info_xml =
            yaserde::ser::to_string_with_config(&comic_info, &cfg).map_err(|err_msg| {
                anyhow!("章节`{chapter_title}`序列化`ComicInfo.xml`失败: {err_msg}")
            })?;

        let temp_path = atomic_fs::temp_path(&save_path)?;
        let _temp_file_cleanup = TempFileCleanup(temp_path.clone());
        let zip_file = std::fs::File::create(&temp_path).context(format!(
            "章节`{chapter_title}`创建文件`{}`失败",
            temp_path.display()
        ))?;
        let mut zip_writer = ZipWriter::new(zip_file);
        // 把ComicInfo.xml写入cbz
        zip_writer
            .start_file("ComicInfo.xml", SimpleFileOptions::default())
            .context(format!(
                "章节`{chapter_title}`在`{}`创建`ComicInfo.xml`失败",
                save_path.display()
            ))?;
        zip_writer
            .write_all(comic_info_xml.as_bytes())
            .context(format!("章节`{chapter_title}`写入`ComicInfo.xml`失败"))?;
        for image_path in image_paths {
            let filename = match image_path.file_name() {
                Some(name) => name.to_string_lossy(),
                None => continue,
            };
            // 将文件写入cbz
            crate::zip_limits::ensure_entry_size(&image_path)?;
            zip_writer
                .start_file(&filename, SimpleFileOptions::default())
                .context(format!(
                    "章节`{chapter_title}`在`{}`创建`{filename}`失败",
                    save_path.display()
                ))?;
            let mut file = std::fs::File::open(&image_path)
                .context(format!("打开`{}`失败", image_path.display()))?;
            std::io::copy(&mut file, &mut zip_writer).context(format!(
                "章节`{chapter_title}`将`{}`写入`{}`失败",
                image_path.display(),
                save_path.display()
            ))?;
        }

        let zip_file = zip_writer.finish().context(format!(
            "章节`{chapter_title}`关闭`{}`失败",
            temp_path.display()
        ))?;
        zip_file
            .sync_all()
            .context(format!("同步临时CBZ`{}`失败", temp_path.display()))?;
        if let Err(err) = atomic_fs::commit_file(&temp_path, &save_path) {
            let _ = std::fs::remove_file(&temp_path);
            return Err(err).context(format!("提交CBZ`{}`失败", save_path.display()));
        }
        // 更新导出cbz的进度
        let current = current.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        // 发送导出cbz进度事件
        let _ = ExportCbzEvent::Progress {
            uuid: event_uuid.clone(),
            current,
        }
        .emit(app);
        Ok(())
    })?;
    // 标记为成功，后面drop时就不会发送Error事件
    error_event_guard.success = true;
    // 发送导出cbz完成事件
    let _ = ExportCbzEvent::End {
        uuid: event_uuid,
        chapter_export_dir,
    }
    .emit(app);

    Ok(())
}

#[allow(clippy::cast_possible_wrap)]
#[allow(clippy::cast_possible_truncation)]
pub fn merge_cbz(app: &AppHandle, comic: &Comic) -> anyhow::Result<()> {
    let mut downloaded_chapter_infos: Vec<&ChapterInfo> = comic
        .chapter_infos
        .iter()
        .filter(|chapter_info| chapter_info.is_downloaded.unwrap_or(false))
        .collect();
    ensure!(
        downloaded_chapter_infos.len() >= 2,
        "同一本漫画至少需要两话独立的本地下载才能合并"
    );
    validate_merge_sources(comic, &downloaded_chapter_infos)?;
    downloaded_chapter_infos.sort_by_key(|c| (c.order, c.chapter_id));
    validate_download_paths(app, comic, &downloaded_chapter_infos)?;

    let total_chapters = downloaded_chapter_infos.len();
    let event_uuid = uuid::Uuid::new_v4().to_string();
    let _ = ExportCbzEvent::Start {
        uuid: event_uuid.clone(),
        comic_title: format!("{} (合并CBZ)", comic.name),
        total: total_chapters as u32,
    }
    .emit(app);

    let mut error_event_guard = CbzErrorEventGuard {
        uuid: event_uuid.clone(),
        app: app.clone(),
        success: false,
    };

    let comic_export_dir = comic
        .get_comic_export_dir(app)
        .context("获取导出目录失败")?;
    std::fs::create_dir_all(&comic_export_dir).context(format!(
        "创建漫画导出目录`{}`失败",
        comic_export_dir.display()
    ))?;

    let save_path = comic_export_dir.join(merged_cbz_filename(comic));

    let event_uuid_clone = event_uuid.clone();
    let app_handle_clone = app.clone();
    write_merged_cbz_archive(
        comic,
        &downloaded_chapter_infos,
        &save_path,
        move |current| {
            let _ = ExportCbzEvent::Progress {
                uuid: event_uuid_clone.clone(),
                current,
            }
            .emit(&app_handle_clone);
        },
    )?;

    let _ = copy_cover(comic, &comic_export_dir);

    error_event_guard.success = true;
    let _ = ExportCbzEvent::End {
        uuid: event_uuid,
        chapter_export_dir: save_path.clone(),
    }
    .emit(app);

    tracing::info!(
        comic_title = %comic.name,
        save_path = %save_path.display(),
        "已成功将漫画所有话整合为单体CBZ"
    );

    Ok(())
}

pub(crate) fn write_merged_cbz_archive(
    comic: &Comic,
    downloaded_chapter_infos: &[&ChapterInfo],
    save_path: &Path,
    mut on_progress: impl FnMut(u32),
) -> anyhow::Result<()> {
    validate_merge_sources(comic, downloaded_chapter_infos)?;
    validate_merge_target(comic, downloaded_chapter_infos, save_path)?;
    ensure!(!downloaded_chapter_infos.is_empty(), "没有可合并的章节");
    let mut downloaded_chapter_infos = downloaded_chapter_infos.to_vec();
    downloaded_chapter_infos.sort_by_key(|chapter| (chapter.order, chapter.chapter_id));
    let total_chapters = downloaded_chapter_infos.len();
    // 话目录名以 order 定位（选择/切片都依赖它）；order 重复会让两话写入同名目录而串页。
    let unique_orders: HashSet<i64> = downloaded_chapter_infos
        .iter()
        .map(|chapter| chapter.order)
        .collect();
    ensure!(
        unique_orders.len() == total_chapters,
        "存在话序号重复的章节，无法安全合并（请刷新章节列表后重试）"
    );
    let temp_path = atomic_fs::temp_path(save_path)?;
    let _temp_file_cleanup = TempFileCleanup(temp_path.clone());
    let zip_file = std::fs::File::create(&temp_path)
        .context(format!("创建临时合并CBZ文件`{}`失败", temp_path.display()))?;
    let mut zip_writer = ZipWriter::new(zip_file);

    // 写入 元数据.json
    let mut comic_clone = comic.clone();
    comic_clone.chapter_infos = downloaded_chapter_infos
        .iter()
        .map(|chapter| (**chapter).clone())
        .collect();
    comic_clone.is_favorite = false;
    comic_clone.liked = false;
    comic_clone.is_downloaded = None;
    comic_clone.comic_download_dir = None;
    comic_clone.local_cover_data = None;
    for ch in &mut comic_clone.chapter_infos {
        ch.is_downloaded = None;
        ch.chapter_download_dir = None;
    }
    let comic_json = serde_json::to_string_pretty(&comic_clone)
        .map_err(|err| anyhow!("序列化`元数据.json`失败: {err}"))?;
    zip_writer
        .start_file("元数据.json", SimpleFileOptions::default())
        .context("在合并CBZ创建`元数据.json`条目失败")?;
    zip_writer
        .write_all(comic_json.as_bytes())
        .context("写入`元数据.json`失败")?;

    let pad_width = if total_chapters > 99 { 3 } else { 2 };
    // 合并包的总页数（各话页数之和），写入 ComicInfo 的 PageCount
    let mut total_pages: i64 = 0;

    for (chapter_idx, chapter_info) in downloaded_chapter_infos.iter().enumerate() {
        let chapter_download_dir = chapter_info.chapter_download_dir.as_ref().context(format!(
            "章节`{}`的`chapter_download_dir`字段为`None`",
            chapter_info.chapter_title
        ))?;

        let chapter_dir_name = format!(
            "[{:0width$}] {}",
            chapter_info.order,
            utils::filename_filter(&chapter_info.chapter_title),
            width = pad_width
        );

        if chapter_download_dir.is_file() {
            let file = std::fs::File::open(chapter_download_dir).context(format!(
                "打开章节CBZ`{}`失败",
                chapter_download_dir.display()
            ))?;
            let mut archive = zip::ZipArchive::new(file).context(format!(
                "解析章节CBZ`{}`失败",
                chapter_download_dir.display()
            ))?;

            let mut img_names: Vec<String> = (0..archive.len())
                .filter_map(|i| {
                    let entry = archive.by_index(i).ok()?;
                    let name = entry.name().to_string();
                    if Path::new(&name).is_img() && !name.starts_with('.') && !name.starts_with('_')
                    {
                        Some(name)
                    } else {
                        None
                    }
                })
                .collect();
            // 来源是独立章节文件时使用全部图片；来源是整本归档（无本章身份）时按话切片，
            // 避免把其他话的页面重复写入合并结果。
            let is_own_chapter_file =
                crate::comic_scanner::read_chapter_identity_from_archive(&mut archive)?
                    == Some((comic.id, chapter_info.chapter_id));
            if !is_own_chapter_file {
                img_names =
                    crate::comic_scanner::select_chapter_pages(&img_names, chapter_info.order)
                        .with_context(|| {
                            format!(
                                "整本归档`{}`中找不到章节`{}`的页面",
                                chapter_download_dir.display(),
                                chapter_info.chapter_title
                            )
                        })?;
            }
            img_names.sort_by(|a, b| utils::natural_cmp(a, b));
            total_pages += i64::try_from(img_names.len()).unwrap_or(0);

            for (page_idx, entry_name) in img_names.iter().enumerate() {
                let ext = Path::new(entry_name)
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("jpg")
                    .to_lowercase();
                let target_entry_name = if total_chapters == 1 {
                    format!("{:04}.{ext}", page_idx + 1)
                } else {
                    format!("{chapter_dir_name}/{:04}.{ext}", page_idx + 1)
                };

                zip_writer
                    .start_file(&target_entry_name, SimpleFileOptions::default())
                    .context(format!("在合并CBZ创建条目`{target_entry_name}`失败"))?;
                let mut entry = archive
                    .by_name(entry_name)
                    .context(format!("读取原章节图片`{entry_name}`失败"))?;
                crate::zip_limits::ensure_archive_entry_size(&entry, entry_name)?;
                std::io::copy(&mut entry, &mut zip_writer)
                    .context(format!("将图片写入合并CBZ条目`{target_entry_name}`失败"))?;
            }
        } else if chapter_download_dir.is_dir() {
            let mut image_paths: Vec<PathBuf> = std::fs::read_dir(chapter_download_dir)
                .context(format!(
                    "读取章节目录`{}`失败",
                    chapter_download_dir.display()
                ))?
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.is_img())
                .collect();
            image_paths.sort_by(|a, b| {
                let name_a = a.file_name().and_then(|n| n.to_str()).unwrap_or("");
                let name_b = b.file_name().and_then(|n| n.to_str()).unwrap_or("");
                utils::natural_cmp(name_a, name_b)
            });
            total_pages += i64::try_from(image_paths.len()).unwrap_or(0);

            for (page_idx, img_path) in image_paths.into_iter().enumerate() {
                let ext = img_path
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("jpg")
                    .to_lowercase();
                let target_entry_name = if total_chapters == 1 {
                    format!("{:04}.{ext}", page_idx + 1)
                } else {
                    format!("{chapter_dir_name}/{:04}.{ext}", page_idx + 1)
                };

                crate::zip_limits::ensure_entry_size(&img_path)?;
                zip_writer
                    .start_file(&target_entry_name, SimpleFileOptions::default())
                    .context(format!("在合并CBZ创建条目`{target_entry_name}`失败"))?;
                let mut file = std::fs::File::open(&img_path)
                    .context(format!("打开图片`{}`失败", img_path.display()))?;
                std::io::copy(&mut file, &mut zip_writer)
                    .context(format!("将图片写入合并CBZ条目`{target_entry_name}`失败"))?;
            }
        }

        on_progress((chapter_idx + 1) as u32);
    }

    // 图片写完后才能确定总页数：在归档尾部补写 ComicInfo.xml（阅读器按名称查找，位置无关）。
    // 合并包在第三方阅读器里是"一本书一话"，PageCount 取整本页数。
    let mut merged_comic_info = ComicInfo::from(comic, downloaded_chapter_infos[0]);
    merged_comic_info.title = comic.name.clone();
    merged_comic_info.number = None;
    merged_comic_info.count = total_chapters as i64;
    merged_comic_info.page_count = total_pages;
    let cfg = yaserde::ser::Config {
        perform_indent: true,
        ..Default::default()
    };
    let comic_info_xml = yaserde::ser::to_string_with_config(&merged_comic_info, &cfg)
        .map_err(|err_msg| anyhow!("序列化`ComicInfo.xml`失败: {err_msg}"))?;
    zip_writer
        .start_file("ComicInfo.xml", SimpleFileOptions::default())
        .context("在合并CBZ创建`ComicInfo.xml`条目失败")?;
    zip_writer
        .write_all(comic_info_xml.as_bytes())
        .context("写入`ComicInfo.xml`失败")?;

    let zip_file = zip_writer
        .finish()
        .context(format!("关闭临时合并CBZ`{}`失败", temp_path.display()))?;
    zip_file
        .sync_all()
        .context(format!("同步临时合并CBZ`{}`失败", temp_path.display()))?;

    if let Err(err) = atomic_fs::commit_file_checked(&temp_path, save_path, |existing| {
        validate_merge_target(comic, &downloaded_chapter_infos, existing)
    }) {
        let _ = std::fs::remove_file(&temp_path);
        return Err(err).context(format!("提交合并CBZ`{}`失败", save_path.display()));
    }

    Ok(())
}

fn copy_cover(comic: &Comic, chapter_export_dir: &Path) -> anyhow::Result<()> {
    let src_cover_path = comic.get_cover_path().context("获取封面路径失败")?;
    let cover_filename = src_cover_path.file_name().context("获取封面的文件名失败")?;

    if src_cover_path.exists() {
        let dst_cover_path = chapter_export_dir.join(cover_filename);
        let cover_data = std::fs::read(&src_cover_path)
            .context(format!("读取封面`{}`失败", src_cover_path.display()))?;
        atomic_fs::write(&dst_cover_path, cover_data)
            .context(format!("写入封面`{}`失败", dst_cover_path.display()))?;
    }

    Ok(())
}

struct PdfCreateErrorEventGuard {
    uuid: String,
    app: AppHandle,
    success: bool,
}

impl Drop for PdfCreateErrorEventGuard {
    fn drop(&mut self) {
        if self.success {
            return;
        }

        let uuid = self.uuid.clone();
        let _ = ExportPdfEvent::CreateError { uuid }.emit(&self.app);
    }
}

struct PdfMergeErrorEventGuard {
    uuid: String,
    app: AppHandle,
    success: bool,
}

impl Drop for PdfMergeErrorEventGuard {
    fn drop(&mut self) {
        if self.success {
            return;
        }

        let uuid = self.uuid.clone();
        let _ = ExportPdfEvent::MergeError { uuid }.emit(&self.app);
    }
}

#[allow(clippy::cast_possible_truncation)]
pub fn pdf(app: &AppHandle, comic: &Comic) -> anyhow::Result<()> {
    let downloaded_chapter_infos: Vec<&ChapterInfo> = comic
        .chapter_infos
        .iter()
        .filter(|chapter_info| chapter_info.is_downloaded.unwrap_or(false))
        .collect();
    if downloaded_chapter_infos.is_empty() {
        return Err(anyhow!("漫画`{}`没有可导出的已下载章节", comic.name));
    }
    validate_download_paths(app, comic, &downloaded_chapter_infos)?;
    // 整本归档被多话引用时需要用 cbz 命名规则区分每一话的 PDF 文件名
    let downloaded_chapter_count = downloaded_chapter_infos.len();
    let event_uuid = uuid::Uuid::new_v4().to_string();
    // 发送开始创建pdf事件
    let _ = ExportPdfEvent::CreateStart {
        uuid: event_uuid.clone(),
        comic_title: comic.name.clone(),
        total: downloaded_chapter_infos.len() as u32,
    }
    .emit(app);
    // 如果success为false，drop时发送CreateError事件
    let mut create_error_event_guard = PdfCreateErrorEventGuard {
        uuid: event_uuid.clone(),
        app: app.clone(),
        success: false,
    };
    // 用来记录创建pdf的进度
    let current = Arc::new(AtomicU32::new(0));

    let extension = ExportArchive::Pdf.extension();
    let comic_export_dir = comic
        .get_comic_export_dir(app)
        .context("获取导出目录失败")?;
    let chapter_export_dir = utils::create_dir_all_beneath(&comic_export_dir, Path::new(extension))
        .context(format!(
            "在漫画导出目录`{}`下安全创建`{extension}`目录失败",
            comic_export_dir.display()
        ))?;
    let chapter_with_pdf_path = Mutex::new(Vec::new());
    // 并发处理
    let downloaded_chapter_infos = downloaded_chapter_infos.into_par_iter();
    downloaded_chapter_infos.try_for_each(|chapter_info| -> anyhow::Result<()> {
        let chapter_title = &chapter_info.chapter_title;

        let chapter_download_dir = chapter_info.chapter_download_dir.as_ref().context(format!(
            "章节`{chapter_title}`的`chapter_download_dir`字段为`None`"
        ))?;
        let (image_paths, temp_cleanup_dir) = if chapter_download_dir.is_file() {
            let temp_dir = chapter_export_dir.join(format!(".tmp-pdf-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&temp_dir)
                .context(format!("创建临时提取目录`{}`失败", temp_dir.display()))?;
            let file = std::fs::File::open(chapter_download_dir).context(format!(
                "打开CBZ文件`{}`失败",
                chapter_download_dir.display()
            ))?;
            let mut archive = zip::ZipArchive::new(file).context(format!(
                "解析CBZ文件`{}`失败",
                chapter_download_dir.display()
            ))?;

            let mut img_names: Vec<String> = (0..archive.len())
                .filter_map(|i| {
                    let entry = archive.by_index(i).ok()?;
                    let name = entry.name().to_string();
                    if Path::new(&name).is_common_img()
                        && !name.starts_with('.')
                        && !name.starts_with('_')
                    {
                        Some(name)
                    } else {
                        None
                    }
                })
                .collect();
            // 来源是整本合并归档时只取本话页面，否则每话都会把整本书导入 PDF
            let is_own_chapter_file =
                crate::comic_scanner::read_chapter_identity_from_archive(&mut archive)?.is_some();
            if !is_own_chapter_file {
                img_names =
                    crate::comic_scanner::select_chapter_pages(&img_names, chapter_info.order)
                        .with_context(|| {
                            format!(
                                "整本归档`{}`中找不到章节`{}`的页面",
                                chapter_download_dir.display(),
                                chapter_title
                            )
                        })?;
            }
            img_names.sort_by(|a, b| utils::natural_cmp(a, b));

            let mut paths = Vec::new();
            for (page_idx, name) in img_names.iter().enumerate() {
                let mut entry = archive
                    .by_name(name)
                    .context(format!("读取原章节图片`{name}`失败"))?;
                // 统一成序号文件名：整本归档里不同话的文件名可能相同，直接按原名落盘会互相覆盖
                let extension = Path::new(name)
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .unwrap_or("jpg");
                let out_path = temp_dir.join(format!("{:04}.{extension}", page_idx + 1));
                let mut out_file = std::fs::File::create(&out_path)
                    .context(format!("创建临时图片文件`{}`失败", out_path.display()))?;
                std::io::copy(&mut entry, &mut out_file)
                    .context(format!("提取图片`{name}`失败"))?;
                paths.push(out_path);
            }
            (paths, Some(temp_dir))
        } else {
            let mut image_paths: Vec<PathBuf> = std::fs::read_dir(chapter_download_dir)
                .context(format!(
                    "章节`{chapter_title}`读取目录`{}`失败",
                    chapter_download_dir.display()
                ))?
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| path.is_common_img())
                .collect();
            // PDF（lopdf）只支持 jpg/png 等格式；gif 等会被跳过，日志提示避免"静默缺页"
            if let Ok(all_files) = std::fs::read_dir(chapter_download_dir) {
                let skipped = all_files
                    .filter_map(Result::ok)
                    .map(|entry| entry.path())
                    .filter(|path| path.is_img() && !path.is_common_img())
                    .count();
                if skipped > 0 {
                    tracing::warn!(chapter_title, skipped, "部分图片格式无法写入PDF，已跳过");
                }
            }
            image_paths.sort_by(|a, b| {
                let na = a.file_name().and_then(|n| n.to_str()).unwrap_or("");
                let nb = b.file_name().and_then(|n| n.to_str()).unwrap_or("");
                utils::natural_cmp(na, nb)
            });
            (image_paths, None)
        };

        let chapter_download_dir_name = &chapter_info
            .get_chapter_download_dir_name()
            .context(format!("章节`{chapter_title}`获取章节下载目录名失败"))?;
        // 整本归档被多话引用时，来源文件名完全相同；用现有 cbz 命名规则区分每一话
        let chapter_file_stem = if chapter_download_dir.is_file() {
            cbz_filename(comic, chapter_info, downloaded_chapter_count)
                .trim_end_matches(".cbz")
                .to_string()
        } else {
            Path::new(chapter_download_dir_name)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(chapter_download_dir_name)
                .to_string()
        };
        // 创建pdf
        let save_path = chapter_export_dir.join(format!("{chapter_file_stem}.{extension}"));

        let pdf_result = create_pdf(image_paths, &save_path)
            .context(format!("章节`{chapter_title}`创建pdf失败"));

        if let Some(dir) = temp_cleanup_dir {
            let _ = std::fs::remove_dir_all(dir);
        }

        pdf_result?;

        chapter_with_pdf_path.lock().push((chapter_info, save_path));
        // 更新创建pdf的进度
        let current = current.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        // 发送创建pdf进度事件
        let _ = ExportPdfEvent::CreateProgress {
            uuid: event_uuid.clone(),
            current,
        }
        .emit(app);
        Ok(())
    })?;

    let mut chapter_with_pdf_path = std::mem::take(&mut *chapter_with_pdf_path.lock());
    chapter_with_pdf_path.sort_by_key(|(chapter, _)| chapter.order);
    let chapter_pdf_paths: Vec<PathBuf> = chapter_with_pdf_path
        .into_iter()
        .map(|(_, pdf_path)| pdf_path)
        .collect();

    // 标记为成功，后面drop时就不会发送CreateError事件
    create_error_event_guard.success = true;
    // 发送创建pdf完成事件
    let _ = ExportPdfEvent::CreateEnd {
        uuid: event_uuid,
        chapter_export_dir,
    }
    .emit(app);

    let event_uuid = uuid::Uuid::new_v4().to_string();
    // 发送开始合并pdf事件
    let _ = ExportPdfEvent::MergeStart {
        uuid: event_uuid.clone(),
        comic_title: comic.name.clone(),
    }
    .emit(app);
    // 如果success为false，drop时发送MergeError事件
    let mut merge_error_event_guard = PdfMergeErrorEventGuard {
        uuid: event_uuid.clone(),
        app: app.clone(),
        success: false,
    };

    // 整本归档的“漫画目录”其实是一个 CBZ 文件，file_name 会带上 .cbz 扩展名，
    // 直接用会把残缺名字拼进合并 PDF（xxx.cbz.pdf）；统一用漫画名 + ID 命名。
    let save_path = comic_export_dir.join(format!(
        "{} [JM{}].{extension}",
        utils::filename_filter(&comic.name),
        comic.id
    ));
    merge_pdf(chapter_pdf_paths, &save_path).context("合并pdf失败")?;
    // 标记为成功，后面drop时就不会发送MergeError事件
    merge_error_event_guard.success = true;
    // 发送合并pdf完成事件
    let _ = ExportPdfEvent::MergeEnd {
        uuid: event_uuid,
        chapter_export_dir: save_path,
    }
    .emit(app);
    Ok(())
}

/// 用`image_paths`中的图片按顺序创建PDF，保存到`save_path`中
#[allow(clippy::similar_names)]
#[allow(clippy::cast_possible_truncation)]
fn create_pdf(image_paths: Vec<PathBuf>, save_path: &Path) -> anyhow::Result<()> {
    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let mut page_ids = vec![];
    // 页数校验：源图缺失或无法读取时明确失败，避免静默产出缺页 PDF
    let expected_pages = image_paths.len();

    for image_path in image_paths {
        if !image_path.is_file() {
            return Err(anyhow!("PDF源图片`{}`不存在", image_path.display()));
        }

        let buffer = read_image_to_buffer(&image_path)
            .context(format!("将`{}`读取到buffer失败", image_path.display()))?;
        let (width, height) = image::image_dimensions(&image_path)
            .context(format!("获取`{}`的尺寸失败", image_path.display()))?;
        let image_stream = image_to_pdf_stream(buffer)
            .context(format!("创建`{}`的图片流失败", image_path.display()))?;
        // 将图片流添加到doc中
        let img_id = doc.add_object(image_stream);
        // 图片的名称，用于 Do 操作在页面上显示图片
        let img_name = format!("X{}", img_id.0);
        // 用于设置图片在页面上的位置和大小
        let cm_operation = Operation::new(
            "cm",
            vec![
                width.into(),
                0.into(),
                0.into(),
                height.into(),
                0.into(),
                0.into(),
            ],
        );
        // 用于显示图片
        let do_operation = Operation::new("Do", vec![Object::Name(img_name.as_bytes().to_vec())]);
        // 创建页面，设置图片的位置和大小，然后显示图片
        // 因为是从零开始创建PDF，所以没必要用 q 和 Q 操作保存和恢复图形状态
        let content = Content {
            operations: vec![cm_operation, do_operation],
        };
        let content_id = doc.add_object(Stream::new(dictionary! {}, content.encode()?));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "Contents" => content_id,
            "MediaBox" => vec![0.into(), 0.into(), width.into(), height.into()],
        });
        // 将图片以 XObject 的形式添加到文档中
        // Do 操作只能引用 XObject(所以前面定义的 Do 操作的参数是 img_name, 而不是 img_id)
        doc.add_xobject(page_id, img_name.as_bytes(), img_id)?;
        // 记录新创建的页面的 ID
        page_ids.push(page_id);
    }
    ensure!(
        page_ids.len() == expected_pages,
        "PDF页数与源图片数量不一致（{}/{expected_pages}）",
        page_ids.len()
    );
    // 将"Pages"添加到doc中
    let pages_dict = dictionary! {
        "Type" => "Pages",
        "Count" => page_ids.len() as u32,
        "Kids" => page_ids.into_iter().map(Object::Reference).collect::<Vec<_>>(),
    };
    doc.objects.insert(pages_id, Object::Dictionary(pages_dict));
    // 新建一个"Catalog"对象，将"Pages"对象添加到"Catalog"对象中，然后将"Catalog"对象添加到doc中
    let catalog_id = doc.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    });
    doc.trailer.set("Root", catalog_id);

    doc.compress();

    save_document_atomically(&mut doc, save_path)?;

    Ok(())
}

/// 读取`image_path`中的图片数据到buffer中
fn read_image_to_buffer(image_path: &Path) -> anyhow::Result<Vec<u8>> {
    let file =
        std::fs::File::open(image_path).context(format!("打开`{}`失败", image_path.display()))?;
    let mut reader = std::io::BufReader::new(file);
    let mut buffer = vec![];
    reader
        .read_to_end(&mut buffer)
        .context(format!("读取`{}`失败", image_path.display()))?;
    Ok(buffer)
}

/// 将图片字节转换为 PDF 图片 XObject。
///
/// JPEG 直接以 `DCTDecode` 原样嵌入（无需解码，速度快）；其余格式用 `image` 解码后
/// 按像素写入并 Flate 压缩。只依赖本项目已启用的 jpeg/png/webp/gif 解码器，
/// 不使用 lopdf 的 `embed_image` 特性（它会连带引入 tiff/exr/avif 等全套编解码器，
/// 显著增加体积与 Android 交叉编译风险）。
#[allow(clippy::cast_possible_truncation)]
fn image_to_pdf_stream(buffer: Vec<u8>) -> anyhow::Result<Stream> {
    let format = image::guess_format(&buffer).context("无法识别图片格式")?;

    let mut dict = dictionary! {
        "Type" => "XObject",
        "Subtype" => "Image",
    };

    if format == image::ImageFormat::Jpeg {
        let (width, height) = image::ImageReader::new(std::io::Cursor::new(&buffer))
            .with_guessed_format()
            .context("读取 JPEG 格式失败")?
            .into_dimensions()
            .context("读取 JPEG 尺寸失败")?;
        dict.set("Width", width);
        dict.set("Height", height);
        dict.set("ColorSpace", "DeviceRGB");
        dict.set("BitsPerComponent", 8);
        dict.set("Filter", "DCTDecode");
        return Ok(Stream::new(dict, buffer));
    }

    let img = image::load_from_memory_with_format(&buffer, format).context("解码图片失败")?;
    let (width, height) = (img.width(), img.height());
    let color = img.color();

    // PDF 不支持 alpha 通道，统一转成 Gray/RGB；16 位按大端字节序写入
    let (bits, color_space, content): (u32, &str, Vec<u8>) = match color {
        image::ColorType::L8 => (8, "DeviceGray", img.into_luma8().into_raw()),
        image::ColorType::La8 => (8, "DeviceGray", img.into_luma8().into_raw()),
        image::ColorType::Rgb8 => (8, "DeviceRGB", img.into_rgb8().into_raw()),
        image::ColorType::Rgba8 => (8, "DeviceRGB", img.into_rgb8().into_raw()),
        image::ColorType::L16 => (
            16,
            "DeviceGray",
            img.into_luma16()
                .into_raw()
                .iter()
                .flat_map(|pixel| pixel.to_be_bytes())
                .collect(),
        ),
        image::ColorType::La16 => (
            16,
            "DeviceGray",
            img.into_luma16()
                .into_raw()
                .iter()
                .flat_map(|pixel| pixel.to_be_bytes())
                .collect(),
        ),
        image::ColorType::Rgb16 => (
            16,
            "DeviceRGB",
            img.into_rgb16()
                .into_raw()
                .iter()
                .flat_map(|pixel| pixel.to_be_bytes())
                .collect(),
        ),
        image::ColorType::Rgba16 => (
            16,
            "DeviceRGB",
            img.into_rgb16()
                .into_raw()
                .iter()
                .flat_map(|pixel| pixel.to_be_bytes())
                .collect(),
        ),
        other => return Err(anyhow!("PDF 不支持的图片颜色类型：{other:?}")),
    };

    dict.set("Width", width);
    dict.set("Height", height);
    dict.set("ColorSpace", color_space);
    dict.set("BitsPerComponent", bits);
    let mut stream = Stream::new(dict, content);
    // 压缩失败时保留未压缩内容即可，不影响 PDF 有效性
    let _ = stream.compress();
    Ok(stream)
}

/// 将`chapter_pdf_paths`中的PDF按顺序合并成一个，保存到`save_path`中
#[allow(clippy::cast_possible_truncation)]
fn merge_pdf(chapter_pdf_paths: Vec<PathBuf>, save_path: &Path) -> anyhow::Result<()> {
    let mut doc = Document::with_version("1.5");
    let mut doc_page_ids = vec![];
    let mut doc_objects = BTreeMap::new();

    for chapter_pdf_path in chapter_pdf_paths {
        let mut chapter_doc = Document::load(&chapter_pdf_path)
            .context(format!("加载`{}`失败", chapter_pdf_path.display()))?;
        // 重新编号这个章节PDF的对象，避免与doc的对象编号冲突
        chapter_doc.renumber_objects_with(doc.max_id);
        doc.max_id = chapter_doc.max_id + 1;
        // 获取这个章节PDF中的所有页面，并给第一个页面添加书签
        let mut chapter_page_ids = vec![];
        for (page_num, object_id) in chapter_doc.get_pages() {
            // 第一个页面需要添加书签
            if page_num == 1 {
                let chapter_title = chapter_pdf_path
                    .file_stem()
                    .context(format!("获取`{}`的文件名失败", chapter_pdf_path.display()))?
                    .to_string_lossy()
                    .to_string();
                let bookmark = Bookmark::new(chapter_title, [0.0, 0.0, 1.0], 0, object_id);
                doc.add_bookmark(bookmark, None);
            }
            chapter_page_ids.push(object_id);
        }

        doc_page_ids.extend(chapter_page_ids);
        doc_objects.extend(chapter_doc.objects);
    }
    // 在doc中新建一个"Pages"对象，将所有章节的页面添加到这个"Pages"对象中
    let pages_id = doc.add_object(dictionary! {
        "Type" => "Pages",
        "Count" => doc_page_ids.len() as u32,
        "Kids" => doc_page_ids.into_iter().map(Object::Reference).collect::<Vec<_>>(),
    });

    for (object_id, mut object) in doc_objects {
        match object.type_name().unwrap_or("") {
            "Page" => {
                if let Ok(page_dict) = object.as_dict_mut() {
                    // 将页面对象的"Parent"字段设置为新建的"Pages"对象，这样这个页面就成为了"Pages"对象的子页面
                    page_dict.set("Parent", pages_id);
                    doc.objects.insert(object_id, object);
                };
            }
            // 忽略这些对象
            "Catalog" | "Pages" | "Outlines" | "Outline" => {}
            // 将所有其他对象添加到doc中
            _ => {
                doc.objects.insert(object_id, object);
            }
        }
    }
    // 新建一个"Catalog"对象，将"Pages"对象添加到"Catalog"对象中，然后将"Catalog"对象添加到doc中
    let catalog_id = doc.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    });
    doc.trailer.set("Root", catalog_id);
    // 如果有书签没有关联到具体页面，将这些书签指向第一个页面
    doc.adjust_zero_pages();
    // 将书签添加到doc中
    if let Some(outline_id) = doc.build_outline() {
        if let Ok(Object::Dictionary(catalog_dict)) = doc.get_object_mut(catalog_id) {
            catalog_dict.set("Outlines", Object::Reference(outline_id));
        }
    }
    // 重新编号doc的对象
    doc.renumber_objects();

    doc.compress();

    save_document_atomically(&mut doc, save_path)?;
    Ok(())
}

fn save_document_atomically(doc: &mut Document, save_path: &Path) -> anyhow::Result<()> {
    let temp_path = atomic_fs::temp_path(save_path)?;
    let result = (|| -> anyhow::Result<()> {
        let file = doc
            .save(&temp_path)
            .context(format!("保存临时PDF`{}`失败", temp_path.display()))?;
        file.sync_all()
            .context(format!("同步临时PDF`{}`失败", temp_path.display()))?;
        atomic_fs::commit_file(&temp_path, save_path)
            .context(format!("提交PDF`{}`失败", save_path.display()))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp_path);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::{
        cbz_filename, copy_cbz_if_needed, image_to_pdf_stream, merged_cbz_filename,
        write_merged_cbz_archive,
    };
    use crate::types::{ChapterInfo, Comic};
    use std::{
        io::{Read, Write},
        path::Path,
    };
    use zip::{write::SimpleFileOptions, ZipWriter};

    fn chapter(title: &str, id: i64, order: i64) -> ChapterInfo {
        ChapterInfo {
            chapter_id: id,
            chapter_title: title.to_string(),
            order,
            is_downloaded: Some(true),
            chapter_download_dir: None,
        }
    }

    #[test]
    fn single_chapter_cbz_uses_comic_name() {
        let comic = Comic {
            id: 42,
            name: "本子：名称".to_string(),
            ..Comic::default()
        };

        assert_eq!(
            cbz_filename(&comic, &chapter("第1话", 1001, 1), 1),
            "本子：名称.cbz"
        );
    }

    #[test]
    fn multiple_chapter_cbz_names_include_comic_order_and_chapter() {
        let comic = Comic {
            id: 42,
            name: "书/名".to_string(),
            ..Comic::default()
        };

        assert_eq!(
            cbz_filename(&comic, &chapter("第2话/特别篇", 1002, 2), 3),
            "书 名 - 0002 - 第2话 特别篇.cbz"
        );
    }

    #[test]
    fn empty_filtered_names_fall_back_to_ids() {
        let comic = Comic {
            id: 42,
            name: "...".to_string(),
            ..Comic::default()
        };

        assert_eq!(
            cbz_filename(&comic, &chapter("...", 1002, 2), 3),
            "comic-42 - 0002 - chapter-1002.cbz"
        );
    }

    #[test]
    fn merge_cbz_combines_chapters_into_single_archive_preserving_sources() {
        use std::io::Write;
        use zip::write::SimpleFileOptions;

        let test_dir = std::env::temp_dir().join(format!("jm_merge_test_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&test_dir).unwrap();

        let ch1_cbz_path = test_dir.join("ch1.cbz");
        let ch2_cbz_path = test_dir.join("ch2.cbz");

        // Create mock ch1.cbz
        {
            let file = std::fs::File::create(&ch1_cbz_path).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            zip.start_file("0001.jpg", SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"fake image 1").unwrap();
            zip.finish().unwrap();
        }

        // Create mock ch2.cbz
        {
            let file = std::fs::File::create(&ch2_cbz_path).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            zip.start_file("0001.png", SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"fake image 2").unwrap();
            zip.finish().unwrap();
        }

        let ch1 = ChapterInfo {
            chapter_id: 101,
            chapter_title: "第1话".to_string(),
            order: 1,
            is_downloaded: Some(true),
            chapter_download_dir: Some(ch1_cbz_path.clone()),
        };
        let ch2 = ChapterInfo {
            chapter_id: 102,
            chapter_title: "第2话".to_string(),
            order: 2,
            is_downloaded: Some(true),
            chapter_download_dir: Some(ch2_cbz_path.clone()),
        };

        let comic = Comic {
            id: 1234,
            name: "测试漫画".to_string(),
            chapter_infos: vec![
                ch1.clone(),
                ch2.clone(),
                ChapterInfo {
                    chapter_id: 103,
                    chapter_title: "第3话".into(),
                    order: 3,
                    is_downloaded: Some(false),
                    chapter_download_dir: None,
                },
            ],
            ..Comic::default()
        };

        let before1 = std::fs::read(&ch1_cbz_path).unwrap();
        let before2 = std::fs::read(&ch2_cbz_path).unwrap();
        let save_path = test_dir.join("测试漫画.cbz");
        let mut progress_count = 0;
        super::write_merged_cbz_archive(&comic, &[&ch2, &ch1], &save_path, |p| {
            progress_count = p;
        })
        .unwrap();

        assert_eq!(progress_count, 2);
        assert!(save_path.is_file());

        // 原章节文件必须保留 (2不需要：不删除原分话文件)
        assert!(ch1_cbz_path.is_file());
        assert!(ch2_cbz_path.is_file());

        let file = std::fs::File::open(&save_path).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        let entry_names: Vec<String> = (0..archive.len())
            .map(|i| archive.by_index(i).unwrap().name().to_string())
            .collect();

        assert!(entry_names.contains(&"ComicInfo.xml".to_string()));
        assert!(entry_names.contains(&"元数据.json".to_string()));
        assert!(entry_names.contains(&"[01] 第1话/0001.jpg".to_string()));
        assert!(entry_names.contains(&"[02] 第2话/0001.png".to_string()));
        let mut metadata = String::new();
        archive
            .by_name("元数据.json")
            .unwrap()
            .read_to_string(&mut metadata)
            .unwrap();
        let metadata: serde_json::Value = serde_json::from_str(&metadata).unwrap();
        assert_eq!(metadata["chapterInfos"].as_array().unwrap().len(), 2);
        assert_eq!(metadata["chapterInfos"][0]["chapterId"], 101);
        let mut xml = String::new();
        archive
            .by_name("ComicInfo.xml")
            .unwrap()
            .read_to_string(&mut xml)
            .unwrap();
        assert!(xml.contains("<Count>2</Count>"));
        assert_eq!(std::fs::read(&ch1_cbz_path).unwrap(), before1);
        assert_eq!(std::fs::read(&ch2_cbz_path).unwrap(), before2);

        let _ = std::fs::remove_dir_all(&test_dir);
    }
    #[test]
    fn merge_allows_single_chapter_but_rejects_cross_comic_and_source_replacement() {
        let root = std::env::temp_dir().join(format!("jm-merge-guards-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let make = |path: &Path, comic_id: i64, chapter_id: i64| {
            let mut zip = ZipWriter::new(std::fs::File::create(path).unwrap());
            zip.start_file("chapter.json", SimpleFileOptions::default())
                .unwrap();
            zip.write_all(
                serde_json::to_string(
                    &serde_json::json!({"comicId":comic_id,"chapterId":chapter_id}),
                )
                .unwrap()
                .as_bytes(),
            )
            .unwrap();
            zip.start_file("元数据.json", SimpleFileOptions::default())
                .unwrap();
            zip.write_all(
                serde_json::to_string(&serde_json::json!({"id":comic_id}))
                    .unwrap()
                    .as_bytes(),
            )
            .unwrap();
            zip.start_file("0001.png", SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"page").unwrap();
            zip.finish().unwrap();
        };
        let a = root.join("a.cbz");
        let b = root.join("b.cbz");
        make(&a, 10, 101);
        make(&b, 11, 102);
        let ch1 = ChapterInfo {
            chapter_id: 101,
            chapter_title: "one".into(),
            order: 1,
            is_downloaded: Some(true),
            chapter_download_dir: Some(a.clone()),
        };
        let mut ch2 = ChapterInfo {
            chapter_id: 102,
            chapter_title: "two".into(),
            order: 2,
            is_downloaded: Some(true),
            chapter_download_dir: Some(b.clone()),
        };
        let comic = Comic {
            id: 10,
            chapter_infos: vec![ch1.clone(), ch2.clone()],
            ..Comic::default()
        };
        let output = root.join(merged_cbz_filename(&comic));
        // 单话写入是合法操作（单话漫画的自动整合路径）
        assert!(write_merged_cbz_archive(&comic, &[&ch1], &output, |_| {}).is_ok());
        assert!(output.exists());
        // 跨漫画来源必须拒绝（b 属于漫画 11）
        assert!(write_merged_cbz_archive(&comic, &[&ch1, &ch2], &output, |_| {}).is_err());
        // 两个话指向同一份独立章节文件必须拒绝
        make(&b, 10, 102);
        let source = std::fs::read(&a).unwrap();
        assert!(write_merged_cbz_archive(&comic, &[&ch1, &ch2], &a, |_| {}).is_err());
        assert_eq!(std::fs::read(&a).unwrap(), source);
        ch2.chapter_download_dir = Some(a.clone());
        assert!(write_merged_cbz_archive(&comic, &[&ch1, &ch2], &output, |_| {}).is_err());
        ch2.chapter_download_dir = Some(b);
        // 目标已被其他漫画占用必须拒绝
        make(&output, 99, 999);
        let unrelated = std::fs::read(&output).unwrap();
        assert!(write_merged_cbz_archive(&comic, &[&ch1, &ch2], &output, |_| {}).is_err());
        assert_eq!(std::fs::read(&output).unwrap(), unrelated);
        copy_cbz_if_needed(&a, &a).unwrap();
        assert_eq!(std::fs::read(&a).unwrap(), source);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn remerge_from_merged_archive_slices_each_chapter_once() {
        let root = std::env::temp_dir().join(format!("jm-remerge-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let make = |path: &Path, comic_id: i64, chapter_id: i64, page: &str| {
            let mut zip = ZipWriter::new(std::fs::File::create(path).unwrap());
            zip.start_file("chapter.json", SimpleFileOptions::default())
                .unwrap();
            zip.write_all(
                serde_json::to_string(
                    &serde_json::json!({"comicId":comic_id,"chapterId":chapter_id}),
                )
                .unwrap()
                .as_bytes(),
            )
            .unwrap();
            zip.start_file("0001.png", SimpleFileOptions::default())
                .unwrap();
            zip.write_all(page.as_bytes()).unwrap();
            zip.finish().unwrap();
        };
        let ch1_path = root.join("ch1.cbz");
        let ch2_path = root.join("ch2.cbz");
        let ch3_path = root.join("ch3.cbz");
        make(&ch1_path, 10, 101, "p1");
        make(&ch2_path, 10, 102, "p2");
        make(&ch3_path, 10, 103, "p3");
        let chapter = |id: i64, title: &str, order: i64, dir: &Path| ChapterInfo {
            chapter_id: id,
            chapter_title: title.into(),
            order,
            is_downloaded: Some(true),
            chapter_download_dir: Some(dir.to_path_buf()),
        };
        let ch1 = chapter(101, "one", 1, &ch1_path);
        let ch2 = chapter(102, "two", 2, &ch2_path);
        let ch3 = chapter(103, "three", 3, &ch3_path);
        let comic = Comic {
            id: 10,
            chapter_infos: vec![ch1.clone(), ch2.clone(), ch3.clone()],
            ..Comic::default()
        };

        let merged_path = root.join("整本.cbz");
        write_merged_cbz_archive(&comic, &[&ch1, &ch2], &merged_path, |_| {}).unwrap();

        // 更新场景：ch1/ch2 的来源已变成整本归档（无 chapter.json），ch3 是新增的独立章节
        let ch1_in_book = chapter(101, "one", 1, &merged_path);
        let ch2_in_book = chapter(102, "two", 2, &merged_path);
        let final_path = root.join("整本-更新.cbz");
        write_merged_cbz_archive(
            &comic,
            &[&ch1_in_book, &ch2_in_book, &ch3],
            &final_path,
            |_| {},
        )
        .unwrap();

        let file = std::fs::File::open(&final_path).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        let entry_names: Vec<String> = (0..archive.len())
            .map(|i| archive.by_index(i).unwrap().name().to_string())
            .collect();
        let count_prefix =
            |prefix: &str| entry_names.iter().filter(|n| n.starts_with(prefix)).count();
        assert_eq!(
            count_prefix("[01] "),
            1,
            "第1话页面不能重复: {entry_names:?}"
        );
        assert_eq!(
            count_prefix("[02] "),
            1,
            "第2话页面不能重复: {entry_names:?}"
        );
        assert_eq!(
            count_prefix("[03] "),
            1,
            "第3话页面不能重复: {entry_names:?}"
        );
        let metadata = {
            let mut text = String::new();
            archive
                .by_name("元数据.json")
                .unwrap()
                .read_to_string(&mut text)
                .unwrap();
            text
        };
        let metadata: serde_json::Value = serde_json::from_str(&metadata).unwrap();
        assert_eq!(metadata["chapterInfos"].as_array().unwrap().len(), 3);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn jpeg_is_embedded_without_reencoding() {
        let mut jpeg = std::io::Cursor::new(Vec::new());
        let img = image::RgbImage::from_pixel(2, 3, image::Rgb([10, 20, 30]));
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut jpeg, image::ImageFormat::Jpeg)
            .unwrap();
        let source = jpeg.into_inner();

        let stream = image_to_pdf_stream(source.clone()).unwrap();
        // JPEG 直接走 DCTDecode，内容必须与源字节完全一致（不重新编码，避免质量损失）
        assert_eq!(stream.content, source);
        assert_eq!(
            stream.dict.get(b"Filter").unwrap().as_name().unwrap(),
            b"DCTDecode"
        );
        assert_eq!(stream.dict.get(b"Width").unwrap().as_i64().unwrap(), 2);
        assert_eq!(stream.dict.get(b"Height").unwrap().as_i64().unwrap(), 3);
        assert_eq!(
            stream.dict.get(b"ColorSpace").unwrap().as_name().unwrap(),
            b"DeviceRGB"
        );
    }

    #[test]
    fn png_is_decoded_to_rgb_and_compressed() {
        let mut png = std::io::Cursor::new(Vec::new());
        let img = image::RgbaImage::from_pixel(4, 2, image::Rgba([1, 2, 3, 0]));
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();

        let stream = image_to_pdf_stream(png.into_inner()).unwrap();
        // alpha 通道被丢弃，输出 4x2 的 RGB 原始像素
        assert_eq!(stream.content.len(), 4 * 2 * 3);
        assert_eq!(
            stream.dict.get(b"ColorSpace").unwrap().as_name().unwrap(),
            b"DeviceRGB"
        );
        assert_eq!(
            stream
                .dict
                .get(b"BitsPerComponent")
                .unwrap()
                .as_i64()
                .unwrap(),
            8
        );
    }

    #[test]
    fn unknown_image_format_is_rejected() {
        assert!(image_to_pdf_stream(b"not an image".to_vec()).is_err());
    }
}
