use std::{
    collections::HashSet,
    io::Write,
    path::{Path, PathBuf},
    sync::{atomic::AtomicU32, Arc},
};

use anyhow::{anyhow, ensure, Context};

use rayon::iter::{IntoParallelIterator, ParallelIterator};
use tauri::AppHandle;
use tauri_specta::Event;
use zip::{write::SimpleFileOptions, ZipWriter};

use crate::{
    atomic_fs,
    events::ExportCbzEvent,
    extensions::{AnyhowErrorToStringChain, AppHandleExt, PathIsImg},
    types::{ChapterInfo, Comic, ComicInfo},
    utils,
};

struct TempFileCleanup(PathBuf);

impl Drop for TempFileCleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
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
            let identity = crate::comic_scanner::read_chapter_identity(path)?;
            if let Some(identity) = identity {
                ensure!(
                    chapters.len() == 1 && identity == (comic.id, chapter.chapter_id),
                    "合并文件不能覆盖原章节"
                );
            } else {
                // 无 chapter.json 的整本归档：内嵌的话清单必须全部属于本次重写范围。
                // 站点下架后本地可能只剩部分话，此时重写会永久丢掉其他话的页面。
                ensure!(
                    merged_archive_is_not_wider_than(chapters, path)?,
                    "目标归档还包含本次未整理的话，保留原文件"
                );
            }
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

/// 目标整本归档内嵌的话清单是否不比本次重写范围更宽。
///
/// 整本归档没有 chapter.json，无法用话身份判断归属；若归档里还留着本次未参与
/// 重写的话（例如站点下架后本地只剩一话），原位重写会永久丢掉那些话的页面。
fn merged_archive_is_not_wider_than(
    chapters: &[&ChapterInfo],
    path: &Path,
) -> anyhow::Result<bool> {
    let mut archive = zip::ZipArchive::new(std::fs::File::open(path)?)?;
    let Ok(mut entry) = archive.by_name("元数据.json") else {
        // 缺少元数据时交给后面的漫画身份校验给出明确错误
        return Ok(true);
    };
    let size = entry.size();
    let bytes =
        crate::image_data::read_limited(&mut entry, size, crate::image_data::MAX_METADATA_BYTES)?;
    let embedded: Comic = serde_json::from_slice(&bytes)?;
    let keep: HashSet<i64> = chapters.iter().map(|chapter| chapter.chapter_id).collect();
    Ok(embedded
        .chapter_infos
        .iter()
        .all(|chapter| keep.contains(&chapter.chapter_id)))
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

    let extension = "cbz";
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
            let source_pages = crate::page_size::read_pages_entry(&mut archive);
            let mut page_sizes: std::collections::BTreeMap<String, crate::types::PageSize> =
                std::collections::BTreeMap::new();
            for (page_idx, entry_name) in img_names.iter().enumerate() {
                let extension = Path::new(entry_name)
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .unwrap_or("jpg")
                    .to_lowercase();
                let target_name = format!("{:04}.{extension}", page_idx + 1);
                // 整本源归档里的尺寸表键是原条目名，导出后条目改名，表也要跟着改
                let size = match source_pages.get(entry_name) {
                    Some(size) => Some(*size),
                    None => crate::page_size::size_from_archive(&mut archive, entry_name),
                };
                if let Some(size) = size {
                    page_sizes.insert(target_name.clone(), size);
                }
                let mut entry = archive
                    .by_name(entry_name)
                    .context(format!("读取原章节图片`{entry_name}`失败"))?;
                // 从已有归档复制时同样要保证单条目不超过 ZIP32 上限
                crate::zip_limits::ensure_archive_entry_size(&entry, entry_name)?;
                zip_writer
                    .start_file(&target_name, crate::zip_limits::image_entry_options())
                    .context(format!("在分部CBZ创建条目`{target_name}`失败"))?;
                std::io::copy(&mut entry, &mut zip_writer)
                    .context(format!("将图片写入分部CBZ条目`{target_name}`失败"))?;
            }
            crate::page_size::write_pages_entry(&mut zip_writer, page_sizes)
                .context("在分部CBZ写入`pages.json`失败")?;
            let zip_file = zip_writer.finish().context(format!(
                "章节`{chapter_title}`关闭`{}`失败",
                temp_path.display()
            ))?;
            zip_file
                .sync_all()
                .context(format!("同步临时CBZ`{}`失败", temp_path.display()))?;
            // 见 download_manager/package.rs：公共存储上打开中的文件不能改名
            drop(zip_file);
            // 阅读器读句柄由 atomic_fs 的提交钩子在替换锁内释放
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
        let image_paths = utils::collect_page_images(chapter_download_dir)?;

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
        let mut page_sizes: std::collections::BTreeMap<String, crate::types::PageSize> =
            std::collections::BTreeMap::new();
        for image_path in image_paths {
            let filename = match image_path.file_name() {
                Some(name) => name.to_string_lossy(),
                None => continue,
            };
            // 将文件写入cbz
            crate::zip_limits::ensure_entry_size(&image_path)?;
            let mut file = std::fs::File::open(&image_path)
                .context(format!("打开`{}`失败", image_path.display()))?;
            // 同 package.rs：同一句柄先读文件头记尺寸，复位后整文件复制
            if let Some(size) = crate::page_size::size_from_opened_file(&mut file) {
                page_sizes.insert(filename.to_string(), size);
            }
            zip_writer
                .start_file(&filename, crate::zip_limits::image_entry_options())
                .context(format!(
                    "章节`{chapter_title}`在`{}`创建`{filename}`失败",
                    save_path.display()
                ))?;
            std::io::copy(&mut file, &mut zip_writer).context(format!(
                "章节`{chapter_title}`将`{}`写入`{}`失败",
                image_path.display(),
                save_path.display()
            ))?;
        }
        crate::page_size::write_pages_entry(&mut zip_writer, page_sizes)
            .context("在分部CBZ写入`pages.json`失败")?;

        let zip_file = zip_writer.finish().context(format!(
            "章节`{chapter_title}`关闭`{}`失败",
            temp_path.display()
        ))?;
        zip_file
            .sync_all()
            .context(format!("同步临时CBZ`{}`失败", temp_path.display()))?;
        // 见 download_manager/package.rs：公共存储上打开中的文件不能改名
        drop(zip_file);
        // 阅读器读句柄由 atomic_fs 的提交钩子在替换锁内释放
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
    // 逐话搬运 pages.json，避免合并后阅读器又要为整本书重读一遍文件头
    let mut merged_page_sizes: std::collections::BTreeMap<String, crate::types::PageSize> =
        std::collections::BTreeMap::new();
    // 再合并时多个话指向同一个整本源归档：pages.json 只解压解析一次
    let mut source_pages_cache: std::collections::HashMap<
        std::path::PathBuf,
        std::collections::BTreeMap<String, crate::types::PageSize>,
    > = std::collections::HashMap::new();

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

            // 来源归档自带的尺寸表：改名后照搬，省掉合并后的重读。
            // 再合并时同一源归档只解压解析一次；来源没有该页的尺寸时现读文件头
            // （归档已在手上），旧版书库/导入源合并后也能带上完整尺寸表。
            let source_pages = match source_pages_cache.entry(chapter_download_dir.to_path_buf()) {
                std::collections::hash_map::Entry::Occupied(occupied) => occupied.get().clone(),
                std::collections::hash_map::Entry::Vacant(vacant) => {
                    let pages = crate::page_size::read_pages_entry(&mut archive);
                    vacant.insert(pages).clone()
                }
            };
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
                let size = match source_pages.get(entry_name) {
                    Some(size) => Some(*size),
                    None => crate::page_size::size_from_archive(&mut archive, entry_name),
                };
                if let Some(size) = size {
                    merged_page_sizes.insert(target_entry_name.clone(), size);
                }

                let mut entry = archive
                    .by_name(entry_name)
                    .context(format!("读取原章节图片`{entry_name}`失败"))?;
                crate::zip_limits::ensure_archive_entry_size(&entry, entry_name)?;
                zip_writer
                    .start_file(&target_entry_name, crate::zip_limits::image_entry_options())
                    .context(format!("在合并CBZ创建条目`{target_entry_name}`失败"))?;
                std::io::copy(&mut entry, &mut zip_writer)
                    .context(format!("将图片写入合并CBZ条目`{target_entry_name}`失败"))?;
            }
        } else if chapter_download_dir.is_dir() {
            let image_paths = utils::collect_page_images(chapter_download_dir)?;
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
                let mut file = std::fs::File::open(&img_path)
                    .context(format!("打开图片`{}`失败", img_path.display()))?;
                // 同 package.rs：同一句柄先读文件头记尺寸，复位后整文件复制
                if let Some(size) = crate::page_size::size_from_opened_file(&mut file) {
                    merged_page_sizes.insert(target_entry_name.clone(), size);
                }
                zip_writer
                    .start_file(&target_entry_name, crate::zip_limits::image_entry_options())
                    .context(format!("在合并CBZ创建条目`{target_entry_name}`失败"))?;
                std::io::copy(&mut file, &mut zip_writer)
                    .context(format!("将图片写入合并CBZ条目`{target_entry_name}`失败"))?;
            }
        }

        on_progress((chapter_idx + 1) as u32);
    }

    crate::page_size::write_pages_entry(&mut zip_writer, merged_page_sizes)
        .context("在合并CBZ写入`pages.json`失败")?;

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
    // 见 download_manager/package.rs：公共存储上打开中的文件不能改名
    drop(zip_file);

    // 阅读器读句柄由 atomic_fs 的提交钩子在替换锁内释放（见 download_manager/package.rs）
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

#[cfg(test)]
mod tests {
    use super::{cbz_filename, copy_cbz_if_needed, merged_cbz_filename, write_merged_cbz_archive};
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

    /// 合并时条目会改名（`0001.png` -> `[01] 第1话/0001.png`），尺寸表必须跟着改名，
    /// 否则阅读器拿到的键一个都对不上，整本退回逐页现算。
    #[test]
    fn merged_archive_carries_page_sizes_under_renamed_entries() {
        let test_dir = std::env::temp_dir().join(format!("jm_pages_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&test_dir).unwrap();

        let png = |width: u32, height: u32| {
            let mut bytes = Vec::new();
            image::RgbImage::new(width, height)
                .write_to(
                    &mut std::io::Cursor::new(&mut bytes),
                    image::ImageFormat::Png,
                )
                .unwrap();
            bytes
        };

        // 两话才有 `[NN] 标题/` 前缀，单话平铺时改名与不改名是同一个键，测不出问题
        let write_chapter = |file_name: &str, chapter_id: i64, width: u32, height: u32| {
            let path = test_dir.join(file_name);
            let mut zip = ZipWriter::new(std::fs::File::create(&path).unwrap());
            let mut pages = std::collections::BTreeMap::new();
            pages.insert(
                "0001.png".to_string(),
                crate::types::PageSize { width, height },
            );
            crate::page_size::write_pages_entry(&mut zip, pages).unwrap();
            zip.start_file("0001.png", SimpleFileOptions::default())
                .unwrap();
            zip.write_all(&png(width, height)).unwrap();
            zip.finish().unwrap();
            ChapterInfo {
                chapter_id,
                chapter_title: format!("第{chapter_id}话"),
                order: chapter_id,
                is_downloaded: Some(true),
                chapter_download_dir: Some(path),
            }
        };
        let ch1 = write_chapter("ch1.cbz", 1, 600, 900);
        let ch2 = write_chapter("ch2.cbz", 2, 800, 1200);

        let comic = Comic {
            id: 999,
            name: "尺寸书".to_string(),
            chapter_infos: vec![ch1.clone(), ch2.clone()],
            ..Comic::default()
        };
        let save_path = test_dir.join("尺寸书.cbz");
        super::write_merged_cbz_archive(&comic, &[&ch1, &ch2], &save_path, |_| {}).unwrap();

        let mut archive = zip::ZipArchive::new(std::fs::File::open(&save_path).unwrap()).unwrap();
        let entry_names: Vec<String> = (0..archive.len())
            .map(|i| archive.by_index(i).unwrap().name().to_string())
            .collect();
        assert!(
            entry_names.contains(&"[01] 第1话/0001.png".to_string()),
            "{entry_names:?}"
        );
        assert!(entry_names.contains(&crate::page_size::PAGES_ENTRY.to_string()));

        let cached = crate::page_size::read_pages_entry(&mut archive);
        // 键必须跟着改名，否则阅读器一个都命中不了
        assert_eq!(
            cached.get("[01] 第1话/0001.png").map(|size| size.width),
            Some(600)
        );
        assert_eq!(
            cached.get("[02] 第2话/0001.png").map(|size| size.height),
            Some(1200)
        );
        // 源归档里的旧键（`0001.png`）不能留在合并结果里：它不对应任何条目，
        // 还会在两张表合并时盖掉真正的键
        assert!(!cached.contains_key("0001.png"));

        let _ = std::fs::remove_dir_all(&test_dir);
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
    fn single_chapter_comic_can_rewrite_its_own_merged_archive_in_place() {
        // 单话本子下载后会被自动整理成整本归档（无 chapter.json）。
        // 再次「整理为整本」时目标就是同一路径，此时不能因为没有章节身份而拒绝。
        let root = std::env::temp_dir().join(format!("jm-remerge-self-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("ch1.cbz");
        let mut zip = ZipWriter::new(std::fs::File::create(&source).unwrap());
        zip.start_file("chapter.json", SimpleFileOptions::default())
            .unwrap();
        zip.write_all(br#"{"comicId":10,"chapterId":101}"#).unwrap();
        zip.start_file("0001.png", SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"page").unwrap();
        zip.finish().unwrap();
        let ch1 = ChapterInfo {
            chapter_id: 101,
            chapter_title: "one".into(),
            order: 1,
            is_downloaded: Some(true),
            chapter_download_dir: Some(source.clone()),
        };
        let comic = Comic {
            id: 10,
            name: "单话本".into(),
            chapter_infos: vec![ch1.clone()],
            ..Comic::default()
        };
        let merged = root.join("单话本.cbz");
        write_merged_cbz_archive(&comic, &[&ch1], &merged, |_| {}).unwrap();

        // 书库扫描后该话指向整本归档，原位再整理一次必须成功
        let in_book = ChapterInfo {
            chapter_download_dir: Some(merged.clone()),
            ..ch1.clone()
        };
        write_merged_cbz_archive(&comic, &[&in_book], &merged, |_| {}).unwrap();
        let mut archive = zip::ZipArchive::new(std::fs::File::open(&merged).unwrap()).unwrap();
        assert!(archive.by_name("0001.png").is_ok());

        // 其他漫画的同名归档仍不能被覆盖
        let foreign = root.join("foreign.cbz");
        let mut zip = ZipWriter::new(std::fs::File::create(&foreign).unwrap());
        zip.start_file("元数据.json", SimpleFileOptions::default())
            .unwrap();
        zip.write_all(br#"{"id":99}"#).unwrap();
        zip.finish().unwrap();
        let foreign_chapter = ChapterInfo {
            chapter_download_dir: Some(foreign.clone()),
            ..ch1.clone()
        };
        assert!(write_merged_cbz_archive(&comic, &[&foreign_chapter], &foreign, |_| {}).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn single_chapter_rewrite_refuses_archive_that_still_holds_other_chapters() {
        // 站点下架章节后本地只剩一话，但整本归档里还留着其他话的页面：
        // 原位重写会永久丢掉那些页面，必须拒绝。
        let root = std::env::temp_dir().join(format!("jm-remerge-wide-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let merged = root.join("多话本.cbz");
        let mut zip = ZipWriter::new(std::fs::File::create(&merged).unwrap());
        zip.start_file("元数据.json", SimpleFileOptions::default())
            .unwrap();
        zip.write_all(
            serde_json::to_string(&serde_json::json!({
                "id": 20,
                "name": "多话本",
                "chapterInfos": [
                    {"chapterId": 201, "chapterTitle": "一", "order": 1},
                    {"chapterId": 202, "chapterTitle": "二", "order": 2}
                ]
            }))
            .unwrap()
            .as_bytes(),
        )
        .unwrap();
        zip.start_file("[01] 一/0001.png", SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"page1").unwrap();
        zip.start_file("[02] 二/0001.png", SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"page2").unwrap();
        zip.finish().unwrap();

        let only_first = ChapterInfo {
            chapter_id: 201,
            chapter_title: "一".into(),
            order: 1,
            is_downloaded: Some(true),
            chapter_download_dir: Some(merged.clone()),
        };
        let comic = Comic {
            id: 20,
            name: "多话本".into(),
            chapter_infos: vec![only_first.clone()],
            ..Comic::default()
        };
        assert!(write_merged_cbz_archive(&comic, &[&only_first], &merged, |_| {}).is_err());
        // 归档原样保留，第二话页面还在
        let mut archive = zip::ZipArchive::new(std::fs::File::open(&merged).unwrap()).unwrap();
        assert!(archive.by_name("[02] 二/0001.png").is_ok());
        std::fs::remove_dir_all(root).unwrap();
    }
}
