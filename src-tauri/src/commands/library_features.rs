use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, ensure, Context};
use serde::Serialize;
use specta::Type;
use tauri::{AppHandle, Manager};

use crate::comic_scanner::is_page_image;
use crate::errors::{CommandError, CommandResult};
use crate::extensions::{AppHandleExt, PathIsCbz, PathIsImg};
use crate::image_data::MAX_PAGE_BYTES;
use crate::library_catalog::{catalog_path, DownloadedLibraryCache, LibraryCatalog, CATALOG_LOCK};
use crate::types::{ChapterInfo, Comic, ComicInfo};
use crate::utils;

const MAX_CBZ_IMPORT_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_PAGES: usize = 10_000;

#[derive(Default, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    imported: usize,
    skipped: usize,
    errors: Vec<String>,
}

enum PreparedImport {
    RestoreKnown { comic_id: i64 },
    Imported { imported: bool, id: Option<i64> },
    RegisterPath { label: PathBuf, canonical: PathBuf },
    Error { label: PathBuf, error: String },
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
        // 扫描、拷贝与哈希全部在 CATALOG_LOCK 之外完成；只把加载/回写记录锁在最后。
        let prepared = prepare_imports(&paths, &download_dir, true);
        let _guard = CATALOG_LOCK.lock();
        let mut catalog = LibraryCatalog::load(&records_path)?;
        let report = apply_prepared_imports(&download_dir, &mut catalog, prepared);
        catalog.save(&records_path)?;
        Ok::<_, anyhow::Error>(report)
    })
    .await
    .map_err(|err| CommandError::from("导入任务失败", err))?
    .map_err(|err| CommandError::from("保存导入记录失败", err))?;
    app.state::<DownloadedLibraryCache>().invalidate();
    Ok(report)
}

#[cfg(test)]
fn import_batch(
    paths: Vec<String>,
    download_dir: &Path,
    catalog: &mut LibraryCatalog,
) -> ImportReport {
    let prepared = prepare_imports(&paths, download_dir, false);
    apply_prepared_imports(download_dir, catalog, prepared)
}

fn build_known_path_index(known: &[Comic]) -> std::collections::HashMap<PathBuf, i64> {
    let mut index = std::collections::HashMap::new();
    for comic in known {
        for chapter in &comic.chapter_infos {
            if let Some(path) = chapter.chapter_download_dir.as_ref() {
                if let Ok(canonical) = std::fs::canonicalize(path) {
                    index.entry(canonical).or_insert(comic.id);
                }
            }
        }
    }
    index
}

fn prepare_imports(paths: &[String], download_dir: &Path, copy_files: bool) -> Vec<PreparedImport> {
    let mut prepared = Vec::new();
    let known = super::library_commands::scan_downloaded_comics(download_dir).comics;
    let known_paths = build_known_path_index(&known);
    for source_str in paths {
        let source = Path::new(source_str);
        let files = if source.is_dir() {
            find_cbz_files(source)
        } else {
            Ok(vec![source.to_path_buf()])
        };
        match files {
            Ok(files) if files.is_empty() => match import_source(source, download_dir) {
                Ok((imported, id)) => prepared.push(PreparedImport::Imported { imported, id }),
                Err(error) => prepared.push(PreparedImport::Error {
                    label: source.to_path_buf(),
                    error: format!("{error:#}"),
                }),
            },
            Ok(files) => {
                for file in files {
                    let item = (|| -> anyhow::Result<PreparedImport> {
                        ensure!(file.is_cbz(), "请选择 CBZ 文件");
                        let canonical = std::fs::canonicalize(&file)?;
                        if let Some(&comic_id) = known_paths.get(&canonical) {
                            validate_archive_structure(&canonical)?;
                            return Ok(PreparedImport::RestoreKnown { comic_id });
                        }
                        if copy_files {
                            let (imported, id) = import_source(&canonical, download_dir)?;
                            Ok(PreparedImport::Imported { imported, id })
                        } else {
                            validate_archive_structure(&canonical)?;
                            Ok(PreparedImport::RegisterPath {
                                label: file.clone(),
                                canonical,
                            })
                        }
                    })();
                    match item {
                        Ok(item) => prepared.push(item),
                        Err(error) => prepared.push(PreparedImport::Error {
                            label: file,
                            error: format!("{error:#}"),
                        }),
                    }
                }
            }
            Err(error) => prepared.push(PreparedImport::Error {
                label: source.to_path_buf(),
                error: format!("{error:#}"),
            }),
        }
    }
    prepared
}

fn apply_prepared_imports(
    download_dir: &Path,
    catalog: &mut LibraryCatalog,
    prepared: Vec<PreparedImport>,
) -> ImportReport {
    let mut report = ImportReport::default();
    for item in prepared {
        match item {
            PreparedImport::RestoreKnown { comic_id } => {
                let restored = catalog.restore(download_dir, comic_id);
                if restored {
                    report.imported += 1;
                } else {
                    report.skipped += 1;
                }
            }
            PreparedImport::Imported { imported, id } => {
                let restored = id.is_some_and(|id| catalog.restore(download_dir, id));
                if restored || imported {
                    report.imported += 1;
                } else {
                    report.skipped += 1;
                }
            }
            PreparedImport::RegisterPath { label, canonical } => {
                match catalog.register(download_dir, &canonical) {
                    Ok((_, true)) => report.imported += 1,
                    Ok((_, false)) => report.skipped += 1,
                    Err(error) => report
                        .errors
                        .push(format!("{}：{error:#}", label.display())),
                }
            }
            PreparedImport::Error { label, error } => {
                report.errors.push(format!("{}：{error}", label.display()));
            }
        }
    }
    report
}

fn find_cbz_files(source: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in walkdir::WalkDir::new(source)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
    {
        let entry = entry.context("读取导入文件夹失败")?;
        if entry.file_type().is_file() && entry.path().is_cbz() {
            ensure!(files.len() < MAX_PAGES, "单次最多导入 10000 个 CBZ 文件");
            files.push(entry.into_path());
        }
    }
    files.sort_by(|a, b| utils::natural_cmp(&a.to_string_lossy(), &b.to_string_lossy()));
    Ok(files)
}

#[tauri::command(async)]
#[specta::specta]
pub async fn remove_library_comic(
    app: AppHandle,
    comic_id: i64,
    delete_files: bool,
) -> CommandResult<()> {
    tauri::async_runtime::spawn_blocking(move || {
        remove_library_comic_blocking(&app, comic_id, delete_files)
    })
    .await
    .map_err(|err| CommandError::from("移除漫画任务失败", err))?
}

fn remove_library_comic_blocking(
    app: &AppHandle,
    comic_id: i64,
    delete_files: bool,
) -> CommandResult<()> {
    let root = app.get_config().read().download_dir.clone();
    // 归档整合会在后台合并/重命名/删除这本漫画的章节文件。与删除并发时，
    // 可能出现"刚删掉的漫画又被合并写回"或整合中途失败。这里与清空下载目录
    // 采用同一保护：整合进行中直接拒绝，等它结束再删。
    let _removal_guard = app
        .try_state::<crate::archive_finalize::ArchiveFinalizeState>()
        .map(|state| {
            state
                .try_lock_comic_for_removal(app, comic_id)
                .ok_or_else(|| {
                    CommandError::from("移除漫画失败", anyhow!("该漫画正在整理 CBZ，请稍后再试"))
                })
        })
        .transpose()?;
    let refreshed = if delete_files {
        let manager = app.get_download_manager();
        for task in manager
            .snapshots()
            .into_iter()
            .filter(|task| task.comic.id == comic_id)
        {
            // cancel_download_task 对已完成/已取消任务是无操作，这里不必再复制一份
            // 可取消状态白名单（白名单漏掉 Failed 时会留下仍在重试的任务）。
            if let Err(error) = manager.cancel_download_task(task.chapter_info.chapter_id) {
                // 取消失败不阻断删除：任务可能刚好在取消前完成，继续收集文件路径。
                tracing::warn!(
                    comic_id,
                    chapter_id = task.chapter_info.chapter_id,
                    error = %error,
                    "删除前取消下载任务失败，继续删除"
                );
            }
        }
        Some(resolve_comic_for_delete(app, comic_id)?)
    } else {
        // 只删库存记录不依赖扫描结果：漫画已经不在书库里（或文件损坏被跳过）时也要能删掉记录。
        None
    };
    let _guard = CATALOG_LOCK.lock();
    let records_path = catalog_path(app);
    let mut catalog = LibraryCatalog::load(&records_path)
        .map_err(|err| CommandError::from("读取书库记录失败", err))?;
    let previous: LibraryCatalog = serde_json::from_value(
        serde_json::to_value(&catalog)
            .map_err(|err| CommandError::from("准备书库操作失败", err))?,
    )
    .map_err(|err| CommandError::from("准备书库操作失败", err))?;
    let paths = if let Some(refreshed) = refreshed.as_ref() {
        collect_delete_paths(&root, refreshed, &catalog)
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
    app.state::<DownloadedLibraryCache>().remove(comic_id);
    tracing::info!(comic_id, delete_files, "移除书库记录成功");
    Ok(())
}

/// 删除文件前解析漫画：优先用书库扫描缓存；缓存里没有时一次扫描即可，
/// 并且包含已移除记录，避免「文件还在却再也删不掉」或重复全量扫描。
fn resolve_comic_for_delete(app: &AppHandle, comic_id: i64) -> CommandResult<Comic> {
    super::reader_commands::local_comic_including_removed(app, comic_id)
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
    let mut files = HashSet::new();
    let mut dir_roots = Vec::new();
    if canonical_source.is_dir() && canonical_source != canonical_root {
        utils::validate_delete_target(&canonical_root, &canonical_source)?;
        dir_roots.push(canonical_source.clone());
        files.insert(canonical_source.clone());
    }
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
        ensure!(
            path.is_file() || (path.is_dir() && path != canonical_root),
            "删除路径无效"
        );
        if dir_roots.iter().any(|dir| path.starts_with(dir)) {
            continue;
        }
        files.insert(path);
    }
    if comic.id > 0 {
        let comic_name_filtered = utils::filename_filter(&comic.name);
        let id_chapter_tag = format!("[JM{}-", comic.id);
        let id_merged_tag = format!("[JM{}].cbz", comic.id);
        if let Ok(entries) = std::fs::read_dir(&canonical_root) {
            for entry in entries.filter_map(Result::ok) {
                let entry_path = entry.path();
                if !entry_path.is_file() || !entry_path.is_cbz() {
                    continue;
                }
                let Some(file_name) = entry_path.file_name().and_then(|name| name.to_str()) else {
                    continue;
                };
                let name_candidate = file_name.contains(&id_chapter_tag)
                    || file_name.ends_with(&id_merged_tag)
                    || (!comic_name_filtered.is_empty()
                        && file_name.starts_with(&comic_name_filtered));
                if !name_candidate {
                    continue;
                }
                if crate::comic_scanner::read_comic_id_from_cbz(&entry_path).ok() == Some(comic.id)
                {
                    if let Ok(canonical_entry) = std::fs::canonicalize(&entry_path) {
                        if utils::validate_delete_target(&canonical_root, &canonical_entry).is_ok()
                        {
                            files.insert(canonical_entry);
                        }
                    }
                }
            }
        }
    }
    // 若目录本身在待删列表中，去掉位于其内部的冗余子路径，避免二次移动出错。
    let dirs: Vec<PathBuf> = files.iter().filter(|path| path.is_dir()).cloned().collect();
    files.retain(|path| !dirs.iter().any(|dir| path != dir && path.starts_with(dir)));
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

fn fingerprint_to_id(fingerprint: md5::Context) -> anyhow::Result<i64> {
    let key = format!("{:x}", fingerprint.finalize());
    Ok(-i64::from_str_radix(&key[..13], 16)?.max(1))
}

fn copy_file_with_md5(source: &Path, dest: &Path) -> anyhow::Result<i64> {
    use std::io::{Read, Write};
    let metadata = std::fs::metadata(source)?;
    ensure!(metadata.len() <= MAX_CBZ_IMPORT_BYTES, "归档文件超过 2 GB");
    let mut src = std::fs::File::open(source)?;
    let mut dst = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dest)?;
    let mut fingerprint = md5::Context::new();
    let mut buffer = [0_u8; 65536];
    let mut total_written: u64 = 0;
    loop {
        let count = src.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        total_written += count as u64;
        ensure!(total_written <= MAX_CBZ_IMPORT_BYTES, "归档文件超过 2 GB");
        fingerprint.consume(&buffer[..count]);
        dst.write_all(&buffer[..count])?;
    }
    dst.flush()?;
    drop(dst);
    fingerprint_to_id(fingerprint)
}

fn source_id(source: &Path) -> anyhow::Result<i64> {
    use std::io::Read;
    let metadata = std::fs::metadata(source)?;
    let mut fingerprint = md5::Context::new();
    if source.is_file() {
        ensure!(metadata.len() <= MAX_CBZ_IMPORT_BYTES, "归档文件超过 2 GB");
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
            if entry.file_type().is_file() && (entry.path().is_img() || entry.path().is_cbz()) {
                let metadata = entry.metadata()?;
                fingerprint.consume(
                    entry
                        .path()
                        .strip_prefix(source)?
                        .to_string_lossy()
                        .as_bytes(),
                );
                ensure!(metadata.len() <= MAX_CBZ_IMPORT_BYTES, "导入文件过大");
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
    fingerprint_to_id(fingerprint)
}

fn import_cbz_source(source: &Path, download_dir: &Path) -> anyhow::Result<(bool, Option<i64>)> {
    ensure!(source.is_cbz(), "目前支持 CBZ 文件和图片文件夹");
    let imported_root = utils::create_dir_all_beneath(download_dir, Path::new("本地导入"))?;
    let staging = imported_root.join(format!(".import-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&staging)?;

    let mut result_id = None;
    let outcome = (|| -> anyhow::Result<bool> {
        let title = format!(
            "第1话 {}",
            source.file_stem().unwrap_or_default().to_string_lossy()
        );
        let staged_cbz = staging.join(format!("{}.cbz", utils::filename_filter(&title)));
        // 单个 CBZ 在锁外完成拷贝与流式 MD5 计算（单遍读盘），并一次性验证结构与提取轻量元数据。
        let id = copy_file_with_md5(source, &staged_cbz)?;
        result_id = Some(id);
        let target = imported_root.join(format!("local-{}", -id));
        if target.exists() {
            return Ok(false);
        }

        let summary = validate_cbz_and_extract_metadata(&staged_cbz)?;
        let default_name = source
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or("本地漫画")
            .to_owned();
        let (name, author, tags) = match summary {
            Some((parsed_name, parsed_author, parsed_tags)) => (
                parsed_name.unwrap_or(default_name),
                parsed_author,
                parsed_tags,
            ),
            None => (default_name, vec!["本地导入".into()], Vec::new()),
        };

        let chapters = crate::comic_scanner::inspect_cbz_chapters(&staged_cbz)?;
        let chapter_infos = if chapters.is_empty() {
            vec![ChapterInfo {
                chapter_id: id - 1,
                chapter_title: title,
                order: 1,
                is_downloaded: None,
                chapter_download_dir: None,
            }]
        } else {
            chapters
                .into_iter()
                .enumerate()
                .map(|(idx, (order, chapter_title))| ChapterInfo {
                    chapter_id: id.saturating_sub(i64::try_from(idx + 1).unwrap_or(i64::MAX)),
                    chapter_title,
                    order,
                    is_downloaded: None,
                    chapter_download_dir: None,
                })
                .collect()
        };

        let comic = Comic {
            id,
            name,
            author,
            tags,
            comic_download_dir: Some(staging.clone()),
            chapter_infos,
            ..Comic::default()
        };
        comic.save_comic_metadata()?;
        ensure!(!target.exists(), "该文件已由另一项导入任务加入书库");
        std::fs::rename(&staging, &target)?;
        Ok(true)
    })();

    match outcome {
        Ok(true) => Ok((true, result_id)),
        Ok(false) => {
            let _ = std::fs::remove_dir_all(&staging);
            Ok((false, result_id))
        }
        Err(error) => {
            let _ = std::fs::remove_dir_all(&staging);
            Err(error)
        }
    }
}

fn import_source(source: &Path, download_dir: &Path) -> anyhow::Result<(bool, Option<i64>)> {
    let source = std::fs::canonicalize(source).context("源文件不存在或无法访问")?;
    if std::fs::canonicalize(download_dir).is_ok_and(|root| source.starts_with(root)) {
        if source.is_dir() {
            let metadata_path = source.join("元数据.json");
            if metadata_path.is_file() {
                if let Ok(comic) = Comic::from_metadata(&metadata_path) {
                    return Ok((false, Some(comic.id)));
                }
            }
        }
        return Ok((false, None));
    }
    if source.is_file() {
        return import_cbz_source(&source, download_dir);
    }

    let id = source_id(&source)?;
    let imported_root = utils::create_dir_all_beneath(download_dir, Path::new("本地导入"))?;
    let target = imported_root.join(format!("local-{}", -id));
    if target.exists() {
        return Ok((false, Some(id)));
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
        let children = sorted_files(&source)?;
        let groups = if children.iter().any(|path| path.is_file() && path.is_img()) {
            vec![source.clone()]
        } else {
            children
                .into_iter()
                .filter(|path| path.is_dir() || path.is_cbz())
                .collect()
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
    Ok((true, Some(id)))
}

type ImportedComicSummary = (Option<String>, Vec<String>, Vec<String>);

fn extract_comic_summary_from_zip<R: std::io::Read + std::io::Seek>(
    zip: &mut zip::ZipArchive<R>,
) -> Option<ImportedComicSummary> {
    for metadata_name in ["元数据.json", "metadata.json"] {
        if let Ok(mut entry) = zip.by_name(metadata_name) {
            let declared_size = entry.size();
            if let Ok(content) = crate::image_data::read_limited(
                &mut entry,
                declared_size,
                crate::image_data::MAX_METADATA_BYTES,
            ) {
                if let Ok(comic) = serde_json::from_slice::<Comic>(&content) {
                    return Some((Some(comic.name), comic.author, comic.tags));
                }
            }
        }
    }
    if let Ok(mut entry) = zip.by_name("ComicInfo.xml") {
        let declared_size = entry.size();
        if let Ok(content) = crate::image_data::read_limited(
            &mut entry,
            declared_size,
            crate::image_data::MAX_METADATA_BYTES,
        ) {
            if let Ok(info) = yaserde::de::from_reader::<_, ComicInfo>(content.as_slice()) {
                let authors = if info.writer.is_empty() {
                    Vec::new()
                } else {
                    info.writer
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .collect()
                };
                let tags = if info.tags.is_empty() {
                    Vec::new()
                } else {
                    info.tags.split(',').map(|s| s.trim().to_string()).collect()
                };
                let name = (!info.series.is_empty()).then_some(info.series);
                return Some((name, authors, tags));
            }
        }
    }
    None
}

fn validate_zip_entries<R: std::io::Read + std::io::Seek>(
    zip: &mut zip::ZipArchive<R>,
) -> anyhow::Result<()> {
    ensure!(zip.len() <= MAX_PAGES + 100, "压缩包条目过多");
    let mut pages = 0;
    for index in 0..zip.len() {
        let entry = zip.by_index(index)?;
        ensure!(entry.size() <= MAX_PAGE_BYTES, "压缩包中的文件过大");
        ensure!(
            entry.enclosed_name().is_some(),
            "压缩包包含不安全的文件路径"
        );
        let entry_name = entry.name()?;
        if is_page_image(entry_name.as_ref()) {
            ensure!(entry.size() <= MAX_PAGE_BYTES, "压缩包中的图片过大");
            pages += 1;
        }
    }
    ensure!(pages > 0, "压缩包没有漫画图片");
    Ok(())
}

fn validate_cbz_and_extract_metadata(path: &Path) -> anyhow::Result<Option<ImportedComicSummary>> {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(path)?)?;
    validate_zip_entries(&mut zip)?;
    Ok(extract_comic_summary_from_zip(&mut zip))
}

fn validate_archive_structure(path: &Path) -> anyhow::Result<()> {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(path)?)?;
    validate_zip_entries(&mut zip)
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
        assert!(import_source(&source, &library).unwrap().0);
        assert!(!import_source(&source, &library).unwrap().0);
        let comics = super::super::library_commands::scan_downloaded_comics(&library).comics;
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
        assert!(import_source(&source, &library).unwrap().0);
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
        assert!(import_source(&source, &library).unwrap().0);
        let comics = super::super::library_commands::scan_downloaded_comics(&library).comics;
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
            super::super::library_commands::scan_downloaded_comics(&library)
                .comics
                .len(),
            1
        );
        assert!(source.exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn import_rejects_cbz_with_only_hidden_images() {
        let root = std::env::temp_dir().join(format!("jm-hidden-cbz-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("hidden.cbz");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&source).unwrap());
        zip.start_file("_cover.jpg", SimpleFileOptions::default())
            .unwrap();
        zip.write_all(&png()).unwrap();
        zip.finish().unwrap();
        let library = root.join("library");
        let error = import_source(&source, &library).unwrap_err();
        assert!(format!("{error:#}").contains("压缩包没有漫画图片"));
        assert!(
            super::super::library_commands::scan_downloaded_comics(&library)
                .comics
                .is_empty()
        );
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
        // 删除后该书不再出现在可见导入中，图片协议与删除逻辑都不会再放行它的文件
        assert!(!catalog
            .visible_imports(&library)
            .iter()
            .any(|record| record.id == comic.id));
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

    #[test]
    fn single_pass_cbz_import_matches_source_id() {
        let root = std::env::temp_dir().join(format!("jm-cbz-md5-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("单遍测试.cbz");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&source).unwrap());
        zip.start_file("0001.png", SimpleFileOptions::default())
            .unwrap();
        zip.write_all(&png()).unwrap();
        zip.finish().unwrap();

        let expected_id = source_id(&source).unwrap();
        let library = root.join("library");
        let (imported, actual_id) = import_source(&source, &library).unwrap();
        assert!(imported);
        assert_eq!(actual_id, Some(expected_id));
        assert!(library
            .join("本地导入")
            .join(format!("local-{}", -expected_id))
            .is_dir());
        std::fs::remove_dir_all(root).unwrap();
    }
}
