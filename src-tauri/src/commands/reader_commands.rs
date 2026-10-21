use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{anyhow, Context};
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

/// 阅读器当前打开的 CBZ。阅读是一页一页的，单槽缓存足够，且避免长期占着文件句柄。
static ACTIVE_ARCHIVE: OnceLock<Mutex<Option<CachedArchive>>> = OnceLock::new();

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
        let image_paths = utils::collect_page_images(chapter_target)
            .map_err(|err| CommandError::from("读取章节图片失败", err))?;

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

fn mime_for_extension(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("jpeg")
        .to_lowercase()
        .as_str()
    {
        "png" => "image/png",
        "webp" => "image/webp",
        "gif" => "image/gif",
        _ => "image/jpeg",
    }
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
            && !is_imported_cbz(app, &download_dir, &canonical_cbz_path)
        {
            return Err(anyhow!("CBZ文件不在下载根目录内"));
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

        let mime = mime_for_extension(Path::new(entry_name));
        return Ok((bytes, mime));
    }

    let canonical_path = std::fs::canonicalize(file_path)
        .with_context(|| format!("无法解析图片路径`{file_path}`"))?;
    if !canonical_path.starts_with(&canonical_download_dir) {
        return Err(anyhow!("图片路径不在下载根目录内"));
    }
    let bytes = crate::image_data::read_image(&canonical_path)
        .with_context(|| format!("读取文件`{}`失败", canonical_path.display()))?;
    Ok((bytes, mime_for_extension(&canonical_path)))
}

/// 判断 CBZ 是否来自「导入」：书库记录里登记过的外部归档。
///
/// 阅读时每页都会调用，所以结果按记录文件的修改时间缓存，避免逐页同步读盘。
fn is_imported_cbz(app: &AppHandle, download_dir: &Path, cbz_path: &Path) -> bool {
    let catalog_path = crate::library_catalog::catalog_path(app);
    let fingerprint = std::fs::metadata(&catalog_path)
        .ok()
        .and_then(|meta| meta.modified().ok().map(|modified| (modified, meta.len())));
    let root = PathBuf::from(crate::library_catalog::root_key(download_dir));
    let index_lock = IMPORTED_CBZ_INDEX.get_or_init(|| Mutex::new(None));
    let mut guard = index_lock.lock();
    let stale = guard
        .as_ref()
        .is_none_or(|index| index.fingerprint != fingerprint || index.root != root);
    if stale {
        let imported = {
            let _guard = crate::library_catalog::CATALOG_LOCK.lock();
            crate::library_catalog::LibraryCatalog::load(&catalog_path)
                .map(|catalog| {
                    catalog
                        .visible_imports(download_dir)
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
            root,
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
    let mut decoded = Vec::with_capacity(encoded.len());
    let mut bytes = encoded.bytes();
    while let Some(byte) = bytes.next() {
        if byte == b'%' {
            let pair = [
                bytes.next().context("图片地址编码不完整")?,
                bytes.next().context("图片地址编码不完整")?,
            ];
            let text = std::str::from_utf8(&pair).context("图片地址编码无效")?;
            decoded.push(u8::from_str_radix(text, 16).context("图片地址编码无效")?);
        } else {
            decoded.push(byte);
        }
    }
    String::from_utf8(decoded).context("图片地址不是有效的 UTF-8")
}

/// `jmimg://` 协议处理器：直接返回图片字节，省掉 base64 编码与 IPC 往返。
pub fn serve_image(
    app: &AppHandle,
    request: tauri::http::Request<Vec<u8>>,
) -> tauri::http::Response<Vec<u8>> {
    let respond = |status: u16, mime: &str, body: Vec<u8>| {
        tauri::http::Response::builder()
            .status(status)
            .header(tauri::http::header::CONTENT_TYPE, mime)
            // 与 ReaderDialog 的 `new Image()` 预取配合：允许 WebView 复用同一页的
            // 解码结果，翻页时不必重新读盘/解压。`no-store` 会让预取请求白白作废。
            // TTL 取短值：同一路径的图片可能因重新下载而被覆盖，不能让旧图长期驻留。
            // 失败重试通过变更 URL（`?retry=`）绕过缓存。
            .header(tauri::http::header::CACHE_CONTROL, "private, max-age=300")
            .body(body)
            .unwrap_or_else(|_| tauri::http::Response::new(Vec::new()))
    };
    let url = match tauri::Url::parse(&request.uri().to_string()) {
        Ok(url) => url,
        Err(error) => {
            tracing::warn!(error = %error, "解析图片协议地址失败");
            return respond(400, "text/plain", b"bad request".to_vec());
        }
    };
    let file_path = match image_path_from_url(&url) {
        Ok(file_path) => file_path,
        Err(error) => {
            tracing::warn!(error = %error, "解析图片协议路径失败");
            return respond(400, "text/plain", b"bad request".to_vec());
        }
    };
    match read_page_bytes(app, &file_path) {
        Ok((bytes, mime)) => respond(200, mime, bytes),
        Err(error) => {
            tracing::warn!(error = %error, "读取阅读器图片失败");
            respond(404, "text/plain", error.to_string().into_bytes())
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
