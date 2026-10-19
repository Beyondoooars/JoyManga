use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context};
use parking_lot::Mutex;
use tauri::{AppHandle, Manager};
use tauri_specta::Event;

use crate::archive_intents::ArchiveIntentStore;
use crate::atomic_fs;
use crate::events::ComicArchiveEvent;
use crate::extensions::AppHandleExt;
use crate::types::{CbzMode, ChapterInfo, Comic};
use crate::utils;

/// 正在执行归档最终整合的漫画 ID 集合（同一本漫画同时只允许一个整合任务）。
#[derive(Default)]
pub struct ArchiveFinalizeState {
    running: Mutex<HashSet<i64>>,
}

impl ArchiveFinalizeState {
    fn try_begin(&self, comic_id: i64) -> bool {
        self.running.lock().insert(comic_id)
    }

    fn end(&self, comic_id: i64) {
        self.running.lock().remove(&comic_id);
    }

    /// 该漫画当前是否正在整合（供新建下载任务前检查，避免与清理来源文件交叉）。
    pub fn is_running(&self, comic_id: i64) -> bool {
        self.running.lock().contains(&comic_id)
    }

    /// 是否有任意漫画正在整合（供清理类命令判断）。
    pub fn is_any_running(&self) -> bool {
        !self.running.lock().is_empty()
    }
}

/// 运行期守卫：无论 finalize 正常返回还是 panic，都会释放 running 标记，
/// 避免一次 panic 让该漫画在本进程内永远无法再整合。
struct FinalizeGuard {
    app: AppHandle,
    comic_id: i64,
}

impl Drop for FinalizeGuard {
    fn drop(&mut self) {
        self.app.state::<ArchiveFinalizeState>().end(self.comic_id);
    }
}

/// 章节任务完成且该漫画没有其他活跃任务时，在后台执行归档最终整合。
///
/// 整合方式取自归档意图存储（创建下载任务时写入），缺省使用配置中的默认方式。
pub fn spawn_finalize_if_idle(app: &AppHandle, comic_id: i64) {
    if app
        .get_download_manager()
        .has_active_tasks_for_comic(comic_id)
    {
        return;
    }
    if !app.state::<ArchiveFinalizeState>().try_begin(comic_id) {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = FinalizeGuard {
            app: app.clone(),
            comic_id,
        };
        finalize_comic_archive(&app, comic_id);
    });
}

fn finalize_comic_archive(app: &AppHandle, comic_id: i64) {
    // 排队期间可能又创建了新任务，再次确认空闲状态；运行期间新章节完成会重新调度
    if app
        .get_download_manager()
        .has_active_tasks_for_comic(comic_id)
    {
        return;
    }
    let comic = match local_comic(app, comic_id) {
        Ok(comic) => comic,
        Err(error) => {
            tracing::error!(comic_id, error = %error, "归档最终整合：读取本地漫画失败");
            return;
        }
    };
    let mode = {
        let intents = app.state::<ArchiveIntentStore>();
        intents
            .get(comic_id)
            .unwrap_or_else(|| app.get_config().read().default_cbz_mode)
    };
    let comic_title = comic.name.clone();
    let _ = ComicArchiveEvent::Start {
        comic_id,
        comic_title: comic_title.clone(),
        mode,
    }
    .emit(app);

    // 清理条件：本次执行期间没有新任务被创建。若新章节已排队/下载，保留意图，
    // 待任务完成触发下一次整合时再处理；否则清除本次已消费的意图。
    let has_new_tasks = app
        .get_download_manager()
        .has_active_tasks_for_comic(comic_id);
    match run_finalize(app, &comic, mode) {
        Ok(archive_path) => {
            if !has_new_tasks {
                // 运行期间用户可能重新选择了整理方式：只有仍与本次执行一致时才清除意图，
                // 否则保留用户的新选择，交给下一次整合执行。
                let intents = app.state::<ArchiveIntentStore>();
                if intents.get(comic_id) == Some(mode) {
                    intents.remove(comic_id);
                }
            }
            if let Some(cache) = app.try_state::<utils::DownloadedLibraryCache>() {
                cache.invalidate();
            }
            let _ = ComicArchiveEvent::End {
                comic_id,
                comic_title,
                mode,
                archive_path,
            }
            .emit(app);
        }
        Err(error) => {
            let message = format!("{error:#}");
            tracing::error!(comic_id, error = %error, "归档最终整合失败");
            let _ = ComicArchiveEvent::Error {
                comic_id,
                comic_title,
                message,
            }
            .emit(app);
        }
    }

    // 整合期间若有新章节完成（完成事件被 try_begin 挡下），这里补一次调度，
    // 避免“合并漏章且永不重试”。
    if has_new_tasks
        || app
            .get_download_manager()
            .has_active_tasks_for_comic(comic_id)
    {
        spawn_finalize_if_idle(app, comic_id);
    }
}

fn local_comic(app: &AppHandle, comic_id: i64) -> anyhow::Result<Comic> {
    let root = app.get_config().read().download_dir.clone();
    if let Some(comic) = app
        .state::<utils::DownloadedLibraryCache>()
        .comic(&root, comic_id)
    {
        return Ok(comic);
    }
    crate::commands::library_commands::get_downloaded_comics(app.clone())
        .into_iter()
        .find(|comic| comic.id == comic_id)
        .with_context(|| format!("本地书库没有漫画ID `{comic_id}`"))
}

fn run_finalize(app: &AppHandle, comic: &Comic, mode: CbzMode) -> anyhow::Result<PathBuf> {
    let download_dir = app.get_config().read().download_dir.clone();
    std::fs::create_dir_all(&download_dir)
        .context(format!("创建下载目录`{}`失败", download_dir.display()))?;
    let canonical_root = std::fs::canonicalize(&download_dir)
        .context(format!("无法解析下载根目录`{}`", download_dir.display()))?;

    let mut downloaded: Vec<&ChapterInfo> = comic
        .chapter_infos
        .iter()
        .filter(|chapter| {
            chapter.is_downloaded == Some(true) && chapter.chapter_download_dir.is_some()
        })
        .collect();
    downloaded.sort_by_key(|chapter| (chapter.order, chapter.chapter_id));
    if downloaded.is_empty() {
        return Err(anyhow!("没有可整合的已下载章节"));
    }

    match mode {
        CbzMode::Merge => {
            let target = resolve_merged_target(&canonical_root, comic)?;
            let total = downloaded.len() as u32;
            let comic_id = comic.id;
            let progress_app = app.clone();
            crate::export::write_merged_cbz_archive(comic, &downloaded, &target, move |current| {
                let _ = ComicArchiveEvent::Progress {
                    comic_id,
                    current,
                    total,
                }
                .emit(&progress_app);
            })?;
            cleanup_merged_sources(&canonical_root, &target, comic, &downloaded);
            tracing::info!(
                comic_title = %comic.name,
                archive = %target.display(),
                chapters = downloaded.len(),
                "已自动合并为整本CBZ并清理来源章节文件"
            );
            Ok(target)
        }
        CbzMode::PerChapter => {
            let total_chapters = comic.chapter_infos.len();
            for chapter in &downloaded {
                if let Err(error) =
                    rename_chapter_archive(&canonical_root, comic, total_chapters, chapter)
                {
                    tracing::warn!(
                        comic_title = %comic.name,
                        chapter = %chapter.chapter_title,
                        error = %error,
                        "章节归档命名整理失败，保留原文件名"
                    );
                }
            }
            // 逐话模式的最终形态就是各章节文件本身，回传书库目录便于前端提示定位
            Ok(canonical_root)
        }
    }
}

/// 解析整本合并的目标路径：优先 `<本子名>.cbz`，被其他漫画占用时回退 `[JM{id}]` 后缀。
fn resolve_merged_target(root: &Path, comic: &Comic) -> anyhow::Result<PathBuf> {
    const MAX_TOTAL_BYTES: usize = 180;
    let title = utils::filename_filter(&comic.name);
    let title = if title.is_empty() {
        format!("comic-{}", comic.id)
    } else {
        title
    };

    let plain = root.join(format!(
        "{}.cbz",
        utils::truncate_utf8_by_bytes(&title, MAX_TOTAL_BYTES.saturating_sub(".cbz".len()))
    ));
    let id_suffix = format!(" [JM{}].cbz", comic.id);
    let with_id = root.join(format!(
        "{}{}",
        utils::truncate_utf8_by_bytes(&title, MAX_TOTAL_BYTES.saturating_sub(id_suffix.len())),
        id_suffix
    ));
    if !plain.exists() || archive_belongs_to_comic(&plain, comic.id) {
        return Ok(plain);
    }
    if !with_id.exists() || archive_belongs_to_comic(&with_id, comic.id) {
        return Ok(with_id);
    }
    Err(anyhow!(
        "下载目录中存在同名但属于其他漫画的 CBZ 文件，无法确定合并目标"
    ))
}

fn archive_belongs_to_comic(path: &Path, comic_id: i64) -> bool {
    (|| -> anyhow::Result<bool> {
        let mut archive = zip::ZipArchive::new(std::fs::File::open(path)?)?;
        let mut entry = archive.by_name("元数据.json")?;
        let size = entry.size();
        let bytes = crate::image_data::read_limited(
            &mut entry,
            size,
            crate::image_data::MAX_METADATA_BYTES,
        )?;
        let value: serde_json::Value = serde_json::from_slice(&bytes)?;
        Ok(value["id"].as_i64() == Some(comic_id))
    })()
    .unwrap_or(false)
}

/// 合并成功后清理来源章节文件：只删除身份匹配的独立章节 CBZ（本应用下载产物），
/// 目录来源、整本归档与本漫画之外的文件一律不动。
fn cleanup_merged_sources(root: &Path, target: &Path, comic: &Comic, sources: &[&ChapterInfo]) {
    let canonical_target = std::fs::canonicalize(target).unwrap_or_else(|_| target.to_path_buf());
    for chapter in sources {
        let Some(source) = chapter.chapter_download_dir.as_deref() else {
            continue;
        };
        if !source.is_file() {
            continue;
        }
        let Ok(canonical_source) = std::fs::canonicalize(source) else {
            continue;
        };
        if canonical_source == canonical_target || !canonical_source.starts_with(root) {
            continue;
        }
        match crate::comic_scanner::read_chapter_identity(&canonical_source) {
            Ok(Some(identity)) if identity == (comic.id, chapter.chapter_id) => {
                if let Err(error) = std::fs::remove_file(&canonical_source) {
                    tracing::warn!(
                        path = %canonical_source.display(),
                        error = %error,
                        "清理来源章节文件失败，将在下次整合时重试"
                    );
                }
            }
            _ => {}
        }
    }
}

/// 逐话模式：把身份匹配的章节 CBZ 整理为「本子名 - 序号 - 话名.cbz」。
/// 目标名被其他文件占用时保留原存储名，不做破坏性替换。
fn rename_chapter_archive(
    root: &Path,
    comic: &Comic,
    total_chapters: usize,
    chapter: &ChapterInfo,
) -> anyhow::Result<()> {
    let Some(source) = chapter.chapter_download_dir.as_deref() else {
        return Ok(());
    };
    if !source.is_file() {
        return Ok(());
    }
    let canonical_source =
        std::fs::canonicalize(source).context(format!("解析章节文件`{}`失败", source.display()))?;
    if !canonical_source.starts_with(root) {
        return Ok(());
    }
    if crate::comic_scanner::read_chapter_identity(&canonical_source)?
        != Some((comic.id, chapter.chapter_id))
    {
        return Ok(());
    }

    let desired_name = utils::cbz_filename(
        &comic.name,
        comic.id,
        &chapter.chapter_title,
        chapter.chapter_id,
        chapter.order,
        total_chapters,
    );
    let desired = root.join(desired_name);
    if desired == canonical_source {
        return Ok(());
    }
    if desired.exists() {
        // 同名文件必须属于本话才允许替换；否则保留存储名
        let existing_identity = crate::comic_scanner::read_chapter_identity(&desired)?;
        if existing_identity != Some((comic.id, chapter.chapter_id)) {
            return Ok(());
        }
        atomic_fs::copy_file(&canonical_source, &desired)?;
        let _ = std::fs::remove_file(&canonical_source);
        return Ok(());
    }
    match std::fs::rename(&canonical_source, &desired) {
        Ok(()) => Ok(()),
        Err(_) => {
            // 跨卷等无法直接改名的情况：复制到目标后再删除来源
            atomic_fs::copy_file(&canonical_source, &desired)?;
            let _ = std::fs::remove_file(&canonical_source);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn make_chapter_cbz(path: &Path, comic_id: i64, chapter_id: i64) {
        let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        zip.start_file("chapter.json", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(
            serde_json::to_string(&serde_json::json!({"comicId":comic_id,"chapterId":chapter_id}))
                .unwrap()
                .as_bytes(),
        )
        .unwrap();
        zip.start_file("0001.png", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"page").unwrap();
        zip.finish().unwrap();
    }

    fn make_merged_book(path: &Path, comic_id: i64, chapter_ids: &[i64]) {
        let chapters: Vec<ChapterInfo> = chapter_ids
            .iter()
            .enumerate()
            .map(|(index, id)| ChapterInfo {
                chapter_id: *id,
                chapter_title: format!("第{}话", index + 1),
                order: index as i64 + 1,
                is_downloaded: None,
                chapter_download_dir: None,
            })
            .collect();
        let comic = Comic {
            id: comic_id,
            name: "书".into(),
            chapter_infos: chapters,
            ..Comic::default()
        };
        let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        zip.start_file("元数据.json", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(&serde_json::to_vec(&comic).unwrap()).unwrap();
        zip.start_file("0001.jpg", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"page").unwrap();
        zip.finish().unwrap();
    }

    fn chapter(id: i64, title: &str, order: i64, dir: &Path) -> ChapterInfo {
        ChapterInfo {
            chapter_id: id,
            chapter_title: title.into(),
            order,
            is_downloaded: Some(true),
            chapter_download_dir: Some(dir.to_path_buf()),
        }
    }

    #[test]
    fn merged_target_prefers_comic_name_and_avoids_other_comics_files() {
        let root = std::env::temp_dir().join(format!("jm-target-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let comic = Comic {
            id: 77,
            name: "山间旅行".into(),
            ..Comic::default()
        };
        // 初始：<本子名>.cbz 可用
        let target = resolve_merged_target(&root, &comic).unwrap();
        assert_eq!(target.file_name().unwrap(), "山间旅行.cbz");

        // 被其他漫画占用 -> 回退 ID 后缀
        make_merged_book(&root.join("山间旅行.cbz"), 999, &[1]);
        let target = resolve_merged_target(&root, &comic).unwrap();
        assert_eq!(target.file_name().unwrap(), "山间旅行 [JM77].cbz");

        // 自己占用 -> 仍然使用 <本子名>.cbz（原位更新）
        make_merged_book(&root.join("山间旅行.cbz"), 77, &[1]);
        let target = resolve_merged_target(&root, &comic).unwrap();
        assert_eq!(target.file_name().unwrap(), "山间旅行.cbz");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rename_uses_clean_names_and_never_replaces_other_chapters() {
        let root = std::env::temp_dir().join(format!("jm-rename-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        // 被调函数用 canonicalize 后的路径做包含判断；Windows 上临时目录可能是 8.3 短名，
        // canonicalize 会展开长名并附加 `\\?\` 前缀，测试先规范化再使用，避免混用比较。
        let root = std::fs::canonicalize(&root).unwrap();
        let comic = Comic {
            id: 5,
            name: "书".into(),
            ..Comic::default()
        };
        let src = root.join(utils::storage_cbz_filename("书", 5, 501));
        make_chapter_cbz(&src, 5, 501);
        let ch = chapter(501, "第1话", 1, &src);
        rename_chapter_archive(&root, &comic, 3, &ch).unwrap();
        let renamed = root.join("书 - 0001 - 第1话.cbz");
        assert!(renamed.is_file());
        assert!(!src.exists());

        // 目标名被其他话占用时保持原样
        let src2 = root.join(utils::storage_cbz_filename("书", 5, 502));
        make_chapter_cbz(&src2, 5, 502);
        let ch2 = chapter(502, "第1话", 1, &src2);
        rename_chapter_archive(&root, &comic, 3, &ch2).unwrap();
        assert!(src2.is_file(), "不能覆盖其他话已占用的目标名");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cleanup_only_removes_own_chapter_files() {
        let root = std::env::temp_dir().join(format!("jm-cleanup-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        // 同 rename 测试：先规范化根目录，保证与函数内部的 canonicalize 比较同一形式。
        let root = std::fs::canonicalize(&root).unwrap();
        let comic = Comic {
            id: 8,
            name: "书".into(),
            ..Comic::default()
        };
        let own = root.join(utils::storage_cbz_filename("书", 8, 801));
        make_chapter_cbz(&own, 8, 801);
        let foreign = root.join("other.cbz");
        make_chapter_cbz(&foreign, 42, 801);
        let target = root.join("书.cbz");
        make_merged_book(&target, 8, &[801]);

        let own_chapter = chapter(801, "第1话", 1, &own);
        let foreign_chapter = chapter(801, "第1话", 1, &foreign);
        let same_as_target = chapter(801, "第1话", 1, &target);
        cleanup_merged_sources(
            &root,
            &target,
            &comic,
            &[&own_chapter, &foreign_chapter, &same_as_target],
        );
        assert!(!own.exists(), "自己的章节文件应被清理");
        assert!(foreign.exists(), "其他漫画的文件不能被删除");
        assert!(target.exists(), "合并目标不能被删除");
        std::fs::remove_dir_all(root).unwrap();
    }
}
