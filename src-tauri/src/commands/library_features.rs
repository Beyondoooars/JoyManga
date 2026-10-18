use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, ensure, Context};
use serde::Serialize;
use specta::Type;
use tauri::{AppHandle, Manager};

use crate::errors::{CommandError, CommandResult};
use crate::extensions::{AppHandleExt, PathIsImg};
use crate::library_catalog::{catalog_path, LibraryCatalog, CATALOG_LOCK};
use crate::types::{ChapterInfo, Comic};
use crate::utils;

const MAX_PAGE_BYTES: u64 = 32 * 1024 * 1024;
const MAX_PAGES: usize = 10_000;

#[derive(Default, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    imported: usize,
    skipped: usize,
    errors: Vec<String>,
}

#[tauri::command(async)]
#[specta::specta]
pub async fn import_local_comics(
    app: AppHandle,
    paths: Vec<String>,
) -> CommandResult<ImportReport> {
    let download_dir = app.get_config().read().download_dir.clone();
    ensure_import_batch(&paths).map_err(|err| CommandError::from("导入失败", err))?;
    let records_path = catalog_path(&app);
    let report = tokio::task::spawn_blocking(move || {
        let _guard = CATALOG_LOCK.lock();
        let mut catalog = LibraryCatalog::load(&records_path)?;
        let report = import_batch_with_mode(paths, &download_dir, &mut catalog, true);
        catalog.save(&records_path)?;
        Ok::<_, anyhow::Error>(report)
    })
    .await
    .map_err(|err| CommandError::from("导入任务失败", err))?
    .map_err(|err| CommandError::from("保存导入记录失败", err))?;
    app.state::<utils::DownloadedLibraryCache>().invalidate();
    Ok(report)
}

#[cfg(test)]
fn import_batch(
    paths: Vec<String>,
    download_dir: &Path,
    catalog: &mut LibraryCatalog,
) -> ImportReport {
    import_batch_with_mode(paths, download_dir, catalog, false)
}

fn import_batch_with_mode(
    paths: Vec<String>,
    download_dir: &Path,
    catalog: &mut LibraryCatalog,
    copy_files: bool,
) -> ImportReport {
    let mut report = ImportReport::default();
    let known = super::library_commands::scan_downloaded_comics(download_dir);
    for source in paths {
        let source = Path::new(&source);
        let files = if source.is_dir() {
            find_cbz_files(source)
        } else {
            Ok(vec![source.to_path_buf()])
        };
        match files {
            Ok(files) if files.is_empty() => {
                record_import_result(&mut report, source, import_source(source, download_dir));
            }
            Ok(files) => {
                for file in files {
                    let result = (|| -> anyhow::Result<bool> {
                        ensure!(is_cbz(&file), "请选择 CBZ 文件");
                        validate_archive_structure(&file)?;
                        let canonical = std::fs::canonicalize(&file)?;
                        if let Some(comic) = known.iter().find(|comic| {
                            comic.chapter_infos.iter().any(|chapter| {
                                chapter.chapter_download_dir.as_ref().is_some_and(|path| {
                                    std::fs::canonicalize(path).is_ok_and(|path| path == canonical)
                                })
                            })
                        }) {
                            return Ok(catalog.restore(download_dir, comic.id));
                        }
                        if copy_files {
                            let imported = import_source(&canonical, download_dir)?;
                            let id = source_id(&canonical)?;
                            Ok(catalog.restore(download_dir, id) || imported)
                        } else {
                            Ok(catalog.register(download_dir, &canonical)?.1)
                        }
                    })();
                    record_import_result(&mut report, &file, result);
                }
            }
            Err(error) => report
                .errors
                .push(format!("{}：{error:#}", source.display())),
        }
    }
    report
}

fn record_import_result(report: &mut ImportReport, source: &Path, result: anyhow::Result<bool>) {
    match result {
        Ok(true) => report.imported += 1,
        Ok(false) => report.skipped += 1,
        Err(error) => report
            .errors
            .push(format!("{}：{error:#}", source.display())),
    }
}

fn find_cbz_files(source: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in walkdir::WalkDir::new(source)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
    {
        let entry = entry.context("读取导入文件夹失败")?;
        if entry.file_type().is_file() && is_cbz(entry.path()) {
            ensure!(files.len() < MAX_PAGES, "单次最多导入 10000 个 CBZ 文件");
            files.push(entry.into_path());
        }
    }
    files.sort_by(|a, b| utils::natural_cmp(&a.to_string_lossy(), &b.to_string_lossy()));
    Ok(files)
}

#[tauri::command(async)]
#[specta::specta]
pub fn remove_library_comic(
    app: AppHandle,
    comic_id: i64,
    delete_files: bool,
) -> CommandResult<()> {
    let root = app.get_config().read().download_dir.clone();
    // 归档整合会在后台合并/重命名/删除这本漫画的章节文件。与删除并发时，
    // 可能出现"刚删掉的漫画又被合并写回"或整合中途失败。这里与清空下载目录
    // 采用同一保护：整合进行中直接拒绝，等它结束再删。
    if let Some(state) = app.try_state::<crate::archive_finalize::ArchiveFinalizeState>() {
        if state.is_running(comic_id) {
            return Err(CommandError::from(
                "移除漫画失败",
                anyhow!("该漫画正在整理 CBZ，请稍后再试"),
            ));
        }
    }
    if delete_files {
        let manager = app.get_download_manager();
        for task in manager
            .snapshots()
            .into_iter()
            .filter(|task| task.comic.id == comic_id)
        {
            manager
                .cancel_download_task(task.chapter_info.chapter_id)
                .map_err(|err| CommandError::from("停止下载任务失败", err))?;
        }
    }
    let refreshed = super::reader_commands::local_comic(&app, comic_id)?;
    let _guard = CATALOG_LOCK.lock();
    let records_path = catalog_path(&app);
    let mut catalog = LibraryCatalog::load(&records_path)
        .map_err(|err| CommandError::from("读取书库记录失败", err))?;
    let previous: LibraryCatalog = serde_json::from_value(
        serde_json::to_value(&catalog)
            .map_err(|err| CommandError::from("准备书库操作失败", err))?,
    )
    .map_err(|err| CommandError::from("准备书库操作失败", err))?;
    let paths = if delete_files {
        collect_delete_paths(&root, &refreshed, &catalog)
            .map_err(|err| CommandError::from("检查删除目标失败", err))?
    } else {
        Vec::new()
    };
    catalog.remove(&root, comic_id);
    if delete_files {
        crate::state_transactions::remove_with_catalog(&records_path, paths, &previous, &catalog)
            .map_err(|err| CommandError::from("移除漫画失败", err))?;
    } else {
        catalog
            .save(&records_path)
            .map_err(|err| CommandError::from("保存书库记录失败", err))?;
    }
    app.state::<utils::DownloadedLibraryCache>()
        .remove(comic_id);
    tracing::info!(comic_id, delete_files, "移除书库记录成功");
    Ok(())
}

fn collect_delete_paths(
    root: &Path,
    comic: &Comic,
    catalog: &LibraryCatalog,
) -> anyhow::Result<Vec<PathBuf>> {
    let source = comic
        .comic_download_dir
        .as_ref()
        .context("漫画没有本地路径")?;
    if !source.exists() {
        return Ok(Vec::new());
    }
    let canonical_source = std::fs::canonicalize(source)?;
    if comic.id < 0
        && canonical_source.is_file()
        && catalog.visible_imports(root).iter().any(|record| {
            record.id == comic.id
                && std::fs::canonicalize(&record.path).is_ok_and(|path| path == canonical_source)
        })
    {
        return Ok(vec![canonical_source]);
    }
    let canonical_root = std::fs::canonicalize(root)?;
    if canonical_source.is_dir() && canonical_source != canonical_root {
        utils::validate_delete_target(&canonical_root, &canonical_source)?;
        return Ok(vec![canonical_source]);
    }
    let mut files = HashSet::new();
    for path in comic
        .chapter_infos
        .iter()
        .filter_map(|chapter| chapter.chapter_download_dir.as_ref())
        .chain(canonical_source.is_file().then_some(&canonical_source))
    {
        if !path.exists() {
            continue;
        }
        let path = std::fs::canonicalize(path)?;
        utils::validate_delete_target(&canonical_root, &path)?;
        ensure!(path.is_file(), "章节路径不是普通文件");
        files.insert(path);
    }
    Ok(files.into_iter().collect())
}

#[cfg(test)]
fn delete_comic_files(root: &Path, comic: &Comic, catalog: &LibraryCatalog) -> anyhow::Result<()> {
    for path in collect_delete_paths(root, comic, catalog)? {
        if path.is_dir() {
            std::fs::remove_dir_all(path)?;
        } else {
            std::fs::remove_file(path)?;
        }
    }
    Ok(())
}
fn ensure_import_batch(paths: &[String]) -> anyhow::Result<()> {
    ensure!(
        !paths.is_empty() && paths.len() <= 100,
        "每次请选择 1 至 100 个文件或文件夹"
    );
    Ok(())
}

fn sorted_files(dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        ensure!(
            !entry.file_type()?.is_symlink(),
            "导入目录包含链接，请选择普通文件夹"
        );
        files.push(entry.path());
    }
    files.sort_by(|a, b| utils::natural_cmp(&a.to_string_lossy(), &b.to_string_lossy()));
    Ok(files)
}

fn is_cbz(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("cbz"))
}

fn source_id(source: &Path) -> anyhow::Result<i64> {
    use std::io::Read;
    let metadata = std::fs::metadata(source)?;
    let mut fingerprint = md5::Context::new();
    if source.is_file() {
        ensure!(
            metadata.len() <= 2 * 1024 * 1024 * 1024,
            "归档文件超过 2 GB"
        );
        let mut file = std::fs::File::open(source)?;
        let mut buffer = [0_u8; 65536];
        loop {
            let count = file.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            fingerprint.consume(&buffer[..count]);
        }
    } else {
        for entry in walkdir::WalkDir::new(source)
            .max_depth(2)
            .follow_links(false)
            .sort_by_file_name()
            .into_iter()
            .take(MAX_PAGES + 1)
        {
            let entry = entry?;
            ensure!(!entry.file_type().is_symlink(), "源目录包含链接");
            if entry.file_type().is_file() && (entry.path().is_img() || is_cbz(entry.path())) {
                let metadata = entry.metadata()?;
                fingerprint.consume(
                    entry
                        .path()
                        .strip_prefix(source)?
                        .to_string_lossy()
                        .as_bytes(),
                );
                ensure!(metadata.len() <= 2 * 1024 * 1024 * 1024, "导入文件过大");
                let mut file = std::fs::File::open(entry.path())?;
                let mut buffer = [0_u8; 65536];
                loop {
                    let count = file.read(&mut buffer)?;
                    if count == 0 {
                        break;
                    }
                    fingerprint.consume(&buffer[..count]);
                }
            }
        }
    }
    let key = format!("{:x}", fingerprint.compute());
    Ok(-i64::from_str_radix(&key[..13], 16)?.max(1))
}

fn import_source(source: &Path, download_dir: &Path) -> anyhow::Result<bool> {
    let source = std::fs::canonicalize(source).context("源文件不存在或无法访问")?;
    if std::fs::canonicalize(download_dir).is_ok_and(|root| source.starts_with(root)) {
        return Ok(false);
    }
    let id = source_id(&source)?;
    let imported_root = utils::create_dir_all_beneath(download_dir, Path::new("本地导入"))?;
    let target = imported_root.join(format!("local-{}", -id));
    if target.exists() {
        return Ok(false);
    }
    let staging = imported_root.join(format!(".import-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&staging)?;
    let result = (|| -> anyhow::Result<()> {
        let mut comic = Comic {
            id,
            name: source
                .file_stem()
                .and_then(|name| name.to_str())
                .unwrap_or("本地漫画")
                .to_owned(),
            author: vec!["本地导入".into()],
            comic_download_dir: Some(staging.clone()),
            ..Comic::default()
        };
        let groups = if source.is_file() {
            ensure!(is_cbz(&source), "目前支持 CBZ 文件和图片文件夹");
            validate_archive_structure(&source)?;
            if let Ok(original) = Comic::from_cbz(&source) {
                comic.name = original.name;
                comic.author = original.author;
                comic.tags = original.tags;
            }
            vec![source.clone()]
        } else {
            let children = sorted_files(&source)?;
            if children.iter().any(|path| path.is_file() && path.is_img()) {
                vec![source.clone()]
            } else {
                children
                    .into_iter()
                    .filter(|path| path.is_dir() || is_cbz(path))
                    .collect()
            }
        };
        ensure!(
            !groups.is_empty() && groups.len() <= 1_000,
            "未找到图片、章节文件夹或 CBZ 文件"
        );
        for group in groups {
            let order = i64::try_from(comic.chapter_infos.len() + 1)?;
            let title = format!(
                "第{order}话 {}",
                group.file_stem().unwrap_or_default().to_string_lossy()
            );
            if group.is_file() {
                validate_archive_structure(&group)?;
                std::fs::copy(
                    &group,
                    staging.join(format!("{}.cbz", utils::filename_filter(&title))),
                )?;
            } else {
                let images: Vec<_> = sorted_files(&group)?
                    .into_iter()
                    .filter(|path| path.is_file() && path.is_img())
                    .collect();
                if images.is_empty() {
                    continue;
                }
                ensure!(images.len() <= MAX_PAGES, "单章图片过多");
                let chapter_dir = staging.join(utils::filename_filter(&title));
                std::fs::create_dir(&chapter_dir)?;
                for (index, image) in images.iter().enumerate() {
                    ensure!(
                        std::fs::metadata(image)?.len() <= MAX_PAGE_BYTES,
                        "图片过大：{}",
                        image.display()
                    );
                    let extension = image.extension().unwrap().to_string_lossy();
                    std::fs::copy(
                        image,
                        chapter_dir.join(format!("{:04}.{extension}", index + 1)),
                    )?;
                }
            }
            comic.chapter_infos.push(ChapterInfo {
                chapter_id: id - order,
                chapter_title: title,
                order,
                is_downloaded: None,
                chapter_download_dir: None,
            });
        }
        ensure!(!comic.chapter_infos.is_empty(), "未找到可导入的漫画页面");
        comic.save_comic_metadata()?;
        ensure!(!target.exists(), "该文件已由另一项导入任务加入书库");
        std::fs::rename(&staging, &target)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    result?;
    Ok(true)
}

fn validate_archive_structure(path: &Path) -> anyhow::Result<()> {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(path)?)?;
    ensure!(zip.len() <= MAX_PAGES + 100, "压缩包条目过多");
    let mut pages = 0;
    for index in 0..zip.len() {
        let entry = zip.by_index(index)?;
        ensure!(entry.size() <= MAX_PAGE_BYTES, "压缩包中的文件过大");
        ensure!(
            entry.enclosed_name().is_some(),
            "压缩包包含不安全的文件路径"
        );
        if Path::new(entry.name()).is_img() {
            ensure!(entry.size() <= MAX_PAGE_BYTES, "压缩包中的图片过大");
            pages += 1;
        }
    }
    ensure!(pages > 0, "压缩包没有漫画图片");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    fn png() -> Vec<u8> {
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::RgbImage::new(2, 2)
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        bytes.into_inner()
    }

    #[test]
    fn import_plain_images_preserves_source_and_is_idempotent() {
        let root = std::env::temp_dir().join(format!("jm-import-{}", uuid::Uuid::new_v4()));
        let source = root.join("原始漫画");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("page2.png"), png()).unwrap();
        std::fs::write(source.join("page10.png"), png()).unwrap();
        let library = root.join("library");
        assert!(import_source(&source, &library).unwrap());
        assert!(!import_source(&source, &library).unwrap());
        let comics = super::super::library_commands::scan_downloaded_comics(&library);
        assert_eq!(comics.len(), 1);
        assert!(comics[0].id < 0);
        assert!(comics[0].chapter_infos[0].is_downloaded.unwrap());
        assert_eq!(
            super::super::reader_commands::chapter_reader_data_from_comic(
                comics[0].clone(),
                comics[0].chapter_infos[0].chapter_id,
            )
            .unwrap()
            .total_images,
            2
        );
        assert!(source.join("page2.png").exists());
        assert!(comics[0].local_cover_data.is_some());
        std::fs::write(source.join("page2.png"), b"new-source-content").unwrap();
        assert!(import_source(&source, &library).unwrap());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn import_cbz_without_metadata_and_reject_unsafe_archives() {
        let root = std::env::temp_dir().join(format!("jm-cbz-import-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("普通漫画.cbz");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&source).unwrap());
        zip.start_file("0001.png", SimpleFileOptions::default())
            .unwrap();
        zip.write_all(&png()).unwrap();
        zip.finish().unwrap();
        let library = root.join("library");
        assert!(import_source(&source, &library).unwrap());
        let comics = super::super::library_commands::scan_downloaded_comics(&library);
        assert_eq!(comics.len(), 1);
        assert_eq!(
            super::super::reader_commands::chapter_reader_data_from_comic(
                comics[0].clone(),
                comics[0].chapter_infos[0].chapter_id,
            )
            .unwrap()
            .total_images,
            1
        );
        let invalid = root.join("unsafe.cbz");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&invalid).unwrap());
        zip.start_file("../0001.png", SimpleFileOptions::default())
            .unwrap();
        zip.write_all(&png()).unwrap();
        zip.finish().unwrap();
        assert!(import_source(&invalid, &library).is_err());
        assert_eq!(
            super::super::library_commands::scan_downloaded_comics(&library).len(),
            1
        );
        assert!(source.exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn folder_import_registers_nested_cbz_books_individually_and_deletion_is_explicit() {
        let root = std::env::temp_dir().join(format!("jm-folder-import-{}", uuid::Uuid::new_v4()));
        let source = root.join("source");
        let nested = source.join("nested/deeper");
        let library = root.join("library");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::create_dir_all(&library).unwrap();
        for path in [source.join("book1.cbz"), nested.join("book2.CBZ")] {
            let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
            zip.start_file("0001.png", SimpleFileOptions::default())
                .unwrap();
            zip.write_all(&png()).unwrap();
            zip.finish().unwrap();
        }
        std::fs::write(source.join("broken.cbz"), b"not an archive").unwrap();
        let mut catalog = LibraryCatalog::default();
        let report = import_batch(
            vec![source.to_string_lossy().into_owned()],
            &library,
            &mut catalog,
        );
        assert_eq!(report.imported, 2);
        assert_eq!(report.errors.len(), 1);
        let records = catalog.visible_imports(&library);
        assert_eq!(records.len(), 2);
        let comic = crate::library_catalog::registered_cbz_comic(&records[0]).unwrap();
        assert_eq!(
            super::super::reader_commands::chapter_reader_data_from_comic(
                comic.clone(),
                comic.chapter_infos[0].chapter_id,
            )
            .unwrap()
            .total_images,
            1
        );
        catalog.remove(&library, comic.id);
        assert!(comic.comic_download_dir.as_ref().unwrap().is_file());
        assert!(!catalog.allows_cbz(&library, comic.comic_download_dir.as_ref().unwrap()));
        catalog.restore(&library, comic.id);
        let again = import_batch(
            vec![source.to_string_lossy().into_owned()],
            &library,
            &mut catalog,
        );
        assert_eq!(again.skipped, 2);
        delete_comic_files(&library, &comic, &catalog).unwrap();
        assert!(!comic.comic_download_dir.as_ref().unwrap().exists());
        assert!(records[1].path.exists());
        let mut forged = comic.clone();
        forged.comic_download_dir = Some(records[1].path.clone());
        forged.id = 123;
        assert!(delete_comic_files(&library, &forged, &catalog).is_err());
        assert!(records[1].path.exists());
        std::fs::remove_dir_all(root).unwrap();
    }
}
