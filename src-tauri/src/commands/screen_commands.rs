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
pub fn set_network_metered(metered: bool) -> CommandResult<()> {
    crate::network_state::set_metered(metered);
    tracing::debug!(metered, "网络计费状态已更新");
    Ok(())
}

/// Android 侧把当前默认网络是否由 VPN 承载推给后端。
///
/// 手动 HTTP 代理只在没有 VPN 时生效（见 `Config::effective_proxy_url`），
/// 所以状态一旦变化就必须重建已缓存的 HTTP 客户端，否则要等到下次改设置。
#[tauri::command(async)]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub fn set_network_vpn(app: tauri::AppHandle, active: bool) -> CommandResult<()> {
    if crate::network_state::set_vpn(active) {
        tracing::info!(active, "VPN 状态已变化，重建 HTTP 客户端");
        app.get_jm_client().reload_client();
    }
    Ok(())
}
