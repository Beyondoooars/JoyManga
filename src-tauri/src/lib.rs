use anyhow::Context;
use events::{
    DownloadAllFavoritesEvent, DownloadSleepingEvent, DownloadSpeedEvent, DownloadTaskEvent,
    ExportCbzEvent, LogEvent, UpdateDownloadedComicsEvent,
};
use parking_lot::RwLock;
use tauri::{Manager, WebviewWindowBuilder, Wry};

// TODO: 用prelude来消除警告
use crate::commands::*;
use crate::config::{Config, ConfigStorage};
use crate::download_manager::DownloadManager;
use crate::jm_client::JmClient;

mod archive_finalize;
mod archive_intents;
mod atomic_fs;
mod comic_scanner;
mod commands;
mod config;
mod config_updates;
mod cover_cache;
mod crash;
mod domain_health;
mod download_manager;
mod download_queue;
mod download_scheduler;
mod errors;
mod events;
mod export;
mod extensions;
mod image_data;
mod jm_client;
mod library_catalog;
mod logger;
mod network_check;
mod network_state;
mod responses;
mod state_transactions;
mod types;
mod utils;
mod zip_limits;

fn generate_context() -> tauri::Context<Wry> {
    tauri::generate_context!()
}

fn ipc_builder() -> tauri_specta::Builder<Wry> {
    tauri_specta::Builder::<Wry>::new()
        .commands(tauri_specta::collect_commands![
            get_config,
            get_api_domain_presets,
            get_image_domain_presets,
            check_network,
            get_api_domain_status,
            initialize_android_storage,
            get_session,
            cancel_login,
            logout,
            set_favorite_comic,
            show_logs_in_file_manager,
            clear_logs,
            clear_cache,
            clear_all_downloads,
            reset_app_state,
            backup_app_data,
            restore_app_data,
            save_config,
            login,
            search,
            get_comic,
            get_favorite_folder,
            get_weekly_info,
            get_weekly,
            get_user_profile,
            create_download_task,
            get_download_tasks,
            promote_comic_archive,
            pause_download_task,
            resume_download_task,
            cancel_download_task,
            download_comic,
            download_all_favorites,
            update_downloaded_comics,
            show_path_in_file_manager,
            sync_favorite_folder,
            get_downloaded_comics,
            delete_downloaded_comic,
            export_cbz,
            export_merged_cbz,
            get_logs_dir_size,
            export_logs,
            get_synced_comic,
            get_synced_comic_in_favorite,
            get_synced_comic_in_search,
            get_synced_comic_in_weekly,
            test_domain_latency,
            get_chapter_reader_data,
            get_local_image_data_url,
            set_network_metered,
            import_local_comics,
            remove_library_comic,
        ])
        .events(tauri_specta::collect_events![
            DownloadSpeedEvent,
            DownloadSleepingEvent,
            DownloadTaskEvent,
            DownloadAllFavoritesEvent,
            UpdateDownloadedComicsEvent,
            ExportCbzEvent,
            LogEvent,
            crate::events::ConfigChangedEvent,
            crate::events::SessionChangedEvent,
            crate::events::FavoriteChangedEvent,
            crate::events::ComicArchiveEvent,
        ])
}

#[cfg(feature = "bindings")]
pub fn export_bindings(path: &std::path::Path) -> anyhow::Result<()> {
    ipc_builder()
        .export(
            specta_typescript::Typescript::default()
                .bigint(specta_typescript::BigIntExportBehavior::Number)
                .formatter(specta_typescript::formatter::prettier)
                .header("// @ts-nocheck"),
            path,
        )
        .map_err(|err| anyhow::anyhow!("{err}"))?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // 最先安装崩溃兜底：后续任何初始化步骤 panic 都能留下记录（release 无控制台）。
    // 此时配置目录尚未确定，先写系统临时目录；ConfigStorage 建好后改写到日志目录。
    crash::install(&std::env::temp_dir());

    let builder = ipc_builder();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(builder.invoke_handler())
        .setup(move |app| {
            builder.mount_events(app);

            let config_storage = ConfigStorage::new(app.handle())?;
            // 配置目录确定后，把崩溃报告从临时目录改到日志目录，便于用户随日志一起反馈
            crash::set_dir(config_storage.logs_dir());
            // 封面缩略图缓存放在配置目录下，随应用数据一起管理
            cover_cache::set_dir(Some(config_storage.base_dir().join(".covers")));
            crate::state_transactions::recover(config_storage.config_path());
            let intent_store_path = config_storage
                .config_path()
                .with_file_name("archive-intents.json");
            let config = RwLock::new(Config::new(&config_storage)?);
            let download_dir = config.read().download_dir.clone();
            let _ = std::fs::create_dir_all(&download_dir);
            tauri::async_runtime::spawn_blocking(move || {
                if let Err(err) = atomic_fs::cleanup_residual_trash(&download_dir) {
                    tracing::warn!(error = %err, "清理残留隔离目录失败");
                }
            });
            app.manage(config_storage);
            app.manage(config);
            app.manage(crate::utils::DownloadedLibraryCache::new());
            app.manage(crate::archive_intents::ArchiveIntentStore::load(
                intent_store_path,
            )?);
            app.manage(crate::archive_finalize::ArchiveFinalizeState::default());

            let jm_client = JmClient::new(app.handle().clone());
            app.manage(jm_client);

            // 启动后后台刷新最新 API 域名与 APP 接口版本号（失败静默，不阻塞启动）
            let jm_client_for_refresh = app.state::<JmClient>().inner().clone();
            tauri::async_runtime::spawn(async move {
                jm_client_for_refresh.refresh_api_domains().await;
                jm_client_for_refresh.refresh_app_version().await;
            });

            logger::init(app.handle())?;
            let download_manager = DownloadManager::new(app.handle().clone())?;
            app.manage(download_manager);
            app.state::<DownloadManager>().restore_pending();

            let window_config = app.config().app.windows.first().context("缺少主窗口配置")?;
            WebviewWindowBuilder::from_config(app.handle(), window_config)
                .context("读取主窗口配置失败")?
                .build()
                .context("创建主窗口失败")?;

            Ok(())
        })
        .run(generate_context())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    #[test]
    fn ipc_contract_exports_recoverable_tasks_and_failure_details() {
        let config = specta_typescript::Typescript::default()
            .bigint(specta_typescript::BigIntExportBehavior::Number);
        let snapshot =
            specta_typescript::export::<super::download_queue::DownloadTaskSnapshot>(&config)
                .unwrap();
        let event = specta_typescript::export::<super::events::DownloadTaskEvent>(&config).unwrap();
        let normalize = |source: &str| {
            source
                .chars()
                .filter(|c| !c.is_whitespace() && *c != ';')
                .collect::<String>()
        };
        let bindings = normalize(include_str!("../../src/bindings.ts"));
        assert!(
            bindings.contains(&normalize(&snapshot)),
            "快照类型需要重新生成：{snapshot}"
        );
        assert!(
            bindings.contains(&normalize(&event)),
            "事件类型需要重新生成：{event}"
        );
    }
}
