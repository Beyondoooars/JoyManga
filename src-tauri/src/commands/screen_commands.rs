//! 阅读时保持屏幕常亮，以及网络计费状态上报。

use crate::errors::CommandResult;

/// 打开/关闭常亮。`enabled` 为 false 时恢复系统默认息屏行为。
#[tauri::command(async)]
#[specta::specta]
pub fn set_keep_screen_on(enabled: bool) -> CommandResult<()> {
    #[cfg(target_os = "windows")]
    {
        imp::apply(enabled);
    }
    #[cfg(not(target_os = "windows"))]
    {
        // Android 由原生层处理，其他平台不承诺；这里保持 no-op 以免前端需要分支。
        let _ = enabled;
    }
    Ok(())
}

/// Android 侧把当前网络是否计费推给后端（桌面端不会调用）。
/// 前端在启动时与网络变化时各调一次，后端据此判断"仅 Wi-Fi 下载"。
#[tauri::command(async)]
#[specta::specta]
pub fn set_network_metered(metered: bool) -> CommandResult<()> {
    crate::network_state::set_metered(metered);
    tracing::debug!(metered, "网络计费状态已更新");
    Ok(())
}

#[cfg(target_os = "windows")]
mod imp {
    use std::sync::atomic::{AtomicBool, Ordering};

    #[link(name = "kernel32")]
    extern "system" {
        fn SetThreadExecutionState(flags: u32) -> u32;
    }

    const ES_CONTINUOUS: u32 = 0x8000_0000;
    const ES_DISPLAY_REQUIRED: u32 = 0x0000_0002;

    /// 记录当前是否已置位，避免重复调用（每次调用都会重置系统计时器）。
    static ACTIVE: AtomicBool = AtomicBool::new(false);

    pub fn apply(enabled: bool) {
        let already = ACTIVE.swap(enabled, Ordering::SeqCst);
        if already == enabled {
            return;
        }
        let flags = if enabled {
            ES_CONTINUOUS | ES_DISPLAY_REQUIRED
        } else {
            ES_CONTINUOUS
        };
        // SAFETY: SetThreadExecutionState 只接受位标志，无指针参数，失败仅返回 0。
        unsafe {
            SetThreadExecutionState(flags);
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn command_accepts_both_states() {
        // 非 Windows 平台是 no-op；Windows 上也是幂等的位标志设置，可在 CI 安全调用。
        assert!(super::set_keep_screen_on(true).is_ok());
        assert!(super::set_keep_screen_on(false).is_ok());
    }
}
