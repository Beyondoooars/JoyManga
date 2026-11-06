use crate::config::{Config, ImageDomainMode, ImageDomainPreset};
use crate::responses::GetWeeklyInfoRespData;
use futures_util::stream::{FuturesUnordered, StreamExt};
use serde::{Deserialize, Serialize};
use specta::Type;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct NetworkProbe {
    pub label: String,
    pub state: String,
    pub elapsed_ms: Option<u64>,
    pub message: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct NetworkCheckReport {
    pub api: NetworkProbe,
    pub image: NetworkProbe,
    pub domain: NetworkProbe,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ImageSpeedResult {
    pub mode: ImageDomainMode,
    pub label: String,
    pub domain: String,
    pub bytes: u64,
    pub elapsed_ms: u64,
    /// 请求失败时没有吞吐量。
    #[specta(optional)]
    pub bytes_per_second: Option<f64>,
    /// 实际成功并完成校验的样本数；低于 `sample_requests` 时吞吐只按成功样本统计。
    pub successful_samples: usize,
    #[specta(optional)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ImageSpeedReport {
    pub results: Vec<ImageSpeedResult>,
    pub best: Option<ImageDomainMode>,
    pub sample_requests: usize,
    pub vpn_active: bool,
}

const IMAGE_SPEED_REQUESTS: usize = 4;
const IMAGE_SPEED_MAX_BYTES: usize = 4 * 1024 * 1024;
/// 每条线路的测量预算；预热有独立的小预算，不再挤占测量时间。
const IMAGE_SPEED_TIMEOUT: Duration = Duration::from_secs(12);
/// 预热只建立连接：最多读一小段响应，避免每条候选线路完整下载一张封面。
const IMAGE_SPEED_WARMUP_TIMEOUT: Duration = Duration::from_secs(4);
const IMAGE_SPEED_WARMUP_BYTES: usize = 64 * 1024;

/// 进程级测速互斥：前端丢弃旧结果并不会停止后端请求，手动再次测速若并行执行
/// 会一起抢占带宽。第二个请求直接返回可读的忙碌提示，而不是排队后重复测速。
static IMAGE_SPEED_TEST_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
/// 测速取消标记：前端离开设置页或改动线路时通知后端停止剩余线路。
static IMAGE_SPEED_CANCEL: AtomicBool = AtomicBool::new(false);

/// 请求尽快停止当前图片线路测速；后端会在当前线路探测结束后退出。
pub fn cancel_image_speed_test() {
    IMAGE_SPEED_CANCEL.store(true, Ordering::SeqCst);
}

fn ensure_speed_test_active() -> anyhow::Result<()> {
    anyhow::ensure!(!IMAGE_SPEED_CANCEL.load(Ordering::SeqCst), "测速已取消");
    Ok(())
}

/// 同一封面在每条线路上并发下载数次，以总吞吐而不是单次握手延迟排序。
/// 每条线路单独使用客户端，避免某个域名的连接池状态污染另一条线路。
/// 只比较当前网络与代理/VPN 路径下的结果，不声称 VPN 本身一定提速。
pub async fn image_speed_test(config: Config, comic_id: i64) -> anyhow::Result<ImageSpeedReport> {
    config.validate()?;
    anyhow::ensure!(comic_id > 0, "先选择一本在线漫画，再测速图片线路");
    let _speed_test_guard = IMAGE_SPEED_TEST_LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .try_lock()
        .map_err(|_| anyhow::anyhow!("已有图片线路测速正在进行，请等待完成后再试"))?;
    IMAGE_SPEED_CANCEL.store(false, Ordering::SeqCst);

    // 测速开始时固定 VPN 状态与网络代际；中途切换 VPN/代理/默认网络时整轮作废，
    // 避免前半段走代理、后半段走 VPN 的结果被放在一起排序。
    let network_epoch = crate::network_state::network_epoch();
    let vpn_active = crate::network_state::is_vpn_active();
    let candidates = speed_test_candidates(&config);
    let referer = format!("https://{}", config.get_api_domain());

    let mut results = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        ensure_speed_test_active()?;
        ensure_same_network(network_epoch)?;
        let result = tokio::time::timeout(
            IMAGE_SPEED_TIMEOUT,
            probe_image_domain(
                &config,
                &candidate.domain,
                comic_id,
                network_epoch,
                &referer,
            ),
        )
        .await;
        // 网络变化优先于单线路超时报出：否则用户会误以为只是线路慢。
        ensure_same_network(network_epoch)?;
        ensure_speed_test_active()?;
        let result = match result {
            Ok(result) => result,
            Err(_) => Err(anyhow::anyhow!(
                "线路测速超过 {} 秒",
                IMAGE_SPEED_TIMEOUT.as_secs()
            )),
        };
        let (bytes, elapsed_ms, successful_samples, error) = match result {
            Ok(summary) => (
                summary.bytes,
                summary.elapsed_ms,
                summary.successful_samples,
                None,
            ),
            Err(error) => (
                0,
                0,
                0,
                Some(format!("{error:#}").chars().take(240).collect()),
            ),
        };
        results.push(ImageSpeedResult {
            bytes_per_second: (error.is_none()).then(|| bytes_per_second(bytes, elapsed_ms)),
            mode: candidate.mode,
            label: candidate.label,
            domain: candidate.domain,
            bytes,
            elapsed_ms,
            successful_samples,
            error,
        });
    }
    // 线路按实际下载吞吐展示；不可用的排最后，部分样本失败的排在完整成功之后。
    results.sort_by(|a, b| {
        b.error
            .is_none()
            .cmp(&a.error.is_none())
            .then_with(|| {
                (b.successful_samples == IMAGE_SPEED_REQUESTS)
                    .cmp(&(a.successful_samples == IMAGE_SPEED_REQUESTS))
            })
            .then_with(|| match (a.bytes_per_second, b.bytes_per_second) {
                (Some(a), Some(b)) => b.total_cmp(&a),
                _ => std::cmp::Ordering::Equal,
            })
    });
    let best = usable_best(&results);
    Ok(ImageSpeedReport {
        results,
        best,
        sample_requests: IMAGE_SPEED_REQUESTS,
        vpn_active,
    })
}

/// 只有全部样本成功且确有字节的线路才能被持久化为“最快线路”。
fn usable_best(results: &[ImageSpeedResult]) -> Option<ImageDomainMode> {
    results
        .iter()
        .find(|result| {
            result.error.is_none()
                && result.bytes > 0
                && result.successful_samples == IMAGE_SPEED_REQUESTS
        })
        .map(|result| result.mode)
}

/// 测速候选：内置线路 + 自定义线路。自定义域名与某条内置线路相同时
/// 用自定义标签替换该行，避免同一地址被测两次、界面出现两行重复成绩。
fn speed_test_candidates(config: &Config) -> Vec<ImageDomainPreset> {
    let mut candidates = crate::config::image_domain_presets();
    if config.image_domain_mode == ImageDomainMode::Custom {
        let custom = ImageDomainPreset {
            mode: ImageDomainMode::Custom,
            label: "自定义".into(),
            domain: config.get_image_domain(),
        };
        let custom_domain = normalized_domain(&custom.domain);
        if let Some(existing) = candidates
            .iter_mut()
            .find(|candidate| normalized_domain(&candidate.domain) == custom_domain)
        {
            *existing = custom;
        } else {
            candidates.push(custom);
        }
    }
    candidates
}

fn normalized_domain(domain: &str) -> String {
    domain.trim().trim_end_matches('.').to_ascii_lowercase()
}

fn ensure_same_network(epoch: u64) -> anyhow::Result<()> {
    anyhow::ensure!(
        crate::network_state::epoch_is_current(epoch),
        "测速期间网络路径已变化（VPN/代理/默认网络切换），请重新测速"
    );
    Ok(())
}

fn bytes_per_second(bytes: u64, elapsed_ms: u64) -> f64 {
    if elapsed_ms == 0 {
        return 0.0;
    }
    bytes as f64 * 1000.0 / elapsed_ms as f64
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ProbeSummary {
    bytes: u64,
    elapsed_ms: u64,
    successful_samples: usize,
}

/// 汇总一次线路探测的四个并发样本：单个样本失败不再拖垮整条线路，
/// 只按成功样本的字节数除以实际耗时计算吞吐；全部失败时保留首个错误。
fn summarize_samples(
    samples: impl IntoIterator<Item = Result<u64, String>>,
    elapsed_ms: u64,
) -> anyhow::Result<ProbeSummary> {
    let mut bytes = 0u64;
    let mut successful_samples = 0usize;
    let mut first_error = None;
    for sample in samples {
        match sample {
            Ok(sample_bytes) => {
                bytes = bytes.saturating_add(sample_bytes);
                successful_samples += 1;
            }
            Err(error) => {
                first_error.get_or_insert(error);
            }
        }
    }
    anyhow::ensure!(
        successful_samples > 0,
        "{}",
        first_error.unwrap_or_else(|| "测速样本全部失败".to_string())
    );
    Ok(ProbeSummary {
        bytes,
        elapsed_ms: elapsed_ms.max(1),
        successful_samples,
    })
}

async fn probe_image_domain(
    config: &Config,
    domain: &str,
    comic_id: i64,
    network_epoch: u64,
    referer: &str,
) -> anyhow::Result<ProbeSummary> {
    let client = config
        .apply_proxy_best_effort(
            crate::tls::client_builder()
                .connect_timeout(Duration::from_secs(3))
                .timeout(Duration::from_secs(8)),
            "图片线路测速",
        )
        .build()?;
    let url = format!("https://{domain}/media/albums/{comic_id}_3x4.jpg");

    // 预热只建立 TCP/TLS 连接（最多读 64KB），不占用测量预算；
    // 服务器忽略 Range 时最多多读一小段，不再为预热完整下载一张封面。
    let _ = tokio::time::timeout(
        IMAGE_SPEED_WARMUP_TIMEOUT,
        warm_up_connection(client.clone(), url.clone(), referer.to_string()),
    )
    .await;
    ensure_same_network(network_epoch)?;

    let started = Instant::now();
    let mut tasks = FuturesUnordered::new();
    for _ in 0..IMAGE_SPEED_REQUESTS {
        let client = client.clone();
        let url = url.clone();
        let referer = referer.to_string();
        tasks.push(download_speed_sample(client, url, referer));
    }
    let mut samples = Vec::with_capacity(IMAGE_SPEED_REQUESTS);
    while let Some(result) = tasks.next().await {
        samples.push(
            result.map_err(|error| format!("{error:#}").chars().take(240).collect::<String>()),
        );
    }
    let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    summarize_samples(samples, elapsed_ms)
}

/// 用 Range 请求预热连接：响应足够小时完整读完，让连接回到连接池；
/// 服务器忽略 Range 时最多只读 `IMAGE_SPEED_WARMUP_BYTES` 就关闭。
async fn warm_up_connection(
    client: reqwest::Client,
    url: String,
    referer: String,
) -> anyhow::Result<()> {
    let mut response = apply_image_speed_headers(client.get(url), &referer)
        .header("range", format!("bytes=0-{}", IMAGE_SPEED_WARMUP_BYTES - 1))
        .send()
        .await?
        .error_for_status()?;
    let mut read = 0usize;
    while let Some(chunk) = response.chunk().await? {
        read = read.saturating_add(chunk.len());
        if read >= IMAGE_SPEED_WARMUP_BYTES {
            break;
        }
    }
    Ok(())
}

/// 使用与正式下载一致的请求头，避免测速路径与章节图片路径得到不同响应。
fn apply_image_speed_headers(
    request: reqwest::RequestBuilder,
    referer: &str,
) -> reqwest::RequestBuilder {
    request
        .header("user-agent", crate::jm_client::IMAGE_USER_AGENT)
        .header("accept", crate::jm_client::IMAGE_ACCEPT)
        .header("x-requested-with", crate::jm_client::IMAGE_REQUESTED_WITH)
        .header("accept-language", crate::jm_client::IMAGE_ACCEPT_LANGUAGE)
        .header("referer", referer)
}

async fn download_speed_sample(
    client: reqwest::Client,
    url: String,
    referer: String,
) -> anyhow::Result<u64> {
    let mut response = apply_image_speed_headers(client.get(url), &referer)
        .send()
        .await?
        .error_for_status()?;
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        anyhow::ensure!(
            bytes.len().saturating_add(chunk.len()) <= IMAGE_SPEED_MAX_BYTES,
            "测速图片超过4MB上限"
        );
        bytes.extend_from_slice(&chunk);
    }
    anyhow::ensure!(!bytes.is_empty(), "测速图片为空");
    // HTTP 200 也可能是代理或 CDN 的 HTML 验证页，不能据此选出不可用线路。
    crate::image_data::validate_image(&bytes)?;
    Ok(bytes.len() as u64)
}

fn completed(
    label: &str,
    started: Instant,
    result: anyhow::Result<()>,
    success: &str,
) -> NetworkProbe {
    NetworkProbe {
        label: label.into(),
        state: if result.is_ok() { "Passed" } else { "Failed" }.into(),
        elapsed_ms: Some(u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)),
        message: result.map_or_else(
            |error| format!("{error:#}").chars().take(320).collect(),
            |()| success.into(),
        ),
    }
}

pub async fn check(
    config: Config,
    comic_id: Option<i64>,
    domain_pool_count: usize,
    domain_pool_updated_at: Option<u64>,
    app_version: &str,
    domain_health: (u32, bool),
) -> anyhow::Result<NetworkCheckReport> {
    config.validate()?;
    // 连接检查期间网络路径变化时整份报告作废，避免两条路径的结果混在一起。
    let network_epoch = crate::network_state::network_epoch();
    let client = config
        .apply_proxy(
            crate::tls::client_builder()
                .connect_timeout(Duration::from_secs(3))
                .timeout(Duration::from_secs(8)),
        )?
        .build()?;
    let api = async {
        let started = Instant::now();
        let result = async {
            let ts = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
            // 验证无账号数据的漫画信息接口，不拿域名首页或任意 HTTP 响应代替业务可用性。
            let response = client.get(format!("https://{}/week", config.get_api_domain()))
                .header("token", crate::utils::md5_hex(&format!("{ts}{}", crate::jm_client::APP_TOKEN_SECRET)))
                .header("tokenparam", format!("{ts},{app_version}"))
                .header("user-agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/128.0.0.0 Safari/537.36")
                .send().await?;
            crate::jm_client::parse_encrypted_resp::<GetWeeklyInfoRespData>(response, ts, "信息连接检查").await?;
            Ok::<_,anyhow::Error>(())
        }.await;
        completed(
            "漫画信息",
            started,
            result,
            "已验证信息接口响应；登录状态未改变。",
        )
    };
    let image = async {
        let Some(id) = comic_id.filter(|id| *id > 0) else {
            return NetworkProbe {
                label: "图片下载".into(),
                state: "Skipped".into(),
                elapsed_ms: None,
                message: "先选择一本在线漫画，再检查它的封面下载。".into(),
            };
        };
        let started = Instant::now();
        let result = async {
            let mut response = client
                .get(format!(
                    "https://{}/media/albums/{id}_3x4.jpg",
                    config.get_image_domain()
                ))
                .header("user-agent", "Mozilla/5.0")
                .send()
                .await?
                .error_for_status()?;
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await? {
                anyhow::ensure!(
                    bytes.len().saturating_add(chunk.len()) <= 4 * 1024 * 1024,
                    "检查图片超过4MB上限"
                );
                bytes.extend_from_slice(&chunk);
            }
            crate::image_data::validate_image(&bytes)?;
            Ok::<_, anyhow::Error>(())
        }
        .await;
        completed(
            "图片下载",
            started,
            result,
            "已验证封面图片下载；耗时不代表整章下载速度。",
        )
    };
    let (api, image) = tokio::join!(api, image);
    anyhow::ensure!(
        crate::network_state::epoch_is_current(network_epoch),
        "检查期间网络路径已变化（VPN/代理/默认网络切换），请重新检查"
    );
    let current_api_domain = config.get_api_domain();
    let (failures, cooling_down) = domain_health;
    let domain = domain_probe(
        &current_api_domain,
        domain_pool_count,
        domain_pool_updated_at,
        failures,
        cooling_down,
    );
    Ok(NetworkCheckReport { api, image, domain })
}

/// 动态域名状态行：展示当前线路与动态域名池的新鲜度（纯本地信息，不发起网络请求）。
pub fn domain_probe(
    current_api_domain: &str,
    domain_pool_count: usize,
    updated_at_unix: Option<u64>,
    consecutive_failures: u32,
    cooling_down: bool,
) -> NetworkProbe {
    // 线路正在冷却时明确告知，否则用户会疑惑"为什么首选线路没被使用"
    let health_note = if cooling_down {
        format!("；该线路连续失败 {consecutive_failures} 次，已暂时降级")
    } else if consecutive_failures > 0 {
        format!("；该线路连续失败 {consecutive_failures} 次")
    } else {
        String::new()
    };
    let (state, message) = match (domain_pool_count, updated_at_unix) {
        (count, Some(updated_at)) if count > 0 => (
            "Passed",
            format!(
                "动态域名 {count} 个已就绪，{}更新；当前线路 {current_api_domain}{health_note}。",
                format_age(updated_at)
            ),
        ),
        _ => (
            "Skipped",
            format!("尚未获取动态域名，暂用内置线路；当前线路 {current_api_domain}{health_note}。"),
        ),
    };
    NetworkProbe {
        label: "动态域名".into(),
        state: state.into(),
        elapsed_ms: None,
        message,
    }
}

/// 把 Unix 时间戳描述为“多久之前”。
fn format_age(updated_at_unix: u64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    let age = now.saturating_sub(updated_at_unix);
    if age < 60 {
        "刚刚".into()
    } else if age < 3600 {
        format!("{} 分钟前", age / 60)
    } else if age < 86400 {
        format!("{} 小时前", age / 3600)
    } else {
        format!("{} 天前", age / 86400)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn throughput_is_missing_when_a_probe_fails() {
        let report = ImageSpeedReport {
            results: vec![ImageSpeedResult {
                mode: ImageDomainMode::Domain1,
                label: "线路1".into(),
                domain: "img.example".into(),
                bytes: 0,
                elapsed_ms: 0,
                bytes_per_second: None,
                successful_samples: 0,
                error: Some("timeout".into()),
            }],
            best: None,
            sample_requests: IMAGE_SPEED_REQUESTS,
            vpn_active: false,
        };
        let json = serde_json::to_value(report).unwrap();
        assert_eq!(json["sampleRequests"], 4);
        assert_eq!(json["vpnActive"], false);
        assert_eq!(json["results"][0]["successfulSamples"], 0);
        assert!(json["results"][0]["bytesPerSecond"].is_null());
    }

    #[tokio::test]
    async fn speed_sample_rejects_successful_html_responses() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 2048];
            assert!(socket.read(&mut request).await.unwrap() > 0);
            let body = b"<html><body>Verification required</body></html>";
            socket
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            socket.write_all(body).await.unwrap();
        });

        let result = download_speed_sample(
            crate::tls::client_builder().build().unwrap(),
            format!("http://{address}/media/albums/1_3x4.jpg"),
            "https://api.example".into(),
        )
        .await;
        server.await.unwrap();
        assert!(result.is_err());
        assert!(format!("{:#}", result.unwrap_err()).contains("无法识别图片内容"));
    }

    #[test]
    fn throughput_uses_actual_transfer_window() {
        assert_eq!(bytes_per_second(2 * 1024 * 1024, 2_000), 1024.0 * 1024.0);
        assert_eq!(bytes_per_second(1024, 0), 0.0);
    }

    #[test]
    fn partial_sample_failures_rank_by_successful_bytes() {
        let summary = summarize_samples(
            vec![Ok(100), Err("sample timeout".to_string()), Ok(200), Ok(300)],
            2_000,
        )
        .unwrap();
        assert_eq!(summary.bytes, 600);
        assert_eq!(summary.successful_samples, 3);
        assert_eq!(summary.elapsed_ms, 2_000);
        assert_eq!(bytes_per_second(summary.bytes, summary.elapsed_ms), 300.0);
    }

    #[test]
    fn only_fully_successful_lines_can_be_best() {
        let result = |mode, successful_samples| ImageSpeedResult {
            mode,
            label: format!("{mode:?}"),
            domain: format!("{mode:?}.example"),
            bytes: 1024,
            elapsed_ms: 1_000,
            bytes_per_second: Some(1.0),
            successful_samples,
            error: None,
        };
        let partial = result(ImageDomainMode::Domain1, 1);
        let full = result(ImageDomainMode::Domain2, IMAGE_SPEED_REQUESTS);
        // 部分样本成功的线路即使单流吞吐更高，也不能被“使用最快线路”持久化。
        assert_eq!(
            usable_best(&[partial.clone(), full.clone()]),
            Some(ImageDomainMode::Domain2)
        );
        assert_eq!(usable_best(&[partial]), None);
    }

    #[test]
    fn all_failed_samples_preserve_a_readable_error() {
        let error = summarize_samples(
            vec![
                Err("first failure".to_string()),
                Err("second failure".to_string()),
            ],
            500,
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("first failure"));
    }

    #[test]
    fn custom_candidate_replaces_duplicate_preset_instead_of_repeating_it() {
        let mut config = Config::default(std::path::Path::new("app-data"));
        config.image_domain_mode = ImageDomainMode::Custom;
        // Config::default 的自定义域名与线路三相同。
        let candidates = speed_test_candidates(&config);
        let custom: Vec<&ImageDomainPreset> = candidates
            .iter()
            .filter(|candidate| candidate.mode == ImageDomainMode::Custom)
            .collect();
        assert_eq!(custom.len(), 1);
        assert_eq!(custom[0].label, "自定义");
        assert_eq!(candidates.len(), 6);
        let matching = candidates
            .iter()
            .filter(|candidate| {
                normalized_domain(&candidate.domain) == normalized_domain(&custom[0].domain)
            })
            .count();
        assert_eq!(matching, 1);
    }

    #[test]
    fn unique_custom_candidate_is_appended() {
        let mut config = Config::default(std::path::Path::new("app-data"));
        config.image_domain_mode = ImageDomainMode::Custom;
        config.custom_image_domain = "custom-cdn.example.com".into();
        let candidates = speed_test_candidates(&config);
        assert_eq!(candidates.len(), 7);
        assert!(candidates.iter().any(|candidate| {
            candidate.mode == ImageDomainMode::Custom
                && candidate.domain == "custom-cdn.example.com"
        }));
    }

    #[test]
    fn stale_network_generation_is_rejected() {
        let _guard = crate::network_state::STATE_TEST_LOCK.lock();
        let epoch = crate::network_state::network_epoch();
        crate::network_state::change_network_path(|| {});
        let error = ensure_same_network(epoch).unwrap_err();
        assert!(format!("{error:#}").contains("请重新测速"));
        ensure_same_network(crate::network_state::network_epoch()).unwrap();
    }
}
