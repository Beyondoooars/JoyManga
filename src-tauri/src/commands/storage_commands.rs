use std::path::{Path, PathBuf};

use anyhow::{anyhow, ensure};
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
    ensure!(
        candidate.is_absolute(),
        "路径必须是绝对路径（以 / 开头）"
    );
    let canonical = std::fs::canonicalize(candidate)
        .map_err(|err| anyhow!("目录`{}`不存在或无法访问: {err}", candidate.display()))?;
    let root = Path::new(BROWSABLE_ROOT);
    ensure!(
        canonical.starts_with(root),
        "只能浏览公共存储（/storage）下的目录"
    );
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
        ensure!(
            candidate.is_absolute(),
            "路径必须是绝对路径（以 / 开头）"
        );
        ensure_browsable(&candidate)?;
        std::fs::create_dir_all(&candidate)
            .map_err(|err| anyhow!("无法创建目录`{}`: {err}", candidate.display()))?;
        // 写探测：确认应用对该目录有写权限（如「所有文件访问」未授予时公共目录不可写）
        let test_file = candidate.join(format!(".write_test_{}", uuid::Uuid::new_v4()));
        std::fs::write(&test_file, b"test")
            .map_err(|_| anyhow!("目录`{}`不可写，请检查存储权限", candidate.display()))?;
        let _ = std::fs::remove_file(&test_file);
        let canonical = std::fs::canonicalize(&candidate)
            .map_err(|err| anyhow!("规范化目录`{}`失败: {err}", candidate.display()))?;
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
