use std::path::{Path, PathBuf};

use anyhow::{anyhow, ensure, Context};
use serde::{Deserialize, Serialize};
use specta::Type;

use crate::errors::{CommandError, CommandResult};

/// 内置目录浏览器可浏览的根：Android 公共存储卷。
const BROWSABLE_ROOT: &str = "/storage";

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct DirectoryEntry {
    pub name: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ValidatedDownloadDir {
    pub canonical_path: String,
}

/// 目录浏览的安全边界：必须在 `/storage` 之下（公共卷），
/// 拒绝浏览 `/data`、`/proc` 等系统目录与相对路径。
fn ensure_browsable(candidate: &Path) -> anyhow::Result<PathBuf> {
    ensure!(candidate.is_absolute(), "路径必须是绝对路径（以 / 开头）");
    let canonical = std::fs::canonicalize(candidate)
        .map_err(|err| anyhow!("目录`{}`不存在或无法访问: {err}", candidate.display()))?;
    ensure_under_storage(&canonical)?;
    Ok(canonical)
}

fn ensure_under_storage(canonical: &Path) -> anyhow::Result<()> {
    ensure!(
        canonical.starts_with(BROWSABLE_ROOT),
        "只能浏览公共存储（/storage）下的目录"
    );
    Ok(())
}

/// 校验一个**可能尚不存在**的目标路径：先规范化最近的已存在祖先，
/// 确认它位于 `/storage` 之下，再创建缺失的目录。
///
/// 顺序很重要：先 `create_dir_all` 再 `canonicalize` 会让「新建文件夹」永远失败
/// （不存在的路径无法规范化），而只校验未规范化的字符串又会被符号链接绕过。
fn prepare_browsable_dir(candidate: &Path) -> anyhow::Result<PathBuf> {
    ensure!(candidate.is_absolute(), "路径必须是绝对路径（以 / 开头）");
    let existing_ancestor = candidate
        .ancestors()
        .find(|ancestor| ancestor.exists())
        .context("路径没有可用的上级目录")?;
    let canonical_ancestor = std::fs::canonicalize(existing_ancestor).map_err(|err| {
        anyhow!(
            "目录`{}`不存在或无法访问: {err}",
            existing_ancestor.display()
        )
    })?;
    ensure_under_storage(&canonical_ancestor)?;
    std::fs::create_dir_all(candidate)
        .map_err(|err| anyhow!("无法创建目录`{}`: {err}", candidate.display()))?;
    // 创建后再规范化一次：防止创建过程中祖先被替换为指向 /storage 之外的链接
    let canonical = std::fs::canonicalize(candidate)
        .map_err(|err| anyhow!("规范化目录`{}`失败: {err}", candidate.display()))?;
    ensure_under_storage(&canonical)?;
    Ok(canonical)
}

#[tauri::command(async)]
#[specta::specta]
pub fn list_directory_entries(path: String) -> CommandResult<Vec<DirectoryEntry>> {
    let canonical = ensure_browsable(Path::new(&path))
        .map_err(|err| CommandError::from("读取目录失败", err))?;
    let entries = std::fs::read_dir(&canonical)
        .map_err(|err| CommandError::from("读取目录失败", anyhow!("{err}")))?;
    let mut directories = Vec::new();
    for entry in entries.filter_map(Result::ok) {
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        // 隐藏目录（. 开头）对用户没有浏览价值
        if name.starts_with('.') {
            continue;
        }
        directories.push(DirectoryEntry {
            name,
            path: entry.path().to_string_lossy().to_string(),
        });
    }
    directories.sort_by(|a, b| crate::utils::natural_cmp(&a.name, &b.name));
    Ok(directories)
}

#[tauri::command(async)]
#[specta::specta]
pub fn validate_download_dir(path: String) -> CommandResult<ValidatedDownloadDir> {
    let validate = || -> anyhow::Result<ValidatedDownloadDir> {
        let candidate = PathBuf::from(&path);
        let canonical = prepare_browsable_dir(&candidate)?;
        // 写探测：确认应用对该目录有写权限（如「所有文件访问」未授予时公共目录不可写）
        if !crate::utils::probe_directory_writable(&canonical) {
            return Err(anyhow!(
                "目录`{}`不可写，请检查存储权限",
                canonical.display()
            ));
        }
        Ok(ValidatedDownloadDir {
            canonical_path: canonical.to_string_lossy().to_string(),
        })
    };
    validate().map_err(|err| CommandError::from("目录不可用", err))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browse_requires_absolute_path() {
        assert!(ensure_browsable(Path::new("relative/path")).is_err());
        assert!(prepare_browsable_dir(Path::new("relative/path")).is_err());
    }

    #[test]
    fn creating_a_missing_directory_outside_storage_is_rejected() {
        // 不存在的路径不能直接 canonicalize；这里验证「先校验已存在祖先」不会
        // 因为路径不存在而误报，也不会把目录建到 /storage 之外。
        let outside = if cfg!(windows) {
            PathBuf::from("C:/jm-test-not-exists/child")
        } else {
            PathBuf::from("/tmp/jm-test-not-exists/child")
        };
        assert!(prepare_browsable_dir(&outside).is_err());
        assert!(!outside.exists(), "被拒绝的路径不应被创建");
    }

    #[cfg(unix)]
    #[test]
    fn browse_rejects_system_directories() {
        assert!(ensure_browsable(Path::new("/data")).is_err());
        assert!(ensure_browsable(Path::new("/proc")).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn browse_rejects_system_directories() {
        assert!(ensure_browsable(Path::new("C:/Windows")).is_err());
        assert!(ensure_browsable(Path::new("C:/Users")).is_err());
    }
}
