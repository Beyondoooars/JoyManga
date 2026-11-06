//! 运行期网络状态：是否计费（用于"仅 Wi-Fi 下载"）与是否处于 VPN。
//!
//! Android 上由 Kotlin 侧通过 `ConnectivityManager` 判断并在变化时推送；
//! 非 Android 环境（开发/测试）一律视为不计费、无 VPN。

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use parking_lot::{Mutex, RwLock};

/// 当前网络是否按流量计费。默认 false（未知情况放行）。
static METERED: AtomicBool = AtomicBool::new(false);
/// 是否已经收到过明确网络状态。未收到时不要拦截，
/// 否则应用刚启动、事件还没到时会把所有下载挡掉。
static KNOWN: AtomicBool = AtomicBool::new(false);
/// 当前默认网络是否由 VPN 承载。
static VPN: AtomicBool = AtomicBool::new(false);
/// 平台给出的默认网络标识（Wi-Fi/蜂窝的具体网络）。仅用于识别“换了网络”，
/// 不写入日志，也不参与代理选择。
static NETWORK_KEY: RwLock<Option<String>> = RwLock::new(None);
/// 快照写入锁：VPN 标志与网络标识必须成对更新，避免两次并发推送交错出
/// 从未存在过的 (VPN, key) 组合。
static NETWORK_SNAPSHOT_LOCK: Mutex<()> = Mutex::new(());
/// 网络路径代际。VPN、手动代理或默认网络变化时递增；请求在启动时记录代际，
/// 写回线路健康状态前核对，避免旧路径的结果污染新路径。
static NETWORK_EPOCH: AtomicU64 = AtomicU64::new(0);
/// 代际校验与状态写回必须和“推进代际并清理”互斥，避免检查后的竞态窗口。
static NETWORK_EPOCH_LOCK: RwLock<()> = RwLock::new(());

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
    VPN.swap(active, Ordering::SeqCst) != active
}

/// 同时更新 VPN 标志与默认网络标识，返回网络路径是否发生变化。
///
/// 只翻转 VPN 布尔值不够：Wi-Fi 与蜂窝网络互切时 VPN 可能都是 false，
/// 但连接池和线路健康状态同样不能跨网络复用。`network_key` 为 `None`
/// 表示平台未提供标识（旧版原生层），此时只比较 VPN 状态。
/// 首次拿到标识只建立基线，不视为换网：冷启动不应因此重建客户端。
pub fn set_network_snapshot(active: bool, network_key: Option<&str>) -> bool {
    let _snapshot_guard = NETWORK_SNAPSHOT_LOCK.lock();
    let vpn_changed = set_vpn(active);
    let key_changed = match network_key {
        Some(key) => {
            let next = key.trim();
            let next = (!next.is_empty()).then(|| next.to_string());
            let mut current = NETWORK_KEY.write();
            let known = current.is_some();
            if *current == next {
                false
            } else if !known {
                // 首个快照只记录基线；旧版原生没有 key，不能把“第一次拿到标识”当成换网。
                *current = next;
                false
            } else {
                *current = next;
                true
            }
        }
        None => false,
    };
    vpn_changed || key_changed
}

pub fn is_vpn_active() -> bool {
    VPN.load(Ordering::SeqCst)
}

/// 网络路径代际快照。请求开始时记录，写回健康状态前用
/// [`epoch_is_current`] 核对。
pub fn network_epoch() -> u64 {
    NETWORK_EPOCH.load(Ordering::SeqCst)
}

/// 原子推进代际并清理旧路径状态。清理期间旧请求不能完成健康写回。
pub fn change_network_path(clear: impl FnOnce()) {
    let _guard = NETWORK_EPOCH_LOCK.write();
    NETWORK_EPOCH.fetch_add(1, Ordering::SeqCst);
    clear();
}

/// 仅在代际仍有效时执行写回；校验和写入不能被网络切换插入。
pub fn with_current_epoch<T>(epoch: u64, write_back: impl FnOnce() -> T) -> Option<T> {
    let _guard = NETWORK_EPOCH_LOCK.read();
    if epoch_is_current(epoch) {
        Some(write_back())
    } else {
        None
    }
}

/// 结果是否仍属于当前网络路径。返回 false 时不得写回线路健康状态或
/// 最近成功线路。
pub fn epoch_is_current(epoch: u64) -> bool {
    network_epoch() == epoch
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
    use std::time::Duration;

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

    #[test]
    fn network_snapshot_detects_key_and_vpn_changes() {
        let _guard = STATE_TEST_LOCK.lock();
        *NETWORK_KEY.write() = None;
        let _ = set_vpn(false);
        // 首个快照只建立基线，不算网络变化。
        assert!(!set_network_snapshot(false, Some("test-wifi-a")));
        assert!(!set_network_snapshot(false, Some("test-wifi-a")));
        // Wi-Fi ↔ 蜂窝：VPN 标志不变，但默认网络已切换
        assert!(set_network_snapshot(false, Some("test-cell-b")));
        // 未提供标识时只比较 VPN
        assert!(set_network_snapshot(true, None));
        assert!(!set_network_snapshot(true, None));
        assert!(set_network_snapshot(false, None));
        assert!(!set_network_snapshot(false, None));
        *NETWORK_KEY.write() = None;
    }

    #[test]
    fn stale_epoch_is_rejected_after_network_change() {
        let _guard = STATE_TEST_LOCK.lock();
        let snapshot = network_epoch();
        change_network_path(|| {});
        assert!(!epoch_is_current(snapshot));
        assert!(epoch_is_current(network_epoch()));
    }

    #[test]
    fn network_change_waits_for_current_epoch_write_back() {
        let _guard = STATE_TEST_LOCK.lock();
        let epoch = network_epoch();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let write_back = std::thread::spawn(move || {
            with_current_epoch(epoch, || {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            })
        });
        entered_rx.recv().unwrap();
        let (changed_tx, changed_rx) = std::sync::mpsc::channel();
        let change = std::thread::spawn(move || {
            change_network_path(|| {});
            changed_tx.send(()).unwrap();
        });
        assert!(changed_rx.recv_timeout(Duration::from_millis(30)).is_err());
        release_tx.send(()).unwrap();
        assert_eq!(write_back.join().unwrap(), Some(()));
        change.join().unwrap();
        assert!(!epoch_is_current(epoch));
    }
}
