use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Context};
use bytes::{Bytes, BytesMut};
use image::ImageFormat;
use parking_lot::RwLock;
use reqwest::StatusCode;
use reqwest_middleware::ClientWithMiddleware;
use reqwest_retry::policies::ExponentialBackoff;
use reqwest_retry::{Jitter, RetryTransientMiddleware};
use serde_json::json;
use tauri::{AppHandle, Manager};

use crate::config::{ApiDomainMode, Config, ConfigStorage};
use crate::events::{FavoriteChangedEvent, SessionChangedEvent};
use crate::extensions::AppHandleExt;
use crate::responses::{
    DailyCheckInRespData, DailyCheckInResultRespData, GetChapterRespData, GetComicRespData,
    GetFavoriteRespData, GetUserProfileRespData, GetWeeklyInfoRespData, GetWeeklyRespData, JmResp,
    LoginRespData, RedirectRespData, SearchResp, SearchRespData, ToggleFavoriteRespData,
    ToggleType,
};
use crate::types::{DailyCheckInOutcome, DailyCheckInState, FavoriteSort, SearchSort};
use crate::utils;
use tauri_specta::Event;

pub(crate) const APP_TOKEN_SECRET: &str = "18comicAPP";
const APP_TOKEN_SECRET_2: &str = "18comicAPPContent";
const APP_DATA_SECRET: &str = "185Hcomic3PAPP7R";
pub(crate) const APP_VERSION: &str = "2.0.13";
/// 动态更新移动端 API 域名的官方地址（返回 AES 加密的域名列表）
const API_DOMAIN_UPDATE_URLS: [&str; 3] = [
    "https://rup4a04-c01.tos-ap-southeast-1.bytepluses.com/newsvr-2025.txt",
    "https://rup4a04-c02.tos-cn-hongkong.bytepluses.com/newsvr-2025.txt",
    "https://rup4a04-c03.tos-cn-beijing.bytepluses.com.cn/newsvr-2025.txt",
];
/// 域名更新载荷的解密密钥（与官方实现一致）
const API_DOMAIN_SERVER_SECRET: &str = "diosfjckwpqpdfjkvnqQjsik";
/// 动态域名刷新节流间隔
const API_DOMAIN_REFRESH_INTERVAL: Duration = Duration::from_secs(600);
/// 域名列表保鲜间隔：运行中超过该间隔未刷新时，下一次请求会顺带触发后台刷新
const API_DOMAIN_STALE_REFRESH_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
/// 一轮候选线路尝试的总时间预算。单个请求本身有超时，但没有总预算时
/// "所有线路都卡到超时"会让用户干等 N 倍超时时间。
const CANDIDATE_TOTAL_BUDGET: Duration = Duration::from_secs(45);
/// 动态域名缓存文件名（与配置文件同目录，启动时先读取，刷新成功后覆盖）
const API_DOMAINS_CACHE_FILE: &str = ".api-domains.json";
/// 缓存中最多采纳的域名条目数量（防御损坏文件里的超长列表）
const API_DOMAINS_CACHE_MAX_ENTRIES: usize = 16;
const MAX_API_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const MAX_IMAGE_RESPONSE_BYTES: usize = 64 * 1024 * 1024;
const MAX_ERROR_RESPONSE_BYTES: usize = 16 * 1024;
const MAX_ERROR_BODY_CHARS: usize = 512;

#[derive(Default)]
struct SessionState {
    epoch: u64,
    token: String,
    profile: Option<GetUserProfileRespData>,
}

#[derive(Debug)]
struct ApiFailure {
    status: StatusCode,
    authentication: bool,
    unavailable: bool,
    message: String,
}
impl std::fmt::Display for ApiFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}（HTTP {}）", self.message, self.status)
    }
}
impl std::error::Error for ApiFailure {}

fn extract_api_error_message(value: &serde_json::Value) -> Option<String> {
    ["errorMsg", "error_msg", "msg", "message"]
        .into_iter()
        .filter_map(|key| value.get(key)?.as_str())
        .map(str::trim)
        .find(|s| !s.is_empty())
        .map(|s| s.chars().take(MAX_ERROR_BODY_CHARS).collect())
}

fn api_failure(status: StatusCode, code: Option<i64>, message: &str) -> anyhow::Error {
    let authentication = status == StatusCode::UNAUTHORIZED
        || code == Some(401)
        || [
            "未登录",
            "未登入",
            "請先登入",
            "请先登录",
            "Cookie无效",
            "Cookie已过期",
        ]
        .iter()
        .any(|s| message.contains(s));
    let message: String = message.chars().take(MAX_ERROR_BODY_CHARS).collect();
    // 有明确业务包络时停止容灾，不能把封禁/限流换成另一线路重复提交。
    let gateway_unavailable = code.is_none()
        && matches!(
            status,
            StatusCode::BAD_GATEWAY | StatusCode::SERVICE_UNAVAILABLE | StatusCode::GATEWAY_TIMEOUT
        );
    // 被墙或拦截的线路常返回无业务包络的 403 或劫持 HTML；这类响应允许换线路重试。
    // 带业务包络（code 存在）的 403 是明确业务错误，不能换线路重复提交。
    //
    // 405 同理：不同镜像线路的接口集合并不一致，某条线路不提供 /login 或 /week 时
    // 会直接回 405 Method Not Allowed。它不是账号或网络问题，换一条线路通常就正常；
    // 若按普通业务错误直接返回，用户看到的就是「登录失败/每周必看加载失败：HTTP 405」，
    // 而其余候选线路根本没被尝试过。带业务包络的 405 仍按业务错误处理。
    let blocked = (!authentication && code.is_none() && status == StatusCode::FORBIDDEN)
        || (code.is_none() && status == StatusCode::METHOD_NOT_ALLOWED)
        || (code.is_none() && is_blocked_page(&message));
    anyhow::Error::new(ApiFailure {
        status,
        authentication,
        unavailable: gateway_unavailable || blocked,
        message,
    })
}

/// 网关/拦截/停放页面特征：换一条线路可能恢复，且报文不代表真正的业务响应。
fn is_blocked_page(message: &str) -> bool {
    const MARKERS: &[&str] = &[
        "网页防火墙",
        "Access denied",
        "Access Denied",
        "安全验证",
        "Cloudflare",
        "cf-error",
        "Just a moment",
    ];
    if MARKERS.iter().any(|marker| message.contains(marker)) {
        return true;
    }
    // 域名过期后常被停放商接管：HTTP 200 + 一个 HTML 跳转页（例如
    // `window.location.href="/lander"`）。业务响应一定是 JSON，出现 HTML 就说明
    // 这条线路已经不再提供服务。此前它被当成普通业务错误直接终止整轮请求，
    // 明明刷新后已有可用线路，却一条都不会被尝试。
    is_html_page(message)
}

/// 报文是否是 HTML 文档而不是 API 的 JSON。
fn is_html_page(message: &str) -> bool {
    let trimmed = message.trim_start();
    let head: String = trimmed.chars().take(200).collect::<String>().to_lowercase();
    // 停放页可能以注释、空白或 BOM 开头，不能只看第一个字符
    head.contains("<!doctype html") || head.contains("<html")
}

fn is_authentication_error(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<ApiFailure>()
            .is_some_and(|e| e.authentication)
    })
}

fn is_route_unreachable(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        if let Some(e) = cause.downcast_ref::<ApiFailure>() {
            return e.unavailable;
        }
        if let Some(e) = cause.downcast_ref::<reqwest::Error>() {
            return e.is_connect() || e.is_timeout();
        }
        if let Some(reqwest_middleware::Error::Reqwest(e)) =
            cause.downcast_ref::<reqwest_middleware::Error>()
        {
            return e.is_connect() || e.is_timeout();
        }
        false
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ApiPath {
    // 登录与获取用户信息共用 /login：带 AVS 请求即可取回用户信息，无需账号密码
    Login,
    Search,
    GetComic,
    GetChapter,
    GetScrambleId,
    GetFavoriteFolder,
    GetWeeklyInfo,
    GetWeekly,
    Setting,
    DailyCheckInStatus,
    DailyCheckIn,
}
impl ApiPath {
    fn as_str(&self) -> &'static str {
        match self {
            ApiPath::Login => "/login",
            ApiPath::Search => "/search",
            ApiPath::GetComic => "/album",
            ApiPath::GetChapter => "/chapter",
            ApiPath::GetScrambleId => "/chapter_view_template",
            ApiPath::GetFavoriteFolder => "/favorite",
            ApiPath::GetWeeklyInfo => "/week",
            ApiPath::GetWeekly => "/week/filter",
            ApiPath::Setting => "/setting",
            ApiPath::DailyCheckInStatus => "/daily",
            ApiPath::DailyCheckIn => "/daily_chk",
        }
    }

    /// 解析错误时展示的中文动作名。
    fn action_name(self) -> &'static str {
        match self {
            ApiPath::Login => "登录",
            ApiPath::Search => "搜索",
            ApiPath::GetComic => "获取漫画",
            ApiPath::GetChapter => "获取章节",
            ApiPath::GetScrambleId => "获取图片信息",
            ApiPath::GetFavoriteFolder => "获取收藏夹",
            ApiPath::GetWeeklyInfo | ApiPath::GetWeekly => "获取每周必看信息",
            ApiPath::Setting => "获取设置",
            ApiPath::DailyCheckInStatus => "获取签到状态",
            ApiPath::DailyCheckIn => "签到",
        }
    }
}

#[derive(Clone)]
pub struct JmClient {
    app: AppHandle,
    api_client: Arc<RwLock<ClientWithMiddleware>>,
    api_client_without_retry: Arc<RwLock<ClientWithMiddleware>>,
    session: Arc<RwLock<SessionState>>,
    auth_lock: Arc<tokio::sync::Mutex<()>>,
    favorite_lock: Arc<tokio::sync::Mutex<()>>,
    /// 签到串行锁：写接口不能连点重复提交
    daily_check_in_lock: Arc<tokio::sync::Mutex<()>>,
    auth_generation: Arc<AtomicU64>,
    auth_cancel: tokio::sync::watch::Sender<u64>,
    img_client: Arc<RwLock<ClientWithMiddleware>>,
    /// 从官方地址动态获取的最新 API 域名（优先于内置预设使用）
    updated_api_domains: Arc<RwLock<Vec<String>>>,
    domain_refresh_at: Arc<RwLock<Option<std::time::Instant>>>,
    /// 动态域名列表最近一次成功获取的 Unix 秒时间戳（启动先取本地缓存，刷新后覆盖）
    domain_list_updated_at: Arc<RwLock<Option<u64>>>,
    /// 最近一次成功的线路（内存态，仅用于优先复用，不写入配置）
    last_success_domain: Arc<RwLock<Option<String>>>,
    /// 各线路的连续失败与延迟记录，用于候选排序（见 domain_health 模块）
    domain_health: Arc<crate::domain_health::DomainHealthStore>,
    /// APP 接口版本号（初始化时为内置版本，可从 /setting 更新）
    app_version: Arc<RwLock<String>>,
}

impl JmClient {
    pub fn new(app: AppHandle) -> Self {
        let api_client = create_api_client(&app);
        let api_client = Arc::new(RwLock::new(api_client));
        let api_client_without_retry = create_api_client_without_retry(&app);
        let api_client_without_retry = Arc::new(RwLock::new(api_client_without_retry));

        let img_client = create_img_client(&app);
        let img_client = Arc::new(RwLock::new(img_client));

        // 启动时先应用本地缓存的动态域名（秒级生效），随后仍会后台请求官方地址刷新覆盖。
        let (cached_api_domains, cached_api_domains_updated_at) = load_cached_api_domains(&app);

        Self {
            app,
            api_client,
            api_client_without_retry,
            session: Arc::new(RwLock::new(SessionState::default())),
            auth_lock: Arc::new(tokio::sync::Mutex::new(())),
            favorite_lock: Arc::new(tokio::sync::Mutex::new(())),
            daily_check_in_lock: Arc::new(tokio::sync::Mutex::new(())),
            auth_generation: Arc::new(AtomicU64::new(0)),
            auth_cancel: tokio::sync::watch::channel(0).0,
            img_client,
            updated_api_domains: Arc::new(RwLock::new(cached_api_domains)),
            domain_refresh_at: Arc::new(RwLock::new(None)),
            domain_list_updated_at: Arc::new(RwLock::new(cached_api_domains_updated_at)),
            last_success_domain: Arc::new(RwLock::new(None)),
            domain_health: Arc::new(crate::domain_health::DomainHealthStore::new()),
            app_version: Arc::new(RwLock::new(APP_VERSION.to_string())),
        }
    }

    pub fn reload_client(&self) {
        *self.api_client.write() = create_api_client(&self.app);
        *self.api_client_without_retry.write() = create_api_client_without_retry(&self.app);
        *self.img_client.write() = create_img_client(&self.app);
    }

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
    fn spawn_domain_refresh_in_background(&self) {
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

    /// 从 /setting 获取官方最新 APP 版本号并用于接口签名（失败静默）。
    pub async fn refresh_app_version(&self) {
        let result = self
            .with_candidates(|api_domain, ts| async move {
                let http_resp = self
                    .jm_request_on(
                        reqwest::Method::GET,
                        ApiPath::Setting,
                        None,
                        None,
                        ts,
                        true,
                        &api_domain,
                    )
                    .await?;
                let status = http_resp.status();
                if status != StatusCode::OK {
                    return Err(api_failure(status, None, "获取设置响应失败"));
                }
                let body =
                    read_limited_text_body(http_resp, MAX_API_RESPONSE_BYTES, "设置响应体").await?;

                let mut value: serde_json::Value =
                    serde_json::from_str(&body).context("解析/setting响应失败")?;
                // 响应可能是明文 JSON，也可能是 data 字段的加密包络
                if let Some(data) = value.get("data").and_then(|v| v.as_str()) {
                    if let Ok(decrypted) = decrypt_data(ts, data) {
                        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&decrypted) {
                            value = parsed;
                        }
                    }
                }
                value
                    .get("jm3_version")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
                    .context("/setting响应缺少jm3_version字段")
            })
            .await;

        match result {
            Ok(version) if is_valid_version(&version) => {
                let current = self.app_version();
                if compare_versions(&version, &current) == std::cmp::Ordering::Greater {
                    tracing::info!(from = %current, to = %version, "已更新APP接口版本号");
                    *self.app_version.write() = version;
                }
            }
            Ok(version) => {
                tracing::warn!(version, "官方返回的版本号格式无效，保持使用内置版本");
            }
            Err(error) => {
                tracing::warn!(error = %error, "获取最新APP版本号失败，保持使用内置版本");
            }
        }
    }

    pub fn session_snapshot(&self) -> SessionChangedEvent {
        let state = self.session.read();
        SessionChangedEvent {
            session_epoch: state.epoch,
            profile: state.profile.clone(),
        }
    }

    pub fn cancel_login(&self) {
        let generation = self.auth_generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.auth_cancel.send_replace(generation);
    }

    pub fn invalidate_session(&self) {
        self.cancel_login();
        let snapshot = {
            let mut state = self.session.write();
            state.epoch += 1;
            state.token.clear();
            state.profile = None;
            SessionChangedEvent {
                session_epoch: state.epoch,
                profile: None,
            }
        };
        let _ = snapshot.emit(&self.app);
    }

    pub fn logout(&self) -> anyhow::Result<()> {
        let config = self.app.get_config();
        let next = {
            let mut current = config.write();
            let mut next = current.clone();
            next.username.clear();
            next.password.clear();
            next.auto_login = false;
            // 即使持久化失败也先使会话失效，调用方会明确收到保存错误。
            self.invalidate_session();
            crate::config_updates::commit_locked(&self.app, &mut current, next)?
        };
        crate::config_updates::publish(&self.app, &next);
        Ok(())
    }

    fn app_version(&self) -> String {
        self.app_version.read().clone()
    }

    /// 当前接口签名使用的 APP 版本号（官方可动态升级），供连接诊断等功能复用。
    pub fn current_app_version(&self) -> String {
        self.app_version()
    }

    /// 当前请求的候选线路（按顺序尝试）：
    /// 最近成功的线路 → 配置线路 → 动态获取的最新域名 → 内置预设。
    /// 自定义线路不参与自动切换。
    fn api_domain_candidates(&self) -> Vec<String> {
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

    /// 在指定线路上发送请求（只做传输层，不换线路、不记录成功线路）。
    ///
    /// - `retry=true`：使用带退避重试的客户端，并在网关级异常（502/503/504）时返回
    ///   可换线路的错误；仅用于可安全重放的读接口。
    /// - `retry=false`：写接口只发送一次，结果交由调用方核对。
    #[allow(clippy::too_many_arguments)]
    async fn jm_request_on(
        &self,
        method: reqwest::Method,
        path: ApiPath,
        query: Option<serde_json::Value>,
        form: Option<serde_json::Value>,
        ts: u64,
        retry: bool,
        api_domain: &str,
    ) -> anyhow::Result<reqwest::Response> {
        let tokenparam = format!("{ts},{}", self.app_version());
        let token = if path == ApiPath::GetScrambleId {
            utils::md5_hex(&format!("{ts}{APP_TOKEN_SECRET_2}"))
        } else {
            utils::md5_hex(&format!("{ts}{APP_TOKEN_SECRET}"))
        };
        let avs = self.session.read().token.clone();
        let path_str = path.as_str();
        let is_credential_login = path_str == "/login" && form.is_some();
        let url = format!("https://{api_domain}{path_str}");
        let mut request = if retry {
            self.api_client.read().request(method, &url)
        } else {
            self.api_client_without_retry.read().request(method, &url)
        }
        .header("token", token)
        .header("tokenparam", tokenparam)
        .header(
            "user-agent",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/128.0.0.0 Safari/537.36",
        );

        if !avs.is_empty() && !is_credential_login {
            request = request.header("cookie", format!("AVS={avs}"));
        }

        let send_result = match &form {
            Some(payload) => request.query(&query).form(payload).send().await,
            None => request.query(&query).send().await,
        };
        let http_resp = match send_result {
            Ok(http_resp) => http_resp,
            Err(e) => {
                return Err(if e.is_timeout() {
                    anyhow::Error::from(e).context("网络请求超时，请检查网络或线路后重试")
                } else {
                    anyhow::Error::from(e)
                });
            }
        };
        let status = http_resp.status();
        if retry
            && matches!(
                status,
                StatusCode::BAD_GATEWAY
                    | StatusCode::SERVICE_UNAVAILABLE
                    | StatusCode::GATEWAY_TIMEOUT
            )
        {
            return Err(api_failure(status, None, "网关异常"));
        }
        Ok(http_resp)
    }

    /// 所有请求共用的换线路闭环：传输层不可达、网关异常或被拦截的响应会换下一条
    /// 候选线路重试；业务解析成功后才会把该线路记为最近成功，避免被墙线路被优先复用。
    async fn with_candidates<T, Fut, F>(&self, mut attempt: F) -> anyhow::Result<T>
    where
        F: FnMut(String, u64) -> Fut,
        Fut: std::future::Future<Output = anyhow::Result<T>>,
    {
        self.run_candidates(true, &mut attempt).await
    }

    async fn run_candidates<T, Fut, F>(&self, retry: bool, attempt: &mut F) -> anyhow::Result<T>
    where
        F: FnMut(String, u64) -> Fut,
        Fut: std::future::Future<Output = anyhow::Result<T>>,
    {
        let candidates = self.api_domain_candidates();
        let mut last_error: Option<anyhow::Error> = None;
        // 总预算：候选线路逐个尝试，单个请求自身有超时，但"全部候选都卡到超时"
        // 会让用户等 N×timeout（历史上出现过 5 线路 × 18s ≈ 90 秒转圈）。
        // 这里给整轮一个上限，超时后直接返回当前错误，由前端提示重试。
        let budget = tokio::time::Instant::now() + CANDIDATE_TOTAL_BUDGET;
        for candidate in candidates {
            if tokio::time::Instant::now() >= budget {
                tracing::warn!(retry, "候选线路尝试超过总预算，提前返回");
                break;
            }
            let started = std::time::Instant::now();
            let ts = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
            match attempt(candidate.clone(), ts).await {
                Ok(value) => {
                    self.domain_health
                        .record_success(&candidate, started.elapsed());
                    *self.last_success_domain.write() = Some(candidate);
                    return Ok(value);
                }
                Err(error) if is_route_unreachable(&error) => {
                    self.domain_health.record_failure(&candidate);
                    tracing::warn!(
                        domain = %candidate,
                        retry,
                        error = %error,
                        "线路不可用，尝试下一候选线路"
                    );
                    last_error = Some(error);
                }
                Err(error) => {
                    // 不该换线路的失败（业务错误、认证失败）直接终止；把它记下来，
                    // 否则「登录/每周必看失败」在日志里只有前端一句提示，无法定位。
                    tracing::warn!(
                        domain = %candidate,
                        retry,
                        error = %error,
                        "线路返回不可重试的失败，停止换线路"
                    );
                    return Err(error);
                }
            }
        }
        self.spawn_domain_refresh_in_background();
        let error = last_error.unwrap_or_else(|| anyhow!("没有可用线路"));
        Err(if retry {
            error.context("所有候选线路均连接失败，请在设置 → 网络中检查线路或代理")
        } else {
            error.context("连接失败，请在设置 → 网络中检查线路或代理后重试")
        })
    }

    async fn jm_get_parsed<T: serde::de::DeserializeOwned>(
        &self,
        path: ApiPath,
        query: Option<serde_json::Value>,
    ) -> anyhow::Result<T> {
        let action = path.action_name();
        self.with_candidates(|api_domain, ts| {
            let query = query.clone();
            async move {
                let http_resp = self
                    .jm_request_on(
                        reqwest::Method::GET,
                        path,
                        query,
                        None,
                        ts,
                        true,
                        &api_domain,
                    )
                    .await?;
                parse_encrypted_resp(http_resp, ts, action).await
            }
        })
        .await
    }

    /// 写接口：每条线路上只发送一次（不自动重试），保证不会重复提交；
    /// 传输层失败 / 网关异常 / 被拦截的响应可以安全地换下一条候选线路。
    async fn post_without_retry_parsed<T: serde::de::DeserializeOwned>(
        &self,
        path: ApiPath,
        query: Option<serde_json::Value>,
        payload: Option<serde_json::Value>,
        action: &str,
    ) -> anyhow::Result<T> {
        self.run_candidates(false, &mut |api_domain, ts| {
            let query = query.clone();
            let payload = payload.clone();
            async move {
                let http_resp = self
                    .jm_request_on(
                        reqwest::Method::POST,
                        path,
                        query,
                        payload,
                        ts,
                        false,
                        &api_domain,
                    )
                    .await?;
                parse_encrypted_resp(http_resp, ts, action).await
            }
        })
        .await
    }

    pub async fn login(&self, username: &str, password: &str) -> anyhow::Result<LoginRespData> {
        let started = Instant::now();
        let generation = self.auth_generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.auth_cancel.send_replace(generation);
        let mut cancellation = self.auth_cancel.subscribe();
        tracing::info!(
            generation,
            username_len = username.trim().chars().count(),
            "开始登录"
        );
        let _guard = self.auth_lock.lock().await;
        let current_generation = self.auth_generation.load(Ordering::SeqCst);
        if generation != current_generation {
            tracing::info!(
                generation,
                current_generation,
                elapsed_ms = started.elapsed().as_millis() as u64,
                "登录在排队期间被取消"
            );
        }
        anyhow::ensure!(generation == current_generation, "登录已取消");
        let config = self.app.get_config().read().clone();
        let result = tokio::select! {
            result = tokio::time::timeout(Duration::from_secs(35), self.login_candidates(&config, username, password, generation)) => {
                result.map_err(|_| anyhow!("登录整体超时，请在设置 → 网络中检查线路或代理"))?
            },
            _ = cancellation.changed() => {
                tracing::info!(
                    generation,
                    elapsed_ms = started.elapsed().as_millis() as u64,
                    "登录等待期间收到取消通知"
                );
                Err(anyhow!("登录已取消"))
            },
        };
        match &result {
            Ok(data) => tracing::info!(
                generation,
                session_epoch = data.session_epoch,
                elapsed_ms = started.elapsed().as_millis() as u64,
                "登录成功"
            ),
            Err(error) => tracing::warn!(
                generation,
                elapsed_ms = started.elapsed().as_millis() as u64,
                error = %error,
                "登录失败"
            ),
        }
        result
    }

    async fn login_candidates(
        &self,
        config: &Config,
        username: &str,
        password: &str,
        generation: u64,
    ) -> anyhow::Result<LoginRespData> {
        let client = config
            .apply_proxy(timed_client_builder(Duration::from_secs(12)))?
            .build()?;
        let form = json!({"username": username.trim(), "password": password});
        let candidates = self.api_domain_candidates();
        tracing::info!(
            generation,
            candidate_count = candidates.len(),
            api_domain_mode = ?config.api_domain_mode,
            proxy_mode = ?config.proxy_mode,
            "登录候选线路已准备"
        );
        let mut last_error = None;
        for (index, candidate) in candidates.iter().enumerate() {
            let attempt_started = Instant::now();
            anyhow::ensure!(
                generation == self.auth_generation.load(Ordering::SeqCst),
                "登录已取消"
            );
            tracing::info!(
                generation,
                attempt = index + 1,
                candidate_count = candidates.len(),
                domain = %candidate,
                "尝试登录线路"
            );
            let ts = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
            let request = client
                .post(format!("https://{candidate}/login"))
                .header("token", utils::md5_hex(&format!("{ts}{APP_TOKEN_SECRET}")))
                .header("tokenparam", format!("{ts},{}", self.app_version()))
                .header(
                    "user-agent",
                    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/128.0.0.0 Safari/537.36",
                )
                .form(&form);
            let result = async {
                let response = request.send().await?;
                parse_encrypted_resp::<GetUserProfileRespData>(response, ts, "登录").await
            }
            .await;
            match result {
                Ok(profile) => {
                    tracing::info!(
                        generation,
                        attempt = index + 1,
                        domain = %candidate,
                        elapsed_ms = attempt_started.elapsed().as_millis() as u64,
                        "登录线路成功"
                    );
                    self.domain_health
                        .record_success(candidate, std::time::Duration::ZERO);
                    *self.last_success_domain.write() = Some(candidate.clone());
                    // 成功线路对应内置预设时同步保存该模式；动态域名只记入内存，
                    // 之后的请求会自动优先复用这条可用线路。
                    let mode = if candidate == &config.get_api_domain() {
                        config.api_domain_mode
                    } else {
                        preset_mode_for_domain(candidate).unwrap_or(config.api_domain_mode)
                    };
                    let outcome = self.commit_login(config, mode, profile, generation);
                    if let Err(error) = &outcome {
                        tracing::warn!(
                            generation,
                            domain = %candidate,
                            error = %error,
                            "登录提交失败"
                        );
                    }
                    return outcome;
                }
                Err(error) if is_route_unreachable(&error) => {
                    tracing::warn!(
                        generation,
                        attempt = index + 1,
                        domain = %candidate,
                        elapsed_ms = attempt_started.elapsed().as_millis() as u64,
                        error = %error,
                        "登录线路不可达，尝试下一条"
                    );
                    self.domain_health.record_failure(candidate);
                    last_error = Some(error);
                }
                Err(error) => {
                    tracing::warn!(
                        generation,
                        attempt = index + 1,
                        domain = %candidate,
                        elapsed_ms = attempt_started.elapsed().as_millis() as u64,
                        error = %error,
                        "登录请求失败，停止换线"
                    );
                    return Err(error);
                }
            }
        }
        // 登录候选线路全部失败：同样触发一次后台静默重解析。
        self.spawn_domain_refresh_in_background();
        tracing::warn!(
            generation,
            candidate_count = candidates.len(),
            "登录候选线路全部失败"
        );

        Err(last_error
            .unwrap_or_else(|| anyhow!("没有可用线路"))
            .context("连接失败，请在设置 → 网络中手动切换线路"))
    }

    fn commit_login(
        &self,
        base: &Config,
        mode: ApiDomainMode,
        mut profile: GetUserProfileRespData,
        generation: u64,
    ) -> anyhow::Result<LoginRespData> {
        anyhow::ensure!(
            !profile.uid.is_empty() && !profile.s.is_empty(),
            "登录响应缺少有效账号或会话"
        );
        let config = self.app.get_config();
        let (saved, epoch) = {
            let mut current = config.write();
            anyhow::ensure!(
                generation == self.auth_generation.load(Ordering::SeqCst),
                "登录已取消"
            );
            anyhow::ensure!(
                current.get_api_domain() == base.get_api_domain()
                    && current.proxy_mode == base.proxy_mode
                    && current.proxy_host == base.proxy_host
                    && current.proxy_port == base.proxy_port,
                "登录期间网络设置已改变，请重试"
            );
            let mut next = current.clone();
            next.api_domain_mode = mode;
            let saved = if next.api_domain_mode != current.api_domain_mode {
                crate::config_updates::commit_locked(&self.app, &mut current, next)?
            } else {
                current.clone()
            };
            let token = profile.s.clone();
            profile.s.clear();
            profile.jar.clear();
            profile.photo = absolute_profile_photo(&current.get_image_domain(), &profile.photo);
            let mut state = self.session.write();
            state.epoch += 1;
            state.token = token;
            state.profile = Some(profile.clone());
            (saved, state.epoch)
        };
        crate::config_updates::publish(&self.app, &saved);
        let _ = SessionChangedEvent {
            session_epoch: epoch,
            profile: Some(profile.clone()),
        }
        .emit(&self.app);
        Ok(LoginRespData {
            profile,
            session_epoch: epoch,
            switched_api_domain_mode: (mode != base.api_domain_mode).then_some(mode),
        })
    }

    pub async fn get_user_profile(&self) -> anyhow::Result<GetUserProfileRespData> {
        let snapshot = self.session_snapshot();
        let expected = snapshot.profile.context("尚未登录，请登录后重试")?;
        let mut profile: GetUserProfileRespData = self
            .post_without_retry_parsed(ApiPath::Login, None, None, "刷新用户资料")
            .await?;
        let config = self.app.get_config();
        {
            let config = config.read();
            let mut session = self.session.write();
            anyhow::ensure!(
                session.epoch == snapshot.session_epoch
                    && session.profile.is_some()
                    && profile.uid == expected.uid,
                "账号已改变，忽略过期用户资料"
            );
            if !profile.s.is_empty() {
                session.token = profile.s.clone();
            }
            profile.s.clear();
            profile.jar.clear();
            profile.photo = absolute_profile_photo(&config.get_image_domain(), &profile.photo);
            session.profile = Some(profile.clone());
        }
        let _ = SessionChangedEvent {
            session_epoch: snapshot.session_epoch,
            profile: Some(profile.clone()),
        }
        .emit(&self.app);
        Ok(profile)
    }

    async fn refresh_session(&self, epoch: u64) -> anyhow::Result<()> {
        let _guard = self.auth_lock.lock().await;
        if self.session.read().epoch != epoch {
            return Ok(());
        }
        anyhow::ensure!(
            self.session.read().profile.is_some(),
            "会话已退出，请重新登录"
        );
        let config = self.app.get_config().read().clone();
        anyhow::ensure!(
            config.auto_login && !config.username.is_empty() && !config.password.is_empty(),
            "登录已失效，请重新登录"
        );
        let mut cancellation = self.auth_cancel.subscribe();
        let generation = self.auth_generation.load(Ordering::SeqCst);
        tokio::select! {
            result = tokio::time::timeout(Duration::from_secs(35), self.login_candidates(&config, &config.username, &config.password, generation)) => {
                result.map_err(|_| anyhow!("刷新登录超时"))??; Ok(())
            },
            _ = cancellation.changed() => Err(anyhow!("会话刷新已取消")),
        }
    }

    pub async fn set_favorite(
        &self,
        aid: i64,
        desired: bool,
        session_epoch: u64,
    ) -> anyhow::Result<FavoriteChangedEvent> {
        anyhow::ensure!(aid > 0, "本地导入漫画不能加入云端收藏");
        let _guard = self.favorite_lock.lock().await;
        self.ensure_session_epoch(session_epoch)?;
        let before = self.get_comic(aid).await?;
        self.ensure_session_epoch(session_epoch)?;
        let actual = if before.is_favorite == desired {
            desired
        } else {
            match self.toggle_favorite_comic(aid).await {
                Ok(response) => matches!(response.toggle_type, ToggleType::Add),
                Err(error) => {
                    // 服务端可能已经执行；只读回查，不重复 toggle。
                    let verified = self.get_comic(aid).await;
                    self.ensure_session_epoch(session_epoch)?;
                    match verified {
                        Ok(comic) if comic.is_favorite == desired => desired,
                        Ok(comic) => {
                            let _ = FavoriteChangedEvent {
                                comic_id: aid,
                                is_favorite: comic.is_favorite,
                                session_epoch,
                            }
                            .emit(&self.app);
                            return Err(error.context("收藏操作未确认成功，已重新核对状态"));
                        }
                        Err(_) => {
                            return Err(
                                error.context("收藏结果未知，请刷新状态后重试；未重复提交操作")
                            )
                        }
                    }
                }
            }
        };
        self.ensure_session_epoch(session_epoch)?;
        let event = FavoriteChangedEvent {
            comic_id: aid,
            is_favorite: actual,
            session_epoch,
        };
        let _ = event.emit(&self.app);
        anyhow::ensure!(
            actual == desired,
            "服务端返回的收藏状态与目标不一致，请刷新后重试"
        );
        Ok(event)
    }

    fn ensure_session_epoch(&self, epoch: u64) -> anyhow::Result<()> {
        self.ensure_session_epoch_for(epoch, "收藏操作")
    }

    fn ensure_session_epoch_for(&self, epoch: u64, action: &str) -> anyhow::Result<()> {
        let state = self.session.read();
        anyhow::ensure!(
            state.epoch == epoch && state.profile.is_some(),
            "账号已改变或退出，请重新确认{action}"
        );
        Ok(())
    }

    /// 当前会话的用户 ID；未登录直接报错，避免拿空 uid 去请求签到接口。
    fn require_uid(&self) -> anyhow::Result<String> {
        let state = self.session.read();
        let uid = state.profile.as_ref().map(|p| p.uid.clone());
        uid.filter(|uid| !uid.is_empty())
            .context("尚未登录，请登录后签到")
    }

    /// 当月签到面板。`GET /daily?user_id=...`
    pub async fn get_daily_check_in(&self) -> anyhow::Result<DailyCheckInState> {
        let uid = self.require_uid()?;
        let resp: DailyCheckInRespData = self
            .jm_get_parsed(ApiPath::DailyCheckInStatus, Some(json!({ "user_id": uid })))
            .await?;
        Ok(DailyCheckInState::from_resp_data(resp))
    }

    /// 今日签到。`POST /daily_chk`
    ///
    /// 写接口语义：服务端对「今天已签过」同样回 200 + 一句文案，所以成败不能只看
    /// HTTP 状态，也不能匹配中文文案（文案随时会改）。做法是签到后重新拉一次面板，
    /// 用「已签天数是否增加」判定，并把最新面板一并返回供界面刷新。
    pub async fn daily_check_in(&self, session_epoch: u64) -> anyhow::Result<DailyCheckInOutcome> {
        let uid = self.require_uid()?;
        // 写操作按账号串行，避免连点造成重复提交；与收藏互不阻塞
        let _guard = self.daily_check_in_lock.lock().await;
        self.ensure_session_epoch_for(session_epoch, "签到")?;

        let before = self.get_daily_check_in().await?;
        anyhow::ensure!(
            before.daily_id > 0,
            "签到活动信息缺失，请在「我的」页下拉刷新后重试"
        );

        let payload = json!({ "user_id": uid, "daily_id": before.daily_id.to_string() });
        let result: DailyCheckInResultRespData = self
            .post_without_retry_parsed(ApiPath::DailyCheckIn, None, Some(payload), "签到")
            .await
            .context("签到请求失败，请稍后重试")?;

        // 回查：接口文案不可靠，以面板状态为准。回查失败时不能猜结果——
        // 服务端已受理，但界面显示的状态会是旧的，必须让用户知道要去确认。
        let after = match self.get_daily_check_in().await {
            Ok(state) => state,
            Err(error) => {
                let server_msg = result.msg.trim();
                let detail = if server_msg.is_empty() {
                    String::new()
                } else {
                    format!("（服务端返回：{server_msg}）")
                };
                return Err(error.context(format!(
                    "签到已提交{detail}，但刷新签到状态失败，请稍后下拉刷新确认"
                )));
            }
        };
        // 期间切换过账号（或退出）时，这份面板属于别的会话，不能交给界面显示
        self.ensure_session_epoch_for(session_epoch, "签到")?;
        let just_signed = after.signed_days > before.signed_days;

        let message = if !result.msg.trim().is_empty() {
            result.msg
        } else if just_signed {
            "签到成功".to_string()
        } else {
            "今日已签到".to_string()
        };

        if just_signed {
            // 金币与经验都挂靠在用户资料上，签到后需要重新拉取才能看到新数值
            if let Err(error) = self.get_user_profile().await {
                tracing::warn!(error = %error, "签到成功但刷新用户资料失败");
            }
        }

        Ok(DailyCheckInOutcome {
            just_signed,
            message,
            state: after,
        })
    }

    pub async fn search(
        &self,
        keyword: &str,
        page: i64,
        sort: SearchSort,
    ) -> anyhow::Result<SearchResp> {
        let query = json!({
            "main_tag": 0,
            "search_query": keyword,
            "page": page,
            "o": sort.as_str(),
        });
        let data = self
            .with_candidates(|api_domain, ts| {
                let query = query.clone();
                async move {
                    let http_resp = self
                        .jm_request_on(
                            reqwest::Method::GET,
                            ApiPath::Search,
                            Some(query),
                            None,
                            ts,
                            true,
                            &api_domain,
                        )
                        .await?;
                    parse_encrypted_raw_data(http_resp, ts, "搜索").await
                }
            })
            .await?;
        if let Ok(redirect_resp_data) = serde_json::from_str::<RedirectRespData>(&data) {
            let comic_resp_data = self.get_comic(redirect_resp_data.redirect_aid).await?;
            return Ok(SearchResp::ComicRespData(Box::new(comic_resp_data)));
        }
        if let Ok(search_resp_data) = serde_json::from_str::<SearchRespData>(&data) {
            return Ok(SearchResp::SearchRespData(search_resp_data));
        }
        Err(anyhow!(
            "将解密后的数据解析为SearchRespData或RedirectRespData失败: {data}"
        ))
    }

    pub async fn get_comic(&self, aid: i64) -> anyhow::Result<GetComicRespData> {
        let query = json!({"id": aid});
        self.jm_get_parsed(ApiPath::GetComic, Some(query)).await
    }

    pub async fn get_chapter(&self, id: i64) -> anyhow::Result<GetChapterRespData> {
        let query = json!({"id": id});
        self.jm_get_parsed(ApiPath::GetChapter, Some(query)).await
    }

    pub async fn get_scramble_id(&self, id: i64) -> anyhow::Result<i64> {
        let query_base = json!({
            "id": id,
            "mode": "vertical",
            "page": 0,
            "app_img_shunt": 1,
            "express": "off",
        });
        self.with_candidates(|api_domain, ts| {
            let mut query = query_base.clone();
            query["v"] = json!(ts);
            async move {
                let http_resp = self
                    .jm_request_on(
                        reqwest::Method::GET,
                        ApiPath::GetScrambleId,
                        Some(query),
                        None,
                        ts,
                        true,
                        &api_domain,
                    )
                    .await?;
                let status = http_resp.status();
                let body =
                    read_limited_text_body(http_resp, MAX_API_RESPONSE_BYTES, "API响应体").await?;
                if status != reqwest::StatusCode::OK {
                    return Err(api_failure(status, None, "获取scramble_id失败"));
                }
                // 解析失败必须报错而不是猜默认值：块数错误会让反混淆后的图片
                // “能打开但内容错乱”，静默写入会污染整章。
                body.split("var scramble_id = ")
                    .nth(1)
                    .and_then(|s| s.split(';').next())
                    .and_then(|s| s.trim().parse::<i64>().ok())
                    .context("解析scramble_id失败，页面结构可能已变化")
            }
        })
        .await
    }

    pub async fn get_favorite_folder(
        &self,
        folder_id: i64,
        page: i64,
        sort: FavoriteSort,
    ) -> anyhow::Result<GetFavoriteRespData> {
        let fetch = || async {
            let query = json!({
                "page": page,
                "o": sort.as_str(),
                "folder_id": folder_id,
            });
            self.jm_get_parsed::<GetFavoriteRespData>(ApiPath::GetFavoriteFolder, Some(query))
                .await
        };

        let epoch = self.session.read().epoch;
        match fetch().await {
            Ok(data) => Ok(data),
            Err(error) if is_authentication_error(&error) => {
                self.refresh_session(epoch).await?;
                fetch().await
            }
            Err(error) => Err(error),
        }
    }

    pub async fn get_weekly_info(&self) -> anyhow::Result<GetWeeklyInfoRespData> {
        self.jm_get_parsed(ApiPath::GetWeeklyInfo, None).await
    }

    pub async fn get_weekly(
        &self,
        category_id: &str,
        type_id: &str,
    ) -> anyhow::Result<GetWeeklyRespData> {
        let query = json!({
            "id": category_id,
            "type": type_id,
        });
        self.jm_get_parsed(ApiPath::GetWeekly, Some(query)).await
    }

    pub async fn toggle_favorite_comic(&self, aid: i64) -> anyhow::Result<ToggleFavoriteRespData> {
        let form = json!({
            "aid": aid,
        });
        self.post_without_retry_parsed(
            ApiPath::GetFavoriteFolder,
            None,
            Some(form),
            "收藏/取消收藏",
        )
        .await
    }

    pub async fn get_img_data_and_format(&self, url: &str) -> anyhow::Result<(Bytes, ImageFormat)> {
        // 与官方 App 一致的图片请求头，降低被 CDN 拒绝的概率
        let referer = format!("https://{}", self.app.get_config().read().get_api_domain());
        let build_request = || {
            self.img_client
                .read()
                .get(url)
                .header("user-agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/128.0.0.0 Safari/537.36")
                .header("accept", "image/avif,image/webp,image/apng,image/svg+xml,image/*,*/*;q=0.8")
                // 官方 App 的包名标识，CDN 据此放行；与本应用自身的包名无关，勿随品牌改名
                .header("x-requested-with", "com.JMComic3.app")
                .header("accept-language", "zh-CN,zh;q=0.9,en-US;q=0.8,en;q=0.7")
                .header("referer", referer.clone())
        };
        let http_resp = build_request().send().await.map_err(image_request_error)?;

        let status = http_resp.status();
        if status != StatusCode::OK {
            let text =
                read_limited_text_body(http_resp, MAX_ERROR_RESPONSE_BYTES, "图片错误响应体")
                    .await?;
            let text: String = text.chars().take(MAX_ERROR_BODY_CHARS).collect();
            let err = anyhow!("下载图片`{url}`失败，预料之外的状态码`{status}`: {text}");
            return Err(err);
        }

        let mut image_data = read_limited_image_body(http_resp).await?;

        if image_data.is_empty() {
            // 如果图片为空，说明jm那边缓存失效了，带上时间戳再次请求，以避免缓存
            let ts = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
            let query = json!({"ts": ts});
            let request = build_request().query(&query);

            let http_resp = request.send().await.map_err(image_request_error)?;
            let status = http_resp.status();
            if status != StatusCode::OK {
                let text =
                    read_limited_text_body(http_resp, MAX_ERROR_RESPONSE_BYTES, "图片错误响应体")
                        .await?;
                let text: String = text.chars().take(MAX_ERROR_BODY_CHARS).collect();
                let err = anyhow!("下载图片`{url}`失败，预料之外的状态码`{status}`: {text}");
                return Err(err);
            }

            image_data = read_limited_image_body(http_resp).await?;
        }

        let format = image::guess_format(&image_data)
            .context("无法从图片数据中猜测出图片格式，可能图片数据不完整或已损坏")?;

        Ok((image_data, format))
    }
}

fn image_request_error(error: reqwest_middleware::Error) -> anyhow::Error {
    if error.is_timeout() {
        anyhow::Error::from(error).context("图片请求超时，请检查网络或线路后重试")
    } else {
        anyhow::Error::from(error).context("图片请求失败")
    }
}

fn checked_accumulated_len(
    current_len: usize,
    chunk_len: usize,
    limit: usize,
) -> anyhow::Result<usize> {
    let next_len = current_len
        .checked_add(chunk_len)
        .context("响应体长度计算溢出")?;
    if next_len > limit {
        return Err(anyhow!(
            "响应体实际大小至少为`{next_len}`字节，超过上限`{limit}`字节"
        ));
    }
    Ok(next_len)
}

async fn read_limited_body(
    mut http_resp: reqwest::Response,
    limit: usize,
    body_name: &str,
) -> anyhow::Result<Bytes> {
    if let Some(content_length) = http_resp.content_length() {
        let limit_u64 = u64::try_from(limit).unwrap_or(u64::MAX);
        if content_length > limit_u64 {
            return Err(anyhow!(
                "{body_name}声明的大小`{content_length}`字节超过上限`{limit}`字节"
            ));
        }
    }

    let initial_capacity = http_resp
        .content_length()
        .and_then(|length| usize::try_from(length).ok())
        .map_or(0, |length| length.min(limit));
    let mut body = BytesMut::with_capacity(initial_capacity);
    while let Some(chunk) = http_resp.chunk().await.map_err(|error| {
        let reason = if error.is_timeout() {
            "读取超时"
        } else {
            "读取失败"
        };
        anyhow::Error::from(error).context(format!("{body_name}{reason}"))
    })? {
        checked_accumulated_len(body.len(), chunk.len(), limit)
            .context(format!("读取{body_name}失败"))?;
        body.extend_from_slice(&chunk);
    }
    Ok(body.freeze())
}

async fn read_limited_text_body(
    http_resp: reqwest::Response,
    limit: usize,
    body_name: &str,
) -> anyhow::Result<String> {
    let body = read_limited_body(http_resp, limit, body_name).await?;
    Ok(String::from_utf8_lossy(&body).into_owned())
}

async fn read_limited_image_body(http_resp: reqwest::Response) -> anyhow::Result<Bytes> {
    read_limited_body(http_resp, MAX_IMAGE_RESPONSE_BYTES, "图片响应体").await
}

fn timed_client_builder(total_timeout: Duration) -> reqwest::ClientBuilder {
    reqwest::ClientBuilder::new()
        .connect_timeout(Duration::from_secs(10))
        .read_timeout(Duration::from_secs(30))
        .timeout(total_timeout)
}

pub fn create_api_client(app: &AppHandle) -> ClientWithMiddleware {
    let client = create_api_reqwest_client(app);
    let retry_policy = ExponentialBackoff::builder()
        .base(1) // 指数为1，保证重试间隔为1秒不变
        .jitter(Jitter::Bounded) // 重试间隔在1秒左右波动
        .build_with_total_retry_duration(Duration::from_secs(5)); // 重试总时长为5秒

    reqwest_middleware::ClientBuilder::new(client)
        .with(RetryTransientMiddleware::new_with_policy(retry_policy))
        .build()
}

fn create_api_client_without_retry(app: &AppHandle) -> ClientWithMiddleware {
    reqwest_middleware::ClientBuilder::new(create_api_reqwest_client(app)).build()
}

fn create_api_reqwest_client(app: &AppHandle) -> reqwest::Client {
    let builder = timed_client_builder(Duration::from_secs(20));
    let builder = app
        .get_config()
        .read()
        .apply_proxy_best_effort(builder, "JmClient");
    builder.build().unwrap()
}

pub fn create_img_client(app: &AppHandle) -> ClientWithMiddleware {
    let builder = timed_client_builder(Duration::from_secs(120));
    let builder = app
        .get_config()
        .read()
        .apply_proxy_best_effort(builder, "JmClient图片");

    let retry_policy = ExponentialBackoff::builder().build_with_max_retries(2);

    reqwest_middleware::ClientBuilder::new(builder.build().unwrap())
        .with(RetryTransientMiddleware::new_with_policy(retry_policy))
        .build()
}

fn decrypt_data(ts: u64, data: &str) -> anyhow::Result<String> {
    let key = utils::md5_hex(&format!("{ts}{APP_DATA_SECRET}"));
    let decrypted =
        utils::aes256_ecb_decrypt_base64(key.as_bytes(), data).context("解密API响应失败")?;
    let decrypted = String::from_utf8(decrypted).context("解密后的数据不是UTF-8")?;
    Ok(decrypted)
}

/// 头像地址：接口通常只返回文件名（如 `u/xxx.jpg`），补全为图片域名下的完整地址；
/// 已带协议或为空的返回值保持原样，避免拼出 `https://cdn/media/users/https://...`。
fn absolute_profile_photo(image_domain: &str, photo: &str) -> String {
    let photo = photo.trim();
    if photo.is_empty() || photo.starts_with("http://") || photo.starts_with("https://") {
        return photo.to_string();
    }
    let photo = photo.trim_start_matches('/');
    format!("https://{image_domain}/media/users/{photo}")
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
fn load_cached_api_domains(app: &AppHandle) -> (Vec<String>, Option<u64>) {
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

fn preset_mode_for_domain(domain: &str) -> Option<ApiDomainMode> {
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

/// 请求官方域名更新地址并解密：去掉开头非 ASCII 字符 → base64 解码 →
/// AES-256-ECB 解密（key = md5(密钥)）→ JSON 中的 Server 字段。
async fn fetch_domain_list(client: &reqwest::Client, url: &str) -> anyhow::Result<Vec<String>> {
    let resp = client
        .get(url)
        .send()
        .await
        .context("请求域名更新地址失败")?;
    anyhow::ensure!(resp.status().is_success(), "HTTP状态码`{}`", resp.status());
    let text = resp.text().await.context("读取域名更新载荷失败")?;
    let cleaned: String = text.chars().skip_while(|c| !c.is_ascii()).collect();
    let cleaned = cleaned.trim();
    anyhow::ensure!(!cleaned.is_empty(), "域名更新载荷为空");

    let key = utils::md5_hex(API_DOMAIN_SERVER_SECRET);
    let decrypted = utils::aes256_ecb_decrypt_base64(key.as_bytes(), cleaned)
        .context("解密域名更新载荷失败")?;
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

fn is_valid_version(version: &str) -> bool {
    let parts: Vec<&str> = version.split('.').collect();
    (1..=4).contains(&parts.len())
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()))
}

fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    let parse = |version: &str| -> Vec<u64> {
        version
            .split('.')
            .map(|part| part.parse::<u64>().unwrap_or(0))
            .collect()
    };
    let mut left = parse(a);
    let mut right = parse(b);
    let length = left.len().max(right.len());
    left.resize(length, 0);
    right.resize(length, 0);
    left.cmp(&right)
}

async fn parse_encrypted_raw_data(
    http_resp: reqwest::Response,
    ts: u64,
    action_name: &str,
) -> anyhow::Result<String> {
    let status = http_resp.status();
    let body = read_limited_text_body(http_resp, MAX_API_RESPONSE_BYTES, "API响应体").await?;
    let value: serde_json::Value = match serde_json::from_str(&body) {
        Ok(value) => value,
        Err(_) => {
            // 非 JSON 响应通常是网关/拦截页面：截取开头作为特征判断能否换线路
            let snippet: String = body.chars().take(MAX_ERROR_BODY_CHARS).collect();
            return Err(api_failure(status, None, &snippet));
        }
    };
    let message = extract_api_error_message(&value);
    let code = value.get("code").and_then(serde_json::Value::as_i64);
    if status != StatusCode::OK || code != Some(200) {
        return Err(api_failure(
            status,
            code,
            message.as_deref().unwrap_or("服务器未返回成功状态"),
        ));
    }
    let jm_resp: JmResp = serde_json::from_value(value).context("服务器响应包络格式无效")?;
    let data = jm_resp
        .data
        .as_str()
        .context(format!("{action_name}失败，data字段不是字符串"))?;
    decrypt_data(ts, data)
}

pub(crate) async fn parse_encrypted_resp<T: serde::de::DeserializeOwned>(
    http_resp: reqwest::Response,
    ts: u64,
    action_name: &str,
) -> anyhow::Result<T> {
    let data = parse_encrypted_raw_data(http_resp, ts, action_name).await?;
    let result = serde_json::from_str::<T>(&data).context("解密后的响应字段格式无效")?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::{
        absolute_profile_photo, api_failure, checked_accumulated_len, is_plausible_domain,
        is_route_unreachable,
    };

    #[test]
    fn profile_photo_becomes_absolute_only_once() {
        assert_eq!(
            absolute_profile_photo("img.example.com", "u/123.jpg"),
            "https://img.example.com/media/users/u/123.jpg"
        );
        assert_eq!(
            absolute_profile_photo("img.example.com", "https://other.example/x.jpg"),
            "https://other.example/x.jpg"
        );
        assert_eq!(absolute_profile_photo("img.example.com", ""), "");
    }

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
    fn blocked_and_gateway_responses_are_retryable_on_other_lines() {
        use reqwest::StatusCode;
        let blocked = api_failure(StatusCode::OK, None, "<html>Access Denied</html>");
        assert!(is_route_unreachable(&blocked));
        // 域名被停放商接管：HTTP 200 + HTML 跳转页，必须换线路而不是直接失败
        let parked = api_failure(
            StatusCode::OK,
            None,
            r#"<!DOCTYPE html><html><head><script>window.onload=function(){window.location.href="/lander"}</script></head></html>"#,
        );
        assert!(is_route_unreachable(&parked));
        // 业务响应是 JSON：不能因为报文里出现 html 字样就误判
        let business = api_failure(StatusCode::OK, Some(201), "目标漫画不存在");
        assert!(!is_route_unreachable(&business));
        let forbidden = api_failure(StatusCode::FORBIDDEN, None, "forbidden");
        assert!(is_route_unreachable(&forbidden));
        let gateway = api_failure(StatusCode::BAD_GATEWAY, None, "bad gateway");
        assert!(is_route_unreachable(&gateway));
        // 某条镜像线路不提供该接口时回 405：换线路可能恢复，不能直接判失败
        let method_not_allowed = api_failure(StatusCode::METHOD_NOT_ALLOWED, None, "");
        assert!(is_route_unreachable(&method_not_allowed));
        // 带业务包络的 405 是明确业务错误
        let business_405 = api_failure(StatusCode::METHOD_NOT_ALLOWED, Some(405), "不支持");
        assert!(!is_route_unreachable(&business_405));
        // 业务错误不能换线路重复提交
        let business = api_failure(StatusCode::OK, Some(201), "目标漫画不存在");
        assert!(!is_route_unreachable(&business));
        // 登录失效不换线路
        let auth = api_failure(StatusCode::FORBIDDEN, Some(401), "未登录");
        assert!(!is_route_unreachable(&auth));
    }

    #[test]
    fn accumulated_body_length_accepts_exact_limit() {
        assert_eq!(checked_accumulated_len(4, 6, 10).unwrap(), 10);
        assert_eq!(checked_accumulated_len(0, 0, 0).unwrap(), 0);
    }

    #[test]
    fn accumulated_body_length_rejects_limit_overrun_and_overflow() {
        assert!(checked_accumulated_len(10, 1, 10).is_err());
        assert!(checked_accumulated_len(usize::MAX, 1, usize::MAX).is_err());
    }

    #[tokio::test]
    async fn stalled_response_body_times_out_with_actionable_error() {
        use std::time::Duration;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 2048];
            assert!(socket.read(&mut request).await.unwrap() > 0);
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nx")
                .await
                .unwrap();
            std::future::pending::<()>().await;
        });
        let result = tokio::time::timeout(Duration::from_secs(3), async {
            let client = super::timed_client_builder(Duration::from_secs(2))
                .read_timeout(Duration::from_millis(50))
                .no_proxy()
                .build()
                .unwrap();
            let response = client
                .get(format!("http://{address}"))
                .send()
                .await
                .unwrap();
            super::read_limited_body(response, 100, "测试响应体")
                .await
                .unwrap_err()
        })
        .await;
        server.abort();
        assert!(format!("{:#}", result.unwrap()).contains("读取超时"));
    }
}

#[cfg(test)]
mod review_regression_tests {
    use super::*;
    #[test]
    fn messages_are_soft_fields_and_business_errors_do_not_switch_routes() {
        for body in [
            r#"{"code":200,"data":"x","errorMsg":null,"message":"ok","msg":""}"#,
            r#"{"code":200,"data":"x","errorMsg":{},"msg":17}"#,
        ] {
            let response: JmResp = serde_json::from_str(body).unwrap();
            assert_eq!(response.code, 200);
        }
        assert_eq!(
            extract_api_error_message(&json!({"errorMsg": null, "msg": "  帳號或密碼錯誤  "}))
                .as_deref(),
            Some("帳號或密碼錯誤")
        );
        assert!(!is_route_unreachable(&api_failure(
            StatusCode::UNAUTHORIZED,
            None,
            "密码错误"
        )));
        // 403 + 业务包络（如"拒绝访问"）是明确业务错误，不应换线路重复提交；
        // 无包络的 403 才按拦截页处理（见 tests::blocked_and_gateway_responses_are_retryable_on_other_lines）
        assert!(!is_route_unreachable(&api_failure(
            StatusCode::FORBIDDEN,
            Some(403),
            "拒绝访问"
        )));
        assert!(!is_route_unreachable(&api_failure(
            StatusCode::SERVICE_UNAVAILABLE,
            Some(400),
            "账号受限"
        )));
        assert!(is_route_unreachable(&api_failure(
            StatusCode::BAD_GATEWAY,
            None,
            "gateway"
        )));
        assert!(is_authentication_error(&api_failure(
            StatusCode::OK,
            Some(401),
            "未登录"
        )));
    }

    #[test]
    fn domain_update_payload_decodes_to_server_list() {
        // 2026-10 从官方地址抓取的真实载荷（含开头 BOM 与封装），
        // 验证「去非 ASCII 前缀 → base64 → AES-256-ECB → Server 字段」链路。
        let payload = "\u{feff}X+bnzYIcwF6C7Rd3T7njPDNH08zsH9zyqCrrjCr7qcnHb1LsmIZGIHtrNVR/GiraHE6OuhvrxEzwciVvhdU0I9OYcmWTxF1K7fLfcwkn7kMQg2DZ2qpE7dKGkqKCmQijaSUOswxL1/p9pSVe/vRYEzbB5pfcAB6Yz/zVVIendBJK629QiqQndRXM9bijtZuYJtKw3YBAA26a+fy06dNszfw9v/4R8akVaSTWLOJc0nJy+9vm2t2W997vcqFL91iklKuKVEZHTtdpaLTgWExXaLjtIz2zlVZfy3jYrzKZ7x+LL7o02c6WB4HV69s1VqCJYl+3l3RNwDjJ0iRNnG9p/caZL/y6sT8i78Wc38WZhOAxkDsOFiGNpvS3eojKA0wGhpTtWsuUrXgSZ7ilEPttJvOhrJGGRJJ7Ux4HgzDYO1A=";
        let cleaned: String = payload.chars().skip_while(|c| !c.is_ascii()).collect();
        let key = utils::md5_hex(API_DOMAIN_SERVER_SECRET);
        let decrypted = utils::aes256_ecb_decrypt_base64(key.as_bytes(), cleaned.trim()).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&decrypted).unwrap();
        let servers: Vec<&str> = value["Server"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|server| server.as_str())
            .collect();
        assert!(!servers.is_empty());
        assert!(servers.iter().all(|server| server.contains('.')));
    }

    #[test]
    fn version_helpers_validate_and_compare_versions() {
        assert!(is_valid_version("2.1.7"));
        assert!(is_valid_version("2.0.13.1"));
        assert!(!is_valid_version(""));
        assert!(!is_valid_version("2.x"));
        assert!(!is_valid_version("2."));
        assert_eq!(
            compare_versions("2.1.7", "2.0.13"),
            std::cmp::Ordering::Greater
        );
        assert_eq!(
            compare_versions("2.0.13", "2.0.13"),
            std::cmp::Ordering::Equal
        );
        assert_eq!(compare_versions("1.9", "2.0.1"), std::cmp::Ordering::Less);
        assert_eq!(compare_versions("2.0", "2.0.0"), std::cmp::Ordering::Equal);
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
}

#[cfg(test)]
mod api_domain_cache_tests {
    use super::{parse_cached_domains, API_DOMAINS_CACHE_MAX_ENTRIES};

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
