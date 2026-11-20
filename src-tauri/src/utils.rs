use std::path::{Component, Path, PathBuf};

use anyhow::{ensure, Context};

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

/// 在`dir`下写入并删除一个临时文件，确认目录可写；不存在时先创建。
///
/// 日志目录与书库位置校验共用同一语义，避免 FUSE 挂载上"已存在但无写权限"时
/// `create_dir_all` 误报可写。
pub(crate) fn probe_directory_writable(dir: &Path) -> bool {
    if !dir.is_dir() && std::fs::create_dir_all(dir).is_err() {
        return false;
    }
    let test_file = dir.join(format!(".write-probe-{}", uuid::Uuid::new_v4()));
    if std::fs::write(&test_file, b"1").is_err() {
        return false;
    }
    let _ = std::fs::remove_file(&test_file);
    true
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
    fn probe_directory_writable_rejects_regular_file() {
        let sandbox = TestDir::new();
        let file = sandbox.path().join("not-a-dir");
        std::fs::write(&file, b"x").unwrap();
        assert!(!super::probe_directory_writable(&file));
        assert!(super::probe_directory_writable(&sandbox.path().join("sub")));
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
}
