//! 网络计费状态上报。
//!
//! 屏幕常亮完全由 Android 原生层的 window flag 处理（见 `MainActivity.setKeepScreenOn`），
//! 不再需要 Rust 命令。

use crate::errors::CommandResult;
use crate::extensions::AppHandleExt;

/// Android 侧把当前网络是否计费推给后端。
/// 前端在启动时与网络变化时各调一次，后端据此判断"仅 Wi-Fi 下载"。
#[tauri::command(async)]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub fn set_network_metered(app: tauri::AppHandle, metered: bool) -> CommandResult<()> {
    crate::network_state::set_metered(metered);
    tracing::debug!(metered, "网络计费状态已更新");
    if metered && app.get_config().read().download_only_wifi {
        let paused = app
            .get_download_manager()
            .pause_active_tasks_for_metered_network();
        if paused > 0 {
            tracing::info!(
                paused,
                "已切换至计费网络且开启仅 Wi-Fi 下载，自动暂停在途下载任务"
            );
        }
    }
    Ok(())
}

/// Android 侧把当前默认网络是否由 VPN 承载、以及默认网络标识推给后端。
///
/// 手动 HTTP 代理只在没有 VPN 时生效（见 `Config::effective_proxy_url`），
/// 所以状态一旦变化就必须重建已缓存的 HTTP 客户端，否则要等到下次改设置。
/// `network_key` 用来识别 Wi-Fi 与蜂窝网络互切：这种切换 VPN 布尔值通常不变，
/// 但旧连接池与线路健康状态同样不能跨网络复用。
#[tauri::command(async)]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub fn set_network_vpn(
    app: tauri::AppHandle,
    active: bool,
    network_key: Option<String>,
) -> CommandResult<()> {
    if crate::network_state::set_network_snapshot(active, network_key.as_deref()) {
        // 不记录 network_key 具体值：它可能包含系统网络标识。
        tracing::info!(active, "网络路径已变化，重建 HTTP 客户端并取消在途登录");
        app.get_jm_client().cancel_login_for_network_change();
        app.get_jm_client().reload_client_for_network_change(true);
    }
    Ok(())
}
