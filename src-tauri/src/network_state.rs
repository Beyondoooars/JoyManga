//! 运行期网络状态：是否计费（用于"仅 Wi-Fi 下载"）与是否处于 VPN。
//!
//! Android 上由 Kotlin 侧通过 `ConnectivityManager` 判断并在变化时推送；
//! 非 Android 环境（开发/测试）一律视为不计费、无 VPN。

use std::sync::atomic::{AtomicBool, Ordering};

/// 当前网络是否按流量计费。默认 false（未知情况放行）。
static METERED: AtomicBool = AtomicBool::new(false);
/// 是否已经收到过明确网络状态。未收到时不要拦截，
/// 否则应用刚启动、事件还没到时会把所有下载挡掉。
static KNOWN: AtomicBool = AtomicBool::new(false);
/// 当前默认网络是否由 VPN 承载。
static VPN: AtomicBool = AtomicBool::new(false);

/// 测试串行锁：这两个开关是进程级全局状态，而 `cargo test` 默认并行执行用例。
/// 本模块与 `config` 里的代理选择用例都必须持有它，否则会读到对方刚设置的值。
#[cfg(test)]
pub(crate) static STATE_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

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

/// 更新 VPN 状态，返回是否发生了变化。
///
/// 变化时调用方要重建 HTTP 客户端：`Config::apply_proxy` 会据此决定是否跳过手动代理。
pub fn set_vpn(active: bool) -> bool {
    VPN.swap(active, Ordering::Relaxed) != active
}

pub fn is_vpn_active() -> bool {
    VPN.load(Ordering::Relaxed)
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
        let _guard = STATE_TEST_LOCK.lock();
        set_metered(true);
        assert!(download_allowed(false).is_ok());
        let error = download_allowed(true).unwrap_err();
        assert!(error.contains("仅 Wi-Fi"));
    }

    #[test]
    fn unmetered_network_always_allowed() {
        let _guard = STATE_TEST_LOCK.lock();
        set_metered(false);
        assert!(download_allowed(true).is_ok());
    }

    #[test]
    fn vpn_flag_reports_only_real_transitions() {
        let _guard = STATE_TEST_LOCK.lock();
        assert!(set_vpn(true));
        assert!(!set_vpn(true));
        assert!(is_vpn_active());
        assert!(set_vpn(false));
        assert!(!is_vpn_active());
    }
}
