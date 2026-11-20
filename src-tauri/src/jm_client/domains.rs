use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Context;
use tauri::{AppHandle, Manager};

use crate::config::{ApiDomainMode, ConfigStorage};
use crate::extensions::AppHandleExt;

use super::crypto::decrypt_domain_list_payload;
use super::{timed_client_builder, JmClient};

/// 动态更新移动端 API 域名的官方地址（返回 AES 加密的域名列表）
const API_DOMAIN_UPDATE_URLS: [&str; 3] = [
    "https://rup4a04-c01.tos-ap-southeast-1.bytepluses.com/newsvr-2025.txt",
    "https://rup4a04-c02.tos-cn-hongkong.bytepluses.com/newsvr-2025.txt",
    "https://rup4a04-c03.tos-cn-beijing.bytepluses.com.cn/newsvr-2025.txt",
];
/// 动态域名刷新节流间隔
const API_DOMAIN_REFRESH_INTERVAL: Duration = Duration::from_secs(600);
/// 域名列表保鲜间隔：运行中超过该间隔未刷新时，下一次请求会顺带触发后台刷新
const API_DOMAIN_STALE_REFRESH_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
/// 动态域名缓存文件名（与配置文件同目录，启动时先读取，刷新成功后覆盖）
const API_DOMAINS_CACHE_FILE: &str = ".api-domains.json";
/// 缓存中最多采纳的域名条目数量（防御损坏文件里的超长列表）
const API_DOMAINS_CACHE_MAX_ENTRIES: usize = 16;

impl JmClient {
    /// 从官方地址动态获取最新的移动端 API 域名列表（10 分钟内只刷新一次）。
    /// 失败时保留已有列表并静默降级为内置预设。
    pub async fn refresh_api_domains(&self) -> Vec<String> {
        {
            let mut last = self.domain_refresh_at.write();
            let now = std::time::Instant::now();
            if let Some(previous) = *last {
                if now.duration_since(previous) < API_DOMAIN_REFRESH_INTERVAL {
                    return self.updated_api_domains.read().clone();
                }
            }
            *last = Some(now);
        }

        let builder = self.app.get_config().read().apply_proxy_best_effort(
            timed_client_builder(Duration::from_secs(12)),
            "JmClient域名更新",
        );
        let Ok(client) = builder.build() else {
            return self.updated_api_domains.read().clone();
        };

        for url in API_DOMAIN_UPDATE_URLS {
            match fetch_domain_list(&client, url).await {
                Ok(domains) if !domains.is_empty() => {
                    tracing::info!(?domains, "已获取最新的API域名列表");
                    *self.updated_api_domains.write() = domains.clone();
                    let updated_at_unix = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map_or(0, |elapsed| elapsed.as_secs());
                    *self.domain_list_updated_at.write() = Some(updated_at_unix);
                    save_cached_api_domains(&self.app, &domains, updated_at_unix);
                    return domains;
                }
                Ok(_) => {
                    tracing::warn!(url, "域名更新载荷为空，尝试下一个地址");
                }
                Err(error) => {
                    tracing::warn!(url, error = %error, "获取API域名更新失败，尝试下一个地址");
                }
            }
        }
        // 全部失败：回滚节流时间戳，网络恢复后的下一次请求会立即重试，而不是再等 10 分钟
        *self.domain_refresh_at.write() = None;
        self.updated_api_domains.read().clone()
    }

    /// 全部候选线路失败后调用：后台静默重新解析域名列表，供后续请求与自动重试使用。
    /// 10 分钟节流内的重复调用只做快速检查，不产生额外网络请求。
    pub(super) fn spawn_domain_refresh_in_background(&self) {
        let throttled = self
            .domain_refresh_at
            .read()
            .is_some_and(|previous| previous.elapsed() < API_DOMAIN_REFRESH_INTERVAL);
        if throttled {
            return;
        }
        tracing::debug!("后台静默重新解析动态域名列表");
        let client = self.clone();
        tauri::async_runtime::spawn(async move {
            client.refresh_api_domains().await;
        });
    }

    /// 距上次刷新超过保鲜间隔（含从未刷新）时，触发一次后台静默刷新。
    fn spawn_domain_refresh_if_stale(&self) {
        let stale = self
            .domain_refresh_at
            .read()
            .is_none_or(|previous| previous.elapsed() >= API_DOMAIN_STALE_REFRESH_INTERVAL);
        if stale {
            self.spawn_domain_refresh_in_background();
        }
    }

    /// 动态域名池状态：缓存中的域名数量与最近一次成功更新的 Unix 秒时间戳。
    /// 供设置页“检查连接”展示，不发起任何网络请求。
    pub fn domain_pool_status(&self) -> (usize, Option<u64>) {
        (
            self.updated_api_domains.read().len(),
            *self.domain_list_updated_at.read(),
        )
    }

    /// 当前线路的健康摘要：连续失败次数与是否处于冷却。
    /// 只读内存状态，供设置页诊断使用。
    pub fn domain_health_status(&self, domain: &str) -> (u32, bool) {
        (
            self.domain_health.failure_count(domain),
            self.domain_health.is_cooling_down(domain),
        )
    }

    /// 当前请求的候选线路（按顺序尝试）：
    /// 最近成功的线路 → 配置线路 → 动态获取的最新域名 → 内置预设。
    /// 自定义线路不参与自动切换。
    pub(super) fn api_domain_candidates(&self) -> Vec<String> {
        // 域名列表长时间未刷新时顺带触发后台保鲜；不会阻塞当前请求。
        self.spawn_domain_refresh_if_stale();
        let config = self.app.get_config();
        let config = config.read();
        let current = config.get_api_domain();
        if config.api_domain_mode == ApiDomainMode::Custom {
            return vec![current];
        }
        let mut list: Vec<String> = Vec::new();
        if let Some(last) = self.last_success_domain.read().clone() {
            push_unique(&mut list, &last);
        }
        // 配置线路已确认失效（停放页/拦截/连不上）时不要继续排在动态域名前面：
        // 每条请求都先撞它一次，用户要多吃一次往返甚至一次 12 秒超时。
        if !self.domain_health.is_failing(&current) {
            push_unique(&mut list, &current);
        }
        for domain in self.updated_api_domains.read().iter() {
            push_unique(&mut list, domain);
        }
        // 放在动态域名之后：实测失效的配置线路仍保留为兜底，但不再优先。
        push_unique(&mut list, &current);
        for mode in [
            ApiDomainMode::Domain2,
            ApiDomainMode::Domain3,
            ApiDomainMode::Domain4,
            ApiDomainMode::Domain1,
            ApiDomainMode::Domain5,
        ] {
            if let Some(preset) = mode.preset_domain() {
                push_unique(&mut list, preset);
            }
        }
        // 健康度排序：冷却中的线路排到最后，其余快的优先。
        // 首位（最近成功的线路 / 用户指定线路）保持不动，只重排其余候选。
        let mut sorted_tail = list[1..].to_vec();
        self.domain_health.sort_candidates(&mut sorted_tail);
        list.truncate(1);
        list.extend(sorted_tail);
        // 候选上限。原先只留 4 条，而动态域名列表本身就有 4 条，加上配置线路后
        // 被截断，官方刚下发的新线路反而试不到（旧线路全部过期时会一直失败）。
        // 放宽到 8 条：读接口换线是顺序尝试，实际很少走满。
        list.truncate(8);
        list
    }
}

fn push_unique(list: &mut Vec<String>, domain: &str) {
    let domain = domain.trim();
    if !domain.is_empty() && !list.iter().any(|existing| existing == domain) {
        list.push(domain.to_string());
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct ApiDomainsCacheFile {
    domains: Vec<String>,
    #[serde(default)]
    updated_at_unix: u64,
}

fn api_domains_cache_path(app: &AppHandle) -> Option<std::path::PathBuf> {
    let storage = app.try_state::<ConfigStorage>()?;
    Some(storage.config_path().with_file_name(API_DOMAINS_CACHE_FILE))
}

/// 读取本地缓存的动态域名；文件缺失或解析失败时静默返回空列表。
pub(super) fn load_cached_api_domains(app: &AppHandle) -> (Vec<String>, Option<u64>) {
    let Some(path) = api_domains_cache_path(app) else {
        return (Vec::new(), None);
    };
    match std::fs::read_to_string(&path) {
        Ok(contents) => parse_cached_domains(&contents),
        Err(error) => {
            tracing::debug!(error = %error, "读取动态域名缓存失败，忽略");
            (Vec::new(), None)
        }
    }
}

/// 解析缓存文件内容：过滤异常条目、去重并限制数量；时间戳缺失视为未知。
fn parse_cached_domains(contents: &str) -> (Vec<String>, Option<u64>) {
    let Ok(cache) = serde_json::from_str::<ApiDomainsCacheFile>(contents) else {
        return (Vec::new(), None);
    };
    let mut domains = Vec::new();
    for domain in cache.domains.iter().take(API_DOMAINS_CACHE_MAX_ENTRIES) {
        if is_plausible_domain(domain) {
            push_unique(&mut domains, domain);
        }
    }
    let updated_at = (cache.updated_at_unix > 0).then_some(cache.updated_at_unix);
    (domains, updated_at)
}

pub(crate) fn is_plausible_domain(domain: &str) -> bool {
    let domain = domain.trim();
    !domain.is_empty()
        && domain.len() <= 253
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && domain
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.'))
}

/// 将最新域名列表写入本地缓存（失败仅记录日志，不影响使用）。
fn save_cached_api_domains(app: &AppHandle, domains: &[String], updated_at_unix: u64) {
    let Some(path) = api_domains_cache_path(app) else {
        return;
    };
    let cache = ApiDomainsCacheFile {
        domains: domains.to_vec(),
        updated_at_unix,
    };
    let Ok(bytes) = serde_json::to_vec_pretty(&cache) else {
        return;
    };
    if let Err(error) = crate::atomic_fs::write(&path, bytes) {
        tracing::warn!(error = %error, "写入动态域名缓存失败");
    }
}

pub(super) fn preset_mode_for_domain(domain: &str) -> Option<ApiDomainMode> {
    [
        ApiDomainMode::Domain1,
        ApiDomainMode::Domain2,
        ApiDomainMode::Domain3,
        ApiDomainMode::Domain4,
        ApiDomainMode::Domain5,
    ]
    .into_iter()
    .find(|mode| mode.preset_domain() == Some(domain))
}

/// 请求官方域名更新地址并解密，提取 JSON 中的 Server 字段。
async fn fetch_domain_list(client: &reqwest::Client, url: &str) -> anyhow::Result<Vec<String>> {
    let resp = client
        .get(url)
        .send()
        .await
        .context("请求域名更新地址失败")?;
    anyhow::ensure!(resp.status().is_success(), "HTTP状态码`{}`", resp.status());
    let text = resp.text().await.context("读取域名更新载荷失败")?;
    let decrypted = decrypt_domain_list_payload(&text)?;
    let value: serde_json::Value =
        serde_json::from_slice(&decrypted).context("解析域名更新载荷失败")?;
    let domains = value
        .get("Server")
        .and_then(|servers| servers.as_array())
        .map(|servers| {
            servers
                .iter()
                .filter_map(|server| server.as_str())
                .map(str::trim)
                .filter(|server| is_plausible_domain(server))
                .take(API_DOMAINS_CACHE_MAX_ENTRIES)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Ok(domains)
}

#[cfg(test)]
mod tests {
    use super::{
        is_plausible_domain, parse_cached_domains, push_unique, API_DOMAINS_CACHE_MAX_ENTRIES,
    };

    #[test]
    fn plausible_domain_rejects_url_components() {
        assert!(is_plausible_domain("www.cdnhth.cc"));
        assert!(is_plausible_domain(" cdn-msp2.example.com "));
        assert!(!is_plausible_domain("example.com/path"));
        assert!(!is_plausible_domain("user@example.com"));
        assert!(!is_plausible_domain("example.com:8443"));
        assert!(!is_plausible_domain(".example.com"));
        assert!(!is_plausible_domain("localhost"));
    }

    #[test]
    fn candidate_list_keeps_order_dedups_and_caps_length() {
        let mut list: Vec<String> = Vec::new();
        push_unique(&mut list, "a.example");
        push_unique(&mut list, "b.example");
        push_unique(&mut list, "a.example");
        push_unique(&mut list, "  ");
        assert_eq!(list, vec!["a.example", "b.example"]);
    }

    #[test]
    fn cached_domains_parse_filters_invalid_entries_and_deduplicates() {
        let raw = r#"{"domains":["a.example.com","bad domain","nodot","a.example.com","  ok.example.net  "],"updated_at_unix":1700000000}"#;
        let (domains, updated_at) = parse_cached_domains(raw);
        assert_eq!(domains, vec!["a.example.com", "ok.example.net"]);
        assert_eq!(updated_at, Some(1700000000));
    }

    #[test]
    fn cached_domains_parse_rejects_invalid_payloads() {
        assert!(parse_cached_domains("not json").0.is_empty());
        assert!(parse_cached_domains(r#"{"domains":["a.example.com"]}"#)
            .1
            .is_none());
        assert!(
            parse_cached_domains(r#"{"domains":["a.example.com"],"updated_at_unix":0}"#)
                .1
                .is_none()
        );
    }

    #[test]
    fn cached_domains_parse_caps_entry_count() {
        let domains: Vec<String> = (0..API_DOMAINS_CACHE_MAX_ENTRIES + 4)
            .map(|index| format!("d{index}.example.com"))
            .collect();
        let raw = serde_json::json!({ "domains": domains, "updated_at_unix": 1 }).to_string();
        let (parsed, updated_at) = parse_cached_domains(&raw);
        assert_eq!(parsed.len(), API_DOMAINS_CACHE_MAX_ENTRIES);
        assert_eq!(updated_at, Some(1));
    }
}
