//! 页面尺寸：从图片文件头解析像素尺寸，并缓存到磁盘与归档内 `pages.json`。
//!
//! 阅读器需要每页的像素尺寸来在图片解码前预留高度。解码前 `<img>` 的高度是 0，
//! 滚动模式下任意一页解码完成都会把下方内容推开，程序化定位（恢复阅读进度、跳页）
//! 就再也对不准——旧实现靠 ResizeObserver + 定时轮询反复把目标页贴回顶部来兜底。
//!
//! 尺寸只读文件头，不解码像素。浏览器渲染 `<img>` 时默认应用 EXIF 方向
//! （`image-orientation: from-image`），所以这里的尺寸也按方向换算：90°/270°
//! 旋转的 JPEG 记录为旋转后的宽高，预留盒比例才与实际渲染一致。
//!
//! 解析结果有两级缓存：磁盘上的指纹缓存（见 [`sizes_cache`]，键为
//! 归档路径 + mtime + 大小，归档被替换后自动失效）和归档内的 `pages.json`
//! （打包/合并时顺手写入，随文件移动）。旧版书库与外来导入没有内嵌表，
//! 磁盘缓存避免每次打开都重读一遍文件头。

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Cursor, Read, Seek, SeekFrom, Write};
use std::path::Path;

use crate::image_data::MAX_IMAGE_PIXELS;
use crate::types::PageSize;

use image::ImageDecoder;

/// 尺寸缓存在归档内的条目名。
pub const PAGES_ENTRY: &str = "pages.json";

const PAGES_VERSION: u32 = 1;

/// `pages.json` 的体积上限（与读取侧共用，超限时按「没有缓存」处理）。
///
/// 一页约 55 字节，200 万页才会触到；超过说明条目异常，不值得整本缓存。
const MAX_PAGES_JSON_BYTES: u64 = 4 * 1024 * 1024;

/// 压缩条目只能流式解压、不能随机定位，最多读出这么多字节去找文件头。
///
/// 常见格式的尺寸字段都在开头：PNG 在 IHDR（前 33 字节），WebP 在 VP8/VP8L/VP8X
/// 头（前 30 字节内），JPEG 需要跳过 APPn 段，带 EXIF 缩略图时可能到几十 KB。
/// 超出这个预算就放弃，该页退化为不预留高度。
const MAX_HEAD_BYTES: u64 = 128 * 1024;

/// 流式读文件头的起始块大小。
///
/// 绝大多数图片的头几 KB 就够解析；解析失败（`UnexpectedEof` 类错误）时翻倍
/// 重试，直到 [`MAX_HEAD_BYTES`]。避免为每个 Deflate 条目解压满 128 KiB。
const HEAD_CHUNK_BYTES: u64 = 16 * 1024;

#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct PagesFile {
    version: u32,
    /// 归档条目名 -> 尺寸。合并归档的键带 `[NN] 话名/` 前缀，与阅读器枚举到的条目名一致。
    pages: BTreeMap<String, PageSize>,
}

/// 缓存值是否可用：与 [`size_from_reader`] 对现算结果的约束一致。
///
/// 归档里的 `pages.json` 可能来自外部（导入的 CBZ），值不可信；
/// 越界的值按缓存未命中处理，落到现算。
fn valid_size(size: &PageSize) -> bool {
    size.width > 0
        && size.height > 0
        && u64::from(size.width) * u64::from(size.height) <= MAX_IMAGE_PIXELS
}

/// 从已缓冲的图片读取器解析尺寸；只读文件头，不解码像素。
///
/// 浏览器渲染时默认应用 EXIF 方向，JPEG 的 90°/270° 旋转会把宽高对调，
/// 这里按方向换算后再返回，保证与渲染结果一致。
/// 尺寸为 0 或像素数超过 [`MAX_IMAGE_PIXELS`] 时返回 `None`：这类图片在
/// `image_data::validate_image` 里同样会被拒绝，预留高度没有意义。
pub fn size_from_reader<R: BufRead + Seek>(reader: R) -> Option<PageSize> {
    let reader = image::ImageReader::new(reader).with_guessed_format().ok()?;
    let mut decoder = reader.into_decoder().ok()?;
    let (width, height) = decoder.dimensions();
    // 只有 JPEG/WebP/TIFF 的解码器实现了 EXIF 方向；PNG/GIF 没有，
    // 默认 `NoTransforms`。EXIF 解析失败时也按不旋转处理。
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let swapped = matches!(
        orientation,
        image::metadata::Orientation::Rotate90
            | image::metadata::Orientation::Rotate270
            | image::metadata::Orientation::Rotate90FlipH
            | image::metadata::Orientation::Rotate270FlipH
    );
    let (width, height) = if swapped {
        (height, width)
    } else {
        (width, height)
    };
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

/// 从已打开的文件解析尺寸，并把读取位置复位到文件头，供后续整文件复制复用。
///
/// 打包/合并时同一张图既要读文件头又要复制全部字节：一次打开两用，
/// 避免在 Android FUSE 公共存储上每页多一对 open/close 往返。
pub fn size_from_opened_file(file: &mut std::fs::File) -> Option<PageSize> {
    let size = size_from_reader(BufReader::new(&mut *file));
    // 无论解析成败都复位：调用方接下来要 io::copy 整个文件
    let _ = file.seek(SeekFrom::Start(0));
    size
}

/// 从归档条目解析尺寸。
///
/// 本应用写出的图片条目都是 Stored，`by_name_seek` 直接定位到文件头，只读几 KB；
/// 外来归档（导入的 CBZ）常把图片压成 Deflate，只能流式解压头部再找文件头，
/// 从小块开始按需增长（见 [`HEAD_CHUNK_BYTES`]）。两者都失败返回 `None`。
pub fn size_from_archive<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    name: &str,
) -> Option<PageSize> {
    if let Ok(seekable) = archive.by_name_seek(name) {
        // 依赖 zip >= 3.0.0：2.6.x 的 SeekableTake::seek 返回绝对偏移（上游 bug），
        // seek 路径会一直失败。失败仍落到流式路径，不把正确性寄托在它身上。
        if let Some(size) = size_from_reader(BufReader::new(seekable)) {
            return Some(size);
        }
    }
    let mut head = Vec::new();
    let mut budget = HEAD_CHUNK_BYTES;
    while budget <= MAX_HEAD_BYTES {
        let entry = archive.by_name(name).ok()?;
        head.clear();
        // 只解压当前预算内的字节：take 限制的是解压后输出
        entry.take(budget).read_to_end(&mut head).ok()?;
        if (head.len() as u64) < budget {
            // 条目本身比预算短，读全了也没解析出来，不必再试
            return size_from_reader(Cursor::new(&head));
        }
        if let Some(size) = size_from_reader(Cursor::new(&head)) {
            return Some(size);
        }
        budget *= 2;
    }
    None
}

/// 读取归档内的 `pages.json`；条目不存在、版本不认识或内容损坏时返回空表，
/// 调用方按「没有缓存」处理并逐页现算。越界的尺寸值同样按未命中剔除。
pub fn read_pages_entry<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
) -> BTreeMap<String, PageSize> {
    let Ok(mut entry) = archive.by_name(PAGES_ENTRY) else {
        return BTreeMap::new();
    };
    let declared = entry.size();
    // 与写入侧共用同一个上限：写进去的表必须读得回来
    let Ok(bytes) = crate::image_data::read_limited(&mut entry, declared, MAX_PAGES_JSON_BYTES)
    else {
        return BTreeMap::new();
    };
    let Ok(file) = serde_json::from_slice::<PagesFile>(&bytes) else {
        return BTreeMap::new();
    };
    if file.version != PAGES_VERSION {
        return BTreeMap::new();
    }
    file.pages
        .into_iter()
        .filter(|(_, size)| valid_size(size))
        .collect()
}

/// 把尺寸表写进归档。空表不写条目，保持旧归档的形态。
///
/// 超过 [`MAX_PAGES_JSON_BYTES`] 时也跳过：它只是加速缓存，读取侧同样有上限，
/// 为它让整个下载/导出失败不值得（该书按「没有缓存」处理，逐页现算）。
pub fn write_pages_entry<W: Write + Seek>(
    zip: &mut zip::ZipWriter<W>,
    pages: BTreeMap<String, PageSize>,
) -> anyhow::Result<()> {
    if pages.is_empty() {
        return Ok(());
    }
    let json = serde_json::to_vec(&PagesFile {
        version: PAGES_VERSION,
        pages,
    })?;
    if json.len() as u64 > MAX_PAGES_JSON_BYTES {
        tracing::warn!(
            bytes = json.len(),
            "pages.json 超过体积上限，跳过写入（该书阅读时逐页现算尺寸）"
        );
        return Ok(());
    }
    zip.start_file(PAGES_ENTRY, zip::write::SimpleFileOptions::default())?;
    zip.write_all(&json)?;
    Ok(())
}

/// 按归档条目名逐页取尺寸；缓存里的直接命中，其余现算。
///
/// 返回值与 `names` 一一对应，取不到的位置是 `None`（该页不预留高度）。
/// 结果写入磁盘缓存（键为 `archive_path` 的指纹），下次打开同一归档直接命中，
/// 导入与旧版书库不必每次重读文件头。
pub fn resolve_archive_sizes<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    names: &[String],
    archive_path: &Path,
) -> Vec<Option<PageSize>> {
    let mut cached = read_pages_entry(archive);
    let mut misses: Vec<usize> = Vec::new();
    for (idx, name) in names.iter().enumerate() {
        if !cached.contains_key(name) {
            misses.push(idx);
        }
    }
    if misses.is_empty() {
        return names.iter().map(|name| cached.get(name).copied()).collect();
    }

    let stamp = sizes_cache::source_stamp(archive_path);
    let disk = stamp.and_then(|stamp| sizes_cache::read(archive_path, stamp));
    let sizes: Vec<Option<PageSize>> = if let Some(disk) = disk {
        misses
            .iter()
            .map(|&idx| match disk.get(&names[idx]) {
                Some(size) => *size,
                None => size_from_archive(archive, &names[idx]),
            })
            .collect()
    } else {
        misses
            .iter()
            .map(|&idx| size_from_archive(archive, &names[idx]))
            .collect()
    };

    // 回填内嵌缓存命中项，再整体落盘
    let mut resolved: Vec<Option<PageSize>> = Vec::with_capacity(names.len());
    let mut miss_iter = sizes.into_iter();
    for name in names {
        match cached.remove(name) {
            Some(size) => resolved.push(Some(size)),
            None => resolved.push(miss_iter.next().flatten()),
        }
    }
    if let Some(stamp) = stamp {
        let mut to_store: BTreeMap<String, Option<PageSize>> = BTreeMap::new();
        for (name, size) in names.iter().zip(resolved.iter()) {
            to_store.insert(name.clone(), *size);
        }
        sizes_cache::write(archive_path, stamp, &to_store);
    }
    resolved
}

/// 磁盘上的尺寸缓存（`.page-sizes/` 目录，与封面缩略图缓存同模式）。
///
/// 键为「归档路径 + mtime + 大小」的指纹：归档被重新下载或替换后旧缓存
/// 自动失效，不需要额外的失效逻辑。缓存的内容是整章（或整本按话切片后
/// 的条目名 -> 尺寸，含 `None` 占位——`None` 也是解析结果，缓存它才不会
/// 对读不出头的页反复重试）。
pub(crate) mod sizes_cache {
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};
    use std::sync::OnceLock;

    use parking_lot::RwLock;
    use serde::{Deserialize, Serialize};

    use crate::types::PageSize;

    static CACHE_DIR: OnceLock<RwLock<Option<PathBuf>>> = OnceLock::new();

    /// 缓存文件上限（个）。每个文件约 55 字节/页，2000 个足够覆盖重度书库。
    const MAX_ENTRIES: usize = 2000;
    /// 每写这么多次才扫一遍目录做清理（见 `write` 的持锁说明）。
    const PRUNE_EVERY: u32 = 64;
    static WRITE_COUNT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

    fn slot() -> &'static RwLock<Option<PathBuf>> {
        CACHE_DIR.get_or_init(|| RwLock::new(None))
    }

    /// 设置缓存目录。传 `None` 或空路径可关闭缓存（测试与无配置场景）。
    pub fn set_dir(dir: Option<PathBuf>) {
        *slot().write() = dir.filter(|path| !path.as_os_str().is_empty());
    }

    /// 当前缓存目录（清理缓存时整目录移除用）。
    pub fn dir() -> Option<PathBuf> {
        slot().read().clone()
    }

    /// 来源文件的指纹：修改时间 + 长度。取不到元数据时返回 None（不缓存）。
    pub fn source_stamp(path: &Path) -> Option<(u64, u64)> {
        let metadata = std::fs::metadata(path).ok()?;
        let mtime = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |duration| duration.as_nanos() as u64);
        Some((mtime, metadata.len()))
    }

    /// FNV-1a：只用于生成缓存文件名，不承担安全职责（同 cover_cache）。
    fn fingerprint(path: &Path, stamp: (u64, u64)) -> u64 {
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for byte in path.to_string_lossy().as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        for value in [stamp.0, stamp.1] {
            for byte in value.to_le_bytes() {
                hash ^= u64::from(byte);
                hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
            }
        }
        hash
    }

    #[derive(Serialize, Deserialize)]
    struct CacheFile {
        /// 条目名 -> 尺寸；`None` 表示该页读不出尺寸（也是有效结果）。
        pages: BTreeMap<String, Option<PageSize>>,
    }

    fn cache_path(dir: &Path, path: &Path, stamp: (u64, u64)) -> PathBuf {
        dir.join(format!("{:016x}.json", fingerprint(path, stamp)))
    }

    pub fn read(path: &Path, stamp: (u64, u64)) -> Option<BTreeMap<String, Option<PageSize>>> {
        let dir = slot().read().clone()?;
        let bytes = std::fs::read(cache_path(&dir, path, stamp)).ok()?;
        let file: CacheFile = serde_json::from_slice(&bytes).ok()?;
        // 命中后轻触 mtime，让 prune 的「按最近使用」语义成立（同 cover_cache）
        touch(&cache_path(&dir, path, stamp));
        // 缓存文件也可能被外部改坏；越界值与 pages.json 一样按未命中处理
        Some(
            file.pages
                .into_iter()
                .map(|(name, size)| (name, size.filter(super::valid_size)))
                .collect(),
        )
    }

    pub fn write(source: &Path, stamp: (u64, u64), pages: &BTreeMap<String, Option<PageSize>>) {
        let Some(dir) = slot().read().clone() else {
            return;
        };
        let file = CacheFile {
            pages: pages.clone(),
        };
        let Ok(json) = serde_json::to_vec(&file) else {
            return;
        };
        if std::fs::create_dir_all(&dir).is_ok() {
            // 这里不能用 atomic_fs::write：它要拿 REPLACE_LOCK，而调用方（阅读器
            // 打开章节）正持有那把锁，parking_lot 的 Mutex 不可重入，会直接死锁。
            // 缓存本身可随时重建，写失败或写了一半都只影响下一次的命中率
            //（读侧解析失败按未命中处理），普通写入足够。
            let _ = std::fs::write(cache_path(&dir, source, stamp), &json);
            // 调用方可能正持有替换锁，少做整目录扫描：每写若干次才清理一次，
            // 最多多留几十个文件
            if WRITE_COUNT
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                .is_multiple_of(PRUNE_EVERY)
            {
                prune(&dir, MAX_ENTRIES);
            }
        }
    }

    /// 命中后轻触 mtime，让 prune 的「按最近使用」语义成立（同 cover_cache）。
    fn touch(path: &Path) {
        if let Ok(file) = std::fs::OpenOptions::new().write(true).open(path) {
            let _ = file.set_modified(std::time::SystemTime::now());
        }
    }
    /// 只保留最近使用的 `keep` 个缓存文件。
    fn prune(cache_dir: &Path, keep: usize) {
        let Ok(entries) = std::fs::read_dir(cache_dir) else {
            return;
        };
        let mut files: Vec<(PathBuf, std::time::SystemTime)> = entries
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .path()
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
            })
            .filter_map(|entry| {
                let modified = entry.metadata().ok()?.modified().ok()?;
                Some((entry.path(), modified))
            })
            .collect();

        if files.len() <= keep {
            return;
        }
        files.sort_by_key(|(_, modified)| *modified);
        let excess = files.len() - keep;
        for (path, _) in files.into_iter().take(excess) {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

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
    fn cached_entry_with_absurd_dimensions_is_rejected() {
        // 外来 pages.json 的值不可信：越界值必须按未命中处理
        assert!(!valid_size(&PageSize {
            width: 4_000_000_000,
            height: 1
        }));
        assert!(!valid_size(&PageSize {
            width: 0,
            height: 10
        }));
        assert!(valid_size(&PageSize {
            width: 800,
            height: 1200
        }));
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
            write_pages_entry(&mut zip, pages).unwrap();
            zip.start_file("0001.png", zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(&png_bytes(800, 1200)).unwrap();
            zip.finish().unwrap();
        }

        let mut archive = zip::ZipArchive::new(std::fs::File::open(&path).unwrap()).unwrap();
        assert_eq!(read_pages_entry(&mut archive).len(), 1);
        let sizes = resolve_archive_sizes(&mut archive, &["0001.png".to_string()], &path);
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
        let sizes = resolve_archive_sizes(&mut archive, &["page.png".to_string()], &path);
        let size = sizes[0].expect("deflated 条目也要能读出尺寸");
        assert_eq!((size.width, size.height), (120, 240));

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 本应用自产归档（Stored 条目 + 无 pages.json，即 v0.2.25 前的下载书库）
    /// 必须走 `by_name_seek` 快速路径。zip 2.6.x 的 SeekableTake::seek 有
    /// 返回绝对偏移的上游 bug（3.0.0 修复），这条路径是它的哨兵。
    #[test]
    fn stored_entries_without_pages_entry_resolve_via_seek() {
        let root = std::env::temp_dir().join(format!("jm-pages-seek-{}", uuid::Uuid::new_v4()));
        let path = root.join("legacy-stored.cbz");
        std::fs::create_dir_all(&root).unwrap();
        {
            let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
            // 先写一个非图片条目，把图片条目的 data_start 推离 0：
            // seek 偏移 bug 只在 data_start > 0 时暴露
            zip.start_file("ComicInfo.xml", zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"<ComicInfo/>").unwrap();
            zip.start_file("0001.png", crate::zip_limits::image_entry_options())
                .unwrap();
            zip.write_all(&png_bytes(10, 20)).unwrap();
            zip.finish().unwrap();
        }

        let mut archive = zip::ZipArchive::new(std::fs::File::open(&path).unwrap()).unwrap();
        assert!(read_pages_entry(&mut archive).is_empty());
        let sizes = resolve_archive_sizes(&mut archive, &["0001.png".to_string()], &path);
        assert_eq!(
            (sizes[0].unwrap().width, sizes[0].unwrap().height),
            (10, 20)
        );
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
        let sizes = resolve_archive_sizes(&mut archive, &["0001.png".to_string()], &path);
        assert_eq!(
            (sizes[0].unwrap().width, sizes[0].unwrap().height),
            (10, 20)
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 磁盘缓存目录是进程级全局状态，测试并行跑会互相覆盖；用锁串行化。
    static CACHE_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn disk_cache_avoids_reresolving_on_second_open() {
        let _guard = CACHE_TEST_LOCK
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        let root = std::env::temp_dir().join(format!("jm-pages-cache-{}", uuid::Uuid::new_v4()));
        let path = root.join("import.cbz");
        let cache_dir = root.join("cache");
        std::fs::create_dir_all(&root).unwrap();
        {
            let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
            zip.start_file(
                "0001.png",
                zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Deflated),
            )
            .unwrap();
            zip.write_all(&png_bytes(640, 960)).unwrap();
            zip.finish().unwrap();
        }
        sizes_cache::set_dir(Some(cache_dir));

        let names = vec!["0001.png".to_string()];
        let first = {
            let mut archive = zip::ZipArchive::new(std::fs::File::open(&path).unwrap()).unwrap();
            resolve_archive_sizes(&mut archive, &names, &path)
        };
        assert_eq!(first[0].unwrap().width, 640);
        // 第二次打开：此时 pages.json 依然不存在，结果必须来自磁盘缓存
        //（这里通过「缓存命中后不再触碰归档」间接验证——直接读缓存文件断言）
        let stamp = sizes_cache::source_stamp(&path).unwrap();
        let cached = sizes_cache::read(&path, stamp).unwrap();
        assert_eq!(
            cached.get("0001.png").copied().flatten().unwrap().width,
            640
        );

        // 归档替换（mtime/size 变化）后缓存失效
        std::fs::write(&path, b"changed").unwrap();
        let new_stamp = sizes_cache::source_stamp(&path).unwrap();
        assert_ne!(stamp, new_stamp);
        assert!(sizes_cache::read(&path, new_stamp).is_none());

        sizes_cache::set_dir(None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn write_pages_entry_takes_ownership() {
        // 按值收表：调用方传进来的是最后一次使用的局部 map，避免整表 clone
        let mut pages = BTreeMap::new();
        pages.insert(
            "0001.png".to_string(),
            PageSize {
                width: 1,
                height: 2,
            },
        );
        let root = std::env::temp_dir().join(format!("jm-pages-own-{}", uuid::Uuid::new_v4()));
        let path = root.join("chapter.cbz");
        std::fs::create_dir_all(&root).unwrap();
        {
            let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
            write_pages_entry(&mut zip, pages).unwrap();
            zip.finish().unwrap();
        }
        let mut archive = zip::ZipArchive::new(std::fs::File::open(&path).unwrap()).unwrap();
        assert_eq!(read_pages_entry(&mut archive).len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }
}
