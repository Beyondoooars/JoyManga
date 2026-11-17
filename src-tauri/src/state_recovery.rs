use std::path::{Path, PathBuf};

use anyhow::Context;
use tauri_specta::Event;

pub fn background_failure(
    app: Option<&tauri::AppHandle>,
    domain: &str,
    title: &str,
    error: &anyhow::Error,
) {
    tracing::error!(error = %error, domain, "{title}");
    if let Some(app) = app {
        let _ = crate::events::BackgroundFailureEvent {
            domain: domain.into(),
            title: title.into(),
            message: format!("{error:#}"),
            dedupe_key: domain.into(),
        }
        .emit(app);
    }
}

/// 备用文件仅用于隔离上一次未能隔离的损坏原件；下次启动先回读完好的原件。
pub fn active_path(path: &Path, stem: &str) -> PathBuf {
    let fallback = path.with_file_name(format!(".{stem}.runtime.json"));
    if !fallback.is_file() {
        return path.to_path_buf();
    }
    // 不能只探测 File::open：旧主文件可打开但内容损坏时，仍须使用本次有效状态。
    if std::fs::read(path)
        .is_ok_and(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).is_ok())
    {
        return path.to_path_buf();
    }
    if std::fs::read(&fallback)
        .is_ok_and(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).is_ok())
    {
        tracing::warn!(path = %path.display(), fallback = %fallback.display(), "原状态仍不可用，继续使用独立备用文件");
        fallback
    } else {
        // 两份都不可用时交给原加载器隔离，不能用未校验内容覆盖原件。
        tracing::error!(path = %path.display(), fallback = %fallback.display(), "原状态与备用状态均无法读取，保留原路径以便隔离");
        path.to_path_buf()
    }
}

/// 原文件已经不可用时，固定备用文件才是当前状态；备份、事务恢复和诊断共用该路径。
pub fn existing_path(path: &Path, stem: &str) -> PathBuf {
    // 与启动加载使用同一条合并规则，备份/回滚不能又读回旧备用副本。
    active_path(path, stem)
}

/// 原子写恢复是启动提示，不是启动失败。多个备份不做猜测，原文件仍保留。
pub fn recover_file_backup(path: &Path) -> anyhow::Result<Option<String>> {
    match crate::atomic_fs::recover_file_backup(path) {
        Ok(()) => Ok(None),
        Err(error) => {
            tracing::error!(path = %path.display(), error = %error, "恢复状态文件备份失败");
            Ok(Some(format!(
                "状态文件存在未能自动恢复的备份，原文件未删除：{}",
                path.display()
            )))
        }
    }
}

/// 内容无法安全使用时保留原件；移不走原件则切换独立运行路径，后续保存不能覆盖它。
pub fn preserve_or_use_fallback(path: &Path, stem: &str, problem: &str) -> (PathBuf, String) {
    let backup = path.with_file_name(format!("{stem}.corrupt-{}.json", uuid::Uuid::new_v4()));
    match std::fs::rename(path, &backup) {
        Ok(()) => {
            tracing::error!(path = %path.display(), backup = %backup.display(), problem, "状态文件已隔离保留");
            (
                path.to_path_buf(),
                format!("{problem}；原文件已保留在 {}", backup.display()),
            )
        }
        Err(error) => {
            let fallback = path.with_file_name(format!(".{stem}.runtime.json"));
            tracing::error!(
                path = %path.display(),
                fallback = %fallback.display(),
                error = %error,
                problem,
                "无法隔离状态文件，本次使用独立运行文件"
            );
            (
                fallback,
                format!(
                    "{problem}；原件保留在 {}，本次使用独立运行文件",
                    path.display()
                ),
            )
        }
    }
}

/// 事务恢复也遵循活动路径，不能把已隔离的旧文件重新写回固定名称。
pub fn restore_file(
    path: &Path,
    stem: &str,
    value: Option<&serde_json::Value>,
) -> anyhow::Result<()> {
    match value {
        Some(value) => crate::atomic_fs::write(path, serde_json::to_vec_pretty(value)?)
            .with_context(|| format!("恢复状态文件 {} 失败", path.display())),
        None => match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => {
                let (_, message) = preserve_or_use_fallback(path, stem, "旧状态文件无法移除");
                Err(error).context(message)
            }
        },
    }
}
