use std::collections::HashMap;
use std::io::{Read, Seek};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, ensure, Context};
use base64::engine::general_purpose;
use base64::Engine as _;
use walkdir::WalkDir;
use zip::ZipArchive;

use crate::{
    atomic_fs,
    extensions::{mime_for_image, PathIsCbz, PathIsImg, WalkDirEntryExt},
    types::{ChapterInfo, Comic, ComicInfo},
    utils,
};

/// CBZ 条目是否为可展示的漫画页：图片扩展名，且不是隐藏/系统条目。
pub(crate) fn is_page_image(name: &str) -> bool {
    Path::new(name).is_img() && !name.starts_with('.') && !name.starts_with('_')
}

/// 收集归档内所有漫画页条目名（不排序，调用方按需排序/切片）。
pub(crate) fn image_entry_names<R: Read + Seek>(archive: &mut ZipArchive<R>) -> Vec<String> {
    (0..archive.len())
        .filter_map(|index| {
            let entry = archive.by_index(index).ok()?;
            let name = entry.name().ok()?.to_string();
            is_page_image(&name).then_some(name)
        })
        .collect()
}

fn extract_cover_from_archive<R: Read + Seek>(
    cbz_path: &Path,
    archive: &mut ZipArchive<R>,
) -> Option<String> {
    let mut img_names = image_entry_names(archive);
    img_names.sort_by(|a, b| utils::natural_cmp(a, b));
    let first_img = img_names.first()?;
    let mut entry = archive.by_name(first_img).ok()?;
    let declared_size = entry.size();
    let bytes = crate::image_data::read_limited(
        &mut entry,
        declared_size,
        crate::image_data::MAX_PAGE_BYTES,
    )
    .ok()?;
    if bytes.is_empty() {
        return None;
    }
    let mime = mime_for_image(Path::new(first_img));
    Some(match render_thumbnail(&bytes) {
        Some(thumbnail) => {
            crate::cover_cache::write_thumbnail(cbz_path, &thumbnail);
            base64_data_url("image/jpeg", &thumbnail)
        }
        None => raw_cover_data_url(&bytes, mime)?,
    })
}

fn load_cover_from_open_archive<R: Read + Seek>(
    cbz_path: &Path,
    archive: &mut ZipArchive<R>,
) -> Option<String> {
    if let Some(bytes) = crate::cover_cache::read_thumbnail(cbz_path) {
        return Some(base64_data_url("image/jpeg", &bytes));
    }
    extract_cover_from_archive(cbz_path, archive)
}

pub fn load_local_cover_data(comic_download_dir: &Path) -> Option<String> {
    let mut cbz_files = Vec::new();
    if comic_download_dir.is_file() {
        if comic_download_dir.is_cbz() {
            cbz_files.push(comic_download_dir.to_path_buf());
        }
    } else {
        if let Ok(entries) = std::fs::read_dir(comic_download_dir) {
            for entry in entries.filter_map(Result::ok) {
                let path = entry.path();
                if path.is_file() && path.is_cbz() {
                    cbz_files.push(path);
                }
            }
        }
        let cbz_sub = comic_download_dir.join("cbz");
        if cbz_sub.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&cbz_sub) {
                for entry in entries.filter_map(Result::ok) {
                    let path = entry.path();
                    if path.is_file() && path.is_cbz() {
                        cbz_files.push(path);
                    }
                }
            }
        }
    }

    cbz_files.sort_by(|a, b| {
        let name_a = a.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let name_b = b.file_name().and_then(|n| n.to_str()).unwrap_or("");
        utils::natural_cmp(name_a, name_b)
    });

    for cbz_path in &cbz_files {
        if let Some(bytes) = crate::cover_cache::read_thumbnail(cbz_path) {
            return Some(base64_data_url("image/jpeg", &bytes));
        }
        let Ok(file) = std::fs::File::open(cbz_path) else {
            continue;
        };
        let Ok(mut archive) = zip::ZipArchive::new(file) else {
            continue;
        };
        if let Some(cover) = extract_cover_from_archive(cbz_path, &mut archive) {
            return Some(cover);
        }
    }

    let mut candidates = if comic_download_dir.is_file() {
        vec![]
    } else {
        vec![
            comic_download_dir.join("cover.jpg"),
            comic_download_dir.join("封面.jpg"),
        ]
    };
    if comic_download_dir.is_dir() {
        if let Ok(entries) = std::fs::read_dir(comic_download_dir) {
            candidates.extend(
                entries
                    .filter_map(Result::ok)
                    .map(|entry| entry.path())
                    .filter(|path| path.is_img()),
            );
        }
    }
    for path in &candidates {
        if path.is_file()
            && std::fs::metadata(path).is_ok_and(|metadata| metadata.len() <= 32 * 1024 * 1024)
        {
            let thumbnail = crate::cover_cache::cached_thumbnail(path, || {
                std::fs::read(path)
                    .ok()
                    .and_then(|bytes| render_thumbnail(&bytes))
            });
            if let Some(thumbnail) = thumbnail {
                return Some(base64_data_url("image/jpeg", &thumbnail));
            }
            // 解码失败时退回原图（体积很小才允许），与 CBZ 分支一致
            if let Ok(bytes) = std::fs::read(path) {
                return raw_cover_data_url(&bytes, mime_for_image(path));
            }
        }
    }
    None
}

fn base64_data_url(mime: &str, bytes: &[u8]) -> String {
    format!(
        "data:{mime};base64,{}",
        general_purpose::STANDARD.encode(bytes)
    )
}

/// 封面来源不是可解码图片时（例如测试或损坏的极小文件），
/// 只要体积够小就原样返回，保证书库仍能显示缩略图。
fn raw_cover_data_url(bytes: &[u8], fallback_mime: &str) -> Option<String> {
    (bytes.len() <= 256 * 1024).then(|| base64_data_url(fallback_mime, bytes))
}

/// 把任意来源图片渲染成立即能用的封面缩略图。
///
/// 注意：**必须先按原尺寸解码再缩放**——JM 的乱序图分段边界依赖原始高度，
/// 先缩放会让接缝错位（表现为横线）。下载侧已经还原成正常图，这里沿用同一顺序。
fn render_thumbnail(bytes: &[u8]) -> Option<Vec<u8>> {
    if let Ok(reader) = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format() {
        if let Ok((width, height)) = reader.into_dimensions() {
            if u64::from(width) * u64::from(height) > crate::image_data::MAX_IMAGE_PIXELS {
                return None;
            }
        }
    }
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    let mut limits = image::Limits::default();
    // Android 设备内存更紧张，缩略图解码上限固定 64 MB
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    let image = reader.decode().ok()?;
    let mut thumbnail = std::io::Cursor::new(Vec::new());
    image
        .thumbnail(240, 320)
        .write_to(&mut thumbnail, image::ImageFormat::Jpeg)
        .ok()?;
    Some(thumbnail.into_inner())
}

pub fn from_metadata(metadata_path: &Path) -> anyhow::Result<Comic> {
    let comic_json = std::fs::read_to_string(metadata_path)
        .context(format!("读取`{}`失败", metadata_path.display()))?;
    let mut comic = serde_json::from_str::<Comic>(&comic_json).context(format!(
        "将`{}`反序列化为Comic失败",
        metadata_path.display()
    ))?;
    // 来自元数据的章节信息没有`download_dir`和`is_downloaded`字段，需要更新
    let parent = metadata_path
        .parent()
        .context(format!("`{}`没有父目录", metadata_path.display()))?;
    let comic_download_dir = parent.to_path_buf();

    comic.comic_download_dir = Some(comic_download_dir.clone());
    comic.local_cover_data = load_local_cover_data(&comic_download_dir);

    update_chapter_infos_fields(&mut comic).context("更新章节信息字段失败")?;
    if comic.local_cover_data.is_none() {
        comic.local_cover_data = comic
            .chapter_infos
            .iter()
            .filter_map(|chapter| chapter.chapter_download_dir.as_ref())
            .find_map(|directory| load_local_cover_data(directory));
    }
    comic.is_downloaded = Some(
        comic
            .chapter_infos
            .iter()
            .any(|chapter| chapter.is_downloaded == Some(true)),
    );

    Ok(comic)
}

pub fn from_cbz(cbz_path: &Path) -> anyhow::Result<Comic> {
    let file = std::fs::File::open(cbz_path)
        .context(format!("打开CBZ文件`{}`失败", cbz_path.display()))?;
    let mut archive =
        zip::ZipArchive::new(file).context(format!("解析CBZ文件`{}`失败", cbz_path.display()))?;

    // 优先读取内置的元数据.json / metadata.json
    let mut comic_opt = None;
    for metadata_name in &["元数据.json", "metadata.json"] {
        if let Ok(mut entry) = archive.by_name(metadata_name) {
            let declared_size = entry.size();
            if let Ok(content) = crate::image_data::read_limited(
                &mut entry,
                declared_size,
                crate::image_data::MAX_METADATA_BYTES,
            ) {
                if let Ok(comic) = serde_json::from_slice::<Comic>(&content) {
                    comic_opt = Some(comic);
                    break;
                }
            }
        }
    }

    let metadata_present = comic_opt.is_some();
    let mut comic = if let Some(comic) = comic_opt {
        comic
    } else if let Ok(mut entry) = archive.by_name("ComicInfo.xml") {
        let declared_size = entry.size();
        let content = crate::image_data::read_limited(
            &mut entry,
            declared_size,
            crate::image_data::MAX_METADATA_BYTES,
        )
        .context("读取CBZ包内`ComicInfo.xml`失败")?;
        let info: ComicInfo = yaserde::de::from_reader(content.as_slice())
            .map_err(|e| anyhow!("反序列化`ComicInfo.xml`失败: {e}"))?;

        let authors = if info.writer.is_empty() {
            vec![]
        } else {
            info.writer
                .split(',')
                .map(|s| s.trim().to_string())
                .collect()
        };
        let tags = if info.tags.is_empty() {
            vec![]
        } else {
            info.tags.split(',').map(|s| s.trim().to_string()).collect()
        };
        let order = info
            .number
            .as_deref()
            .and_then(|n| n.parse::<i64>().ok())
            .unwrap_or(1);
        let chapter_title = if info.title.is_empty() {
            format!("第{order}话")
        } else {
            info.title
        };

        // 外部 CBZ 没有本应用元数据：用负值稳定哈希作为漫画/章节标识，
        // 既不会与 JM 的正数 ID 冲突（避免被误并进同 ID 的在线漫画并在删除时殃及），
        // 也对同一文件保持稳定。
        let fallback_id = external_cbz_id(cbz_path);

        Comic {
            id: fallback_id,
            name: if info.series.is_empty() {
                cbz_path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("未知漫画")
                    .to_string()
            } else {
                info.series
            },
            addtime: String::new(),
            description: info.summary,
            total_views: "0".to_string(),
            likes: "0".to_string(),
            chapter_infos: vec![ChapterInfo {
                chapter_id: fallback_id,
                chapter_title,
                order,
                is_downloaded: Some(true),
                chapter_download_dir: Some(cbz_path.to_path_buf()),
            }],
            series_id: "0".to_string(),
            comment_total: "0".to_string(),
            author: authors,
            tags,
            works: vec![],
            actors: vec![],
            related_list: vec![],
            liked: false,
            is_favorite: false,
            is_aids: false,
            is_downloaded: Some(true),
            comic_download_dir: Some(cbz_path.to_path_buf()),
            local_cover_data: None,
        }
    } else {
        return Err(anyhow!(
            "CBZ文件`{}`中既没有`元数据.json`也没有`ComicInfo.xml`",
            cbz_path.display()
        ));
    };

    comic.comic_download_dir = Some(cbz_path.to_path_buf());
    comic.local_cover_data = load_cover_from_open_archive(cbz_path, &mut archive);

    let identity = read_chapter_identity_from_archive(&mut archive)?;
    if let Some((comic_id, _)) = identity {
        ensure!(comic_id == comic.id, "CBZ漫画身份与元数据不一致");
    }
    let cbz_file_name = cbz_path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let mut matched_any = false;
    let chapter_count = comic.chapter_infos.len();
    // 无 chapter.json、内嵌元数据包含多话、且归档条目带 `[NN] ` 话目录布局：
    // 本应用的「整本合并」归档，所有内嵌话都视为已下载。
    // 旧式单话章节包同样「无 chapter.json + 元数据含多话清单」但没有话目录布局，须走常规匹配。
    let merged_full_book = identity.is_none()
        && metadata_present
        && chapter_count >= 2
        && archive_has_chapter_layout(&mut archive);
    for chapter in &mut comic.chapter_infos {
        chapter.chapter_download_dir = None;
        chapter.is_downloaded = None;
        if merged_full_book {
            chapter.chapter_download_dir = Some(cbz_path.to_path_buf());
            chapter.is_downloaded = Some(true);
            matched_any = true;
            continue;
        }
        let expected_name = cbz_filename(
            &comic.name,
            comic.id,
            &chapter.chapter_title,
            chapter.chapter_id,
            chapter.order,
            chapter_count,
        );
        let chapter_title_cbz = format!("{}.cbz", utils::filename_filter(&chapter.chapter_title));
        if identity == Some((comic.id, chapter.chapter_id))
            || (identity.is_none()
                && (cbz_file_name == expected_name
                    || cbz_file_name == chapter_title_cbz
                    || chapter_count == 1))
        {
            chapter.chapter_download_dir = Some(cbz_path.to_path_buf());
            chapter.is_downloaded = Some(true);
            matched_any = true;
        }
    }
    ensure!(
        identity.is_none() || matched_any,
        "CBZ章节身份不在漫画信息中"
    );
    if !matched_any && !comic.chapter_infos.is_empty() {
        comic.chapter_infos[0].chapter_download_dir = Some(cbz_path.to_path_buf());
        comic.chapter_infos[0].is_downloaded = Some(true);
    }

    comic.is_downloaded = Some(true);
    Ok(comic)
}

pub fn save_comic_metadata(comic: &Comic) -> anyhow::Result<()> {
    let mut clean_comic = comic.clone();
    // 元数据只保留漫画状态，不携带当前机器上的下载路径与封面缓存。
    clean_comic.clear_local_download_state();

    let comic_download_dir = comic
        .comic_download_dir
        .as_ref()
        .context("`comic_download_dir`字段为`None`")?;
    let metadata_path = comic_download_dir.join("元数据.json");

    std::fs::create_dir_all(comic_download_dir)
        .context(format!("创建目录`{}`失败", comic_download_dir.display()))?;

    let comic_json =
        serde_json::to_string_pretty(&clean_comic).context("将Comic序列化为json失败")?;

    atomic_fs::write_checked(&metadata_path, comic_json, |existing_path| {
        let existing_json = std::fs::read_to_string(existing_path).context(format!(
            "读取已有漫画元数据`{}`失败",
            existing_path.display()
        ))?;
        let existing_comic: Comic = serde_json::from_str(&existing_json).context(format!(
            "解析已有漫画元数据`{}`失败",
            existing_path.display()
        ))?;
        ensure!(
            existing_comic.id == comic.id,
            "下载目录已属于漫画ID `{}`，不能用漫画ID `{}`覆盖",
            existing_comic.id,
            comic.id
        );
        Ok(())
    })
    .context(format!("写入文件`{}`失败", metadata_path.display()))?;

    Ok(())
}

pub fn update_chapter_infos_fields(comic: &mut Comic) -> anyhow::Result<()> {
    let Some(comic_download_dir) = &comic.comic_download_dir else {
        return Err(anyhow!("`comic_download_dir`字段为`None`"));
    };

    if !comic_download_dir.exists() {
        return Ok(());
    }

    let mut cbz_files = Vec::new();
    let search_dirs = if comic_download_dir.is_file() {
        vec![comic_download_dir
            .parent()
            .unwrap_or(comic_download_dir)
            .to_path_buf()]
    } else {
        vec![comic_download_dir.clone(), comic_download_dir.join("cbz")]
    };

    for dir in search_dirs {
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.filter_map(Result::ok) {
                let path = entry.path();
                if path.is_file() && path.is_cbz() {
                    cbz_files.push(path);
                }
            }
        }
    }

    let is_flat_mode = comic_download_dir.is_file();
    let comic_name_filtered = utils::filename_filter(&comic.name);
    let comic_prefix = format!("{comic_name_filtered} - ");
    let merged_suffix = format!("[JM{}]", comic.id);
    let merged_candidates = cbz_files
        .iter()
        .filter(|path| {
            // 平铺模式只需检查本漫画的命名；改名后的整本包有 `[JMid]` 后缀。
            // 非平铺目录虽属于单本漫画，也保留可能由旧版本生成的整本名。
            !is_flat_mode
                || path.as_path() == comic_download_dir
                || path
                    .file_stem()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| {
                        name.contains(&merged_suffix)
                            || (!comic_name_filtered.is_empty()
                                && name.starts_with(&comic_name_filtered))
                    })
        })
        .cloned()
        .collect::<Vec<_>>();

    // 整本合并以包内漫画与章节身份为准：标题截断或云端改名后仍能认回。
    let merged_archives: Vec<(std::path::PathBuf, Vec<ChapterInfo>)> = merged_candidates
        .iter()
        .filter_map(|path| match read_merged_archive_identity(path) {
            Ok(Some((comic_id, chapters))) if comic_id == comic.id => {
                Some((path.clone(), chapters))
            }
            _ => None,
        })
        .collect();

    let chapter_count = comic.chapter_infos.len();
    for chapter in &mut comic.chapter_infos {
        chapter.chapter_download_dir = None;
        chapter.is_downloaded = None;
        let expected_cbz_name = cbz_filename(
            &comic.name,
            comic.id,
            &chapter.chapter_title,
            chapter.chapter_id,
            chapter.order,
            chapter_count,
        );
        let chapter_title_cbz = format!("{}.cbz", utils::filename_filter(&chapter.chapter_title));
        let order_dash_pad = format!(" - {:04} - ", chapter.order);
        let order_dash = format!(" - {} - ", chapter.order);
        let order_space = format!(" {}.cbz", chapter.order);
        let order_prefix = format!("{:04}", chapter.order);

        let matched = cbz_files.iter().find(|cbz_path| {
            if is_flat_mode && cbz_path.as_path() == comic_download_dir && chapter_count == 1 {
                return true;
            }
            let Some(file_name) = cbz_path.file_name().and_then(|n| n.to_str()) else {
                return false;
            };
            if file_name.ends_with(&format!("[JM{}-{}].cbz", comic.id, chapter.chapter_id)) {
                return read_chapter_identity(cbz_path).ok().flatten()
                    == Some((comic.id, chapter.chapter_id));
            }
            if file_name == expected_cbz_name {
                return true;
            }
            if !is_flat_mode {
                if file_name == chapter_title_cbz {
                    return true;
                }
                if chapter_count == 1
                    && cbz_files.len() == 1
                    && file_name.starts_with(&comic_prefix)
                {
                    // 单话本：目录里唯一的 CBZ 视为本章，但文件名必须属于本漫画，
                    // 否则会把同目录里其他漫画的孤本误标记为已下载
                    return true;
                }
                if chapter_count > 1 {
                    if file_name.contains(&order_dash_pad)
                        || file_name.contains(&order_dash)
                        || file_name.ends_with(&order_space)
                        || file_name.starts_with(&order_prefix)
                    {
                        return true;
                    }
                    if !chapter.chapter_title.is_empty()
                        && file_name.contains(&chapter.chapter_title)
                    {
                        return true;
                    }
                }
            } else {
                // 平铺模式下，必须明确归属当前漫画
                if file_name.starts_with(&comic_prefix)
                    || file_name.starts_with(&comic_name_filtered)
                {
                    if file_name.contains(&order_dash_pad)
                        || file_name.contains(&order_dash)
                        || file_name.ends_with(&order_space)
                    {
                        return true;
                    }
                    if !chapter.chapter_title.is_empty()
                        && file_name.contains(&chapter.chapter_title)
                    {
                        return true;
                    }
                }
            }
            false
        });

        if let Some(cbz_path) = matched {
            chapter.chapter_download_dir = Some(cbz_path.clone());
            chapter.is_downloaded = Some(true);
        } else if let Some((archive_path, _)) =
            merged_archives.iter().find(|(_, archive_chapters)| {
                // 只按 chapter_id 认定；服务端增删章节会让 order 整体偏移，
                // 按 order 兜底会把别的章节冒充成本章
                archive_chapters
                    .iter()
                    .any(|archived| archived.chapter_id == chapter.chapter_id)
            })
        {
            chapter.chapter_download_dir = Some(archive_path.clone());
            chapter.is_downloaded = Some(true);
        } else if !is_flat_mode {
            let directory = comic_download_dir.join(utils::filename_filter(&chapter.chapter_title));
            if directory.is_dir()
                && std::fs::read_dir(&directory).is_ok_and(|mut entries| {
                    entries.any(|entry| entry.is_ok_and(|entry| entry.path().is_img()))
                })
            {
                chapter.chapter_download_dir = Some(directory);
                chapter.is_downloaded = Some(true);
            }
        }
    }

    Ok(())
}

pub(crate) fn read_chapter_identity(path: &Path) -> anyhow::Result<Option<(i64, i64)>> {
    let mut archive = zip::ZipArchive::new(std::fs::File::open(path)?)?;
    read_chapter_identity_from_archive(&mut archive)
}

pub(crate) fn read_chapter_identity_from_archive<R: std::io::Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
) -> anyhow::Result<Option<(i64, i64)>> {
    let Ok(mut entry) = archive.by_name("chapter.json") else {
        return Ok(None);
    };
    let size = entry.size();
    let bytes =
        crate::image_data::read_limited(&mut entry, size, crate::image_data::MAX_METADATA_BYTES)?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)?;
    Ok(value["comicId"].as_i64().zip(value["chapterId"].as_i64()))
}

/// 解析整本合并归档条目名中的话序号前缀，如 `[02] 第2话/0001.jpg` -> Some(2)。
///
/// 两侧必须保持一致：写入端用 `[{order:0width$}]`（最少 2 位，话数超过 99 时 3 位，
/// 超过 999 时自然变成 4 位）。早期这里硬编码只接受 2~3 位，导致 >999 话的整本
/// 归档在阅读/切片/导出时全都找不到本话页面。现在接受 2~6 位，并统一按十进制解析。
pub(crate) fn bracket_order(entry_name: &str) -> Option<i64> {
    let rest = entry_name.strip_prefix('[')?;
    let close = rest.find(']')?;
    let digits = &rest[..close];
    if !(2..=6).contains(&digits.len()) || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// 归档条目中是否存在 `[NN] ` 话目录布局（本应用整本合并产物的结构特征）。
fn archive_has_chapter_layout<R: std::io::Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
) -> bool {
    (0..archive.len()).any(|index| {
        archive.by_index(index).ok().is_some_and(|entry| {
            entry
                .name()
                .ok()
                .and_then(|name| bracket_order(name.as_ref()))
                .is_some()
        })
    })
}

/// 从归档的图片条目中选出指定话的页面。
///
/// - 多话合并布局（存在 `[NN] ` 前缀条目）时返回该话前缀下的条目；
/// - 该话在归档中不存在时返回 `None`；
/// - 单话/平铺布局（没有任何话前缀）时返回全部条目。
pub(crate) fn select_chapter_pages(entry_names: &[String], order: i64) -> Option<Vec<String>> {
    let mut own: Vec<String> = entry_names
        .iter()
        .filter(|name| bracket_order(name) == Some(order))
        .cloned()
        .collect();
    if !own.is_empty() {
        own.sort_by(|a, b| utils::natural_cmp(a, b));
        return Some(own);
    }
    let has_chapter_layout = entry_names.iter().any(|name| bracket_order(name).is_some());
    if has_chapter_layout {
        return None;
    }
    let mut all = entry_names.to_vec();
    all.sort_by(|a, b| utils::natural_cmp(a, b));
    Some(all)
}

/// 当 CBZ 是「无 chapter.json 且带 元数据.json」的整本归档（本应用的自动合并产物）时，
/// 返回 `(comic_id, 内嵌章节列表)`；普通单话章节文件或外来归档返回 `None`。
pub(crate) fn read_merged_archive_identity(
    path: &Path,
) -> anyhow::Result<Option<(i64, Vec<ChapterInfo>)>> {
    let mut archive = zip::ZipArchive::new(std::fs::File::open(path)?)?;
    if archive.by_name("chapter.json").is_ok() {
        return Ok(None);
    }
    // 元数据是应用整本包的稳定身份；多话包有 `[NN]` 目录，单话包则平铺页面。
    // 旧式多话单章包没有目录布局，仍按普通章节文件处理。
    let has_chapter_layout = archive_has_chapter_layout(&mut archive);
    let Ok(mut entry) = archive.by_name("元数据.json") else {
        return Ok(None);
    };
    let size = entry.size();
    let bytes =
        crate::image_data::read_limited(&mut entry, size, crate::image_data::MAX_METADATA_BYTES)?;
    let comic: Comic = serde_json::from_slice(&bytes)?;
    if !has_chapter_layout && comic.chapter_infos.len() != 1 {
        return Ok(None);
    }
    Ok(Some((comic.id, comic.chapter_infos)))
}

/// 应用在书库里创建的隐藏工作目录（名字通常带前导 `.`）。
///
/// 只用于区分“应用临时目录”和“用户自己的隐藏漫画目录”，不要扩大成
/// “所有隐藏目录”，否则旧书库中以 `.` 开头的漫画会被永久隐藏。
fn is_app_work_dir_name(name: &str) -> bool {
    let name = name.strip_prefix('.').unwrap_or(name);
    name.starts_with("import-") || name.starts_with("downloading-") || name.contains(".trash-")
}

/// 智能遍历下载目录中的漫画元数据文件。
/// 通过智能剪枝跳过包含章节元数据（即纯图片子目录）的深层递归，避免访问数万张图片文件。
pub fn walk_comic_metadata_files(download_dir: &Path) -> impl Iterator<Item = walkdir::DirEntry> {
    WalkDir::new(download_dir)
        .into_iter()
        .filter_entry(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            // 应用自己的工作目录与删除残留（`.import-`、`.downloading-`、
            // `.原名.trash-*`）必须跳过，避免残留被误识别成新漫画。
            // 但旧版书库允许漫画目录以 `.` 开头：只要它直接包含元数据，
            // 仍按漫画继续扫描，否则这类漫画会从书库里消失且无法删除。
            if entry.depth() > 0 && name.starts_with('.') {
                let is_app_work_dir = is_app_work_dir_name(&name);
                let is_comic_dir =
                    entry.file_type().is_dir() && entry.path().join("元数据.json").is_file();
                if is_app_work_dir || !is_comic_dir {
                    return false;
                }
            }
            if entry.file_type().is_dir() {
                // 如果当前目录是章节目录（包含章节元数据），则跳过下钻
                !entry.path().join("章节元数据.json").exists()
            } else {
                // 文件只关心 元数据.json
                entry.file_name() == "元数据.json"
            }
        })
        .filter_map(Result::ok)
        .filter(|entry| entry.is_comic_metadata())
}

pub fn read_comic_id_from_cbz(path: &Path) -> anyhow::Result<i64> {
    let file = std::fs::File::open(path)?;
    let mut archive = zip::ZipArchive::new(file)?;
    for name in &["元数据.json", "metadata.json"] {
        if let Ok(mut entry) = archive.by_name(name) {
            let declared_size = entry.size();
            let Ok(bytes) = crate::image_data::read_limited(
                &mut entry,
                declared_size,
                crate::image_data::MAX_METADATA_BYTES,
            ) else {
                continue;
            };
            if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                if let Some(id) = v.get("id").and_then(serde_json::Value::as_i64) {
                    return Ok(id);
                }
            }
        }
    }
    Ok(external_cbz_id(path))
}

/// 外部 CBZ（无本应用元数据）的漫画/章节标识。
///
/// 使用固定算法（UTF-8 字节 FNV-1a）取**负值**：
/// - 负值不会与 JM 在线漫画的正数 ID 冲突，避免被误并入同 ID 漫画后在删除时殃及；
/// - 固定算法跨 Rust 版本稳定（`DefaultHasher` 不保证）；
/// - 同一文件名重复扫描得到相同 ID，书库记录可复用。
pub(crate) fn external_cbz_id(path: &Path) -> i64 {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in stem.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01B3);
    }
    let hash = (hash & 0x7FFF_FFFF_FFFF_FFFF) % 1_000_000_000_000;
    -(hash as i64).max(1)
}

pub fn storage_cbz_filename(comic_name: &str, comic_id: i64, chapter_id: i64) -> String {
    let suffix = format!(" [JM{comic_id}-{chapter_id}].cbz");
    let title = utils::filename_filter(comic_name);
    let title = utils::truncate_utf8_by_bytes(&title, 180_usize.saturating_sub(suffix.len()));
    format!("{}{suffix}", if title.is_empty() { "comic" } else { title })
}

pub fn cbz_filename(
    comic_name: &str,
    comic_id: i64,
    chapter_title: &str,
    chapter_id: i64,
    chapter_order: i64,
    chapter_count: usize,
) -> String {
    const MAX_TOTAL_BYTES: usize = 180;
    const EXTENSION: &str = ".cbz";

    let clean_comic_name = utils::filename_filter(comic_name);
    let clean_comic_name = if clean_comic_name.is_empty() {
        format!("comic-{comic_id}")
    } else {
        clean_comic_name
    };

    if chapter_count <= 1 {
        let max_comic_bytes = MAX_TOTAL_BYTES.saturating_sub(EXTENSION.len());
        let truncated = utils::truncate_utf8_by_bytes(&clean_comic_name, max_comic_bytes);
        let final_comic = if truncated.is_empty() {
            format!("comic-{comic_id}")
        } else {
            truncated.to_string()
        };
        return format!("{final_comic}{EXTENSION}");
    }

    let clean_chapter_name = utils::filename_filter(chapter_title);
    let clean_chapter_name = if clean_chapter_name.is_empty() {
        format!("chapter-{chapter_id}")
    } else {
        clean_chapter_name
    };

    let middle = format!(" - {:04} - ", chapter_order);
    let fixed_len = middle.len() + EXTENSION.len();
    let available_for_names = MAX_TOTAL_BYTES.saturating_sub(fixed_len);

    let max_chapter_bytes = 50.min(available_for_names.saturating_sub(30));
    let truncated_chapter = utils::truncate_utf8_by_bytes(&clean_chapter_name, max_chapter_bytes);
    let chapter_part = if truncated_chapter.is_empty() {
        format!("chapter-{chapter_id}")
    } else {
        truncated_chapter.to_string()
    };

    let remaining_for_comic = available_for_names.saturating_sub(chapter_part.len());
    let truncated_comic = utils::truncate_utf8_by_bytes(&clean_comic_name, remaining_for_comic);
    let comic_part = if truncated_comic.is_empty() {
        format!("comic-{comic_id}")
    } else {
        truncated_comic.to_string()
    };

    format!("{comic_part}{middle}{chapter_part}{EXTENSION}")
}

/// 收集章节目录中的有效页面：按自然序排序，并按文件主名去重
/// （同一页可能残留不同扩展名的旧文件）。
///
/// 同一页有多个扩展名时保留**最近写入**的那个：断点续传的 `find_existing_page`
/// 也按同样规则挑选可解码的文件，两边保持一致，避免阅读/导出拿到旧的坏页。
pub fn collect_page_images(dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let name_of = |path: &Path| -> String {
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_owned()
    };

    // 用 DirEntry 自带的文件类型与元数据：不必再为每个条目额外 stat 两次
    // （`path.is_file()` 与 `fs::metadata()` 各自都是一次系统调用）。
    let mut image_paths: Vec<(PathBuf, std::time::SystemTime)> = std::fs::read_dir(dir)
        .with_context(|| format!("读取章节目录`{}`失败", dir.display()))?
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()) && entry.path().is_img())
        .map(|entry| {
            let modified = entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            (entry.path(), modified)
        })
        .collect();

    // 先按自然序排好，再逐页挑最近修改的文件；最终结果仍按自然序输出。
    image_paths.sort_by(|(a, _), (b, _)| utils::natural_cmp(&name_of(a), &name_of(b)));
    let mut newest_by_stem: HashMap<std::ffi::OsString, (PathBuf, std::time::SystemTime)> =
        HashMap::new();
    for (path, modified) in image_paths {
        let stem = path.file_stem().unwrap_or_default().to_owned();
        match newest_by_stem.get(&stem) {
            Some((_, existing)) if *existing >= modified => {}
            _ => {
                newest_by_stem.insert(stem, (path, modified));
            }
        }
    }
    let mut pages: Vec<PathBuf> = newest_by_stem.into_values().map(|(path, _)| path).collect();
    pages.sort_by(|a, b| utils::natural_cmp(&name_of(a), &name_of(b)));
    Ok(pages)
}

#[cfg(test)]
mod scanner_utils_tests {
    use super::{
        cbz_filename, collect_page_images, read_comic_id_from_cbz, storage_cbz_filename,
        walk_comic_metadata_files,
    };
    use std::path::{Path, PathBuf};

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            let path = std::env::temp_dir()
                .join(format!("joymanga-scanner-test-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&path).expect("create test directory");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn collect_page_images_prefers_newest_file_for_duplicate_stems() {
        let sandbox = TestDir::new();
        let dir = sandbox.path().join("chapter");
        std::fs::create_dir_all(&dir).unwrap();
        // 旧实现按自然序保留 jpg；断点续传补下的 webp 才是可用页
        std::fs::write(dir.join("0001.jpg"), b"old").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(dir.join("0001.webp"), b"new").unwrap();
        std::fs::write(dir.join("0002.jpg"), b"page2").unwrap();
        std::fs::write(dir.join("0010.jpg"), b"page10").unwrap();

        let pages = collect_page_images(&dir).unwrap();
        let names: Vec<_> = pages
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, vec!["0001.webp", "0002.jpg", "0010.jpg"]);
    }

    #[test]
    fn walk_comic_metadata_files_prunes_chapter_images() {
        let sandbox = TestDir::new();
        let comic_dir = sandbox.path().join("author").join("comic_1");
        let chapter_dir = comic_dir.join("chapter_1");
        std::fs::create_dir_all(&chapter_dir).unwrap();

        std::fs::write(comic_dir.join("元数据.json"), b"{}").unwrap();
        std::fs::write(chapter_dir.join("章节元数据.json"), b"{}").unwrap();
        std::fs::write(chapter_dir.join("0001.jpg"), b"fake_img").unwrap();
        std::fs::write(chapter_dir.join("0002.jpg"), b"fake_img").unwrap();

        let entries: Vec<_> = walk_comic_metadata_files(sandbox.path()).collect();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].file_name(), "元数据.json");
    }

    #[test]
    fn walk_comic_metadata_files_skips_hidden_work_dirs() {
        let sandbox = TestDir::new();
        let comic_dir = sandbox.path().join("comic_1");
        std::fs::create_dir_all(&comic_dir).unwrap();
        std::fs::write(comic_dir.join("元数据.json"), b"{}").unwrap();

        // 删除残留（.原名.trash-*）与下载/导入临时目录都是隐藏目录，
        // 不能被当成一本新漫画扫描出来。
        for hidden in [".小说.trash-1234", ".downloading-小说-1", ".import-abc"] {
            let dir = sandbox.path().join(hidden);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("元数据.json"), b"{}").unwrap();
        }

        let entries: Vec<_> = walk_comic_metadata_files(sandbox.path()).collect();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path(), comic_dir.join("元数据.json"));
    }

    #[test]
    fn walk_comic_metadata_files_keeps_legacy_hidden_comic_dirs() {
        let sandbox = TestDir::new();
        // 旧版书库允许漫画目录以 `.` 开头；它不是应用的临时目录，不能隐藏。
        let hidden_comic = sandbox.path().join(".hack  G.U");
        std::fs::create_dir_all(&hidden_comic).unwrap();
        std::fs::write(hidden_comic.join("元数据.json"), b"{}").unwrap();
        // 普通隐藏目录仍然剪枝，避免无谓下钻
        std::fs::create_dir_all(sandbox.path().join(".cache")).unwrap();

        let entries: Vec<_> = walk_comic_metadata_files(sandbox.path()).collect();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path(), hidden_comic.join("元数据.json"));
    }

    #[test]
    fn read_comic_id_from_cbz_produces_unique_hashes_for_different_titles() {
        let sandbox = TestDir::new();
        let cbz_a = sandbox.path().join("(C102) [作者A] 作品一.cbz");
        let cbz_b = sandbox.path().join("(C102) [作者B] 作品二.cbz");

        {
            let f = std::fs::File::create(&cbz_a).unwrap();
            let mut z = zip::ZipWriter::new(f);
            z.start_file("0001.jpg", zip::write::SimpleFileOptions::default())
                .unwrap();
            z.finish().unwrap();
        }
        {
            let f = std::fs::File::create(&cbz_b).unwrap();
            let mut z = zip::ZipWriter::new(f);
            z.start_file("0001.jpg", zip::write::SimpleFileOptions::default())
                .unwrap();
            z.finish().unwrap();
        }

        let id_a = read_comic_id_from_cbz(&cbz_a).unwrap();
        let id_b = read_comic_id_from_cbz(&cbz_b).unwrap();

        assert!(id_a < 0, "外部CBZ应使用负值标识，避免与在线漫画ID冲突");
        assert!(id_b < 0);
        assert_ne!(id_a, id_b, "相同数字前缀但不同标题的CBZ不能产生相同的ID");
        // 同一路径重复计算必须稳定
        assert_eq!(id_a, read_comic_id_from_cbz(&cbz_a).unwrap());
    }

    #[test]
    fn cbz_filename_bounds_byte_length_for_long_titles() {
        let long_comic = "不擅交际的未亡人雪女与诅咒戒指2 [ぷうのぷうぷうぷう (ぷう崎ぷう奈)] 人付き合いが苦手な未亡人の雪女さんと呪いの指輪 2 [中国翻訳]";
        let long_chapter = "第2话 淫书馆汉化(可下载) 附录特典全彩高清完全版详细说明特别篇";
        let filename = cbz_filename(long_comic, 12345, long_chapter, 67890, 2, 5);

        assert!(filename.ends_with(".cbz"));
        assert!(
            filename.len() <= 180,
            "CBZ 文件名 UTF-8 字节长度超过安全上限: {}",
            filename.len()
        );
        assert!(filename.contains(" - 0002 - "));
    }

    #[test]
    fn duplicate_titles_and_sanitized_names_keep_distinct_storage_ids() {
        assert_ne!(
            storage_cbz_filename("同名", 111, 111),
            storage_cbz_filename("同名", 222, 222)
        );
        assert_ne!(
            storage_cbz_filename("a/b", 111, 111),
            storage_cbz_filename("a\\b", 222, 222)
        );
        let long = storage_cbz_filename(&"漫画😀".repeat(300), 111, 111);
        assert!(long.len() <= 180);
        assert!(long.ends_with("[JM111-111].cbz"));
    }
}

#[cfg(test)]
mod new_storage_tests {
    use super::*;
    use std::io::Write;
    #[test]
    fn unique_storage_names_are_resolved_per_chapter() {
        let root = std::env::temp_dir().join(format!("jm-storage-scan-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let chapters: Vec<ChapterInfo> = [11, 12]
            .into_iter()
            .enumerate()
            .map(|(i, id)| ChapterInfo {
                chapter_id: id,
                chapter_title: format!("第{}话", i + 1),
                order: i as i64 + 1,
                is_downloaded: None,
                chapter_download_dir: None,
            })
            .collect();
        let mut comic = Comic {
            id: 111,
            name: "同名".into(),
            chapter_infos: chapters,
            comic_download_dir: Some(root.clone()),
            ..Default::default()
        };
        for chapter in &comic.chapter_infos {
            let path = root.join(storage_cbz_filename(
                &comic.name,
                comic.id,
                chapter.chapter_id,
            ));
            let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
            zip.start_file("chapter.json", zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(
                serde_json::to_string(
                    &serde_json::json!({ "comicId": comic.id, "chapterId": chapter.chapter_id }),
                )
                .unwrap()
                .as_bytes(),
            )
            .unwrap();
            zip.finish().unwrap();
        }
        update_chapter_infos_fields(&mut comic).unwrap();
        for chapter in comic.chapter_infos {
            assert_eq!(chapter.is_downloaded, Some(true));
            assert!(chapter
                .chapter_download_dir
                .unwrap()
                .ends_with(storage_cbz_filename("同名", 111, chapter.chapter_id)));
        }
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn flat_archives_keep_distinct_downloaded_chapters_for_merge_filtering() {
        let root = std::env::temp_dir().join(format!("jm-flat-identity-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let chapters: Vec<ChapterInfo> = [9011, 9012]
            .into_iter()
            .enumerate()
            .map(|(index, id)| ChapterInfo {
                chapter_id: id,
                chapter_title: format!("第{}话", index + 1),
                order: index as i64 + 1,
                is_downloaded: None,
                chapter_download_dir: None,
            })
            .collect();
        let comic = Comic {
            id: 901,
            name: "平铺多话".into(),
            chapter_infos: chapters.clone(),
            ..Comic::default()
        };
        for chapter in &chapters {
            let path = root.join(storage_cbz_filename(
                &comic.name,
                comic.id,
                chapter.chapter_id,
            ));
            let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
            zip.start_file("元数据.json", zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(serde_json::to_string(&comic).unwrap().as_bytes())
                .unwrap();
            zip.start_file("chapter.json", zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(
                serde_json::to_string(
                    &serde_json::json!({"comicId":comic.id,"chapterId":chapter.chapter_id}),
                )
                .unwrap()
                .as_bytes(),
            )
            .unwrap();
            zip.start_file("0001.png", zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"page").unwrap();
            zip.finish().unwrap();
        }
        let books = crate::commands::library_commands::scan_downloaded_comics(&root).comics;
        assert_eq!(books.len(), 1);
        let downloaded: Vec<_> = books[0]
            .chapter_infos
            .iter()
            .filter(|chapter| chapter.is_downloaded == Some(true))
            .collect();
        assert_eq!(downloaded.len(), 2);
        assert_ne!(
            downloaded[0].chapter_download_dir,
            downloaded[1].chapter_download_dir
        );
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 2);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
mod page_entry_tests {
    use super::is_page_image;

    #[test]
    fn page_image_filter_skips_hidden_and_non_image_entries() {
        for (name, expected) in [
            ("001.jpg", true),
            ("dir/002.PNG", true),
            ("003.webp", true),
            ("004.gif", true),
            ("005.jpeg", true),
            (".hidden.jpg", false),
            ("_cover.jpg", false),
            ("ComicInfo.xml", false),
            ("chapter.json", false),
        ] {
            assert_eq!(is_page_image(name), expected, "{name}");
        }
    }
}

#[cfg(test)]
mod merged_archive_tests {
    use super::*;
    use std::io::Write;

    fn chapter(id: i64, order: i64) -> ChapterInfo {
        ChapterInfo {
            chapter_id: id,
            chapter_title: format!("第{order}话"),
            order,
            is_downloaded: None,
            chapter_download_dir: None,
        }
    }

    fn write_merged_book(path: &Path, comic_id: i64, chapters: &[ChapterInfo]) {
        write_merged_book_with_layout(path, comic_id, chapters, true);
    }

    fn write_merged_book_with_layout(
        path: &Path,
        comic_id: i64,
        chapters: &[ChapterInfo],
        chapter_layout: bool,
    ) {
        let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        let mut comic = Comic {
            id: comic_id,
            name: "合并之书".into(),
            chapter_infos: chapters.to_vec(),
            ..Comic::default()
        };
        for ch in &mut comic.chapter_infos {
            ch.is_downloaded = None;
        }
        zip.start_file("元数据.json", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(serde_json::to_string(&comic).unwrap().as_bytes())
            .unwrap();
        let stored_chapters = if chapter_layout {
            chapters
        } else {
            &chapters[..chapters.len().min(1)]
        };
        for ch in stored_chapters {
            let prefix = if chapter_layout {
                format!("[{:02}] {}/", ch.order, ch.chapter_title)
            } else {
                String::new()
            };
            for page in [1, 2] {
                zip.start_file(
                    format!("{prefix}000{page}.jpg"),
                    zip::write::SimpleFileOptions::default(),
                )
                .unwrap();
                zip.write_all(format!("page-{}-{page}", ch.order).as_bytes())
                    .unwrap();
            }
        }
        zip.finish().unwrap();
    }

    #[test]
    fn single_chapter_flat_package_is_merged_but_legacy_multi_chapter_one_is_not() {
        let root = std::env::temp_dir().join(format!("jm-merged-flat-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("单章.cbz");
        write_merged_book_with_layout(&path, 321, &[chapter(101, 1)], false);
        assert_eq!(
            read_merged_archive_identity(&path).unwrap(),
            Some((321, vec![chapter(101, 1)]))
        );
        write_merged_book_with_layout(&path, 321, &[chapter(101, 1), chapter(102, 2)], false);
        assert_eq!(read_merged_archive_identity(&path).unwrap(), None);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn select_chapter_pages_slices_multi_chapter_layout() {
        let names = vec![
            "[01] 第1话/0002.jpg".to_string(),
            "[01] 第1话/0001.jpg".to_string(),
            "[02] 第2话/0001.jpg".to_string(),
        ];
        let first = select_chapter_pages(&names, 1).unwrap();
        assert_eq!(first, vec!["[01] 第1话/0001.jpg", "[01] 第1话/0002.jpg"]);
        let second = select_chapter_pages(&names, 2).unwrap();
        assert_eq!(second, vec!["[02] 第2话/0001.jpg"]);
        // 该话在归档中不存在 -> None（不能退化为全部条目）
        assert!(select_chapter_pages(&names, 3).is_none());
        // 平铺布局 -> 全部条目
        let flat = vec!["0001.jpg".to_string(), "0002.jpg".to_string()];
        assert_eq!(select_chapter_pages(&flat, 1).unwrap().len(), 2);
    }

    #[test]
    fn bracket_order_recognizes_padded_prefixes_only() {
        assert_eq!(bracket_order("[01] 第1话/0001.jpg"), Some(1));
        assert_eq!(bracket_order("[105] x/1.jpg"), Some(105));
        assert_eq!(bracket_order("[1] x/1.jpg"), None);
        assert_eq!(bracket_order("0001.jpg"), None);
        assert_eq!(bracket_order("[ab] x/1.jpg"), None);
    }

    #[test]
    fn bracket_order_handles_more_than_999_chapters() {
        // 写入端 `[{order:0width$}]` 在话数超过 999 时会产生 4 位前缀，
        // 解析端必须能认出来，否则整本归档在阅读/切片/导出时找不到本话页面。
        assert_eq!(bracket_order("[999] x/1.jpg"), Some(999));
        assert_eq!(bracket_order("[1000] 千话/0001.jpg"), Some(1000));
        assert_eq!(bracket_order("[12345] x/1.jpg"), Some(12345));
        assert_eq!(bracket_order("[1234567] x/1.jpg"), None);
    }

    #[test]
    fn from_cbz_marks_all_chapters_of_merged_archive() {
        let root = std::env::temp_dir().join(format!("jm-merged-scan-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("合并之书.cbz");
        write_merged_book(&path, 321, &[chapter(101, 1), chapter(102, 2)]);
        let comic = from_cbz(&path).unwrap();
        assert_eq!(comic.id, 321);
        assert_eq!(comic.chapter_infos.len(), 2);
        for ch in &comic.chapter_infos {
            assert_eq!(ch.is_downloaded, Some(true));
            assert_eq!(ch.chapter_download_dir.as_deref(), Some(path.as_path()));
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn network_comic_sync_marks_chapters_contained_in_merged_archive() {
        let root = std::env::temp_dir().join(format!("jm-merged-sync-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("合并之书.cbz");
        // 归档只包含第1、2话；网络漫画共有3话
        write_merged_book(&path, 321, &[chapter(101, 1), chapter(102, 2)]);
        let mut comic = Comic {
            id: 321,
            name: "合并之书".into(),
            chapter_infos: vec![chapter(101, 1), chapter(102, 2), chapter(103, 3)],
            comic_download_dir: Some(path.clone()),
            ..Comic::default()
        };
        update_chapter_infos_fields(&mut comic).unwrap();
        assert_eq!(comic.chapter_infos[0].is_downloaded, Some(true));
        assert_eq!(comic.chapter_infos[1].is_downloaded, Some(true));
        assert_eq!(
            comic.chapter_infos[0].chapter_download_dir.as_deref(),
            Some(path.as_path())
        );
        assert_eq!(comic.chapter_infos[2].is_downloaded, None);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn merged_archive_of_another_comic_is_never_borrowed() {
        let root = std::env::temp_dir().join(format!("jm-merged-odd-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("合并之书.cbz");
        write_merged_book(&path, 999, &[chapter(101, 1), chapter(102, 2)]);
        let mut comic = Comic {
            id: 321,
            name: "合并之书".into(),
            chapter_infos: vec![chapter(101, 1), chapter(102, 2)],
            comic_download_dir: Some(path.clone()),
            ..Comic::default()
        };
        update_chapter_infos_fields(&mut comic).unwrap();
        // 归档身份是 999，不能给 321 用；但文件名前缀相同，只允许走常规匹配（不命中）
        assert!(comic
            .chapter_infos
            .iter()
            .all(|ch| ch.is_downloaded != Some(true)));
        std::fs::remove_dir_all(root).unwrap();
    }
}
