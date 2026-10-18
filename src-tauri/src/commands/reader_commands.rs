use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{anyhow, Context};
use base64::engine::general_purpose;
use base64::Engine as _;
use parking_lot::Mutex;
use tauri::{AppHandle, Manager};

use crate::errors::{CommandError, CommandResult};
use crate::extensions::{AppHandleExt, PathIsImg};
use crate::types::Comic;
use crate::utils;

struct CachedArchive {
    path: PathBuf,
    modified: Option<std::time::SystemTime>,
    archive: zip::ZipArchive<std::fs::File>,
}

static ACTIVE_ARCHIVE: OnceLock<Mutex<Option<CachedArchive>>> = OnceLock::new();

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ChapterNavInfo {
    pub chapter_id: i64,
    pub chapter_title: String,
    pub order: i64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ChapterReaderData {
    pub comic_id: i64,
    pub comic_title: String,
    pub chapter_id: i64,
    pub chapter_title: String,
    pub prev_chapter: Option<ChapterNavInfo>,
    pub next_chapter: Option<ChapterNavInfo>,
    pub total_images: usize,
    pub image_paths: Vec<String>,
}

#[allow(clippy::needless_pass_by_value)]
#[tauri::command(async)]
#[specta::specta]
pub fn get_chapter_reader_data(
    app: AppHandle,
    comic_id: i64,
    chapter_id: i64,
) -> CommandResult<ChapterReaderData> {
    let comic = local_comic(&app, comic_id)?;
    chapter_reader_data_from_comic(comic, chapter_id)
}

pub(crate) fn local_comic(app: &AppHandle, comic_id: i64) -> CommandResult<Comic> {
    let root = app.get_config().read().download_dir.clone();
    if let Some(comic) = app
        .state::<utils::DownloadedLibraryCache>()
        .comic(&root, comic_id)
    {
        return Ok(comic);
    }
    super::library_commands::get_downloaded_comics(app.clone())
        .into_iter()
        .find(|comic| comic.id == comic_id)
        .ok_or_else(|| {
            CommandError::from(
                "读取本地漫画失败",
                anyhow!("本地书库没有漫画ID `{comic_id}`"),
            )
        })
}

pub(crate) fn chapter_reader_data_from_comic(
    comic: Comic,
    chapter_id: i64,
) -> CommandResult<ChapterReaderData> {
    let mut sorted_chapters = comic.chapter_infos.clone();
    sorted_chapters.sort_by_key(|c| c.order);

    let current_index = sorted_chapters
        .iter()
        .position(|c| c.chapter_id == chapter_id)
        .ok_or_else(|| {
            CommandError::from("读取章节失败", anyhow!("未找到章节ID `{chapter_id}`"))
        })?;

    let current_chapter = &sorted_chapters[current_index];

    let nav_info = |chapter: &crate::types::ChapterInfo| ChapterNavInfo {
        chapter_id: chapter.chapter_id,
        chapter_title: chapter.chapter_title.clone(),
        order: chapter.order,
    };
    let prev_chapter = sorted_chapters[..current_index]
        .iter()
        .rev()
        .find(|chapter| chapter.is_downloaded == Some(true))
        .map(nav_info);
    let next_chapter = sorted_chapters[current_index + 1..]
        .iter()
        .find(|chapter| chapter.is_downloaded == Some(true))
        .map(nav_info);

    let chapter_target = current_chapter
        .chapter_download_dir
        .as_ref()
        .ok_or_else(|| CommandError::from("读取章节失败", anyhow!("章节尚未下载完成")))?;

    let image_paths_str = if chapter_target.is_file() {
        let file = std::fs::File::open(chapter_target)
            .context(format!("打开章节CBZ文件`{}`失败", chapter_target.display()))
            .map_err(|err| CommandError::from("读取章节图片失败", err))?;
        let mut archive = zip::ZipArchive::new(file)
            .context(format!("解析章节CBZ文件`{}`失败", chapter_target.display()))
            .map_err(|err| CommandError::from("读取章节图片失败", err))?;

        let mut img_names: Vec<String> = (0..archive.len())
            .filter_map(|i| {
                let entry = archive.by_index(i).ok()?;
                let name = entry.name().to_string();
                if Path::new(&name).is_img() && !name.starts_with('.') && !name.starts_with('_') {
                    Some(name)
                } else {
                    None
                }
            })
            .collect();

        // 独立章节文件直接使用全部页面；整本合并归档按话切片，只显示本章的页面。
        let is_own_chapter_file =
            crate::comic_scanner::read_chapter_identity_from_archive(&mut archive)
                .ok()
                .flatten()
                == Some((comic.id, chapter_id));
        let img_names = if is_own_chapter_file {
            img_names.sort_by(|a, b| utils::natural_cmp(a, b));
            img_names
        } else {
            crate::comic_scanner::select_chapter_pages(&img_names, current_chapter.order)
                .ok_or_else(|| {
                    CommandError::from("读取章节失败", anyhow!("归档中没有该章节的页面"))
                })?
        };

        let chapter_target_str = chapter_target.to_string_lossy();
        img_names
            .into_iter()
            .map(|entry_name| format!("{chapter_target_str}|{entry_name}"))
            .collect::<Vec<_>>()
    } else if chapter_target.is_dir() {
        let mut image_paths: Vec<PathBuf> = std::fs::read_dir(chapter_target)
            .context(format!("读取章节目录`{}`失败", chapter_target.display()))
            .map_err(|err| CommandError::from("读取章节图片失败", err))?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.is_img())
            .collect();

        image_paths.sort_by(|a, b| {
            let name_a = a.file_name().and_then(|n| n.to_str()).unwrap_or("");
            let name_b = b.file_name().and_then(|n| n.to_str()).unwrap_or("");
            utils::natural_cmp(name_a, name_b)
        });

        image_paths
            .into_iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect::<Vec<_>>()
    } else {
        return Err(CommandError::from(
            "读取章节失败",
            anyhow!("章节文件不存在"),
        ));
    };

    let total_images = image_paths_str.len();
    if total_images == 0 {
        return Err(CommandError::from(
            "读取章节失败",
            anyhow!("章节中没有可阅读的图片"),
        ));
    }

    Ok(ChapterReaderData {
        comic_id: comic.id,
        comic_title: comic.name,
        chapter_id,
        chapter_title: current_chapter.chapter_title.clone(),
        prev_chapter,
        next_chapter,
        total_images,
        image_paths: image_paths_str,
    })
}

#[allow(clippy::needless_pass_by_value)]
#[tauri::command(async)]
#[specta::specta]
pub fn get_local_image_data_url(app: AppHandle, file_path: String) -> CommandResult<String> {
    let download_dir = app.get_config().read().download_dir.clone();
    let canonical_download_dir = std::fs::canonicalize(&download_dir)
        .context(format!("无法解析下载根目录`{}`", download_dir.display()))
        .map_err(|err| CommandError::from("读取图片失败", err))?;

    if let Some((cbz_path_str, entry_name)) = file_path.split_once('|') {
        let cbz_path = PathBuf::from(cbz_path_str);
        let canonical_cbz_path = std::fs::canonicalize(&cbz_path)
            .context(format!("无法解析CBZ路径`{}`", cbz_path.display()))
            .map_err(|err| CommandError::from("读取图片失败", err))?;

        let imported_file = {
            let _guard = crate::library_catalog::CATALOG_LOCK.lock();
            crate::library_catalog::LibraryCatalog::load(&crate::library_catalog::catalog_path(
                &app,
            ))
            .map_err(|err| CommandError::from("读取书库记录失败", err))?
            .allows_cbz(&download_dir, &canonical_cbz_path)
        };
        if !canonical_cbz_path.starts_with(&canonical_download_dir) && !imported_file {
            return Err(CommandError::from(
                "读取图片失败",
                anyhow!("CBZ文件不在下载根目录内"),
            ));
        }

        let modified = std::fs::metadata(&canonical_cbz_path)
            .and_then(|m| m.modified())
            .ok();
        let active_lock = ACTIVE_ARCHIVE.get_or_init(|| Mutex::new(None));
        let mut guard = active_lock.lock();
        let is_match = guard
            .as_ref()
            .is_some_and(|cached| cached.path == canonical_cbz_path && cached.modified == modified);
        if !is_match {
            let file = std::fs::File::open(&canonical_cbz_path)
                .context(format!("打开CBZ文件`{}`失败", canonical_cbz_path.display()))
                .map_err(|err| CommandError::from("读取图片失败", err))?;
            let archive = zip::ZipArchive::new(file)
                .context(format!("解析CBZ文件`{}`失败", canonical_cbz_path.display()))
                .map_err(|err| CommandError::from("读取图片失败", err))?;
            *guard = Some(CachedArchive {
                path: canonical_cbz_path.clone(),
                modified,
                archive,
            });
        }
        let cached = guard.as_mut().unwrap();
        let mut entry = cached
            .archive
            .by_name(entry_name)
            .context(format!("CBZ内未找到图片条目`{entry_name}`"))
            .map_err(|err| CommandError::from("读取图片失败", err))?;

        let declared_size = entry.size();
        let bytes = crate::image_data::read_limited(
            &mut entry,
            declared_size,
            crate::image_data::MAX_PAGE_BYTES,
        )
        .and_then(|bytes| {
            crate::image_data::validate_image(&bytes)?;
            Ok(bytes)
        })
        .map_err(|err| CommandError::from("读取图片失败", err))?;
        drop(entry);
        drop(guard);

        let ext = Path::new(entry_name)
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("jpeg")
            .to_lowercase();
        let mime = match ext.as_str() {
            "png" => "image/png",
            "webp" => "image/webp",
            "gif" => "image/gif",
            _ => "image/jpeg",
        };

        let b64 = general_purpose::STANDARD.encode(&bytes);
        return Ok(format!("data:{mime};base64,{b64}"));
    }

    let path = PathBuf::from(&file_path);
    let canonical_path = std::fs::canonicalize(&path)
        .context(format!("无法解析图片路径`{}`", path.display()))
        .map_err(|err| CommandError::from("读取图片失败", err))?;

    if !canonical_path.starts_with(&canonical_download_dir) {
        return Err(CommandError::from(
            "读取图片失败",
            anyhow!("图片路径不在下载根目录内"),
        ));
    }

    let bytes = crate::image_data::read_image(&canonical_path)
        .context(format!("读取文件`{}`失败", canonical_path.display()))
        .map_err(|err| CommandError::from("读取图片失败", err))?;

    let ext = canonical_path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("jpeg")
        .to_lowercase();
    let mime = match ext.as_str() {
        "png" => "image/png",
        "webp" => "image/webp",
        "gif" => "image/gif",
        _ => "image/jpeg",
    };

    let b64 = general_purpose::STANDARD.encode(&bytes);
    Ok(format!("data:{mime};base64,{b64}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ChapterInfo;
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    #[test]
    fn merged_flat_cbz_reader_uses_each_chapters_file_and_skips_undownloaded_gaps() {
        let root = std::env::temp_dir().join(format!("jm-reader-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let comic = Comic {
            id: 100,
            name: "测试漫画".into(),
            chapter_infos: (1..=3)
                .map(|order| ChapterInfo {
                    chapter_id: 100 + order,
                    chapter_title: format!("第{order}话"),
                    order,
                    is_downloaded: None,
                    chapter_download_dir: None,
                })
                .collect(),
            ..Comic::default()
        };
        let mut paths = Vec::new();
        for chapter in [&comic.chapter_infos[0], &comic.chapter_infos[2]] {
            let path = root.join(utils::cbz_filename(
                &comic.name,
                comic.id,
                &chapter.chapter_title,
                chapter.chapter_id,
                chapter.order,
                comic.chapter_infos.len(),
            ));
            let mut archive = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
            archive
                .start_file("元数据.json", SimpleFileOptions::default())
                .unwrap();
            archive
                .write_all(&serde_json::to_vec(&comic).unwrap())
                .unwrap();
            archive
                .start_file("0001.jpg", SimpleFileOptions::default())
                .unwrap();
            archive.write_all(b"test-image").unwrap();
            archive.finish().unwrap();
            paths.push(path);
        }
        let merged = super::super::library_commands::scan_downloaded_comics(&root).remove(0);
        let first = chapter_reader_data_from_comic(merged.clone(), 101).unwrap();
        let third = chapter_reader_data_from_comic(merged, 103).unwrap();
        assert!(first.image_paths[0].starts_with(paths[0].to_str().unwrap()));
        assert!(third.image_paths[0].starts_with(paths[1].to_str().unwrap()));
        assert_eq!(first.next_chapter.unwrap().chapter_id, 103);
        assert_eq!(third.prev_chapter.unwrap().chapter_id, 101);
        std::fs::remove_dir_all(root).unwrap();
    }
}
