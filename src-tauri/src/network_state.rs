//! 网络计费状态（用于"仅 Wi-Fi 下载"）。
//!
//! Android 上由 Kotlin 侧通过 `ConnectivityManager` 判断当前网络是否计费
//! （移动数据/热点为计费，Wi-Fi 通常不计费），并在变化时推送；
//! 非 Android 环境（开发/测试）一律视为不计费。

use std::sync::atomic::{AtomicBool, Ordering};

/// 当前网络是否按流量计费。默认 false（未知情况放行）。
static METERED: AtomicBool = AtomicBool::new(false);
/// 是否已经收到过明确网络状态。未收到时不要拦截，
/// 否则应用刚启动、事件还没到时会把所有下载挡掉。
static KNOWN: AtomicBool = AtomicBool::new(false);

pub fn set_metered(metered: bool) {
    METERED.store(metered, Ordering::Relaxed);
    KNOWN.store(true, Ordering::Relaxed);
}

pub fn is_metered() -> bool {
    METERED.load(Ordering::Relaxed)
}

pub fn is_known() -> bool {
    KNOWN.load(Ordering::Relaxed)
}

/// 结合用户设置判断此刻是否允许下载。
/// - 未开启"仅 Wi-Fi"：永远允许；
/// - 开启但网络状态未知：允许（宁可放行，也不要让用户以为下载功能坏了）；
/// - 已知为计费网络：拒绝，并返回可展示的原因。
pub fn download_allowed(only_wifi: bool) -> Result<(), String> {
    if !only_wifi || !is_known() || !is_metered() {
        return Ok(());
    }
    Err("当前是移动数据/计费网络，已按设置暂停下载。连接 Wi-Fi 后重试，或在设置中关闭「仅 Wi-Fi 下载」".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_wifi_blocks_metered_network() {
        set_metered(true);
        assert!(download_allowed(false).is_ok());
        let error = download_allowed(true).unwrap_err();
        assert!(error.contains("仅 Wi-Fi"));
    }

    #[test]
    fn unmetered_network_always_allowed() {
        set_metered(false);
        assert!(download_allowed(true).is_ok());
    }
}
