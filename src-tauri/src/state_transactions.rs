use crate::atomic_fs;
use anyhow::{ensure, Context};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize)]
pub(crate) struct StateJournal {
    pub previous_config: crate::config::Config,
    #[serde(default)]
    pub previous_queue: Option<serde_json::Value>,
    /// 恢复归档意图时的旧文件快照；`None` 表示旧事务日志，不应触碰意图文件。
    #[serde(default)]
    pub previous_intents: Option<serde_json::Value>,
    #[serde(default)]
    pub intents_managed: bool,
    pub committed: bool,
}

impl StateJournal {
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        atomic_fs::write(path, serde_json::to_vec(self)?)
    }
}

#[derive(Serialize, Deserialize)]
struct RemovalJournal {
    previous_catalog: serde_json::Value,
    moves: Vec<(PathBuf, PathBuf)>,
    /// 隔离改名失败、已被直接删除的路径。回滚/恢复时不能把它们的书库记录写回去；
    /// `serde(default)` 兼容旧版本没有该字段的事务日志。
    #[serde(default)]
    direct_deletes: Vec<PathBuf>,
    committed: bool,
}

fn save_removal(path: &Path, journal: &RemovalJournal) -> anyhow::Result<()> {
    atomic_fs::write(path, serde_json::to_vec(journal)?)
}

fn restore_moves(moves: &[(PathBuf, PathBuf)]) -> anyhow::Result<()> {
    for (original, staged) in moves.iter().rev() {
        ensure!(
            original.parent() == staged.parent(),
            "隔离文件的父目录不匹配"
        );
        let name = staged
            .file_name()
            .context("隔离文件没有文件名")?
            .to_string_lossy();
        let suffix = name.rsplit(".trash-").next().context("隔离文件名无效")?;
        ensure!(
            name.starts_with('.') && uuid::Uuid::parse_str(suffix).is_ok(),
            "隔离文件名无效"
        );
        if staged.exists() {
            ensure!(
                !original.exists(),
                "原路径已出现其他文件，保留隔离文件等待确认"
            );
            std::fs::rename(staged, original).context("恢复隔离文件失败")?;
        }
    }
    Ok(())
}

fn cleanup_moves(moves: &[(PathBuf, PathBuf)]) -> anyhow::Result<()> {
    for (original, staged) in moves {
        ensure!(
            original.parent() == staged.parent(),
            "隔离文件父目录不匹配，拒绝清理"
        );
        let name = staged
            .file_name()
            .context("隔离文件没有文件名")?
            .to_string_lossy();
        let id = name.rsplit(".trash-").next().context("隔离文件名无效")?;
        ensure!(
            name.starts_with('.') && uuid::Uuid::parse_str(id).is_ok(),
            "隔离文件名无效，拒绝清理"
        );
        if staged.is_dir() {
            std::fs::remove_dir_all(staged)?;
        } else if staged.exists() {
            std::fs::remove_file(staged)?;
        }
    }
    Ok(())
}

pub(crate) fn remove_with_catalog(
    records_path: &Path,
    paths: Vec<PathBuf>,
    previous: &crate::library_catalog::LibraryCatalog,
    next: &crate::library_catalog::LibraryCatalog,
) -> anyhow::Result<()> {
    let journal_path =
        records_path.with_file_name(format!(".library-delete-{}.json", uuid::Uuid::new_v4()));
    let mut journal = RemovalJournal {
        previous_catalog: serde_json::to_value(previous)?,
        moves: Vec::new(),
        direct_deletes: Vec::new(),
        committed: false,
    };
    for original in paths {
        let file_name = original
            .file_name()
            .context("删除目标没有文件名")?
            .to_string_lossy();
        let staged = original.with_file_name(format!(
            ".{}.trash-{}",
            crate::utils::truncate_utf8_by_bytes(&file_name, 100),
            uuid::Uuid::new_v4()
        ));
        journal.moves.push((original, staged));
    }
    save_removal(&journal_path, &journal)?;
    let result = (|| -> anyhow::Result<()> {
        for index in 0..journal.moves.len() {
            let original = journal.moves[index].0.clone();
            let staged = journal.moves[index].1.clone();
            if !original.exists() {
                continue;
            }
            // 阅读器缓存着这个 CBZ 的读句柄时，公共存储（FUSE）会拒绝改名；
            // 拿替换锁后释放，避免释放与改名之间被重新打开
            let rename_result = {
                let _guard = crate::atomic_fs::commit_guard();
                crate::commands::reader_commands::release_handle_for(&original);
                std::fs::rename(&original, &staged)
            };
            if let Err(rename_error) = rename_result {
                // Android 公共存储/FUSE 会拒绝改名正被媒体扫描或其它进程打开的文件。
                // 删除本身不需要改名，改名失败时直接删除，避免整本漫画因此删不掉。
                tracing::warn!(
                    path = %original.display(),
                    error = %rename_error,
                    "隔离删除目标失败，改为直接删除"
                );
                // 先持久化“该路径会被直接删除”，再执行删除：即使删除后进程崩溃，
                // 恢复事务也不会把已不存在的文件记录写回书库。
                journal.direct_deletes.push(original.clone());
                save_removal(&journal_path, &journal)?;
                let _guard = crate::atomic_fs::commit_guard();
                crate::commands::reader_commands::release_handle_for(&original);
                if original.is_dir() {
                    std::fs::remove_dir_all(&original)
                } else {
                    std::fs::remove_file(&original)
                }
                .with_context(|| format!("删除目标失败：{}", original.display()))?;
            }
        }
        next.save(records_path)?;
        journal.committed = true;
        save_removal(&journal_path, &journal)?;
        Ok(())
    })();
    if let Err(error) = result {
        let rollback = (|| -> anyhow::Result<()> {
            // 直接删除的文件已经无法恢复；回滚书库记录时要同步去掉这些条目，
            // 否则书库会指向不存在的文件。
            let mut restored = previous.clone();
            restored.forget_missing_paths(&journal.direct_deletes);
            restored.save(records_path)?;
            restore_moves(&journal.moves)?;
            std::fs::remove_file(&journal_path)?;
            Ok(())
        })();
        if let Err(rollback_error) = rollback {
            return Err(error.context(format!("回退尚未完成，已保留恢复记录：{rollback_error:#}")));
        }
        return Err(error);
    }
    if let Err(error) = cleanup_moves(&journal.moves).and_then(|_| {
        std::fs::remove_file(&journal_path)?;
        Ok(())
    }) {
        // 保留已提交的事务日志：启动恢复会按 committed=true 再次清理，
        // 避免启动扫尾把还能回滚的隔离文件提前删掉。
        tracing::warn!(error = %error, "漫画已移除，残留隔离文件将在下次启动清理");
    }
    Ok(())
}

/// 启动时恢复未完成的配置替换与书库删除事务。
///
/// 恢复是尽力而为的：单个日志损坏或某一步失败只记录错误并保留/隔离该日志，
/// 不让一个损坏的事务日志导致应用无法启动。
pub(crate) fn recover(config_path: &Path) {
    let state_path = config_path.with_file_name(".state-replace.json");
    if state_path.is_file() {
        if let Err(error) = recover_state_replace(config_path, &state_path) {
            tracing::error!(error = %error, "恢复配置替换事务失败，已保留日志以便排查");
        }
    }
    let records_path = config_path.with_file_name(".library.json");
    let Some(parent) = config_path.parent() else {
        return;
    };
    let entries = match std::fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(error) => {
            tracing::warn!(error = %error, "读取配置目录以恢复书库删除事务失败");
            return;
        }
    };
    for entry in entries.flatten() {
        if let Err(error) = recover_removal(&records_path, &entry) {
            tracing::error!(path = %entry.path().display(), error = %error, "恢复书库删除事务失败，已保留日志以便排查");
        }
    }
}

fn recover_state_replace(config_path: &Path, state_path: &Path) -> anyhow::Result<()> {
    let journal: StateJournal = match serde_json::from_slice(&std::fs::read(state_path)?) {
        Ok(journal) => journal,
        Err(error) => {
            quarantine(state_path);
            return Err(error).context("配置替换日志损坏，已隔离");
        }
    };
    if !journal.committed {
        // 旧配置可能来自更早的 schema：逐字段修复而不是整体拒绝，避免启动失败
        let base_dir = config_path
            .parent()
            .context("配置路径没有父目录")?
            .to_path_buf();
        let previous = serde_json::to_value(&journal.previous_config)?;
        let repaired = crate::config::Config::merge_config(&previous.to_string(), &base_dir);
        repaired.validate()?;
        // 必须使用存储序列化器：账号/密码在这里重新加密，不能把明文写回 config.json。
        atomic_fs::write(
            config_path,
            crate::config::serialize_config_for_storage(&repaired)?,
        )?;
        let queue_path = config_path.with_file_name("download-queue.json");
        if let Some(queue) = journal.previous_queue {
            atomic_fs::write(&queue_path, serde_json::to_vec_pretty(&queue)?)?;
        } else if queue_path.exists() {
            std::fs::remove_file(&queue_path)?;
        }
        if journal.intents_managed {
            let intents_path = crate::archive_intents::intents_path(config_path);
            if let Err(error) = crate::archive_intents::restore_intents_file(
                &intents_path,
                journal.previous_intents.as_ref(),
            ) {
                // 意图恢复失败不能让整个事务日志留下来：否则每次启动都会重复
                // 回滚配置与队列，用户后续的修改会被反复覆盖。
                tracing::warn!(
                    error = %error,
                    path = %intents_path.display(),
                    "恢复归档意图失败，跳过该步骤以完成事务恢复"
                );
            }
        }
    }
    std::fs::remove_file(state_path)?;
    Ok(())
}

fn recover_removal(records_path: &Path, entry: &std::fs::DirEntry) -> anyhow::Result<()> {
    let name = entry.file_name();
    let name = name.to_string_lossy();
    let Some(id) = name
        .strip_prefix(".library-delete-")
        .and_then(|s| s.strip_suffix(".json"))
    else {
        return Ok(());
    };
    if uuid::Uuid::parse_str(id).is_err() || !entry.file_type()?.is_file() {
        return Ok(());
    }
    let journal: RemovalJournal = match serde_json::from_slice(&std::fs::read(entry.path())?) {
        Ok(journal) => journal,
        Err(error) => {
            quarantine(&entry.path());
            return Err(error).context("书库删除日志损坏，已隔离");
        }
    };
    if journal.committed {
        cleanup_moves(&journal.moves)?;
    } else {
        let mut previous: crate::library_catalog::LibraryCatalog =
            serde_json::from_value(journal.previous_catalog)?;
        // 直接删除的文件无法随回滚恢复，书库记录也不能再保留它们。
        previous.forget_missing_paths(&journal.direct_deletes);
        previous.save(records_path)?;
        restore_moves(&journal.moves)?;
    }
    std::fs::remove_file(entry.path())?;
    Ok(())
}

/// 把无法解析的事务日志改名保留，避免每次启动重复失败，同时不覆盖损坏原件。
fn quarantine(path: &Path) {
    let target = path.with_file_name(format!(
        "{}.corrupt-{}",
        path.file_name().unwrap_or_default().to_string_lossy(),
        uuid::Uuid::new_v4()
    ));
    if let Err(error) = std::fs::rename(path, &target) {
        tracing::warn!(error = %error, path = %path.display(), "隔离损坏事务日志失败");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn interrupted_state_replace_restores_archive_intents() {
        let root = std::env::temp_dir().join(format!("jm-state-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let config_path = root.join("config.json");
        let intents_path = crate::archive_intents::intents_path(&config_path);
        std::fs::write(
            &intents_path,
            serde_json::to_vec(&serde_json::json!({
                "version": 1,
                "entries": { "2": "perChapter" },
            }))
            .unwrap(),
        )
        .unwrap();
        let journal = StateJournal {
            previous_config: crate::config::Config::default(&root),
            previous_queue: None,
            previous_intents: Some(serde_json::json!({
                "version": 1,
                "entries": { "1": "merge" },
            })),
            intents_managed: true,
            committed: false,
        };
        journal.save(&root.join(".state-replace.json")).unwrap();

        recover(&config_path);

        let restored: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&intents_path).unwrap()).unwrap();
        assert_eq!(restored["entries"]["1"], "merge");
        assert!(restored["entries"].get("2").is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn old_state_journal_leaves_archive_intents_untouched() {
        let root = std::env::temp_dir().join(format!("jm-state-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let config_path = root.join("config.json");
        let intents_path = crate::archive_intents::intents_path(&config_path);
        let original = serde_json::json!({
            "version": 1,
            "entries": { "7": "perChapter" },
        });
        std::fs::write(&intents_path, serde_json::to_vec(&original).unwrap()).unwrap();
        let journal = StateJournal {
            previous_config: crate::config::Config::default(&root),
            previous_queue: None,
            previous_intents: None,
            intents_managed: false,
            committed: false,
        };
        journal.save(&root.join(".state-replace.json")).unwrap();

        recover(&config_path);

        let stored: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&intents_path).unwrap()).unwrap();
        assert_eq!(stored, original);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn interrupted_state_replace_re_encrypts_credentials() {
        let root = std::env::temp_dir().join(format!("jm-state-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let config_path = root.join("config.json");
        let password = "test-password";
        let mut previous = crate::config::Config::default(&root);
        previous.username = "alice".to_string();
        previous.password = password.to_string();
        let journal = StateJournal {
            previous_config: previous,
            previous_queue: None,
            previous_intents: None,
            intents_managed: false,
            committed: false,
        };
        journal.save(&root.join(".state-replace.json")).unwrap();

        recover(&config_path);

        let stored: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&config_path).unwrap()).unwrap();
        assert!(stored["username"].as_str().unwrap().starts_with("enc:v1:"));
        assert!(stored["password"].as_str().unwrap().starts_with("enc:v1:"));
        assert!(!stored.to_string().contains(password));
        let restored = crate::config::deserialize_config_value(stored).unwrap();
        assert_eq!(restored.username, "alice");
        assert_eq!(restored.password, password);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn invalid_archive_intents_do_not_block_state_recovery() {
        let root = std::env::temp_dir().join(format!("jm-state-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let config_path = root.join("config.json");
        let state_path = root.join(".state-replace.json");
        let intents_path = crate::archive_intents::intents_path(&config_path);
        let original = serde_json::json!({
            "version": 1,
            "entries": { "2": "perChapter" },
        });
        std::fs::write(&intents_path, serde_json::to_vec(&original).unwrap()).unwrap();
        let journal = StateJournal {
            previous_config: crate::config::Config::default(&root),
            previous_queue: None,
            // 版本不受支持：恢复必然失败，但不应留下事务日志导致每次启动重复回滚。
            previous_intents: Some(serde_json::json!({ "version": 99, "entries": {} })),
            intents_managed: true,
            committed: false,
        };
        journal.save(&state_path).unwrap();

        recover(&config_path);

        assert!(config_path.exists());
        assert!(!state_path.exists(), "恢复失败不能留下事务日志");
        let stored: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&intents_path).unwrap()).unwrap();
        assert_eq!(stored, original, "恢复失败时应保持原意图文件不变");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn interrupted_removal_restores_original_and_catalog() {
        let root = std::env::temp_dir().join(format!("jm-removal-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let original = root.join("book.cbz");
        std::fs::write(&original, b"original").unwrap();
        let staged = original.with_file_name(format!(".book.cbz.trash-{}", uuid::Uuid::new_v4()));
        let records = root.join(".library.json");
        let previous = crate::library_catalog::LibraryCatalog::default();
        previous.save(&records).unwrap();
        let journal = RemovalJournal {
            previous_catalog: serde_json::to_value(&previous).unwrap(),
            moves: vec![(original.clone(), staged.clone())],
            direct_deletes: Vec::new(),
            committed: false,
        };
        save_removal(
            &root.join(format!(".library-delete-{}.json", uuid::Uuid::new_v4())),
            &journal,
        )
        .unwrap();
        std::fs::rename(&original, &staged).unwrap();
        recover(&root.join("config.json"));
        assert_eq!(std::fs::read(original).unwrap(), b"original");
        assert!(!staged.exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn interrupted_removal_drops_catalog_records_for_directly_deleted_files() {
        let root = std::env::temp_dir().join(format!("jm-removal-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let kept = root.join("kept.cbz");
        let deleted = root.join("deleted.cbz");
        std::fs::write(&kept, b"kept").unwrap();
        std::fs::write(&deleted, b"deleted").unwrap();
        let kept = std::fs::canonicalize(kept).unwrap();
        let deleted = std::fs::canonicalize(deleted).unwrap();
        let records = root.join(".library.json");
        let mut catalog = crate::library_catalog::LibraryCatalog::default();
        let (kept_id, _) = catalog.register(&root, &kept).unwrap();
        let (deleted_id, _) = catalog.register(&root, &deleted).unwrap();
        catalog.save(&records).unwrap();
        // 模拟隔离改名失败后的直接删除：文件已不存在，事务日志记录了这个决定。
        std::fs::remove_file(&deleted).unwrap();
        let journal = RemovalJournal {
            previous_catalog: serde_json::to_value(&catalog).unwrap(),
            moves: vec![(
                deleted.clone(),
                deleted.with_file_name(format!(".deleted.cbz.trash-{}", uuid::Uuid::new_v4())),
            )],
            direct_deletes: vec![deleted.clone()],
            committed: false,
        };
        save_removal(
            &root.join(format!(".library-delete-{}.json", uuid::Uuid::new_v4())),
            &journal,
        )
        .unwrap();

        recover(&root.join("config.json"));

        let restored = crate::library_catalog::LibraryCatalog::load(&records).unwrap();
        let ids: Vec<i64> = restored
            .visible_imports(&root)
            .iter()
            .map(|record| record.id)
            .collect();
        assert_eq!(ids, vec![kept_id]);
        assert!(!ids.contains(&deleted_id));
        std::fs::remove_dir_all(root).unwrap();
    }
}
