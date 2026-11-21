use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{anyhow, Context};
use parking_lot::Mutex;
use tauri::AppHandle;

use crate::errors::{CommandError, CommandResult};
use crate::extensions::{mime_for_image, AppHandleExt};
use crate::types::Comic;
use crate::utils;

struct CachedArchive {
    path: PathBuf,
    modified: Option<std::time::SystemTime>,
    archive: zip::ZipArchive<std::fs::File>,
}

/// 阅读器当前打开的 CBZ。阅读是一页一页的，单槽缓存足够，且避免长期占着文件句柄。
static ACTIVE_ARCHIVE: OnceLock<Mutex<Option<CachedArchive>>> = OnceLock::new();

fn archive_matches_target(cached_path: &Path, canonical_target: &Path) -> bool {
    cached_path == canonical_target || cached_path.starts_with(canonical_target)
}

/// 提交（重命名/覆盖）目标文件前释放占用的读句柄；只放掉匹配目标的句柄，
/// 并在单次 `ACTIVE_ARCHIVE` 持锁内完成校验与释放，避免分片锁下的 TOCTOU 竞态。
pub fn release_handle_for(target: &Path) {
    let Some(lock) = ACTIVE_ARCHIVE.get() else {
        return;
    };
    let canonical = std::fs::canonicalize(target).unwrap_or_else(|_| target.to_path_buf());
    let mut guard = lock.lock();
    if guard
        .as_ref()
        .is_some_and(|cached| archive_matches_target(&cached.path, &canonical))
    {
        guard.take();
    }
}

/// 在启动时注册到 atomic_fs：提交方持有替换锁后调用，锁住「释放→改名」窗口。
pub fn install_commit_hook() {
    crate::atomic_fs::set_pre_commit_hook(release_handle_for);
}

/// 导入的 CBZ 路径集合（规范化后）。`jmimg://` 每一页都要校验路径，
/// 若每次都重读书库记录，翻页会产生大量同步读盘；这里按记录文件
/// `(mtime, 长度)` 失效。根目录也必须参与比较：`visible_imports` 以书库
/// 根目录为键，运行时切换书库位置后旧索引对新的根目录并不成立。
struct ImportedCbzIndex {
    root: PathBuf,
    fingerprint: Option<(std::time::SystemTime, u64)>,
    paths: Vec<PathBuf>,
}

static IMPORTED_CBZ_INDEX: OnceLock<Mutex<Option<ImportedCbzIndex>>> = OnceLock::new();

/// 前端读取页面图片使用的自定义协议；Android 上地址为 `http://jmimg.localhost/<path>`。
pub const IMAGE_SCHEME: &str = "jmimg";

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
    /// 与 `image_paths` 等长；`None` 表示该页尺寸不可知（文件损坏或文件头超出读取预算），
    /// 阅读器不为它预留高度。
    pub page_sizes: Vec<Option<crate::types::PageSize>>,
}

#[tauri::command(async)]
#[specta::specta]
pub async fn get_chapter_reader_data(
    app: AppHandle,
    comic_id: i64,
    chapter_id: i64,
) -> CommandResult<ChapterReaderData> {
    tauri::async_runtime::spawn_blocking(move || {
        let comic = local_comic(&app, comic_id)?;
        chapter_reader_data_from_comic(comic, chapter_id)
    })
    .await
    .map_err(|err| CommandError::from("读取章节失败", err))?
}

pub(crate) fn local_comic(app: &AppHandle, comic_id: i64) -> CommandResult<Comic> {
    super::library_commands::find_local_comic(app, comic_id, false)
        .map_err(|err| CommandError::from("读取本地漫画失败", err))
}

/// 删除流程专用：解析漫画时包含已被“移除记录”隐藏的条目。
///
/// 之前的实现先调用 `local_comic`（没有命中时会全量扫描并过滤 removed），
/// 失败后再全量扫描一次；现在只需一次扫描即可找到“文件还在、记录已移除”的漫画。
pub(crate) fn local_comic_including_removed(
    app: &AppHandle,
    comic_id: i64,
) -> CommandResult<Comic> {
    super::library_commands::find_local_comic(app, comic_id, true)
        .map_err(|err| CommandError::from("读取本地漫画失败", err))
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

    // 两个分支各自产出 (路径列表, 等长尺寸列表)，else 分支直接返回错误。
    // 尺寸与路径在构造时一一对应，前端按下标取 `pageSizes` 的契约由此保证。
    let (image_paths_str, page_sizes): (Vec<String>, Vec<Option<crate::types::PageSize>>) =
        if chapter_target.is_file() {
            // 与 read_page_bytes 的句柄协议同序：先拿替换锁再开归档，
            // 提交方「释放句柄 → 重命名」的窗口内不会被这次打开插进
            //（FUSE 下会重新触发 ETXTBSY / 覆盖写截断）。
            // 尺寸解析要逐页读文件头，归档打开的时长不可忽略，必须纳入协议。
            let _replace_guard = crate::atomic_fs::commit_guard(chapter_target);
            let file = std::fs::File::open(chapter_target)
                .context(format!("打开章节CBZ文件`{}`失败", chapter_target.display()))
                .map_err(|err| CommandError::from("读取章节图片失败", err))?;
            let mut archive = zip::ZipArchive::new(file)
                .context(format!("解析章节CBZ文件`{}`失败", chapter_target.display()))
                .map_err(|err| CommandError::from("读取章节图片失败", err))?;

            let mut img_names = crate::comic_scanner::image_entry_names(&mut archive);

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
                let slice_order = crate::comic_scanner::read_merged_chapter_orders_from_archive(
                    &mut archive,
                    comic.id,
                )
                .get(&chapter_id)
                .copied()
                .unwrap_or(current_chapter.order);
                crate::comic_scanner::select_chapter_pages(&img_names, slice_order).ok_or_else(
                    || CommandError::from("读取章节失败", anyhow!("归档中没有该章节的页面")),
                )?
            };

            // 尺寸必须在把 img_names 挪走之前取：优先命中原归档自带的 pages.json，
            // 其次磁盘指纹缓存（导入/旧版书库免重读），最后现读文件头。
            // 取不到就是 None，阅读器该页不预留高度。
            let page_sizes =
                crate::page_size::resolve_archive_sizes(&mut archive, &img_names, chapter_target);

            let chapter_target_str = chapter_target.to_string_lossy();
            let image_paths_str = img_names
                .into_iter()
                .map(|entry_name| format!("{chapter_target_str}|{entry_name}"))
                .collect::<Vec<_>>();
            (image_paths_str, page_sizes)
        } else if chapter_target.is_dir() {
            let image_paths = crate::comic_scanner::collect_page_images(chapter_target)
                .map_err(|err| CommandError::from("读取章节图片失败", err))?;

            let page_sizes = image_paths
                .iter()
                .map(|path| crate::page_size::size_from_path(path))
                .collect();
            let image_paths_str = image_paths
                .into_iter()
                .map(|p| p.to_string_lossy().to_string())
                .collect::<Vec<_>>();
            (image_paths_str, page_sizes)
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
    debug_assert_eq!(
        page_sizes.len(),
        total_images,
        "page_sizes 与 image_paths 必须等长（前端按下标一一对应）"
    );

    Ok(ChapterReaderData {
        comic_id: comic.id,
        comic_title: comic.name,
        chapter_id,
        chapter_title: current_chapter.chapter_title.clone(),
        prev_chapter,
        next_chapter,
        total_images,
        image_paths: image_paths_str,
        page_sizes,
    })
}

/// 读取阅读器的一页，返回原始字节与 MIME。
///
/// `file_path` 有两种形态：目录书库的图片绝对路径，或 CBZ 的
/// `<归档路径>|<条目名>`。两条路径都会重新规范化并校验，防止前端传入越界路径。
fn read_page_bytes(app: &AppHandle, file_path: &str) -> anyhow::Result<(Vec<u8>, &'static str)> {
    let download_dir = app.get_config().read().download_dir.clone();
    let canonical_download_dir = std::fs::canonicalize(&download_dir)
        .with_context(|| format!("无法解析下载根目录`{}`", download_dir.display()))?;

    if let Some((cbz_path_str, entry_name)) = file_path.split_once('|') {
        let canonical_cbz_path = std::fs::canonicalize(cbz_path_str)
            .with_context(|| format!("无法解析CBZ路径`{cbz_path_str}`"))?;

        if !canonical_cbz_path.starts_with(&canonical_download_dir)
            && !is_imported_cbz(app, &canonical_download_dir, &canonical_cbz_path)
        {
            return Err(anyhow!("CBZ文件不在下载根目录内"));
        }

        let modified = std::fs::metadata(&canonical_cbz_path)
            .and_then(|m| m.modified())
            .ok();
        let active_lock = ACTIVE_ARCHIVE.get_or_init(|| Mutex::new(None));
        // 与 atomic_fs::REPLACE_LOCK 同序：先替换锁再归档锁。提交方会在持有替换锁时
        // 释放归档句柄，「释放 → 重命名」之间就不会插进一次重新打开，ETXTBSY 才真正消除。
        // 读写会在此短暂互斥：常规提交是毫秒级改名，只有目标被占用时的整文件覆盖会稍慢。
        let _replace_guard = crate::atomic_fs::commit_guard(&canonical_cbz_path);
        let mut guard = active_lock.lock();
        let is_match = guard
            .as_ref()
            .is_some_and(|cached| cached.path == canonical_cbz_path && cached.modified == modified);
        if !is_match {
            let file = std::fs::File::open(&canonical_cbz_path)
                .with_context(|| format!("打开CBZ文件`{}`失败", canonical_cbz_path.display()))?;
            let archive = zip::ZipArchive::new(file)
                .with_context(|| format!("解析CBZ文件`{}`失败", canonical_cbz_path.display()))?;
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
            .with_context(|| format!("CBZ内未找到图片条目`{entry_name}`"))?;

        let declared_size = entry.size();
        let bytes = crate::image_data::read_limited(
            &mut entry,
            declared_size,
            crate::image_data::MAX_PAGE_BYTES,
        )
        .and_then(|bytes| {
            crate::image_data::validate_image(&bytes)?;
            Ok(bytes)
        })?;
        drop(entry);
        drop(guard);

        let mime = mime_for_image(Path::new(entry_name));
        return Ok((bytes, mime));
    }

    let canonical_path = std::fs::canonicalize(file_path)
        .with_context(|| format!("无法解析图片路径`{file_path}`"))?;
    if !canonical_path.starts_with(&canonical_download_dir) {
        return Err(anyhow!("图片路径不在下载根目录内"));
    }
    let bytes = crate::image_data::read_image(&canonical_path)
        .with_context(|| format!("读取文件`{}`失败", canonical_path.display()))?;
    Ok((bytes, mime_for_image(&canonical_path)))
}

/// 判断 CBZ 是否来自「导入」：书库记录里登记过的外部归档。
///
/// 阅读时每页都会调用，所以结果按记录文件的修改时间缓存，避免逐页同步读盘。
fn is_imported_cbz(app: &AppHandle, canonical_root: &Path, cbz_path: &Path) -> bool {
    let catalog_path = crate::library_catalog::catalog_path(app);
    let fingerprint = std::fs::metadata(&catalog_path)
        .ok()
        .and_then(|meta| meta.modified().ok().map(|modified| (modified, meta.len())));
    // 调用方已规范化过书库根目录，直接用它作缓存键：既省一次 canonicalize，
    // 也避免目录暂时不可访问时键在「规范化/原始」两种形式间摆动导致缓存反复失效。
    let index_lock = IMPORTED_CBZ_INDEX.get_or_init(|| Mutex::new(None));
    let mut guard = index_lock.lock();
    let stale = guard
        .as_ref()
        .is_none_or(|index| index.fingerprint != fingerprint || index.root != canonical_root);
    if stale {
        let imported = {
            let _guard = crate::library_catalog::CATALOG_LOCK.lock();
            crate::library_catalog::LibraryCatalog::load(&catalog_path)
                .map(|catalog| {
                    catalog
                        .visible_imports(canonical_root)
                        .into_iter()
                        .map(|record| record.path)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_else(|error| {
                    tracing::warn!(error = %error, "读取书库记录失败，导入归档校验将回退");
                    Vec::new()
                })
        };
        *guard = Some(ImportedCbzIndex {
            root: canonical_root.to_path_buf(),
            fingerprint,
            paths: imported,
        });
    }
    guard
        .as_ref()
        .is_some_and(|index| index.paths.iter().any(|path| path == cbz_path))
}

/// 解码图片协议地址里的页面标识。
///
/// 前端发出的是 `http://jmimg.localhost/<百分号编码的页面标识>`，wry 会把主机名
/// 改写成自定义 scheme，于是这里收到 `jmimg://localhost/<页面标识>`。
/// 页面标识沿用了原命令的格式：目录页面是绝对路径，CBZ 页面是 `归档路径|条目名`。
fn image_path_from_url(url: &tauri::Url) -> anyhow::Result<String> {
    anyhow::ensure!(url.scheme() == IMAGE_SCHEME, "图片地址协议不匹配");
    let encoded = url.path().trim_start_matches('/');
    percent_encoding::percent_decode_str(encoded)
        .decode_utf8()
        .context("图片地址不是有效的 UTF-8")
        .map(|decoded| decoded.into_owned())
}

const MAX_LAST_IMAGE_ERRORS: usize = 64;
static LAST_IMAGE_ERRORS: OnceLock<Mutex<indexmap::IndexMap<String, String>>> = OnceLock::new();

fn last_image_errors() -> &'static Mutex<indexmap::IndexMap<String, String>> {
    LAST_IMAGE_ERRORS.get_or_init(|| Mutex::new(indexmap::IndexMap::new()))
}

fn record_image_error(file_path: &str, message: &str) {
    let mut errors = last_image_errors().lock();
    errors.shift_remove(file_path);
    while errors.len() >= MAX_LAST_IMAGE_ERRORS {
        errors.shift_remove_index(0);
    }
    errors.insert(file_path.to_string(), message.to_string());
}

fn clear_image_error(file_path: &str) {
    if let Some(lock) = LAST_IMAGE_ERRORS.get() {
        lock.lock().shift_remove(file_path);
    }
}

fn classify_image_error(error: &anyhow::Error) -> (u16, &'static str, String) {
    if let Some(limit_error) = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<crate::image_data::ImageDataError>())
    {
        return match limit_error {
            crate::image_data::ImageDataError::FileTooLarge { limit_bytes } => (
                413,
                "file-too-large",
                format!(
                    "图片体积超过限制（上限 {} MB）",
                    (*limit_bytes / 1024 / 1024).max(1)
                ),
            ),
            crate::image_data::ImageDataError::PixelsTooLarge { max_pixels, .. } => (
                413,
                "pixels-too-large",
                format!("图片像素超过限制（上限 {} 万像素）", max_pixels / 10_000),
            ),
            crate::image_data::ImageDataError::InvalidImage(_) => (
                422,
                "invalid-image",
                "图片无法解码，文件可能已损坏".to_string(),
            ),
        };
    }
    if error
        .chain()
        .any(|cause| cause.downcast_ref::<image::ImageError>().is_some())
    {
        return (
            422,
            "invalid-image",
            "图片无法解码，文件可能已损坏".to_string(),
        );
    }
    (404, "read-error", "图片文件不存在或无法读取".to_string())
}

/// 查询最近一次 `jmimg://` 读取失败的可读原因，供阅读器在 `<img>` `@error` 时区分
/// 「图片尺寸或体积超限」与「文件损坏」。纯内存查询，不重新读取磁盘。
#[tauri::command]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub fn get_reader_image_error(file_path: String) -> Option<String> {
    LAST_IMAGE_ERRORS
        .get()
        .and_then(|lock| lock.lock().get(&file_path).cloned())
}

/// `jmimg://` 协议处理器：直接返回图片字节，省掉 base64 编码与 IPC 往返。
pub fn serve_image(
    app: &AppHandle,
    request: tauri::http::Request<Vec<u8>>,
) -> tauri::http::Response<Vec<u8>> {
    let respond = |status: u16, mime: &str, error_code: Option<&str>, body: Vec<u8>| {
        let cache_control = if status == 200 {
            // 与 ReaderDialog 的预取及重试（`?retry=` 绕过缓存）配合：
            // 仅缓存成功响应；错误响应一律 no-store，避免超限或临时占用被长期缓存。
            "private, max-age=300"
        } else {
            "no-store"
        };
        let mut builder = tauri::http::Response::builder()
            .status(status)
            .header(tauri::http::header::CONTENT_TYPE, mime)
            .header(tauri::http::header::CACHE_CONTROL, cache_control);
        if let Some(code) = error_code {
            builder = builder.header("X-JoyManga-Image-Error", code);
        }
        builder
            .body(body)
            .unwrap_or_else(|_| tauri::http::Response::new(Vec::new()))
    };
    let url = match tauri::Url::parse(&request.uri().to_string()) {
        Ok(url) => url,
        Err(error) => {
            tracing::warn!(error = %error, "解析图片协议地址失败");
            return respond(
                400,
                "text/plain; charset=utf-8",
                None,
                b"bad request".to_vec(),
            );
        }
    };
    let file_path = match image_path_from_url(&url) {
        Ok(file_path) => file_path,
        Err(error) => {
            tracing::warn!(error = %error, "解析图片协议路径失败");
            return respond(
                400,
                "text/plain; charset=utf-8",
                None,
                b"bad request".to_vec(),
            );
        }
    };
    match read_page_bytes(app, &file_path) {
        Ok((bytes, mime)) => {
            clear_image_error(&file_path);
            respond(200, mime, None, bytes)
        }
        Err(error) => {
            let (status, code, user_message) = classify_image_error(&error);
            record_image_error(&file_path, &user_message);
            tracing::warn!(status, code, error = %error, "读取阅读器图片失败");
            respond(
                status,
                "text/plain; charset=utf-8",
                Some(code),
                user_message.into_bytes(),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ChapterInfo;
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    #[test]
    fn image_url_decodes_percent_encoded_page_identity() {
        // 前端用 encodeURIComponent 编码：`/`、空格、`|`、中文都变成 %XX
        let url = tauri::Url::parse(
            "jmimg://localhost/%2Fstorage%2Femulated%2F0%2F%E6%BC%AB%E7%94%BB%20%E7%AC%AC1%E8%AF%9D%2F001.jpg",
        )
        .unwrap();
        assert_eq!(
            image_path_from_url(&url).unwrap(),
            "/storage/emulated/0/漫画 第1话/001.jpg"
        );

        let cbz = tauri::Url::parse("jmimg://localhost/a.cbz%7C001.jpg").unwrap();
        assert_eq!(image_path_from_url(&cbz).unwrap(), "a.cbz|001.jpg");
    }

    #[test]
    fn image_url_rejects_other_schemes() {
        let url = tauri::Url::parse("http://localhost/a.cbz").unwrap();
        assert!(image_path_from_url(&url).is_err());
    }

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
            let path = root.join(crate::comic_scanner::cbz_filename(
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
        let merged = super::super::library_commands::scan_downloaded_comics(&root)
            .comics
            .remove(0);
        let first = chapter_reader_data_from_comic(merged.clone(), 101).unwrap();
        let third = chapter_reader_data_from_comic(merged, 103).unwrap();
        assert!(first.image_paths[0].starts_with(paths[0].to_str().unwrap()));
        assert!(third.image_paths[0].starts_with(paths[1].to_str().unwrap()));
        assert_eq!(first.next_chapter.unwrap().chapter_id, 103);
        assert_eq!(third.prev_chapter.unwrap().chapter_id, 101);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn image_errors_classify_limits_corruption_and_missing_distinctly() {
        let too_large = anyhow::Error::new(crate::image_data::ImageDataError::FileTooLarge {
            limit_bytes: 32 * 1024 * 1024,
        })
        .context("包装上下文");
        let (status, code, msg) = classify_image_error(&too_large);
        assert_eq!(status, 413);
        assert_eq!(code, "file-too-large");
        assert!(msg.contains("32 MB"));

        let too_many_pixels =
            anyhow::Error::new(crate::image_data::ImageDataError::PixelsTooLarge {
                width: 9000,
                height: 8000,
                max_pixels: 64_000_000,
            });
        let (status, code, msg) = classify_image_error(&too_many_pixels);
        assert_eq!(status, 413);
        assert_eq!(code, "pixels-too-large");
        assert!(msg.contains("6400 万像素"));

        let invalid = anyhow::Error::new(crate::image_data::ImageDataError::InvalidImage(
            "bad".into(),
        ));
        let (status, code, msg) = classify_image_error(&invalid);
        assert_eq!(status, 422);
        assert_eq!(code, "invalid-image");
        assert!(msg.contains("无法解码"));

        let (status, code, _) = classify_image_error(&anyhow::anyhow!("missing"));
        assert_eq!(status, 404);
        assert_eq!(code, "read-error");
    }

    #[test]
    fn get_reader_image_error_returns_recorded_message() {
        let key = format!("test-path-{}", uuid::Uuid::new_v4());
        record_image_error(&key, "图片像素超过限制");
        assert_eq!(
            get_reader_image_error(key.clone()).as_deref(),
            Some("图片像素超过限制")
        );
        clear_image_error(&key);
        assert!(get_reader_image_error(key).is_none());
    }
}
