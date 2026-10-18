//! 书库封面缩略图磁盘缓存。
//!
//! 扫描书库时每本书都要从 CBZ 里解出首图再缩放；书多了以后这是首屏和滚动的主要开销。
//! 这里把生成好的缩略图按「来源路径 + 修改时间 + 大小」缓存到配置目录下的 `.covers/`，
//! 内容不变时直接读缓存，避免重复解压与解码。
//!
//! 键里带 mtime/size 而不是 comic id：漫画重新下载或被替换后旧缓存自动失效，
//! 不需要额外的失效逻辑；过期文件由 [`prune`] 按数量上限清理。

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use parking_lot::RwLock;

static CACHE_DIR: OnceLock<RwLock<Option<PathBuf>>> = OnceLock::new();

/// 缓存目录上限（个）。缩略图约 10~20 KB，500 张不到 10 MB。
const MAX_ENTRIES: usize = 500;

fn slot() -> &'static RwLock<Option<PathBuf>> {
    CACHE_DIR.get_or_init(|| RwLock::new(None))
}

/// 设置缓存目录。传 `None` 或空路径可关闭缓存（测试与无配置场景）。
pub fn set_dir(dir: Option<PathBuf>) {
    *slot().write() = dir.filter(|path| !path.as_os_str().is_empty());
}

pub fn dir() -> Option<PathBuf> {
    slot().read().clone()
}

/// 来源文件的指纹：路径 + 修改时间 + 长度。取不到元数据时返回 None（不缓存）。
fn source_stamp(path: &Path) -> Option<(u64, u64)> {
    let metadata = std::fs::metadata(path).ok()?;
    let mtime = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |duration| duration.as_nanos() as u64);
    Some((mtime, metadata.len()))
}

/// FNV-1a：只用于生成缓存文件名，不承担安全职责。
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

/// 命中缓存则直接返回；否则调用 `render` 生成并写盘。
/// `render` 失败时不写缓存，下次扫描会重试。
pub fn cached_thumbnail(
    source: &Path,
    render: impl FnOnce() -> Option<Vec<u8>>,
) -> Option<Vec<u8>> {
    let Some(cache_dir) = dir() else {
        return render();
    };
    let Some(stamp) = source_stamp(source) else {
        return render();
    };
    let name = format!("{:016x}.jpg", fingerprint(source, stamp));
    let cache_path = cache_dir.join(&name);

    if let Ok(bytes) = std::fs::read(&cache_path) {
        if !bytes.is_empty() {
            // 命中后轻触 mtime，让 prune 的"按最近使用"语义成立
            touch(&cache_path);
            return Some(bytes);
        }
    }

    let bytes = render()?;
    if std::fs::create_dir_all(&cache_dir).is_ok() {
        // 缩略图可随时重建，写失败不影响阅读，忽略错误即可
        let _ = crate::atomic_fs::write(&cache_path, &bytes);
        prune(&cache_dir, MAX_ENTRIES);
    }
    Some(bytes)
}

fn touch(path: &Path) {
    // 用改写文件时间的方式标记"最近使用"，避免额外维护一份访问记录
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
                .is_some_and(|ext| ext.eq_ignore_ascii_case("jpg"))
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
        prune(&root, 3);
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 3);
        let _ = std::fs::remove_dir_all(&root);
    }
}
