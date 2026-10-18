//! 网络计费状态上报。
//!
//! 屏幕常亮完全由 Android 原生层的 window flag 处理（见 `MainActivity.setKeepScreenOn`），
//! 不再需要 Rust 命令。

use crate::errors::CommandResult;

/// Android 侧把当前网络是否计费推给后端。
/// 前端在启动时与网络变化时各调一次，后端据此判断"仅 Wi-Fi 下载"。
#[tauri::command(async)]
#[specta::specta]
pub fn set_network_metered(metered: bool) -> CommandResult<()> {
    crate::network_state::set_metered(metered);
    tracing::debug!(metered, "网络计费状态已更新");
    Ok(())
}
