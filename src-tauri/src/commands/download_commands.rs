use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;

use anyhow::{anyhow, Context};
use tauri::{AppHandle, Manager};
use tauri_specta::Event;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tokio::time::sleep;

use crate::archive_intents::ArchiveIntentStore;
use crate::errors::{CommandError, CommandErrorKind, CommandResult};
use crate::events::{DownloadAllFavoritesEvent, UpdateDownloadedComicsEvent};
use crate::extensions::{AnyhowErrorToStringChain, AppHandleExt};
use crate::types::{CbzMode, ChapterInfo, Comic, FavoriteSort};
use crate::utils;

static DOWNLOAD_ALL_FAVORITES_RUNNING: AtomicBool = AtomicBool::new(false);
static UPDATE_DOWNLOADED_COMICS_RUNNING: AtomicBool = AtomicBool::new(false);

/// 「仅 Wi-Fi 下载」的统一入口检查。
///
/// 所有**新建**下载任务的命令都必须经过这里：单章、整本、批量收藏、库存更新。
/// 计费网络下直接拒绝并给出可执行提示，而不是让任务创建后立刻失败
/// （那样用户看不懂原因）。恢复已排队的任务不在此列。
fn ensure_download_allowed(app: &AppHandle, err_title: &str) -> CommandResult<()> {
    let only_wifi = app.get_config().read().download_only_wifi;
    if let Err(reason) = crate::network_state::download_allowed(only_wifi) {
        return Err(CommandError::with_kind(
            err_title,
            anyhow!(reason),
            CommandErrorKind::Network,
        ));
    }
    Ok(())
}

pub(crate) struct SingleFlightGuard<'a>(&'a AtomicBool);

impl<'a> SingleFlightGuard<'a> {
    pub(crate) fn acquire(flag: &'a AtomicBool) -> Option<Self> {
        flag.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| Self(flag))
    }
}

impl Drop for SingleFlightGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

#[tauri::command(async)]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub fn get_download_tasks(app: AppHandle) -> Vec<crate::download_queue::DownloadTaskSnapshot> {
    app.get_download_manager().snapshots()
}

/// 对已下载的漫画立即执行归档整理（合并为整本 CBZ / 逐话命名整理）。
/// 供书库的手动「整理为整本」入口使用；下载完成后的自动整理由任务管线触发。
#[tauri::command(async)]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub fn promote_comic_archive(
    app: AppHandle,
    comic_id: i64,
    cbz_mode: CbzMode,
) -> CommandResult<()> {
    if app
        .get_download_manager()
        .has_active_tasks_for_comic(comic_id)
    {
        return Err(CommandError::from(
            "整本整理失败",
            anyhow!("该漫画仍有未完成的下载任务，请稍后再试"),
        ));
    }
    if app
        .state::<crate::archive_finalize::ArchiveFinalizeState>()
        .is_running(comic_id)
    {
        return Err(CommandError::from(
            "整本整理失败",
            anyhow!("该漫画正在整理中，请稍候"),
        ));
    }
    app.state::<ArchiveIntentStore>().set(comic_id, cbz_mode);
    crate::archive_finalize::spawn_finalize_if_idle(&app, comic_id);
    Ok(())
}

#[allow(clippy::needless_pass_by_value)]
#[tauri::command(async)]
#[specta::specta]
pub fn create_download_task(
    app: AppHandle,
    comic: Comic,
    chapter_id: i64,
    cbz_mode: Option<CbzMode>,
) -> CommandResult<()> {
    let download_manager = app.get_download_manager();
    let comic_title = comic.name.clone();

    ensure_download_allowed(&app, "无法开始下载")?;

    if let Some(cbz_mode) = cbz_mode {
        app.state::<ArchiveIntentStore>().set(comic.id, cbz_mode);
    }

    download_manager
        .create_download_task(comic, chapter_id)
        .map_err(|err| {
            let err_title = format!("`{comic_title}`的章节ID为`{chapter_id}`的下载任务创建失败");
            CommandError::from(&err_title, err)
        })?;

    tracing::debug!("创建章节ID为`{chapter_id}`的下载任务成功");
    Ok(())
}

#[allow(clippy::needless_pass_by_value)]
#[tauri::command(async)]
#[specta::specta]
pub fn pause_download_task(app: AppHandle, chapter_id: i64) -> CommandResult<()> {
    let download_manager = app.get_download_manager();

    download_manager
        .pause_download_task(chapter_id)
        .map_err(|err| {
            CommandError::from(&format!("暂停章节ID为`{chapter_id}`的下载任务失败"), err)
        })?;

    tracing::debug!("暂停章节ID为`{chapter_id}`的下载任务成功");
    Ok(())
}

#[allow(clippy::needless_pass_by_value)]
#[tauri::command(async)]
#[specta::specta]
pub fn resume_download_task(app: AppHandle, chapter_id: i64) -> CommandResult<()> {
    let download_manager = app.get_download_manager();

    download_manager
        .resume_download_task(chapter_id)
        .map_err(|err| {
            CommandError::from(&format!("恢复章节ID为`{chapter_id}`的下载任务失败"), err)
        })?;

    tracing::debug!("恢复章节ID为`{chapter_id}`的下载任务成功");
    Ok(())
}

#[allow(clippy::needless_pass_by_value)]
#[tauri::command(async)]
#[specta::specta]
pub fn cancel_download_task(app: AppHandle, chapter_id: i64) -> CommandResult<()> {
    let download_manager = app.get_download_manager();

    download_manager
        .cancel_download_task(chapter_id)
        .map_err(|err| {
            CommandError::from(&format!("取消章节ID为`{chapter_id}`的下载任务失败"), err)
        })?;

    tracing::debug!("取消章节ID为`{chapter_id}`的下载任务成功");
    Ok(())
}

#[tauri::command(async)]
#[specta::specta]
pub async fn download_comic(
    app: AppHandle,
    aid: i64,
    cbz_mode: Option<CbzMode>,
) -> CommandResult<()> {
    let download_manager = app.get_download_manager();

    ensure_download_allowed(&app, "一键下载漫画失败")?;

    let comic = utils::get_comic(app.clone(), aid)
        .await
        .map_err(|err| CommandError::from(&format!("获取ID为`{aid}`的漫画信息失败"), err))?;

    let comic_title = &comic.name;

    let chapter_ids: Vec<i64> = comic
        .chapter_infos
        .iter()
        .filter(|chapter_info| chapter_info.is_downloaded != Some(true))
        .map(|chapter_info| chapter_info.chapter_id)
        .collect();

    if chapter_ids.is_empty() {
        let err = anyhow!("漫画`{comic_title}`的所有章节都已存在于下载目录，无需重复下载");
        return Err(CommandError::from("一键下载漫画失败", err));
    }

    if let Some(cbz_mode) = cbz_mode {
        app.state::<ArchiveIntentStore>().set(comic.id, cbz_mode);
    }

    for chapter_id in chapter_ids {
        download_manager
            .create_download_task(comic.clone(), chapter_id)
            .map_err(|err| CommandError::from("一键下载漫画失败", err))?;
    }

    tracing::debug!("一键下载漫画成功，已为所有需要下载的章节创建下载任务");
    Ok(())
}

#[allow(clippy::cast_possible_wrap)]
#[allow(clippy::too_many_arguments)]
async fn dispatch_uncompleted_chapters<OnStart, OnProgress, OnEnd>(
    app: &AppHandle,
    comic: &Comic,
    interval_sec: u64,
    batch_errors: &mut Vec<String>,
    task_context: &str,
    mut on_start: OnStart,
    mut on_progress: OnProgress,
    mut on_end: OnEnd,
) where
    OnStart: FnMut(usize),
    OnProgress: FnMut(usize),
    OnEnd: FnMut(),
{
    let chapter_infos: Vec<&ChapterInfo> = comic
        .chapter_infos
        .iter()
        .filter(|chapter_info| chapter_info.is_downloaded != Some(true))
        .collect();

    if chapter_infos.is_empty() {
        sleep(Duration::from_secs(interval_sec)).await;
        return;
    }

    on_start(chapter_infos.len());

    let download_manager = app.get_download_manager();
    for (idx, chapter_info) in chapter_infos.into_iter().enumerate() {
        let current = idx + 1;
        if let Err(err) =
            download_manager.create_download_task(comic.clone(), chapter_info.chapter_id)
        {
            tracing::error!(
                comic_id = comic.id,
                chapter_id = chapter_info.chapter_id,
                error = %err,
                "{task_context}失败"
            );
            batch_errors.push(format!(
                "`{}`的章节`{}`创建任务失败",
                comic.name, chapter_info.chapter_title
            ));
        }

        on_progress(current);
        sleep(Duration::from_millis(100)).await;
    }

    on_end();
    sleep(Duration::from_secs(interval_sec)).await;
}

fn format_batch_errors(action_name: &str, batch_errors: Vec<String>) -> CommandResult<()> {
    if !batch_errors.is_empty() {
        let failed_count = batch_errors.len();
        let examples = batch_errors
            .into_iter()
            .take(5)
            .collect::<Vec<_>>()
            .join("；");
        return Err(CommandError::from(
            action_name,
            anyhow!("共有{failed_count}项失败：{examples}"),
        ));
    }
    Ok(())
}

#[allow(clippy::cast_possible_wrap)]
#[tauri::command(async)]
#[specta::specta]
pub async fn download_all_favorites(app: AppHandle, folder_id: Option<i64>) -> CommandResult<()> {
    ensure_download_allowed(&app, "下载收藏夹失败")?;
    let _single_flight = SingleFlightGuard::acquire(&DOWNLOAD_ALL_FAVORITES_RUNNING)
        .ok_or_else(|| CommandError::from("下载整个收藏夹失败", anyhow!("已有同类任务正在运行")))?;
    let config = app.get_config();
    let jm_client = app.get_jm_client().inner().clone();
    let mut batch_errors = Vec::new();
    let default_cbz_mode = config.read().default_cbz_mode;

    let account_id = jm_client
        .session_snapshot()
        .profile
        .map(|profile| profile.uid)
        .ok_or_else(|| CommandError::from("下载收藏夹失败", anyhow!("请先登录")))?;
    let ensure_account = || {
        anyhow::ensure!(
            jm_client
                .session_snapshot()
                .profile
                .is_some_and(|profile| profile.uid == account_id),
            "账号已改变，已停止创建后续收藏下载任务"
        );
        Ok::<_, anyhow::Error>(())
    };
    let folder_id = folder_id.unwrap_or(0);
    let mut favorite_comics = Vec::new();
    let _ = DownloadAllFavoritesEvent::GetFavoritesStart.emit(&app);
    let first_page = jm_client
        .get_favorite_folder(folder_id, 1, FavoriteSort::FavoriteTime)
        .await
        .map_err(|err| CommandError::from("获取收藏夹失败", err))?;
    ensure_account().map_err(|err| CommandError::from("下载收藏夹中止", err))?;
    favorite_comics.extend(first_page.list);
    let total = first_page.total;
    if total <= 0 {
        tracing::info!("收藏夹为空，无待下载漫画");
        let _ = DownloadAllFavoritesEvent::GetComicsEnd.emit(&app);
        return Ok(());
    }
    let count = first_page.count;
    if count <= 0 {
        return Err(CommandError::from(
            "获取收藏夹失败",
            anyhow!("接口返回了非法的分页大小`{count}`"),
        ));
    }
    let page_count = total / count + i64::from(total % count != 0);
    let sem = Arc::new(Semaphore::new(5));
    let mut join_set = JoinSet::new();
    for page in 2..=page_count {
        let jm_client = jm_client.clone();
        let sem = sem.clone();
        join_set.spawn(async move {
            let _permit = sem.acquire().await?;
            let page_data = jm_client
                .get_favorite_folder(folder_id, page, FavoriteSort::FavoriteTime)
                .await?;
            Ok::<_, anyhow::Error>((page, page_data))
        });
    }
    let mut remaining_pages = Vec::new();
    while let Some(join_result) = join_set.join_next().await {
        let get_favorite_result = join_result
            .map_err(anyhow::Error::from)
            .map_err(|err| CommandError::from("获取收藏夹分页任务异常退出", err))?;
        let page = get_favorite_result.map_err(|err| CommandError::from("获取收藏夹失败", err))?;
        remaining_pages.push(page);
    }
    ensure_account().map_err(|err| CommandError::from("下载收藏夹中止", err))?;
    remaining_pages.sort_by_key(|(page, _)| *page);
    for (_, page) in remaining_pages {
        favorite_comics.extend(page.list);
    }
    let mut seen_ids = std::collections::HashSet::new();
    favorite_comics.retain(|comic| seen_ids.insert(comic.id));
    let total = favorite_comics.len() as i64;
    let interval_sec = config.read().download_all_favorites_interval_sec;
    for (i, comic) in favorite_comics.into_iter().enumerate() {
        ensure_account().map_err(|err| CommandError::from("下载收藏夹中止", err))?;
        let comic_title = &comic.name;
        let comic_id = comic.id;

        let comic = match utils::get_comic(app.clone(), comic_id).await {
            Ok(comic) => comic,
            Err(err) => {
                let err_title = format!("下载收藏夹过程中，获取漫画`{comic_title}`失败，已跳过");
                let err = err.context("服务端可能暂时限流，请稍后重试失败项目");
                let string_chain = err.to_string_chain();
                tracing::error!(err_title, message = string_chain);
                batch_errors.push(format!("获取漫画`{comic_title}`失败"));
                sleep(Duration::from_secs(interval_sec)).await;
                continue;
            }
        };

        ensure_account().map_err(|err| CommandError::from("下载收藏夹中止", err))?;
        app.state::<ArchiveIntentStore>()
            .set(comic.id, default_cbz_mode);
        let current = (i + 1) as i64;
        let _ = DownloadAllFavoritesEvent::GetComicsProgress { current, total }.emit(&app);

        dispatch_uncompleted_chapters(
            &app,
            &comic,
            interval_sec,
            &mut batch_errors,
            "批量创建收藏章节下载任务",
            |total_chapters| {
                let _ = DownloadAllFavoritesEvent::StartCreateDownloadTasks {
                    comic_id: comic.id,
                    comic_title: comic.name.clone(),
                    current: 0,
                    total: total_chapters as i64,
                }
                .emit(&app);
            },
            |cur| {
                let _ = DownloadAllFavoritesEvent::CreatingDownloadTask {
                    comic_id: comic.id,
                    current: cur as i64,
                }
                .emit(&app);
            },
            || {
                let _ = DownloadAllFavoritesEvent::EndCreateDownloadTasks { comic_id: comic.id }
                    .emit(&app);
            },
        )
        .await;
    }
    let _ = DownloadAllFavoritesEvent::GetComicsEnd.emit(&app);

    format_batch_errors("收藏夹下载任务创建不完整", batch_errors)?;

    Ok(())
}

#[allow(clippy::cast_possible_wrap)]
#[tauri::command(async)]
#[specta::specta]
pub async fn update_downloaded_comics(app: AppHandle) -> CommandResult<()> {
    ensure_download_allowed(&app, "更新本地库存失败")?;
    let _single_flight = SingleFlightGuard::acquire(&UPDATE_DOWNLOADED_COMICS_RUNNING)
        .ok_or_else(|| CommandError::from("更新本地库存失败", anyhow!("已有同类任务正在运行")))?;
    let config = app.get_config();
    let mut batch_errors = Vec::new();
    let default_cbz_mode = config.read().default_cbz_mode;

    let downloaded_comics: Vec<_> = super::library_commands::get_downloaded_comics(app.clone())
        .into_iter()
        .filter(|comic| comic.id > 0)
        .collect();

    let total = downloaded_comics.len() as i64;
    let interval_sec = config.read().update_downloaded_comics_interval_sec;
    let _ = UpdateDownloadedComicsEvent::GetComicStart { total }.emit(&app);

    for (i, downloaded_comic) in downloaded_comics.into_iter().enumerate() {
        let comic_title = &downloaded_comic.name;
        let comic_id = downloaded_comic.id;
        let current = (i + 1) as i64;
        let _ = UpdateDownloadedComicsEvent::GetComicProgress { current, total }.emit(&app);

        let comic = match utils::get_comic(app.clone(), comic_id)
            .await
            .context(format!("获取ID为`{comic_id}`的漫画失败"))
        {
            Ok(comic) => comic,
            Err(err) => {
                let err_title = format!("更新库存过程中，获取漫画`{comic_title}`失败，已跳过");
                let err = err.context("服务端可能暂时限流，请稍后重试失败项目");
                let string_chain = err.to_string_chain();
                tracing::error!(err_title, message = string_chain);
                batch_errors.push(format!("更新漫画`{comic_title}`失败"));
                sleep(Duration::from_secs(interval_sec)).await;
                continue;
            }
        };

        let has_downloaded_chapter = comic
            .chapter_infos
            .iter()
            .any(|chapter_info| chapter_info.is_downloaded == Some(true));

        if !has_downloaded_chapter {
            sleep(Duration::from_secs(interval_sec)).await;
            continue;
        }

        app.state::<ArchiveIntentStore>()
            .set(comic.id, default_cbz_mode);

        dispatch_uncompleted_chapters(
            &app,
            &comic,
            interval_sec,
            &mut batch_errors,
            "更新库存时创建章节下载任务",
            |total_chapters| {
                let _ = UpdateDownloadedComicsEvent::CreateDownloadTasksStart {
                    comic_id: comic.id,
                    comic_title: comic.name.clone(),
                    current: 0,
                    total: total_chapters as i64,
                }
                .emit(&app);
            },
            |cur| {
                let _ = UpdateDownloadedComicsEvent::CreateDownloadTaskProgress {
                    comic_id: comic.id,
                    current: cur as i64,
                }
                .emit(&app);
            },
            || {
                let _ = UpdateDownloadedComicsEvent::CreateDownloadTasksEnd { comic_id: comic.id }
                    .emit(&app);
            },
        )
        .await;
    }

    let _ = UpdateDownloadedComicsEvent::GetComicEnd.emit(&app);

    if let Some(cache) = app.try_state::<utils::DownloadedLibraryCache>() {
        cache.invalidate();
    }

    format_batch_errors("更新本地库存不完整", batch_errors)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::SingleFlightGuard;
    use std::sync::atomic::AtomicBool;

    #[test]
    fn single_flight_guard_rejects_overlap_and_releases_on_drop() {
        let flag = AtomicBool::new(false);
        let first = SingleFlightGuard::acquire(&flag).expect("first task should acquire guard");
        assert!(SingleFlightGuard::acquire(&flag).is_none());

        drop(first);
        assert!(SingleFlightGuard::acquire(&flag).is_some());
    }
}
