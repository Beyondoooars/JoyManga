use std::{
    collections::HashMap,
    path::{Component, Path, PathBuf},
};

use aes::cipher::generic_array::GenericArray;
use aes::cipher::{BlockDecrypt, KeyInit};
use aes::Aes256;
use anyhow::{anyhow, ensure, Context};
use base64::engine::general_purpose;
use base64::Engine;
use parking_lot::RwLock;
use tauri::{AppHandle, Manager};
use walkdir::WalkDir;

use crate::{
    extensions::{AppHandleExt, WalkDirEntryExt},
    types::Comic,
};

pub fn filename_filter(s: &str) -> String {
    const MAX_UTF8_BYTES: usize = 240;
    const MAX_UTF16_UNITS: usize = 180;

    let normalized = s
        .chars()
        .map(|c| match c {
            '\\' | '/' | '\n' => ' ',
            ':' => '：',
            '*' => '⭐',
            '?' => '？',
            '"' => '\'',
            '<' => '《',
            '>' => '》',
            '|' => '丨',
            c if c.is_control() => ' ',
            _ => c,
        })
        .collect::<String>()
        .trim()
        .trim_end_matches('.')
        .trim()
        .to_owned();

    let mut filtered = String::new();
    let mut utf8_bytes = 0;
    let mut utf16_units = 0;
    for c in normalized.chars() {
        let next_utf8_bytes = utf8_bytes + c.len_utf8();
        let next_utf16_units = utf16_units + c.len_utf16();
        if next_utf8_bytes > MAX_UTF8_BYTES || next_utf16_units > MAX_UTF16_UNITS {
            break;
        }
        filtered.push(c);
        utf8_bytes = next_utf8_bytes;
        utf16_units = next_utf16_units;
    }

    let filtered = filtered.trim().trim_end_matches('.').trim().to_owned();

    if filtered.is_empty() {
        return filtered;
    }

    let basename = filtered
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    let reserved = matches!(
        basename.as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    );

    if reserved {
        format!("_{filtered}")
    } else {
        filtered
    }
}

pub(crate) fn relative_subpath(root: &Path, descendant: &Path) -> anyhow::Result<PathBuf> {
    let relative = descendant.strip_prefix(root).context(format!(
        "路径`{}`不在根目录`{}`内",
        descendant.display(),
        root.display()
    ))?;
    ensure!(
        relative
            .components()
            .all(|component| matches!(component, Component::Normal(_))),
        "相对路径`{}`包含不安全的路径组件",
        relative.display()
    );
    Ok(relative.to_path_buf())
}

/// 删除操作只接受规范化根目录的严格子目录。
pub(crate) fn validate_delete_target(root: &Path, target: &Path) -> anyhow::Result<()> {
    let relative = relative_subpath(root, target)?;
    ensure!(!relative.as_os_str().is_empty(), "不能删除下载根目录本身");
    Ok(())
}

pub(crate) fn validate_chapter_directory_format(format: &str) -> anyhow::Result<()> {
    let parts: Vec<_> = format
        .split('/')
        .filter(|part| !part.trim().is_empty())
        .collect();
    ensure!(parts.len() >= 2, "下载目录格式至少需要两个层级");
    let (chapter, parents) = parts.split_last().context("下载目录格式不能为空")?;
    let chapter_keys = ["{chapter_id", "{chapter_title", "{order"];
    ensure!(
        chapter_keys.iter().any(|key| chapter.contains(key)),
        "下载目录格式最后一级必须包含章节ID、标题或序号"
    );
    ensure!(
        !parents
            .iter()
            .any(|part| chapter_keys.iter().any(|key| part.contains(key))),
        "章节ID、标题和序号只能出现在下载目录格式的最后一级"
    );
    ensure!(
        parents
            .iter()
            .any(|part| part.contains("{comic_id") || part.contains("{comic_title")),
        "章节目录的上级必须包含漫画ID或标题"
    );
    Ok(())
}

/// 在`root`下安全创建`relative`目录，并返回最终目录的规范化路径。
///
/// `relative`只能包含普通路径组件。创建前会检查当前已存在的最近祖先，创建后会再次
/// 规范化目标目录，避免已有符号链接将目标导向`root`之外。
pub(crate) fn create_dir_all_beneath(root: &Path, relative: &Path) -> anyhow::Result<PathBuf> {
    ensure!(
        relative
            .components()
            .all(|component| matches!(component, Component::Normal(_))),
        "相对路径`{}`包含不安全的路径组件",
        relative.display()
    );

    std::fs::create_dir_all(root).context(format!("创建根目录`{}`失败", root.display()))?;
    let canonical_root =
        std::fs::canonicalize(root).context(format!("规范化根目录`{}`失败", root.display()))?;
    let candidate = canonical_root.join(relative);

    let mut existing_ancestor = candidate.as_path();
    loop {
        match std::fs::symlink_metadata(existing_ancestor) {
            Ok(_) => {
                let canonical_ancestor = std::fs::canonicalize(existing_ancestor).context(
                    format!("规范化已存在路径`{}`失败", existing_ancestor.display()),
                )?;
                ensure!(
                    canonical_ancestor.starts_with(&canonical_root),
                    "路径`{}`通过符号链接指向根目录`{}`之外的`{}`",
                    existing_ancestor.display(),
                    canonical_root.display(),
                    canonical_ancestor.display()
                );
                break;
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                existing_ancestor = existing_ancestor.parent().context(format!(
                    "路径`{}`没有可检查的已存在祖先",
                    candidate.display()
                ))?;
            }
            Err(err) => {
                return Err(err).context(format!(
                    "读取路径`{}`的符号链接元数据失败",
                    existing_ancestor.display()
                ));
            }
        }
    }

    std::fs::create_dir_all(&candidate)
        .context(format!("创建目录`{}`失败", candidate.display()))?;
    let canonical_candidate = std::fs::canonicalize(&candidate)
        .context(format!("规范化目录`{}`失败", candidate.display()))?;
    ensure!(
        canonical_candidate.starts_with(&canonical_root),
        "目录`{}`位于根目录`{}`之外",
        canonical_candidate.display(),
        canonical_root.display()
    );

    Ok(canonical_candidate)
}

// 计算MD5哈希并返回十六进制字符串
pub fn md5_hex(data: &str) -> String {
    format!("{:x}", md5::compute(data))
}

/// 使用 AES-256-ECB 解密 base64 数据（自动去除 PKCS#7 填充）
pub fn aes256_ecb_decrypt_base64(key: &[u8], data: &str) -> anyhow::Result<Vec<u8>> {
    let encrypted = general_purpose::STANDARD
        .decode(data)
        .context("base64解码失败")?;
    if encrypted.is_empty() {
        return Err(anyhow!("密文为空"));
    }
    if encrypted.len() % 16 != 0 {
        return Err(anyhow!(
            "密文长度`{}`不是AES块大小的整数倍",
            encrypted.len()
        ));
    }

    let cipher = Aes256::new(GenericArray::from_slice(key));
    let mut decrypted = Vec::with_capacity(encrypted.len());
    for block in encrypted.chunks(16) {
        let mut block = GenericArray::clone_from_slice(block);
        cipher.decrypt_block(&mut block);
        decrypted.extend_from_slice(&block);
    }

    let pad_byte = *decrypted.last().context("解密结果为空")?;
    let pad = pad_byte as usize;
    ensure!((1..=16).contains(&pad), "非法的PKCS#7填充长度`{pad}`");
    ensure!(decrypted.len() >= pad, "PKCS#7填充长度超过解密结果长度");
    ensure!(
        decrypted[decrypted.len() - pad..]
            .iter()
            .all(|&b| b == pad_byte),
        "PKCS#7填充内容不一致"
    );
    decrypted.truncate(decrypted.len() - pad);
    Ok(decrypted)
}

type ComicIndex = (PathBuf, HashMap<i64, Comic>);

#[derive(Default)]
pub struct DownloadedLibraryCache {
    map: RwLock<Option<HashMap<i64, PathBuf>>>,
    comics: RwLock<Option<ComicIndex>>,
}

impl DownloadedLibraryCache {
    pub fn new() -> Self {
        Self {
            map: RwLock::new(None),
            comics: RwLock::new(None),
        }
    }

    pub fn get(&self) -> Option<HashMap<i64, PathBuf>> {
        self.map.read().clone()
    }

    pub fn set(&self, map: HashMap<i64, PathBuf>) {
        *self.map.write() = Some(map);
    }

    pub fn insert(&self, id: i64, path: PathBuf) {
        *self.comics.write() = None;
        let mut guard = self.map.write();
        if let Some(map) = guard.as_mut() {
            map.insert(id, path);
        }
    }

    pub fn remove(&self, id: i64) {
        *self.comics.write() = None;
        let mut guard = self.map.write();
        if let Some(map) = guard.as_mut() {
            map.remove(&id);
        }
    }

    pub fn invalidate(&self) {
        *self.map.write() = None;
        *self.comics.write() = None;
    }

    pub fn set_comics(&self, root: &Path, comics: &[Comic]) {
        *self.comics.write() = Some((
            root.to_path_buf(),
            comics
                .iter()
                .cloned()
                .map(|mut comic| {
                    comic.local_cover_data = None;
                    (comic.id, comic)
                })
                .collect(),
        ));
    }

    pub fn comic(&self, root: &Path, id: i64) -> Option<Comic> {
        let index = self.comics.read();
        let (stored_root, comics) = index.as_ref()?;
        if stored_root != root {
            return None;
        }
        comics.get(&id).cloned()
    }
}

/// 智能遍历下载目录中的漫画元数据文件。
/// 通过智能剪枝跳过包含章节元数据（即纯图片子目录）的深层递归，避免访问数万张图片文件。
pub fn walk_comic_metadata_files(download_dir: &Path) -> impl Iterator<Item = walkdir::DirEntry> {
    WalkDir::new(download_dir)
        .into_iter()
        .filter_entry(|entry| {
            if entry.file_name().to_string_lossy().starts_with(".import-") {
                return false;
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

pub fn create_id_to_dir_map(app: &AppHandle) -> anyhow::Result<HashMap<i64, PathBuf>> {
    let download_dir = app.get_config().read().download_dir.clone();
    let catalog = {
        let _guard = crate::library_catalog::CATALOG_LOCK.lock();
        crate::library_catalog::LibraryCatalog::load(&crate::library_catalog::catalog_path(app))?
    };
    let removed = catalog.removed_ids(&download_dir);
    if let Some(cache) = app.try_state::<DownloadedLibraryCache>() {
        if let Some(mut cached_map) = cache.get() {
            cached_map.retain(|id, _| !removed.contains(id));
            return Ok(cached_map);
        }
    }

    let mut id_to_dir_map: HashMap<i64, PathBuf> = HashMap::new();
    if !download_dir.exists() {
        if let Some(cache) = app.try_state::<DownloadedLibraryCache>() {
            cache.set(id_to_dir_map.clone());
        }
        return Ok(id_to_dir_map);
    }

    // 1. 扫描下载根目录下的平铺 .cbz 文件
    if let Ok(entries) = std::fs::read_dir(&download_dir) {
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.is_file()
                && path
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(str::to_lowercase)
                    == Some("cbz".to_string())
            {
                if let Ok(id) = read_comic_id_from_cbz(&path) {
                    id_to_dir_map.entry(id).or_insert(path);
                }
            }
        }
    }

    // 2. 扫描历史子目录中的 元数据.json
    for entry in walk_comic_metadata_files(&download_dir) {
        let path = entry.path();
        let id = match read_comic_id(path) {
            Ok(id) => id,
            Err(err) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %err,
                    "跳过损坏的漫画元数据"
                );
                continue;
            }
        };

        let parent = path
            .parent()
            .context(format!("`{}`没有父目录", path.display()))?;

        id_to_dir_map.entry(id).or_insert(parent.to_path_buf());
    }

    for record in catalog.visible_imports(&download_dir) {
        if record.path.is_file() {
            id_to_dir_map.insert(record.id, record.path);
        }
    }
    id_to_dir_map.retain(|id, _| !removed.contains(id));
    if let Some(cache) = app.try_state::<DownloadedLibraryCache>() {
        cache.set(id_to_dir_map.clone());
    }

    Ok(id_to_dir_map)
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

fn read_comic_id(path: &std::path::Path) -> anyhow::Result<i64> {
    let metadata_str =
        std::fs::read_to_string(path).context(format!("读取`{}`失败", path.display()))?;
    let comic_json: serde_json::Value = serde_json::from_str(&metadata_str).context(format!(
        "将`{}`反序列化为serde_json::Value失败",
        path.display()
    ))?;
    comic_json
        .get("id")
        .and_then(serde_json::Value::as_i64)
        .context(format!("`{}`没有有效的`id`字段", path.display()))
}

pub async fn get_comic(app: AppHandle, aid: i64) -> anyhow::Result<Comic> {
    let jm_client = app.get_jm_client();

    let comic_resp_data = jm_client.get_comic(aid).await?;

    let comic = Comic::from_comic_resp_data(&app, comic_resp_data)?;

    Ok(comic)
}

pub fn truncate_utf8_by_bytes(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut boundary = max_bytes;
    while boundary > 0 && !s.is_char_boundary(boundary) {
        boundary -= 1;
    }
    s[..boundary].trim_end_matches([' ', '.', '-', '_'])
}

pub fn storage_cbz_filename(comic_name: &str, comic_id: i64, chapter_id: i64) -> String {
    let suffix = format!(" [JM{comic_id}-{chapter_id}].cbz");
    let title = filename_filter(comic_name);
    let title = truncate_utf8_by_bytes(&title, 180_usize.saturating_sub(suffix.len()));
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

    let clean_comic_name = filename_filter(comic_name);
    let clean_comic_name = if clean_comic_name.is_empty() {
        format!("comic-{comic_id}")
    } else {
        clean_comic_name
    };

    if chapter_count <= 1 {
        let max_comic_bytes = MAX_TOTAL_BYTES.saturating_sub(EXTENSION.len());
        let truncated = truncate_utf8_by_bytes(&clean_comic_name, max_comic_bytes);
        let final_comic = if truncated.is_empty() {
            format!("comic-{comic_id}")
        } else {
            truncated.to_string()
        };
        return format!("{final_comic}{EXTENSION}");
    }

    let clean_chapter_name = filename_filter(chapter_title);
    let clean_chapter_name = if clean_chapter_name.is_empty() {
        format!("chapter-{chapter_id}")
    } else {
        clean_chapter_name
    };

    let middle = format!(" - {:04} - ", chapter_order);
    let fixed_len = middle.len() + EXTENSION.len();
    let available_for_names = MAX_TOTAL_BYTES.saturating_sub(fixed_len);

    let max_chapter_bytes = 50.min(available_for_names.saturating_sub(30));
    let truncated_chapter = truncate_utf8_by_bytes(&clean_chapter_name, max_chapter_bytes);
    let chapter_part = if truncated_chapter.is_empty() {
        format!("chapter-{chapter_id}")
    } else {
        truncated_chapter.to_string()
    };

    let remaining_for_comic = available_for_names.saturating_sub(chapter_part.len());
    let truncated_comic = truncate_utf8_by_bytes(&clean_comic_name, remaining_for_comic);
    let comic_part = if truncated_comic.is_empty() {
        format!("comic-{comic_id}")
    } else {
        truncated_comic.to_string()
    };

    format!("{comic_part}{middle}{chapter_part}{EXTENSION}")
}

pub fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let mut a_chars = a.chars().peekable();
    let mut b_chars = b.chars().peekable();

    while let (Some(&ca), Some(&cb)) = (a_chars.peek(), b_chars.peek()) {
        if ca.is_ascii_digit() && cb.is_ascii_digit() {
            let mut num_a: u64 = 0;
            while let Some(&d) = a_chars.peek() {
                if d.is_ascii_digit() {
                    num_a = num_a
                        .saturating_mul(10)
                        .saturating_add(d.to_digit(10).unwrap() as u64);
                    a_chars.next();
                } else {
                    break;
                }
            }

            let mut num_b: u64 = 0;
            while let Some(&d) = b_chars.peek() {
                if d.is_ascii_digit() {
                    num_b = num_b
                        .saturating_mul(10)
                        .saturating_add(d.to_digit(10).unwrap() as u64);
                    b_chars.next();
                } else {
                    break;
                }
            }

            match num_a.cmp(&num_b) {
                std::cmp::Ordering::Equal => continue,
                other => return other,
            }
        } else {
            let lower_a = ca.to_lowercase().collect::<Vec<_>>();
            let lower_b = cb.to_lowercase().collect::<Vec<_>>();
            match lower_a.cmp(&lower_b) {
                std::cmp::Ordering::Equal => {
                    a_chars.next();
                    b_chars.next();
                }
                other => return other,
            }
        }
    }

    a.len().cmp(&b.len())
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{create_dir_all_beneath, filename_filter, relative_subpath};

    #[test]
    fn natural_cmp_sorts_numbers_naturally() {
        let mut list = vec![
            "0010.jpg",
            "0002.jpg",
            "0001.jpg",
            "page2.jpg",
            "page10.jpg",
            "page1.jpg",
        ];
        list.sort_by(|a, b| super::natural_cmp(a, b));
        assert_eq!(
            list,
            vec![
                "0001.jpg",
                "0002.jpg",
                "0010.jpg",
                "page1.jpg",
                "page2.jpg",
                "page10.jpg"
            ]
        );
    }

    #[test]
    fn deletion_requires_a_strict_descendant_of_the_canonical_root() {
        let sandbox = TestDir::new();
        let root = std::fs::canonicalize(sandbox.path()).unwrap();
        assert!(super::validate_delete_target(&root, &root).is_err());
        assert!(super::validate_delete_target(&root, &root.join("comic")).is_ok());
        assert!(super::validate_delete_target(&root, root.parent().unwrap()).is_err());
        assert!(super::validate_delete_target(&root, &root.join("..").join("outside")).is_err());
        let child = root.join("comic");
        std::fs::create_dir_all(&child).unwrap();
        let disguised_root = std::fs::canonicalize(child.join("..")).unwrap();
        assert!(super::validate_delete_target(&root, &disguised_root).is_err());
    }

    #[test]
    fn directory_format_keeps_chapter_variables_in_last_level() {
        for format in [
            "{comic_title}/{chapter_id}/images",
            "{chapter_id}/{comic_title}",
            "{comic_title}/{order}/{chapter_id}",
            "fixed/{comic_id}-{chapter_id}",
        ] {
            assert!(
                super::validate_chapter_directory_format(format).is_err(),
                "{format}"
            );
        }
        for format in [
            "{comic_title}/{chapter_title}",
            "{author}/{comic_id}/{order}-{chapter_id}",
        ] {
            assert!(
                super::validate_chapter_directory_format(format).is_ok(),
                "{format}"
            );
        }
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

        let entries: Vec<_> = super::walk_comic_metadata_files(sandbox.path()).collect();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].file_name(), "元数据.json");
    }

    #[test]
    fn downloaded_library_cache_operations() {
        let cache = super::DownloadedLibraryCache::new();
        assert!(cache.get().is_none());

        let mut map = std::collections::HashMap::new();
        map.insert(123, PathBuf::from("dir1"));
        cache.set(map);
        assert_eq!(cache.get().unwrap().get(&123), Some(&PathBuf::from("dir1")));

        cache.insert(456, PathBuf::from("dir2"));
        assert_eq!(cache.get().unwrap().get(&456), Some(&PathBuf::from("dir2")));

        cache.remove(123);
        assert_eq!(cache.get().unwrap().get(&123), None);

        cache.invalidate();
        assert!(cache.get().is_none());
    }

    #[test]
    fn chapter_index_is_bound_to_root_and_invalidated_when_library_changes() {
        let cache = super::DownloadedLibraryCache::new();
        let root = Path::new("downloads");
        let comic = super::Comic {
            id: 1,
            local_cover_data: Some("large-cover".into()),
            ..super::Comic::default()
        };
        cache.set_comics(root, std::slice::from_ref(&comic));
        assert!(cache.comic(root, 1).unwrap().local_cover_data.is_none());
        assert!(cache.comic(Path::new("other"), 1).is_none());
        cache.insert(1, root.join("chapter"));
        assert!(cache.comic(root, 1).is_none());
        cache.set_comics(root, &[comic]);
        cache.invalidate();
        assert!(cache.comic(root, 1).is_none());
    }

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("joymanga-utils-test-{}", uuid::Uuid::new_v4()));
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
    fn filename_filter_limits_utf8_bytes_and_utf16_units() {
        let ascii = filename_filter(&"a".repeat(300));
        assert_eq!(ascii.len(), 180);
        assert_eq!(ascii.encode_utf16().count(), 180);

        let cjk = filename_filter(&"界".repeat(300));
        assert_eq!(cjk.len(), 240);
        assert_eq!(cjk.encode_utf16().count(), 80);

        let emoji = filename_filter(&"😀".repeat(300));
        assert_eq!(emoji.len(), 240);
        assert_eq!(emoji.encode_utf16().count(), 120);
        assert!(emoji.is_char_boundary(emoji.len()));
    }

    #[test]
    fn filename_filter_retrims_after_truncation() {
        let input = format!("{}.b", "a".repeat(179));
        let filtered = filename_filter(&input);
        assert_eq!(filtered, "a".repeat(179));
        assert!(!filtered.ends_with('.'));
    }

    #[test]
    fn relative_subpath_rejects_parent_and_absolute_components() {
        let root = Path::new("root");
        assert_eq!(
            relative_subpath(root, &root.join("comic").join("chapter")).unwrap(),
            PathBuf::from("comic").join("chapter")
        );
        assert!(relative_subpath(root, &root.join("comic").join("..").join("other")).is_err());
        assert!(relative_subpath(root, &std::env::temp_dir()).is_err());
    }

    #[test]
    fn create_dir_all_beneath_creates_and_canonicalizes_nested_directory() {
        let sandbox = TestDir::new();
        let root = sandbox.path().join("export");
        let created = create_dir_all_beneath(&root, Path::new("comic/chapter")).unwrap();
        assert!(created.is_dir());
        assert!(created.starts_with(std::fs::canonicalize(root).unwrap()));
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn create_dir_all_beneath_rejects_existing_symlink_escape() {
        let sandbox = TestDir::new();
        let root = sandbox.path().join("export");
        let outside = sandbox.path().join("outside");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let link = root.join("comic");

        #[cfg(unix)]
        let symlink_result = std::os::unix::fs::symlink(&outside, &link);
        #[cfg(windows)]
        let symlink_result = std::os::windows::fs::symlink_dir(&outside, &link);

        if let Err(err) = symlink_result {
            if cfg!(windows)
                && (err.kind() == std::io::ErrorKind::PermissionDenied
                    || err.raw_os_error() == Some(1314))
            {
                return;
            }
            panic!("create directory symlink: {err}");
        }

        let err = create_dir_all_beneath(&root, Path::new("comic/chapter")).unwrap_err();
        assert!(err.to_string().contains("符号链接"));
        assert!(!outside.join("chapter").exists());
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

        let id_a = super::read_comic_id_from_cbz(&cbz_a).unwrap();
        let id_b = super::read_comic_id_from_cbz(&cbz_b).unwrap();

        assert!(id_a < 0, "外部CBZ应使用负值标识，避免与在线漫画ID冲突");
        assert!(id_b < 0);
        assert_ne!(id_a, id_b, "相同数字前缀但不同标题的CBZ不能产生相同的ID");
        // 同一路径重复计算必须稳定
        assert_eq!(id_a, super::read_comic_id_from_cbz(&cbz_a).unwrap());
    }

    #[test]
    fn cbz_filename_bounds_byte_length_for_long_titles() {
        let long_comic = "不擅交际的未亡人雪女与诅咒戒指2 [ぷうのぷうぷうぷう (ぷう崎ぷう奈)] 人付き合いが苦手な未亡人の雪女さんと呪いの指輪 2 [中国翻訳]";
        let long_chapter = "第2话 淫书馆汉化(可下载) 附录特典全彩高清完全版详细说明特别篇";
        let filename = super::cbz_filename(long_comic, 12345, long_chapter, 67890, 2, 5);

        assert!(filename.ends_with(".cbz"));
        assert!(
            filename.len() <= 180,
            "CBZ 文件名 UTF-8 字节长度超过安全上限: {}",
            filename.len()
        );
        assert!(filename.contains(" - 0002 - "));
    }
}

#[cfg(test)]
mod storage_identity_tests {
    #[test]
    fn duplicate_titles_and_sanitized_names_keep_distinct_storage_ids() {
        assert_ne!(
            super::storage_cbz_filename("同名", 111, 111),
            super::storage_cbz_filename("同名", 222, 222)
        );
        assert_ne!(
            super::storage_cbz_filename("a/b", 111, 111),
            super::storage_cbz_filename("a\\b", 222, 222)
        );
        let long = super::storage_cbz_filename(&"漫画😀".repeat(300), 111, 111);
        assert!(long.len() <= 180);
        assert!(long.ends_with("[JM111-111].cbz"));
    }
}
