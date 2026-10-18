use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{Context, Result};
use parking_lot::{Mutex, MutexGuard};

static REPLACE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn replace_guard() -> MutexGuard<'static, ()> {
    REPLACE_LOCK.get_or_init(|| Mutex::new(())).lock()
}

fn sibling_path(path: &Path, marker: &str) -> Result<PathBuf> {
    let parent = path
        .parent()
        .context(format!("`{}`没有父目录", path.display()))?;
    let file_name = path
        .file_name()
        .context(format!("`{}`没有文件名", path.display()))?
        .to_string_lossy();
    // 限制基础文件名长度，防止拼上前缀和UUID后超出系统文件名长度限制(255字节)
    let max_base_bytes = 120;
    let safe_stem = crate::utils::truncate_utf8_by_bytes(&file_name, max_base_bytes);
    let safe_stem = if safe_stem.is_empty() {
        "tmp"
    } else {
        safe_stem
    };
    Ok(parent.join(format!(".{safe_stem}.{marker}-{}", uuid::Uuid::new_v4())))
}

pub fn temp_path(path: &Path) -> Result<PathBuf> {
    sibling_path(path, "part")
}

pub fn write(path: &Path, contents: impl AsRef<[u8]>) -> Result<()> {
    write_checked(path, contents, |_| Ok(()))
}

pub fn write_checked<F>(path: &Path, contents: impl AsRef<[u8]>, validate_existing: F) -> Result<()>
where
    F: FnOnce(&Path) -> Result<()>,
{
    let parent = path
        .parent()
        .context(format!("`{}`没有父目录", path.display()))?;
    std::fs::create_dir_all(parent).context(format!("创建目录`{}`失败", parent.display()))?;

    let temp_path = temp_path(path)?;
    let result = (|| -> Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
            .context(format!("创建临时文件`{}`失败", temp_path.display()))?;
        file.write_all(contents.as_ref())
            .context(format!("写入临时文件`{}`失败", temp_path.display()))?;
        file.sync_all()
            .context(format!("同步临时文件`{}`失败", temp_path.display()))?;
        drop(file);
        commit_file_checked(&temp_path, path, validate_existing)
    })();

    if result.is_err() {
        let _ = std::fs::remove_file(&temp_path);
    }
    result
}

pub fn copy_file(source: &Path, target: &Path) -> Result<()> {
    anyhow::ensure!(source.is_file(), "源文件不存在或不是文件");
    if target.exists() {
        anyhow::ensure!(
            std::fs::canonicalize(source)? != std::fs::canonicalize(target)?,
            "源文件与目标文件相同"
        );
    }
    let parent = target.parent().context("目标没有父目录")?;
    std::fs::create_dir_all(parent)?;
    let staged = temp_path(target)?;
    let result = (|| -> Result<()> {
        std::fs::copy(source, &staged).context("复制到临时文件失败")?;
        std::fs::OpenOptions::new()
            .write(true)
            .open(&staged)?
            .sync_all()?;
        commit_file(&staged, target)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&staged);
    }
    result
}

pub fn commit_file(temp_path: &Path, target_path: &Path) -> Result<()> {
    commit_file_checked(temp_path, target_path, |_| Ok(()))
}

fn is_cross_device_error(err: &std::io::Error) -> bool {
    // EXDEV = 18 on Linux/Android
    err.raw_os_error() == Some(18)
}

fn rename_or_copy_fallback(from: &Path, to: &Path) -> Result<()> {
    match std::fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(err) if is_cross_device_error(&err) => {
            // 跨卷也先在目标卷准备完整文件，禁止直接复制到正式目标。
            let staged = temp_path(to)?;
            let result = (|| -> Result<()> {
                std::fs::copy(from, &staged).context("准备跨卷文件失败")?;
                std::fs::OpenOptions::new()
                    .write(true)
                    .open(&staged)?
                    .sync_all()?;
                std::fs::rename(&staged, to).context("提交跨卷文件失败")?;
                let _ = std::fs::remove_file(from);
                Ok(())
            })();
            if result.is_err() {
                let _ = std::fs::remove_file(&staged);
            }
            result
        }
        Err(err) => Err(err).context(format!(
            "将临时文件`{}`替换为`{}`失败",
            from.display(),
            to.display()
        )),
    }
}

pub fn commit_file_checked<F>(
    temp_path: &Path,
    target_path: &Path,
    validate_existing: F,
) -> Result<()>
where
    F: FnOnce(&Path) -> Result<()>,
{
    let _guard = replace_guard();
    if !target_path.exists() {
        return rename_or_copy_fallback(temp_path, target_path);
    }

    validate_existing(target_path)
        .context(format!("拒绝替换已有文件`{}`", target_path.display()))?;

    // 同目录直接替换文件，避免先移走正式文件留下缺失窗口。
    // Windows 的 std::fs::rename 同样支持替换已有普通文件。
    rename_or_copy_fallback(temp_path, target_path)
}

/// 恢复旧版本在两次 rename 之间退出留下的文件备份。
/// 有多个候选时保留全部文件并报错，避免猜测后覆盖用户配置。
pub fn recover_file_backup(path: &Path) -> Result<()> {
    let _guard = replace_guard();
    if path.exists() {
        return Ok(());
    }
    let parent = path.parent().context("恢复文件没有父目录")?;
    if !parent.exists() {
        return Ok(());
    }
    let name = path
        .file_name()
        .context("恢复文件没有文件名")?
        .to_string_lossy();
    let prefix = format!(".{name}.backup-");
    let mut backups = Vec::new();
    for entry in std::fs::read_dir(parent)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if entry.file_type()?.is_file()
            && name
                .strip_prefix(&prefix)
                .is_some_and(|suffix| uuid::Uuid::parse_str(suffix).is_ok())
        {
            backups.push(entry.path());
        }
    }
    anyhow::ensure!(
        backups.len() <= 1,
        "`{}`存在多个恢复备份，请先确认需要保留的版本",
        path.display()
    );
    if let Some(backup) = backups.first() {
        std::fs::rename(backup, path).context("恢复旧版文件备份失败")?;
        tracing::warn!(path = %path.display(), "已恢复旧版本遗留的文件备份");
    }
    Ok(())
}

#[allow(dead_code)]
pub fn commit_dir_checked<F>(
    temp_path: &Path,
    target_path: &Path,
    validate_existing: F,
) -> Result<()>
where
    F: FnOnce(&Path) -> Result<()>,
{
    let _guard = replace_guard();
    if !target_path.exists() {
        return std::fs::rename(temp_path, target_path).context(format!(
            "将临时目录`{}`替换为`{}`失败",
            temp_path.display(),
            target_path.display()
        ));
    }

    validate_existing(target_path)
        .context(format!("拒绝替换已有目录`{}`", target_path.display()))?;

    let backup_path = sibling_path(target_path, "backup")?;
    std::fs::rename(target_path, &backup_path)
        .context(format!("备份旧目录`{}`失败", target_path.display()))?;

    if let Err(err) = std::fs::rename(temp_path, target_path) {
        let restore_result = std::fs::rename(&backup_path, target_path);
        return match restore_result {
            Ok(()) => Err(err).context(format!(
                "将临时目录`{}`替换为`{}`失败，旧目录已恢复",
                temp_path.display(),
                target_path.display()
            )),
            Err(restore_err) => Err(anyhow::anyhow!(
                "替换目录`{}`失败：{err}；恢复备份`{}`也失败：{restore_err}",
                target_path.display(),
                backup_path.display()
            )),
        };
    }

    if let Err(err) = std::fs::remove_dir_all(&backup_path) {
        tracing::warn!(
            path = %backup_path.display(),
            error = %err,
            "新目录已启用，但清理旧目录备份失败"
        );
    }
    Ok(())
}

/// 将文件或目录原子重命名移出到隐藏的 .trash-UUID 临时目录中，
/// 在文件系统同卷下 rename 属于毫秒级原子操作，实现瞬间与资料库解绑。
#[cfg(test)]
pub fn evict_to_trash(path: &Path) -> Result<PathBuf> {
    let _guard = replace_guard();
    if !path.exists() {
        return Ok(path.to_path_buf());
    }
    let trash_path = sibling_path(path, "trash")?;
    std::fs::rename(path, &trash_path).context(format!(
        "将`{}`原子移出到临时回收区`{}`失败",
        path.display(),
        trash_path.display()
    ))?;
    Ok(trash_path)
}

/// 扫描并清理指定目录下遗留的隐藏回收临时目录（命名形如 `.*.trash-{UUID}`）。
/// 用于应用启动时清理先前崩溃或未正常结束的后台异步删除任务遗留物。
pub fn cleanup_residual_trash(dir: &Path) -> Result<usize> {
    let _guard = replace_guard();
    if !dir.exists() || !dir.is_dir() {
        return Ok(0);
    }
    // 书库删除事务使用同一个隔离命名（`.<name>.trash-<uuid>`）。存在未完成的事务日志时，
    // 这些隔离文件可能还要回滚，必须交给启动恢复处理，不能在扫尾时删除。
    if dir.read_dir()?.flatten().any(|entry| {
        entry
            .file_name()
            .to_string_lossy()
            .starts_with(".library-delete-")
    }) {
        tracing::debug!("存在未完成的删除事务，跳过本次隔离文件清理");
        return Ok(0);
    }
    let mut cleaned_count = 0;
    for entry in std::fs::read_dir(dir)? {
        let entry = match entry {
            Ok(e) => e,
            Err(err) => {
                tracing::warn!(error = %err, "读取目录项失败");
                continue;
            }
        };
        let file_name = entry.file_name();
        let file_name_str = file_name.to_string_lossy();
        if file_name_str.starts_with('.') && file_name_str.contains(".trash-") {
            if let Some(uuid_str) = file_name_str.rsplit(".trash-").next() {
                if uuid::Uuid::parse_str(uuid_str).is_ok() {
                    let path = entry.path();
                    let remove_res = if path.is_dir() {
                        std::fs::remove_dir_all(&path)
                    } else {
                        std::fs::remove_file(&path)
                    };
                    match remove_res {
                        Ok(()) => {
                            tracing::info!(path = %path.display(), "已清理残留的隔离临时目录/文件");
                            cleaned_count += 1;
                        }
                        Err(err) => {
                            tracing::warn!(path = %path.display(), error = %err, "清理残留隔离文件失败");
                        }
                    }
                }
            }
        }
    }
    Ok(cleaned_count)
}

#[cfg(test)]
mod tests {
    use super::{
        cleanup_residual_trash, commit_dir_checked, commit_file, evict_to_trash,
        recover_file_backup, sibling_path, write, write_checked,
    };
    use anyhow::anyhow;
    use std::path::{Path, PathBuf};

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            let path = std::env::temp_dir()
                .join(format!("joymanga-atomic-fs-test-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&path).unwrap();
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
    fn rejected_file_replacement_preserves_existing_contents() {
        let sandbox = TestDir::new();
        let target = sandbox.path().join("metadata.json");
        write(&target, "old").unwrap();

        let result = write_checked(&target, "new", |_| Err(anyhow!("wrong identity")));

        assert!(result.is_err());
        assert_eq!(std::fs::read_to_string(target).unwrap(), "old");
    }

    #[test]
    fn replacement_failure_keeps_original_at_official_path() {
        let sandbox = TestDir::new();
        let target = sandbox.path().join("config.json");
        write(&target, "old").unwrap();
        assert!(commit_file(&sandbox.path().join("missing.part"), &target).is_err());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "old");
        write(&target, "new").unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "new");
        assert_eq!(std::fs::read_dir(sandbox.path()).unwrap().count(), 1);
    }

    #[test]
    fn restores_single_backup_without_promoting_partial_file() {
        let sandbox = TestDir::new();
        let target = sandbox.path().join("config.json");
        let backup = sibling_path(&target, "backup").unwrap();
        std::fs::write(&backup, "old").unwrap();
        std::fs::write(sibling_path(&target, "part").unwrap(), "partial").unwrap();
        recover_file_backup(&target).unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "old");
        assert!(!backup.exists());
        // 已有正式文件时，即使存在旧备份也不能覆盖。
        std::fs::write(&backup, "stale").unwrap();
        recover_file_backup(&target).unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "old");
        assert!(backup.exists());
    }

    #[test]
    fn ambiguous_backups_are_preserved_instead_of_resetting_config() {
        let sandbox = TestDir::new();
        let target = sandbox.path().join("config.json");
        for contents in ["one", "two"] {
            std::fs::write(sibling_path(&target, "backup").unwrap(), contents).unwrap();
        }
        assert!(recover_file_backup(&target).is_err());
        assert!(!target.exists());
        assert_eq!(std::fs::read_dir(sandbox.path()).unwrap().count(), 2);
    }

    #[test]
    fn rejected_directory_replacement_preserves_both_directories() {
        let sandbox = TestDir::new();
        let target = sandbox.path().join("chapter");
        let staged = sandbox.path().join("chapter.part");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::create_dir_all(&staged).unwrap();
        std::fs::write(target.join("old.txt"), "old").unwrap();
        std::fs::write(staged.join("new.txt"), "new").unwrap();

        let result = commit_dir_checked(&staged, &target, |_| Err(anyhow!("wrong chapter")));

        assert!(result.is_err());
        assert!(target.join("old.txt").exists());
        assert!(staged.join("new.txt").exists());
    }

    #[test]
    fn evict_to_trash_moves_path_out_atomically() {
        let sandbox = TestDir::new();
        let target = sandbox.path().join("sample_comic");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("01.jpg"), b"test").unwrap();
        assert!(target.exists());

        let trash = evict_to_trash(&target).expect("evict to trash should succeed");
        assert!(!target.exists());
        assert!(trash.exists());
        assert!(trash.join("01.jpg").exists());

        std::fs::remove_dir_all(&trash).unwrap();
    }

    #[test]
    fn cleanup_residual_trash_removes_trash_folders_and_keeps_normal_files() {
        let sandbox = TestDir::new();
        let normal_comic = sandbox.path().join("normal_comic");
        std::fs::create_dir_all(&normal_comic).unwrap();
        std::fs::write(normal_comic.join("01.jpg"), b"image").unwrap();

        // 模拟先前崩溃留下的 .trash 目录和文件
        let trash_id = uuid::Uuid::new_v4();
        let trash_dir = sandbox.path().join(format!(".comic.trash-{trash_id}"));
        std::fs::create_dir_all(&trash_dir).unwrap();
        std::fs::write(trash_dir.join("01.jpg"), b"trash").unwrap();

        let non_trash_hidden = sandbox.path().join(".other_hidden");
        std::fs::create_dir_all(&non_trash_hidden).unwrap();

        assert!(trash_dir.exists());
        assert!(normal_comic.exists());
        assert!(non_trash_hidden.exists());

        let cleaned = cleanup_residual_trash(sandbox.path()).expect("cleanup should succeed");
        assert_eq!(cleaned, 1);
        assert!(!trash_dir.exists());
        assert!(normal_comic.exists());
        assert!(non_trash_hidden.exists());
    }
}

#[cfg(test)]
mod export_copy_tests {
    use super::*;
    #[test]
    fn missing_source_and_same_file_do_not_destroy_an_existing_export() {
        let root = std::env::temp_dir().join(format!("jm-copy-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let target = root.join("export.cbz");
        std::fs::write(&target, b"old").unwrap();
        assert!(copy_file(&root.join("missing"), &target).is_err());
        assert!(copy_file(&target, &target).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"old");
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
mod successful_copy_tests {
    use super::*;
    #[test]
    fn copying_an_existing_archive_commits_new_bytes_and_preserves_source() {
        let root = std::env::temp_dir().join(format!("jm-copy-success-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("source.cbz");
        let target = root.join("output.cbz");
        std::fs::write(&source, b"new archive bytes").unwrap();
        std::fs::write(&target, b"old archive bytes").unwrap();
        copy_file(&source, &target).unwrap();
        assert_eq!(std::fs::read(&source).unwrap(), b"new archive bytes");
        assert_eq!(std::fs::read(&target).unwrap(), b"new archive bytes");
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 2);
        std::fs::remove_dir_all(root).unwrap();
    }
}
