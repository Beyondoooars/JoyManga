use std::path::Path;

use anyhow::{anyhow, ensure, Context};
use base64::engine::general_purpose;
use base64::Engine as _;

use crate::{
    atomic_fs,
    extensions::{mime_for_image, PathIsImg},
    types::{ChapterInfo, Comic, ComicInfo},
    utils,
};

pub fn load_local_cover_data(comic_download_dir: &Path) -> Option<String> {
    let mut cbz_files = Vec::new();
    if comic_download_dir.is_file() {
        if comic_download_dir
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_lowercase)
            == Some("cbz".to_string())
        {
            cbz_files.push(comic_download_dir.to_path_buf());
        }
    } else {
        if let Ok(entries) = std::fs::read_dir(comic_download_dir) {
            for entry in entries.filter_map(Result::ok) {
                let path = entry.path();
                if path.is_file()
                    && path
                        .extension()
                        .and_then(|e| e.to_str())
                        .map(str::to_lowercase)
                        == Some("cbz".to_string())
                {
                    cbz_files.push(path);
                }
            }
        }
        let cbz_sub = comic_download_dir.join("cbz");
        if cbz_sub.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&cbz_sub) {
                for entry in entries.filter_map(Result::ok) {
                    let path = entry.path();
                    if path.is_file()
                        && path
                            .extension()
                            .and_then(|e| e.to_str())
                            .map(str::to_lowercase)
                            == Some("cbz".to_string())
                    {
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
        let Ok(file) = std::fs::File::open(cbz_path) else {
            continue;
        };
        let Ok(mut archive) = zip::ZipArchive::new(file) else {
            continue;
        };

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

        img_names.sort_by(|a, b| utils::natural_cmp(a, b));

        if let Some(first_img) = img_names.first() {
            if let Ok(mut entry) = archive.by_name(first_img) {
                if entry.size() > 32 * 1024 * 1024 {
                    continue;
                }
                let declared_size = entry.size();
                let Ok(bytes) = crate::image_data::read_limited(
                    &mut entry,
                    declared_size,
                    crate::image_data::MAX_PAGE_BYTES,
                ) else {
                    continue;
                };
                if !bytes.is_empty() {
                    // 缩略图按「CBZ 路径 + mtime + 大小」落盘缓存：同一本书重复扫描时
                    // 不必再解压解码，只有归档被替换后才会重新生成。
                    let thumbnail =
                        crate::cover_cache::cached_thumbnail(cbz_path, || render_thumbnail(&bytes));
                    return Some(match thumbnail {
                        Some(thumbnail) => base64_data_url("image/jpeg", &thumbnail),
                        None => raw_cover_data_url(&bytes, mime_for_image(Path::new(first_img)))?,
                    });
                }
            }
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
        let fallback_id = utils::external_cbz_id(cbz_path);

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
    comic.local_cover_data = load_local_cover_data(cbz_path);

    let identity = read_chapter_identity(cbz_path)?;
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
        let expected_name = utils::cbz_filename(
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
    // 将漫画的is_downloaded和comic_download_dir字段设置为None
    // 这样能使这些字段在序列化时被忽略
    clean_comic.is_downloaded = None;
    clean_comic.comic_download_dir = None;
    clean_comic.local_cover_data = None;
    for chapter in &mut clean_comic.chapter_infos {
        // 将章节的is_downloaded和chapter_download_dir字段设置为None
        // 这样能使这些字段在序列化时被忽略
        chapter.is_downloaded = None;
        chapter.chapter_download_dir = None;
    }

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
                if path.is_file()
                    && path
                        .extension()
                        .and_then(|e| e.to_str())
                        .map(str::to_lowercase)
                        == Some("cbz".to_string())
                {
                    cbz_files.push(path);
                }
            }
        }
    }

    let is_flat_mode = comic_download_dir.is_file();
    let comic_name_filtered = utils::filename_filter(&comic.name);
    let comic_prefix = format!("{comic_name_filtered} - ");

    // 整本合并归档（无 chapter.json 但带 元数据.json）：先按文件名前缀粗筛，命中的话按内嵌章节列表整体标记。
    let merged_archives: Vec<(std::path::PathBuf, Vec<ChapterInfo>)> = cbz_files
        .iter()
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    !comic_name_filtered.is_empty() && name.starts_with(&comic_name_filtered)
                })
        })
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
        let expected_cbz_name = utils::cbz_filename(
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
        archive
            .by_index(index)
            .ok()
            .is_some_and(|entry| bracket_order(entry.name()).is_some())
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
    // 没有 `[NN] ` 话目录布局的归档不是本应用的整本合并产物（例如旧式单话章节包）。
    if !archive_has_chapter_layout(&mut archive) {
        return Ok(None);
    }
    let Ok(mut entry) = archive.by_name("元数据.json") else {
        return Ok(None);
    };
    let size = entry.size();
    let bytes =
        crate::image_data::read_limited(&mut entry, size, crate::image_data::MAX_METADATA_BYTES)?;
    let comic: Comic = serde_json::from_slice(&bytes)?;
    Ok(Some((comic.id, comic.chapter_infos)))
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
            let path = root.join(utils::storage_cbz_filename(
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
                .ends_with(utils::storage_cbz_filename("同名", 111, chapter.chapter_id)));
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
            let path = root.join(utils::storage_cbz_filename(
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
        for ch in chapters {
            let dir = format!("[{:02}] {}", ch.order, ch.chapter_title);
            for page in [1, 2] {
                zip.start_file(
                    format!("{dir}/000{page}.jpg"),
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
