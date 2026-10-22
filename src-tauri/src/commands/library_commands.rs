use std::path::Path;
use std::time::Duration;

use anyhow::{anyhow, Context};
use indexmap::IndexMap;
use tauri::{AppHandle, Manager};

use crate::errors::{CommandError, CommandResult};
use crate::export;
use crate::extensions::{AnyhowErrorToStringChain, AppHandleExt};
use crate::types::{Comic, ComicInFavorite, ComicInSearch, ComicInWeekly};
use crate::utils;

#[allow(clippy::needless_pass_by_value)]
#[allow(clippy::too_many_lines)]
#[tauri::command(async)]
#[specta::specta]
pub fn get_downloaded_comics(app: AppHandle) -> Vec<Comic> {
    let download_dir = app.get_config().read().download_dir.clone();
    let catalog = {
        let _guard = crate::library_catalog::CATALOG_LOCK.lock();
        crate::library_catalog::LibraryCatalog::load(&crate::library_catalog::catalog_path(&app))
    };
    let Ok(catalog) = catalog else {
        tracing::error!(error = ?catalog.err(), "读取书库记录失败");
        return Vec::new();
    };
    let removed = catalog.removed_ids(&download_dir);
    let mut comics = scan_downloaded_comics(&download_dir);
    comics.retain(|comic| !removed.contains(&comic.id));
    for record in catalog.visible_imports(&download_dir) {
        match crate::library_catalog::registered_cbz_comic(&record) {
            Ok(comic) => comics.push(comic),
            Err(error) => {
                tracing::warn!(path = %record.path.display(), error = %error, "跳过无法读取的导入漫画")
            }
        }
    }
    if let Some(cache) = app.try_state::<utils::DownloadedLibraryCache>() {
        let mut map = std::collections::HashMap::new();
        for comic in &comics {
            if let Some(dir) = &comic.comic_download_dir {
                map.insert(comic.id, dir.clone());
            }
        }
        cache.set(map);
        cache.set_comics(&download_dir, &comics);
    }
    comics
}

pub(crate) fn scan_downloaded_comics(download_dir: &Path) -> Vec<Comic> {
    // 1. 扫描下载根目录下的平铺 .cbz 文件
    let mut flat_cbz_items = Vec::new();
    if let Ok(entries) = std::fs::read_dir(download_dir) {
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.is_file()
                && path
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(str::to_lowercase)
                    == Some("cbz".to_string())
            {
                let modify_time = path
                    .metadata()
                    .and_then(|m| m.modified())
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                flat_cbz_items.push((path, modify_time, true));
            }
        }
    }

    // 2. 遍历历史子目录中的 元数据.json
    let mut metadata_items = Vec::new();
    for entry in utils::walk_comic_metadata_files(download_dir) {
        let path = entry.path();
        let modify_time = path
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        metadata_items.push((path.to_path_buf(), modify_time, false));
    }

    let mut all_items = flat_cbz_items;
    all_items.extend(metadata_items);
    // 按照文件修改时间排序，最新的排在最前面
    all_items.sort_by(|(_, a, _), (_, b, _)| b.cmp(a));

    let mut downloaded_comics = Vec::new();
    for (path, _, is_cbz) in all_items {
        let res = if is_cbz {
            Comic::from_cbz(&path).context(format!("从CBZ`{}`转为Comic失败", path.display()))
        } else {
            Comic::from_metadata(&path)
                .context(format!("从元数据`{}`转为Comic失败", path.display()))
        };
        match res {
            Ok(comic) => downloaded_comics.push(comic),
            Err(err) => {
                let err_title = "获取已下载漫画的过程中遇到错误，已跳过";
                let string_chain = err.to_string_chain();
                tracing::error!(err_title, message = string_chain);
            }
        }
    }

    // 按照漫画ID分组，支持多话CBZ合并
    let mut comics_by_id: IndexMap<i64, Vec<Comic>> = IndexMap::new();
    for comic in downloaded_comics {
        comics_by_id.entry(comic.id).or_default().push(comic);
    }

    let mut unique_comics = Vec::new();
    for (_comic_id, mut comics) in comics_by_id {
        let mut base_comic = comics.remove(0);
        for other in comics {
            if base_comic.local_cover_data.is_none() && other.local_cover_data.is_some() {
                base_comic.local_cover_data = other.local_cover_data;
            }
            for other_ch in other.chapter_infos {
                if other_ch.is_downloaded == Some(true) {
                    // 只按 chapter_id 认定同一章。order 只是展示序号：服务端增减章节会让
                    // 它整体偏移，外部导入的 ComicInfo 也可能出现重复 Number；用它兜底
                    // 会把不同章节当成同一章，导致已下载标记与文件路径串位。
                    if let Some(base_ch) = base_comic
                        .chapter_infos
                        .iter_mut()
                        .find(|c| c.chapter_id == other_ch.chapter_id)
                    {
                        base_ch.is_downloaded = Some(true);
                        if base_ch.chapter_download_dir.is_none() {
                            base_ch.chapter_download_dir = other_ch.chapter_download_dir;
                        }
                    } else {
                        base_comic.chapter_infos.push(other_ch);
                    }
                }
            }
        }
        base_comic.chapter_infos.sort_by_key(|c| c.order);
        base_comic.is_downloaded = Some(true);
        unique_comics.push(base_comic);
    }

    unique_comics
}

#[allow(clippy::needless_pass_by_value)]
#[tauri::command(async)]
#[specta::specta]
pub fn delete_downloaded_comic(app: AppHandle, comic: Comic) -> CommandResult<()> {
    // 命令名为“删除已下载漫画”，必须同时删除文件；仅移除书库记录会留下孤儿文件。
    // 新前端统一走 remove_library_comic（可显式选择是否删文件），此命令仅作兼容保留。
    super::library_features::remove_library_comic(app, comic.id, true)
}

#[tauri::command(async)]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub fn export_cbz(app: AppHandle, comic: Comic) -> CommandResult<()> {
    let comic_title = &comic.name;
    export::cbz(&app, &comic)
        .context(format!("漫画`{comic_title}`导出cbz失败"))
        .map_err(|err| CommandError::from("导出cbz失败", err))?;
    Ok(())
}

#[tauri::command(async)]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub fn export_merged_cbz(app: AppHandle, comic: Comic) -> CommandResult<()> {
    // 仅使用当前书库的章节记录，客户端不能把其他漫画的路径混入合并任务。
    let comic = super::reader_commands::local_comic(&app, comic.id)?;
    let comic_title = &comic.name;
    export::merge_cbz(&app, &comic)
        .context(format!("漫画`{comic_title}`合并导出cbz失败"))
        .map_err(|err| CommandError::from("合并导出cbz失败", err))?;
    Ok(())
}

#[allow(clippy::needless_pass_by_value)]
#[tauri::command(async)]
#[specta::specta]
pub fn get_synced_comic(app: AppHandle, mut comic: Comic) -> CommandResult<Comic> {
    let id_to_dir_map = utils::create_id_to_dir_map(&app)
        .context("创建漫画ID到下载目录映射失败")
        .map_err(|err| {
            CommandError::from(&format!("漫画`{}`同步Comic的字段失败", comic.name), err)
        })?;

    comic.update_fields(&id_to_dir_map).map_err(|err| {
        CommandError::from(&format!("漫画`{}`同步Comic的字段失败", comic.name), err)
    })?;

    Ok(comic)
}

#[allow(clippy::needless_pass_by_value)]
#[tauri::command(async)]
#[specta::specta]
pub fn get_synced_comic_in_favorite(
    app: AppHandle,
    mut comic: ComicInFavorite,
) -> CommandResult<ComicInFavorite> {
    let id_to_dir_map = utils::create_id_to_dir_map(&app)
        .context("创建漫画ID到下载目录映射失败")
        .map_err(|err| {
            let err_title = format!("漫画`{}`同步ComicInFavorite的字段失败", comic.name);
            CommandError::from(&err_title, err)
        })?;

    comic.update_fields(&id_to_dir_map);

    Ok(comic)
}

#[allow(clippy::needless_pass_by_value)]
#[tauri::command(async)]
#[specta::specta]
pub fn get_synced_comic_in_search(
    app: AppHandle,
    mut comic: ComicInSearch,
) -> CommandResult<ComicInSearch> {
    let id_to_dir_map = utils::create_id_to_dir_map(&app)
        .context("创建漫画ID到下载目录映射失败")
        .map_err(|err| {
            let err_title = format!("漫画`{}`同步ComicInSearch的字段失败", comic.name);
            CommandError::from(&err_title, err)
        })?;

    comic.update_fields(&id_to_dir_map);

    Ok(comic)
}

#[allow(clippy::needless_pass_by_value)]
#[tauri::command(async)]
#[specta::specta]
pub fn get_synced_comic_in_weekly(
    app: AppHandle,
    mut comic: ComicInWeekly,
) -> CommandResult<ComicInWeekly> {
    let id_to_dir_map = utils::create_id_to_dir_map(&app)
        .context("创建漫画ID到下载目录映射失败")
        .map_err(|err| {
            let err_title = format!("漫画`{}`同步ComicInWeekly的字段失败", comic.name);
            CommandError::from(&err_title, err)
        })?;

    comic.update_fields(&id_to_dir_map);

    Ok(comic)
}

#[allow(clippy::needless_pass_by_value)]
#[tauri::command(async)]
#[specta::specta]
pub async fn test_domain_latency(app: AppHandle, domain: String) -> CommandResult<u64> {
    let domain = domain.trim();
    // 与域名池使用同一套严格校验：拒绝含 `/`、`@`、`:` 等 URL 组件的字符串，
    // 否则可以借该命令向任意 HTTPS 目标发请求
    if !crate::jm_client::is_plausible_domain(domain) {
        return Err(CommandError::from("测速失败", anyhow!("域名格式无效")));
    }

    let builder = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(4));

    let builder = app
        .get_config()
        .read()
        .apply_proxy(builder)
        .map_err(|err| CommandError::from("测速失败", err))?;

    let client = builder
        .build()
        .context("创建HTTP客户端失败")
        .map_err(|err| CommandError::from("测速失败", err))?;

    let url = format!("https://{domain}");
    let head_start = std::time::Instant::now();
    let request_result = client.head(&url).send().await;
    match request_result {
        Ok(_) => Ok(u64::try_from(head_start.elapsed().as_millis()).unwrap_or(u64::MAX)),
        Err(err) => {
            if err.is_connect() || err.is_timeout() {
                return Err(CommandError::from("连接超时", err));
            }
            let get_start = std::time::Instant::now();
            let get_result = client.get(&url).header("Range", "bytes=0-0").send().await;
            match get_result {
                Ok(_) => Ok(u64::try_from(get_start.elapsed().as_millis()).unwrap_or(u64::MAX)),
                Err(err) => Err(CommandError::from("测速失败", err)),
            }
        }
    }
}

#[cfg(test)]
mod end_to_end_scan_tests {
    use super::scan_downloaded_comics;
    use crate::types::{ChapterInfo, Comic};
    use std::io::Write;
    use std::path::Path;

    /// 照下载打包的真实产物写一个单话 CBZ：
    /// 文件名 `本子名 [JM{comic_id}-{chapter_id}].cbz`，包内带 `元数据.json` 与 `chapter.json`。
    fn write_downloaded_chapter(path: &Path, comic: &Comic, chapter: &ChapterInfo) {
        let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        let mut metadata = comic.clone();
        metadata.is_downloaded = None;
        metadata.comic_download_dir = None;
        metadata.local_cover_data = None;
        for ch in &mut metadata.chapter_infos {
            ch.is_downloaded = None;
            ch.chapter_download_dir = None;
        }
        zip.start_file("元数据.json", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(serde_json::to_string(&metadata).unwrap().as_bytes())
            .unwrap();
        zip.start_file("chapter.json", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(
            serde_json::to_string(&serde_json::json!({
                "comicId": comic.id, "chapterId": chapter.chapter_id
            }))
            .unwrap()
            .as_bytes(),
        )
        .unwrap();
        for page in [1, 2] {
            zip.start_file(format!("000{page}.jpg"), zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"not-a-real-jpeg").unwrap();
        }
        zip.finish().unwrap();
    }

    #[test]
    fn freshly_downloaded_chapter_shows_up_in_library_scan() {
        let root = std::env::temp_dir().join(format!("jm-scan-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();

        let chapter = ChapterInfo {
            chapter_id: 456,
            chapter_title: "第1话".into(),
            order: 1,
            is_downloaded: Some(true),
            chapter_download_dir: None,
        };
        let comic = Comic {
            id: 123,
            name: "测试本子".into(),
            chapter_infos: vec![chapter.clone()],
            ..Comic::default()
        };
        let path = root.join(crate::utils::storage_cbz_filename(&comic.name, comic.id, 456));
        write_downloaded_chapter(&path, &comic, &chapter);
        assert!(path.is_file(), "打包产物应当存在于书库根目录");

        let scanned = scan_downloaded_comics(&root);
        assert_eq!(scanned.len(), 1, "刚下载的本子必须出现在书库扫描结果里");
        assert_eq!(scanned[0].id, 123);
        assert_eq!(scanned[0].is_downloaded, Some(true));
        assert_eq!(
            scanned[0].chapter_infos[0].chapter_download_dir.as_deref(),
            Some(path.as_path())
        );

        std::fs::remove_dir_all(root).unwrap();
    }
}
