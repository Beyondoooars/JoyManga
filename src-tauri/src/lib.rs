use anyhow::Context;
use events::{
    DownloadAllFavoritesEvent, DownloadSpeedEvent, DownloadTaskEvent, ExportCbzEvent, LogEvent,
    UpdateDownloadedComicsEvent,
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
mod credential_cipher;
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
mod page_size;
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
            diagnose_library,
            delete_downloaded_comic,
            export_cbz,
            export_merged_cbz,
            get_logs_dir_size,
            export_logs,
            get_synced_comic,
            get_synced_comic_in_favorite,
            get_synced_comic_in_search,
            get_synced_comic_in_weekly,
            list_directory_entries,
            validate_download_dir,
            test_domain_latency,
            get_chapter_reader_data,
            set_network_metered,
            set_network_vpn,
            import_local_comics,
            remove_library_comic,
        ])
        .events(tauri_specta::collect_events![
            DownloadSpeedEvent,
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

/// 应用自身的页面地址：开发时是 dev 服务器，打包后是 tauri 协议
/// （Android 上 wry 会把它改写成 https://tauri.localhost）。只有这些地址才允许在窗口内导航。
fn is_internal_webview_url(url: &tauri::Url) -> bool {
    match url.scheme() {
        "tauri" | "about" | "data" | "blob" => true,
        // 打包后是 tauri.localhost，开发服务器是 localhost；自定义协议（jmimg 等）
        // 在 Android 上也会被改写成 *.localhost
        "http" | "https" => url
            .host_str()
            .is_some_and(|host| host == "localhost" || host.ends_with(".localhost")),
        _ => false,
    }
}

/// 把外部链接交给系统浏览器/默认应用打开；失败只记日志，不打断用户操作。
fn open_in_system_browser(app: &tauri::AppHandle, url: &tauri::Url) {
    use tauri_plugin_opener::OpenerExt;
    if let Err(error) = app.opener().open_url(url.as_str(), None::<&str>) {
        tracing::warn!(url = %url, %error, "无法交给系统打开链接");
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // 最先安装崩溃兜底：后续任何初始化步骤 panic 都能留下记录（release 无控制台）。
    // 此时配置目录尚未确定，先写系统临时目录；ConfigStorage 建好后改写到日志目录。
    crash::install(&std::env::temp_dir());

    let builder = ipc_builder();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        // 阅读器直接通过自定义协议取图，避免每页 base64 编码 + IPC 传输。
        // Android 上会被 wry 重写成 http://jmimg.localhost/<path>，CSP 需放行 https/http。
        .register_uri_scheme_protocol(
            commands::reader_commands::IMAGE_SCHEME,
            |context, request| {
                commands::reader_commands::serve_image(context.app_handle(), request)
            },
        )
        .invoke_handler(builder.invoke_handler())
        .setup(move |app| {
            builder.mount_events(app);

            let config_storage = ConfigStorage::new(app.handle())?;
            // 崩溃报告与滚动日志同目录（书库下的 logs），启动时书库路径还没从配置读出来，
            // 先放在配置目录，等 logger::init 时再改到书库；见下方 set_dir 调用。
            crash::set_dir(config_storage.logs_dir());
            // 封面缩略图缓存放在配置目录下，随应用数据一起管理
            cover_cache::set_dir(Some(config_storage.base_dir().join(".covers")));
            crate::state_transactions::recover(config_storage.config_path());
            // 提交替换 CBZ 前释放阅读器读句柄（持有替换锁后执行，见 atomic_fs）
            crate::commands::reader_commands::install_commit_hook();
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
            let app_handle = app.handle().clone();
            WebviewWindowBuilder::from_config(app.handle(), window_config)
                .context("读取主窗口配置失败")?
                // 兜底：应用里出现的其他站内跳转一律交给系统浏览器，避免整页被网页替换后回不来。
                // 导航回调运行在 WebView 主线程上，而 Android 的插件调用要等主线程处理
                // MainPipe，同步调用会自锁，所以放到后台线程再打开。
                .on_navigation(move |url| {
                    if is_internal_webview_url(url) {
                        return true;
                    }
                    let app_handle = app_handle.clone();
                    let external = url.clone();
                    std::thread::spawn(move || open_in_system_browser(&app_handle, &external));
                    false
                })
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

    #[test]
    fn webview_navigation_only_stays_inside_the_app() {
        // 设置页的 GitHub / LICENSE 链接交给系统浏览器；应用自身页面（开发服务器、
        // tauri 协议、Android 上的 *.localhost 改写）必须继续在窗口内导航。
        for internal in [
            "tauri://localhost",
            "https://tauri.localhost/index.html",
            "http://localhost:5005/",
            "http://jmimg.localhost/a.jpg",
            "about:blank",
        ] {
            assert!(
                super::is_internal_webview_url(&internal.parse().unwrap()),
                "{internal} 应视为应用内部地址"
            );
        }
        for external in [
            "https://github.com/Beyondoooars/JoyManga",
            "https://github.com/Beyondoooars/JoyManga/blob/main/LICENSE",
            "http://example.com/",
            "mailto:someone@example.com",
        ] {
            assert!(
                !super::is_internal_webview_url(&external.parse().unwrap()),
                "{external} 应交给系统浏览器打开"
            );
        }
    }
}
