//! 页面尺寸：从图片文件头解析像素尺寸，并缓存在归档内的 `pages.json` 里。
//!
//! 阅读器需要每页的像素尺寸来在图片解码前预留高度。解码前 `<img>` 的高度是 0，
//! 滚动模式下任意一页解码完成都会把下方内容推开，程序化定位（恢复阅读进度、跳页）
//! 就再也对不准——旧实现靠 ResizeObserver + 定时轮询反复把目标页贴回顶部来兜底。
//!
//! 尺寸只读文件头，不解码像素。

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Cursor, Read, Seek, Write};
use std::path::Path;

use crate::image_data::MAX_IMAGE_PIXELS;
use crate::types::PageSize;

/// 尺寸缓存在归档内的条目名。
pub const PAGES_ENTRY: &str = "pages.json";

const PAGES_VERSION: u32 = 1;

/// 压缩条目只能流式解压、不能随机定位，最多读出这么多字节去找文件头。
///
/// 常见格式的尺寸字段都在开头：PNG 在 IHDR（前 33 字节），WebP 在 VP8/VP8L/VP8X
/// 头（前 30 字节内），JPEG 需要跳过 APPn 段，带 EXIF 缩略图时可能到几十 KB。
/// 超出这个预算就放弃，该页退化为不预留高度。
const MAX_HEAD_BYTES: u64 = 128 * 1024;

#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct PagesFile {
    version: u32,
    /// 归档条目名 -> 尺寸。合并归档的键带 `[NN] 话名/` 前缀，与阅读器枚举到的条目名一致。
    pages: BTreeMap<String, PageSize>,
}

/// 从已缓冲的图片读取器解析尺寸；只读文件头，不解码像素。
///
/// 尺寸为 0 或像素数超过 [`MAX_IMAGE_PIXELS`] 时返回 `None`：这类图片在
/// `image_data::validate_image` 里同样会被拒绝，预留高度没有意义。
pub fn size_from_reader<R: BufRead + Seek>(reader: R) -> Option<PageSize> {
    let (width, height) = image::ImageReader::new(reader)
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()?;
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > MAX_IMAGE_PIXELS {
        return None;
    }
    Some(PageSize { width, height })
}

/// 解析磁盘上的图片文件尺寸。
pub fn size_from_path(path: &Path) -> Option<PageSize> {
    let file = std::fs::File::open(path).ok()?;
    size_from_reader(BufReader::new(file))
}

/// 从归档条目解析尺寸。
///
/// 本应用写出的图片条目都是 Stored，`by_name_seek` 直接定位到文件头，只读几 KB；
/// 外来归档（导入的 CBZ）常把图片压成 Deflate，只能流式解压前 [`MAX_HEAD_BYTES`]
/// 字节再找文件头。两者都失败返回 `None`。
pub fn size_from_archive<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    name: &str,
) -> Option<PageSize> {
    if let Ok(seekable) = archive.by_name_seek(name) {
        if let Some(size) = size_from_reader(BufReader::new(seekable)) {
            return Some(size);
        }
    }
    let entry = archive.by_name(name).ok()?;
    let mut head = Vec::new();
    entry.take(MAX_HEAD_BYTES).read_to_end(&mut head).ok()?;
    size_from_reader(Cursor::new(head))
}

/// 读取归档内的 `pages.json`；条目不存在、版本不认识或内容损坏时返回空表，
/// 调用方按「没有缓存」处理并逐页现算。
pub fn read_pages_entry<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
) -> BTreeMap<String, PageSize> {
    let Ok(mut entry) = archive.by_name(PAGES_ENTRY) else {
        return BTreeMap::new();
    };
    let declared = entry.size();
    let Ok(bytes) = crate::image_data::read_limited(
        &mut entry,
        declared,
        crate::image_data::MAX_METADATA_BYTES,
    ) else {
        return BTreeMap::new();
    };
    let Ok(file) = serde_json::from_slice::<PagesFile>(&bytes) else {
        return BTreeMap::new();
    };
    if file.version != PAGES_VERSION {
        return BTreeMap::new();
    }
    file.pages
}

/// 把尺寸表写进归档。空表不写条目，保持旧归档的形态。
pub fn write_pages_entry<W: Write + Seek>(
    zip: &mut zip::ZipWriter<W>,
    pages: &BTreeMap<String, PageSize>,
) -> anyhow::Result<()> {
    if pages.is_empty() {
        return Ok(());
    }
    let json = serde_json::to_vec(&PagesFile {
        version: PAGES_VERSION,
        pages: pages.clone(),
    })?;
    zip.start_file(PAGES_ENTRY, zip::write::SimpleFileOptions::default())?;
    zip.write_all(&json)?;
    Ok(())
}

/// 按归档条目名逐页取尺寸；缓存里的直接命中，其余现算。
///
/// 返回值与 `names` 一一对应，取不到的位置是 `None`（该页不预留高度）。
pub fn resolve_archive_sizes<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    names: &[String],
) -> Vec<Option<PageSize>> {
    let cached = read_pages_entry(archive);
    names
        .iter()
        .map(|name| match cached.get(name) {
            Some(size) => Some(*size),
            None => size_from_archive(archive, name),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png_bytes(width: u32, height: u32) -> Vec<u8> {
        let image = image::RgbImage::new(width, height);
        let mut bytes = Vec::new();
        image
            .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        bytes
    }

    #[test]
    fn reads_dimensions_without_decoding_pixels() {
        let bytes = png_bytes(37, 53);
        let size = size_from_reader(BufReader::new(Cursor::new(bytes))).unwrap();
        assert_eq!(size.width, 37);
        assert_eq!(size.height, 53);
    }

    #[test]
    fn rejects_data_that_is_not_an_image() {
        assert!(size_from_reader(BufReader::new(Cursor::new(b"not an image".to_vec()))).is_none());
    }

    #[test]
    fn pages_entry_round_trips_through_archive() {
        let root = std::env::temp_dir().join(format!("jm-pages-{}", uuid::Uuid::new_v4()));
        let path = root.join("chapter.cbz");
        std::fs::create_dir_all(&root).unwrap();

        let mut pages = BTreeMap::new();
        pages.insert(
            "0001.png".to_string(),
            PageSize {
                width: 800,
                height: 1200,
            },
        );
        {
            let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
            write_pages_entry(&mut zip, &pages).unwrap();
            zip.start_file("0001.png", zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(&png_bytes(800, 1200)).unwrap();
            zip.finish().unwrap();
        }

        let mut archive = zip::ZipArchive::new(std::fs::File::open(&path).unwrap()).unwrap();
        assert_eq!(read_pages_entry(&mut archive).len(), 1);
        let sizes = resolve_archive_sizes(&mut archive, &["0001.png".to_string()]);
        assert_eq!(sizes.len(), 1);
        assert_eq!(sizes[0].unwrap().width, 800);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn deflated_entries_fall_back_to_streaming_read() {
        let root = std::env::temp_dir().join(format!("jm-pages-deflate-{}", uuid::Uuid::new_v4()));
        let path = root.join("import.cbz");
        std::fs::create_dir_all(&root).unwrap();
        {
            let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
            // 第三方归档常见做法：图片走 Deflate，`by_name_seek` 会明确拒绝
            zip.start_file(
                "page.png",
                zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Deflated),
            )
            .unwrap();
            zip.write_all(&png_bytes(120, 240)).unwrap();
            zip.finish().unwrap();
        }

        let mut archive = zip::ZipArchive::new(std::fs::File::open(&path).unwrap()).unwrap();
        // 没有 pages.json 时必须现算，且 Deflate 条目也要能算出来
        let sizes = resolve_archive_sizes(&mut archive, &["page.png".to_string()]);
        let size = sizes[0].expect("deflated 条目也要能读出尺寸");
        assert_eq!((size.width, size.height), (120, 240));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_pages_entry_is_not_an_error() {
        let root = std::env::temp_dir().join(format!("jm-pages-none-{}", uuid::Uuid::new_v4()));
        let path = root.join("legacy.cbz");
        std::fs::create_dir_all(&root).unwrap();
        {
            let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
            zip.start_file("0001.png", zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(&png_bytes(10, 20)).unwrap();
            zip.finish().unwrap();
        }
        let mut archive = zip::ZipArchive::new(std::fs::File::open(&path).unwrap()).unwrap();
        assert!(read_pages_entry(&mut archive).is_empty());
        let sizes = resolve_archive_sizes(&mut archive, &["0001.png".to_string()]);
        assert_eq!(
            (sizes[0].unwrap().width, sizes[0].unwrap().height),
            (10, 20)
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
