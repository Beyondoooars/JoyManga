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
    use std::sync::mpsc::{self, Sender};
    use std::sync::OnceLock;

    #[link(name = "kernel32")]
    extern "system" {
        fn SetThreadExecutionState(flags: u32) -> u32;
    }

    const ES_CONTINUOUS: u32 = 0x8000_0000;
    const ES_DISPLAY_REQUIRED: u32 = 0x0000_0002;

    /// 记录当前是否已置位，避免重复调用（每次调用都会重置系统计时器）。
    static ACTIVE: AtomicBool = AtomicBool::new(false);
    /// 专用常亮线程的命令通道。
    ///
    /// `SetThreadExecutionState` 是**线程级**状态：只影响调用它的那个线程，
    /// 而 Tauri 的 async 命令运行在会复用/回收的 tokio 工作线程上。若直接在
    /// 命令里调用，置位后线程一换就失效。因此固定用一个常驻线程执行。
    static KEEP_AWAKE_TX: OnceLock<Sender<bool>> = OnceLock::new();

    fn channel() -> &'static Sender<bool> {
        KEEP_AWAKE_TX.get_or_init(|| {
            let (tx, rx) = mpsc::channel::<bool>();
            std::thread::Builder::new()
                .name("keep-screen-on".into())
                .spawn(move || {
                    // 线程创建后先确保 ES_CONTINUOUS 已置位，后续只切换显示器保持位。
                    apply_on_this_thread(true, false);
                    while let Ok(enabled) = rx.recv() {
                        apply_on_this_thread(enabled, true);
                    }
                    // 退出时恢复系统默认息屏策略
                    apply_on_this_thread(false, false);
                })
                .expect("创建常亮线程失败");
            tx
        })
    }

    fn apply_on_this_thread(enabled: bool, track: bool) {
        if track {
            let already = ACTIVE.swap(enabled, Ordering::SeqCst);
            if already == enabled {
                return;
            }
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

    pub fn apply(enabled: bool) {
        // 通道发送失败说明常亮线程已退出（不应发生）；此时退化为直接调用，
        // 至少保证当前线程不必等锁。
        if channel().send(enabled).is_err() {
            apply_on_this_thread(enabled, true);
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
