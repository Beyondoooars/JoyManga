use crate::config::Config;
use crate::responses::GetWeeklyInfoRespData;
use serde::{Deserialize, Serialize};
use specta::Type;
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
    let client = config
        .apply_proxy(
            reqwest::Client::builder()
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
