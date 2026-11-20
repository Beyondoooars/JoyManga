//! 通用按来源指纹命名的磁盘缓存原语（供封面缩略图与页面尺寸缓存复用）。
//!
//! 两个调用方必须保持以下差异语义：
//! - 封面缩略图缓存可走 `atomic_fs::write` 原子写入；
//! - 页面尺寸缓存由阅读器在持有替换锁期间写入，必须走普通 `std::fs::write`，
//!   避免同一分片锁重入死锁。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use parking_lot::RwLock;

/// 提取来源文件指纹所需的元数据 `(修改时间纳秒, 字节长度)`；取不到元数据时返回 `None`。
pub(crate) fn source_stamp(path: &Path) -> Option<(u64, u64)> {
    let metadata = std::fs::metadata(path).ok()?;
    let mtime = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |duration| duration.as_nanos() as u64);
    Some((mtime, metadata.len()))
}

pub(crate) struct DiskCache {
    dir: RwLock<Option<PathBuf>>,
    write_count: AtomicU32,
    max_entries: usize,
    prune_every: u32,
    extension: &'static str,
    atomic_write: bool,
}

impl DiskCache {
    pub(crate) const fn new(
        max_entries: usize,
        prune_every: u32,
        extension: &'static str,
        atomic_write: bool,
    ) -> Self {
        Self {
            dir: RwLock::new(None),
            write_count: AtomicU32::new(0),
            max_entries,
            prune_every,
            extension,
            atomic_write,
        }
    }

    /// 设置缓存目录；传 `None` 或空路径可关闭缓存（测试与无配置场景）。
    pub(crate) fn set_dir(&self, dir: Option<PathBuf>) {
        *self.dir.write() = dir.filter(|path| !path.as_os_str().is_empty());
    }

    pub(crate) fn dir(&self) -> Option<PathBuf> {
        self.dir.read().clone()
    }

    /// FNV-1a：只用于生成缓存文件名，不承担安全职责。
    pub(crate) fn fingerprint(path: &Path, stamp: (u64, u64), extra_salt: &[u64]) -> u64 {
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for byte in path.to_string_lossy().as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        for value in [stamp.0, stamp.1]
            .into_iter()
            .chain(extra_salt.iter().copied())
        {
            for byte in value.to_le_bytes() {
                hash ^= u64::from(byte);
                hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
            }
        }
        hash
    }

    fn cache_path(
        &self,
        cache_dir: &Path,
        source: &Path,
        stamp: (u64, u64),
        extra_salt: &[u64],
    ) -> PathBuf {
        cache_dir.join(format!(
            "{:016x}.{}",
            Self::fingerprint(source, stamp, extra_salt),
            self.extension
        ))
    }

    /// 读取原始缓存字节及对应路径；由调用方校验有效性后再调用 [`Self::touch`]。
    pub(crate) fn read_raw(
        &self,
        source: &Path,
        stamp: (u64, u64),
        extra_salt: &[u64],
    ) -> Option<(PathBuf, Vec<u8>)> {
        let cache_dir = self.dir()?;
        let path = self.cache_path(&cache_dir, source, stamp, extra_salt);
        let bytes = std::fs::read(&path).ok()?;
        Some((path, bytes))
    }

    pub(crate) fn write_raw(
        &self,
        source: &Path,
        stamp: (u64, u64),
        extra_salt: &[u64],
        bytes: &[u8],
    ) {
        let Some(cache_dir) = self.dir() else {
            return;
        };
        let cache_path = self.cache_path(&cache_dir, source, stamp, extra_salt);
        if std::fs::create_dir_all(&cache_dir).is_ok() {
            if self.atomic_write {
                let _ = crate::atomic_fs::write(&cache_path, bytes);
            } else {
                // 缓存本身可随时重建，普通写入足够且避免触碰外层持有的替换锁。
                let _ = std::fs::write(&cache_path, bytes);
            }
            if self
                .write_count
                .fetch_add(1, Ordering::Relaxed)
                .is_multiple_of(self.prune_every)
            {
                self.prune_to(&cache_dir, self.max_entries);
            }
        }
    }

    /// 用改写文件时间的方式标记“最近使用”，避免额外维护一份访问记录。
    pub(crate) fn touch(path: &Path) {
        if let Ok(file) = std::fs::OpenOptions::new().write(true).open(path) {
            let _ = file.set_modified(std::time::SystemTime::now());
        }
    }

    /// 只保留最近使用的 `keep` 个缓存文件。
    pub(crate) fn prune_to(&self, cache_dir: &Path, keep: usize) {
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
                    .is_some_and(|ext| ext.eq_ignore_ascii_case(self.extension))
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
    use super::DiskCache;
    use std::path::Path;

    #[test]
    fn fingerprint_varies_with_extra_salt() {
        let path = Path::new("book.cbz");
        let stamp = (100, 200);
        assert_eq!(
            DiskCache::fingerprint(path, stamp, &[]),
            DiskCache::fingerprint(path, stamp, &[])
        );
        assert_ne!(
            DiskCache::fingerprint(path, stamp, &[]),
            DiskCache::fingerprint(path, stamp, &[64_000_000])
        );
        assert_ne!(
            DiskCache::fingerprint(path, stamp, &[16_000_000]),
            DiskCache::fingerprint(path, stamp, &[64_000_000])
        );
    }
}
