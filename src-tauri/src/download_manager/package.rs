use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context};
use tauri::{AppHandle, Manager};
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

use crate::atomic_fs;
use crate::types::{ChapterInfo, Comic, ComicInfo};
use crate::utils;

/// 收集临时目录中的有效页面：按自然序排序，并按文件主名去重
/// （同一页可能残留不同扩展名的旧文件）。
fn collect_page_images(temp_download_dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut image_paths: Vec<PathBuf> = std::fs::read_dir(temp_download_dir)
        .context(format!(
            "读取临时下载目录`{}`失败",
            temp_download_dir.display()
        ))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            use crate::extensions::PathIsImg;
            path.is_img()
        })
        .collect();

    image_paths.sort_by(|a, b| {
        let name_a = a.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let name_b = b.file_name().and_then(|n| n.to_str()).unwrap_or("");
        utils::natural_cmp(name_a, name_b)
    });
    let mut page_numbers = std::collections::HashSet::new();
    image_paths.retain(|path| page_numbers.insert(path.file_stem().unwrap_or_default().to_owned()));
    Ok(image_paths)
}

pub(crate) fn package_chapter_to_cbz(
    comic: &Comic,
    chapter_info: &ChapterInfo,
    app: &AppHandle,
    temp_download_dir: &Path,
    expected_pages: usize,
) -> anyhow::Result<()> {
    let comic_title = &comic.name;
    let chapter_title = &chapter_info.chapter_title;
    let chapter_cbz_path = chapter_info
        .chapter_download_dir
        .as_ref()
        .context("`chapter_download_dir`字段为`None`")?;

    if let Some(parent) = chapter_cbz_path.parent() {
        std::fs::create_dir_all(parent).context(format!("创建目录`{}`失败", parent.display()))?;
    }

    let temp_cbz_path = atomic_fs::temp_path(chapter_cbz_path)?;
    struct TempFileCleanup(PathBuf);
    impl Drop for TempFileCleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    let _cleanup = TempFileCleanup(temp_cbz_path.clone());

    let zip_file = std::fs::File::create(&temp_cbz_path).context(format!(
        "章节`{chapter_title}`创建临时CBZ`{}`失败",
        temp_cbz_path.display()
    ))?;
    let mut zip_writer = ZipWriter::new(zip_file);

    // 图片列表要提前收集：ComicInfo.xml 的 PageCount 必须在写入条目前确定
    // （ZIP 条目不可原地修改），且后续校验/打包都复用同一份列表。
    let image_paths = collect_page_images(temp_download_dir)?;
    anyhow::ensure!(
        expected_pages > 0 && image_paths.len() == expected_pages,
        "打包图片数量与章节页数不一致"
    );

    // 写入 ComicInfo.xml
    let mut comic_info = ComicInfo::from(comic, chapter_info);
    comic_info.page_count = i64::try_from(image_paths.len()).unwrap_or(0);
    let cfg = yaserde::ser::Config {
        perform_indent: true,
        ..Default::default()
    };
    let comic_info_xml = yaserde::ser::to_string_with_config(&comic_info, &cfg)
        .map_err(|err_msg| anyhow!("序列化`ComicInfo.xml`失败: {err_msg}"))?;
    zip_writer
        .start_file("ComicInfo.xml", SimpleFileOptions::default())
        .context("创建`ComicInfo.xml`条目失败")?;
    zip_writer
        .write_all(comic_info_xml.as_bytes())
        .context("写入`ComicInfo.xml`失败")?;

    // 写入 元数据.json
    let mut comic_clone = comic.clone();
    comic_clone.is_downloaded = None;
    comic_clone.comic_download_dir = None;
    comic_clone.local_cover_data = None;
    comic_clone.is_favorite = false;
    comic_clone.liked = false;
    for ch in &mut comic_clone.chapter_infos {
        ch.is_downloaded = None;
        ch.chapter_download_dir = None;
    }
    let comic_json = serde_json::to_string_pretty(&comic_clone)
        .map_err(|err| anyhow!("序列化`元数据.json`失败: {err}"))?;
    zip_writer
        .start_file("元数据.json", SimpleFileOptions::default())
        .context("创建`元数据.json`条目失败")?;
    zip_writer
        .write_all(comic_json.as_bytes())
        .context("写入`元数据.json`失败")?;

    zip_writer.start_file("chapter.json", SimpleFileOptions::default())?;
    zip_writer.write_all(
        serde_json::to_string(&serde_json::json!({
            "comicId": comic.id, "chapterId": chapter_info.chapter_id
        }))?
        .as_bytes(),
    )?;

    // 写入压缩包
    for (idx, image_path) in image_paths.into_iter().enumerate() {
        let ext = image_path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("jpg")
            .to_lowercase();
        let entry_name = format!("{:04}.{ext}", idx + 1);

        // 单条目超过 ZIP32 上限会让整章打包失败在半路，先按体积给出可读错误
        crate::zip_limits::ensure_entry_size(&image_path)?;
        zip_writer
            .start_file(&entry_name, SimpleFileOptions::default())
            .context(format!("在CBZ创建条目`{entry_name}`失败"))?;
        let mut file = std::fs::File::open(&image_path)
            .context(format!("打开图片`{}`失败", image_path.display()))?;
        std::io::copy(&mut file, &mut zip_writer)
            .context(format!("将图片写入CBZ条目`{entry_name}`失败"))?;
    }

    let zip_file = zip_writer.finish().context("关闭CBZ文件失败")?;
    zip_file.sync_all().context("同步CBZ到磁盘失败")?;

    atomic_fs::commit_file_checked(&temp_cbz_path, chapter_cbz_path, |existing| {
        ensure_archive_identity(existing, comic, chapter_info)
    })
    .context(format!("提交CBZ文件`{}`失败", chapter_cbz_path.display()))?;

    // 清理散图临时目录
    let _ = std::fs::remove_dir_all(temp_download_dir);

    if let Some(cache) = app.try_state::<utils::DownloadedLibraryCache>() {
        cache.invalidate();
    }

    tracing::info!(
        comic_title,
        chapter_title,
        "章节打包CBZ成功: {}",
        chapter_cbz_path.display()
    );
    Ok(())
}

pub(crate) fn ensure_archive_identity(
    path: &Path,
    comic: &Comic,
    chapter: &ChapterInfo,
) -> anyhow::Result<()> {
    let mut archive = zip::ZipArchive::new(std::fs::File::open(path)?)?;
    if let Ok(mut entry) = archive.by_name("chapter.json") {
        let size = entry.size();
        let bytes = crate::image_data::read_limited(
            &mut entry,
            size,
            crate::image_data::MAX_METADATA_BYTES,
        )?;
        let identity: serde_json::Value = serde_json::from_slice(&bytes)?;
        anyhow::ensure!(
            identity["comicId"].as_i64() == Some(comic.id)
                && identity["chapterId"].as_i64() == Some(chapter.chapter_id),
            "已有归档属于其他漫画或章节，拒绝覆盖/复用"
        );
        return Ok(());
    }
    let mut entry = archive
        .by_name("元数据.json")
        .context("已有归档缺少身份信息，拒绝覆盖/复用")?;
    let size = entry.size();
    let bytes =
        crate::image_data::read_limited(&mut entry, size, crate::image_data::MAX_METADATA_BYTES)?;
    let old: Comic = serde_json::from_slice(&bytes)?;
    anyhow::ensure!(old.id == comic.id, "已有归档属于其他漫画，拒绝覆盖/复用");
    drop(entry);
    let mut entry = archive.by_name("ComicInfo.xml")?;
    let size = entry.size();
    let bytes =
        crate::image_data::read_limited(&mut entry, size, crate::image_data::MAX_METADATA_BYTES)?;
    let info: ComicInfo = yaserde::de::from_reader(bytes.as_slice()).map_err(|e| anyhow!(e))?;
    anyhow::ensure!(
        info.number.as_deref() == Some(chapter.order.to_string().as_str())
            && info.title == chapter.chapter_title,
        "已有归档章节不匹配，拒绝覆盖/复用"
    );
    Ok(())
}

#[cfg(test)]
mod identity_tests {
    use super::*;
    #[test]
    fn archive_from_another_comic_or_chapter_is_never_reused() {
        let path = std::env::temp_dir().join(format!("jm-identity-{}.cbz", uuid::Uuid::new_v4()));
        let mut archive = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        archive
            .start_file("chapter.json", SimpleFileOptions::default())
            .unwrap();
        archive
            .write_all(br#"{"comicId":111,"chapterId":11}"#)
            .unwrap();
        archive.finish().unwrap();
        let comic = Comic {
            id: 111,
            ..Default::default()
        };
        let chapter = ChapterInfo {
            chapter_id: 11,
            chapter_title: String::new(),
            order: 1,
            is_downloaded: None,
            chapter_download_dir: None,
        };
        assert!(ensure_archive_identity(&path, &comic, &chapter).is_ok());
        assert!(ensure_archive_identity(
            &path,
            &Comic {
                id: 222,
                ..Default::default()
            },
            &chapter
        )
        .is_err());
        assert!(ensure_archive_identity(
            &path,
            &comic,
            &ChapterInfo {
                chapter_id: 12,
                ..chapter.clone()
            }
        )
        .is_err());
        std::fs::remove_file(path).unwrap();
    }
}
