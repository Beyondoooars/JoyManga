//! 书库封面缩略图磁盘缓存。
//!
//! 扫描书库时每本书都要从 CBZ 里解出首图再缩放；书多了以后这是首屏和滚动的主要开销。
//! 这里把生成好的缩略图按「来源路径 + 修改时间 + 大小」缓存到配置目录下的 `.covers/`，
//! 内容不变时直接读缓存，避免重复解压与解码。
//!
//! 键里带 mtime/size 而不是 comic id：漫画重新下载或被替换后旧缓存自动失效，
//! 不需要额外的失效逻辑；过期文件由 [`prune`] 按数量上限清理。

use std::path::{Path, PathBuf};

use crate::disk_cache::{source_stamp, DiskCache};

/// 缓存目录上限（个）。缩略图约 10~20 KB，500 张不到 10 MB。
const MAX_ENTRIES: usize = 500;
/// 每写这么多次才扫一遍目录做清理（同 page_size 的 sizes_cache），避免首次扫描书库时
/// 每写一张缩略图就全目录扫描一次，退化为 O(N²)。
const PRUNE_EVERY: u32 = 64;

static CACHE: DiskCache = DiskCache::new(MAX_ENTRIES, PRUNE_EVERY, "jpg", true);

/// 设置缓存目录。传 `None` 或空路径可关闭缓存（测试与无配置场景）。
pub fn set_dir(dir: Option<PathBuf>) {
    CACHE.set_dir(dir);
}

pub fn dir() -> Option<PathBuf> {
    CACHE.dir()
}

/// 从封面磁盘缓存读取已生成的缩略图；未命中或未启用缓存时返回 None。
pub fn read_thumbnail(source: &Path) -> Option<Vec<u8>> {
    let stamp = source_stamp(source)?;
    let (cache_path, bytes) = CACHE.read_raw(source, stamp, &[])?;
    if bytes.is_empty() {
        return None;
    }
    // 命中后轻触 mtime，让 prune 的"按最近使用"语义成立
    DiskCache::touch(&cache_path);
    Some(bytes)
}

/// 将生成的缩略图写入封面磁盘缓存，并按周期清理过期旧条目。
pub fn write_thumbnail(source: &Path, bytes: &[u8]) {
    let Some(stamp) = source_stamp(source) else {
        return;
    };
    CACHE.write_raw(source, stamp, &[], bytes);
}

/// 命中缓存则直接返回；否则调用 `render` 生成并写盘。
/// `render` 失败时不写缓存，下次扫描会重试。
pub fn cached_thumbnail(
    source: &Path,
    render: impl FnOnce() -> Option<Vec<u8>>,
) -> Option<Vec<u8>> {
    if let Some(bytes) = read_thumbnail(source) {
        return Some(bytes);
    }
    let bytes = render()?;
    write_thumbnail(source, &bytes);
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::Mutex;

    /// 缓存目录是进程级全局状态，测试并行跑会互相覆盖（A 的 set_dir(None)
    /// 会让 B 的第二次调用重新渲染）。用一把锁把涉及全局状态的用例串行化。
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("jm-cover-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn renders_once_then_serves_from_cache() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|err| err.into_inner());
        let root = temp_dir("hit");
        let source = root.join("book.cbz");
        std::fs::write(&source, b"not-a-real-cbz").unwrap();
        let cache = root.join("cache");
        set_dir(Some(cache.clone()));

        let first = cached_thumbnail(&source, || Some(b"thumb".to_vec()));
        assert_eq!(first.as_deref(), Some(&b"thumb"[..]));

        // 第二次不应再调用 render
        let second = cached_thumbnail(&source, || panic!("应命中缓存"));
        assert_eq!(second.as_deref(), Some(&b"thumb"[..]));

        set_dir(None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn changed_source_invalidates_cache() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|err| err.into_inner());
        let root = temp_dir("stale");
        let source = root.join("book.cbz");
        std::fs::write(&source, b"v1").unwrap();
        let cache = root.join("cache");
        set_dir(Some(cache));

        assert_eq!(
            cached_thumbnail(&source, || Some(b"old".to_vec())).as_deref(),
            Some(&b"old"[..])
        );

        // 改写内容并推进 mtime（文件系统时间精度可能较粗，显式设置）
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .open(&source)
            .unwrap();
        file.write_all(b"v2").unwrap();
        file.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(2))
            .unwrap();
        drop(file);

        assert_eq!(
            cached_thumbnail(&source, || Some(b"new".to_vec())).as_deref(),
            Some(&b"new"[..])
        );

        set_dir(None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn prune_keeps_newest_entries() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|err| err.into_inner());
        let root = temp_dir("prune");
        for index in 0..5 {
            let path = root.join(format!("{index:016x}.jpg"));
            std::fs::write(&path, b"x").unwrap();
            let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
            file.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(index))
                .unwrap();
        }
        CACHE.prune_to(&root, 3);
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 3);
        let _ = std::fs::remove_dir_all(&root);
    }
}
