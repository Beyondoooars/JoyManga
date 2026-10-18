use std::fs::OpenOptions;
use std::path::{Path, PathBuf};

use anyhow::{ensure, Context};

use crate::types::{CbzMode, DownloadFormat, ProxyMode, ReadingDirection};
use crate::{atomic_fs, utils};
use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::{AppHandle, Manager};

const API_DOMAIN_1: &str = "www.cdnzack.cc";
const API_DOMAIN_2: &str = "www.cdnhth.cc";
const API_DOMAIN_3: &str = "www.cdnhth.net";
const API_DOMAIN_4: &str = "www.cdnbea.net";
const API_DOMAIN_5: &str = "www.cdn-mspjmapiproxy.xyz";

// 图片CDN域名，来自官方参考实现的移动端图片域名列表
const IMAGE_DOMAIN_1: &str = "cdn-msp.jmapiproxy1.cc";
const IMAGE_DOMAIN_2: &str = "cdn-msp.jmapiproxy2.cc";
const IMAGE_DOMAIN_3: &str = "cdn-msp2.jmapiproxy2.cc";
const IMAGE_DOMAIN_4: &str = "cdn-msp3.jmapiproxy2.cc";
const IMAGE_DOMAIN_5: &str = "cdn-msp.jmapinodeudzn.net";
const IMAGE_DOMAIN_6: &str = "cdn-msp3.jmapinodeudzn.net";

const CONFIG_FILE_NAME: &str = "config.json";
const LOGS_DIR_NAME: &str = "日志";
const PORTABLE_LOGS_DIR_NAME: &str = "logs";
const PORTABLE_CACHE_DIR_NAME: &str = "cache";
const WEBVIEW_DATA_DIR_NAME: &str = "webview2";

#[derive(Debug)]
pub struct ConfigStorage {
    config_path: PathBuf,
    logs_dir: PathBuf,
    webview_data_dir: PathBuf,
    app_data_dir: PathBuf,
}

impl ConfigStorage {
    pub fn new(app: &AppHandle) -> anyhow::Result<Self> {
        let app_data_dir = app.path().app_data_dir()?;
        let app_local_data_dir = app.path().app_local_data_dir()?;
        std::fs::create_dir_all(&app_data_dir)
            .context(format!("创建应用数据目录`{}`失败", app_data_dir.display()))?;

        let config_path = if cfg!(target_os = "windows") {
            let executable_path = std::env::current_exe().context("无法定位当前可执行文件")?;
            select_config_path(
                &app_data_dir,
                &executable_path,
                true,
                verify_directory_writable,
            )?
        } else {
            app_data_dir.join(CONFIG_FILE_NAME)
        };

        let logs_dir = select_logs_dir(&app_data_dir, &config_path)?;
        let logs_dir = (|| -> anyhow::Result<PathBuf> {
            let logs_parent = logs_dir
                .parent()
                .context(format!("日志目录`{}`没有父目录", logs_dir.display()))?;
            let logs_name = logs_dir
                .file_name()
                .context(format!("日志目录`{}`没有目录名", logs_dir.display()))?;
            utils::create_dir_all_beneath(logs_parent, Path::new(logs_name))
        })()
        .unwrap_or_else(|err| {
            tracing::warn!(error = %err, "无法创建选定的日志目录，回退到应用数据目录");
            let fallback = app_data_dir.join(LOGS_DIR_NAME);
            let _ = std::fs::create_dir_all(&fallback);
            fallback
        });
        let webview_data_dir =
            select_webview_data_dir(&app_data_dir, &app_local_data_dir, &config_path)?;
        let webview_data_dir = if webview_data_dir == app_local_data_dir {
            std::fs::create_dir_all(&webview_data_dir).context(format!(
                "创建WebView2数据目录`{}`失败",
                webview_data_dir.display()
            ))?;
            std::fs::canonicalize(&webview_data_dir).unwrap_or(webview_data_dir)
        } else {
            let portable_root = config_path
                .parent()
                .context(format!("配置`{}`没有父目录", config_path.display()))?;
            utils::create_dir_all_beneath(
                portable_root,
                &Path::new(PORTABLE_CACHE_DIR_NAME).join(WEBVIEW_DATA_DIR_NAME),
            )?
        };

        let storage = Self {
            config_path,
            logs_dir,
            webview_data_dir,
            app_data_dir,
        };
        Ok(storage)
    }

    pub fn config_path(&self) -> &Path {
        &self.config_path
    }

    pub fn config_dir(&self) -> &Path {
        self.config_path.parent().unwrap_or(&self.app_data_dir)
    }

    pub fn base_dir(&self) -> &Path {
        self.config_dir()
    }

    pub fn logs_dir(&self) -> &Path {
        &self.logs_dir
    }

    pub fn webview_data_dir(&self) -> &Path {
        &self.webview_data_dir
    }

    pub fn app_data_dir(&self) -> &Path {
        &self.app_data_dir
    }
}
fn select_logs_dir(app_data_dir: &Path, config_path: &Path) -> anyhow::Result<PathBuf> {
    if config_path == app_data_dir.join(CONFIG_FILE_NAME) {
        Ok(app_data_dir.join(LOGS_DIR_NAME))
    } else {
        Ok(config_path
            .parent()
            .context(format!("配置`{}`没有父目录", config_path.display()))?
            .join(PORTABLE_LOGS_DIR_NAME))
    }
}

fn select_webview_data_dir(
    app_data_dir: &Path,
    app_local_data_dir: &Path,
    config_path: &Path,
) -> anyhow::Result<PathBuf> {
    if config_path == app_data_dir.join(CONFIG_FILE_NAME) {
        Ok(app_local_data_dir.to_path_buf())
    } else {
        Ok(config_path
            .parent()
            .context(format!("配置`{}`没有父目录", config_path.display()))?
            .join(PORTABLE_CACHE_DIR_NAME)
            .join(WEBVIEW_DATA_DIR_NAME))
    }
}

fn select_config_path<F>(
    app_data_dir: &Path,
    executable_path: &Path,
    prefer_portable: bool,
    verify_writable: F,
) -> anyhow::Result<PathBuf>
where
    F: FnOnce(&Path) -> anyhow::Result<()>,
{
    let app_data_config_path = app_data_dir.join(CONFIG_FILE_NAME);
    if !prefer_portable {
        return Ok(app_data_config_path);
    }

    let executable_dir = executable_path.parent().context(format!(
        "可执行文件`{}`没有父目录",
        executable_path.display()
    ))?;
    let portable_config_path = executable_dir.join(CONFIG_FILE_NAME);

    if portable_config_path.exists() {
        verify_writable(executable_dir).context(format!(
            "便携配置`{}`已存在，但所在目录不可写",
            portable_config_path.display()
        ))?;
        return Ok(portable_config_path);
    }

    match verify_writable(executable_dir) {
        Ok(()) => Ok(portable_config_path),
        Err(err) => {
            tracing::warn!(
                executable_dir = %executable_dir.display(),
                error = %err,
                "可执行文件目录不可写，将配置保存在应用数据目录"
            );
            Ok(app_data_config_path)
        }
    }
}

fn verify_directory_writable(directory: &Path) -> anyhow::Result<()> {
    let probe_id = uuid::Uuid::new_v4();
    let source_path = directory.join(format!(".joymanga-config-probe-{probe_id}.part"));
    let target_path = directory.join(format!(".joymanga-config-probe-{probe_id}.done"));

    let result = (|| -> anyhow::Result<()> {
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&source_path)
            .context(format!("无法在`{}`创建临时文件", directory.display()))?;
        file.sync_all().context(format!(
            "同步配置目录写入探针`{}`失败",
            source_path.display()
        ))?;
        drop(file);
        std::fs::rename(&source_path, &target_path)
            .context(format!("无法在`{}`重命名临时文件", directory.display()))?;
        std::fs::remove_file(&target_path)
            .context(format!("无法在`{}`删除临时文件", directory.display()))?;
        Ok(())
    })();

    if result.is_err() {
        let _ = std::fs::remove_file(&source_path);
        let _ = std::fs::remove_file(&target_path);
    }
    result
}

fn validate_custom_domain(value: &str, label: &str) -> anyhow::Result<()> {
    let domain = value.trim();
    ensure!(!domain.is_empty(), "{label}不能为空");
    ensure!(
        !domain.contains("://")
            && !domain.contains('/')
            && !domain.chars().any(char::is_whitespace),
        "{label}只能填写主机名，不能包含协议、路径或空格"
    );
    Ok(())
}

/// 解析自定义域名：去除首尾空白，为空时回退到默认线路。
/// 用于防御用户绕过校验、手改配置文件导致空域名的情况。
fn resolve_custom_domain(value: &str, fallback: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        fallback.to_string()
    } else {
        value.to_string()
    }
}

fn default_true() -> bool {
    true
}

fn migrate_config_schema(value: &mut serde_json::Value) {
    let Some(object) = value.as_object_mut() else {
        return;
    };
    if object
        .get("schemaVersion")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0)
        != 0
    {
        return;
    }
    // 无版本的旧配置无法区分 0.2.2/0.2.3；保留当前版本升级前实际使用的域名，并提示用户核对。
    if let Some(mode) = object
        .get("apiDomainMode")
        .and_then(serde_json::Value::as_str)
    {
        let stable = match mode {
            "Domain1" => "Domain2",
            "Domain2" => "Domain3",
            "Domain3" => "Domain4",
            "Domain4" => "Domain1",
            other => other,
        };
        let pending = stable != "Custom";
        object.insert(
            "apiDomainMode".into(),
            serde_json::Value::String(stable.to_string()),
        );
        object.insert(
            "apiDomainMigrationPending".into(),
            serde_json::Value::Bool(pending),
        );
    }
    let remember = object
        .get("username")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|s| !s.is_empty())
        && object
            .get("password")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|s| !s.is_empty());
    object
        .entry("autoLogin")
        .or_insert(serde_json::Value::Bool(remember));
    object
        .entry("androidStorageInitialized")
        .or_insert(serde_json::Value::Bool(true));
    object.insert("schemaVersion".into(), serde_json::Value::from(1));
}

/// 将配置序列化为磁盘存储格式（明文JSON保存，便于用户查看与自用）。
fn serialize_config_for_storage(config: &Config) -> anyhow::Result<String> {
    serde_json::to_string_pretty(config).context("序列化配置失败")
}

/// 从磁盘存储格式反序列化配置（若包含旧版加密凭据则自动迁移为明文）。
fn deserialize_config_from_storage(config_string: &str) -> anyhow::Result<Config> {
    let mut value: serde_json::Value =
        serde_json::from_str(config_string).context("解析配置JSON失败")?;
    migrate_config_schema(&mut value);
    serde_json::from_value(value).context("将配置JSON反序列化为Config失败")
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    #[serde(default)]
    pub revision: u64,
    #[serde(default)]
    pub schema_version: u32,
    #[serde(default)]
    pub api_domain_migration_pending: bool,
    #[serde(default)]
    pub auto_login: bool,
    #[serde(default = "default_true")]
    pub android_storage_initialized: bool,
    pub username: String,
    pub password: String,
    pub download_dir: PathBuf,
    pub export_dir: PathBuf,
    pub download_format: DownloadFormat,
    pub dir_fmt: String,
    pub proxy_mode: ProxyMode,
    pub proxy_host: String,
    pub proxy_port: u16,
    pub enable_file_logger: bool,
    pub chapter_concurrency: usize,
    pub chapter_download_interval_sec: u64,
    pub img_concurrency: usize,
    pub img_download_interval_sec: u64,
    pub download_all_favorites_interval_sec: u64,
    pub update_downloaded_comics_interval_sec: u64,
    pub api_domain_mode: ApiDomainMode,
    pub custom_api_domain: String,
    pub image_domain_mode: ImageDomainMode,
    pub custom_image_domain: String,
    pub should_download_cover: bool,
    /// 下载完成后 CBZ 自动整合的默认方式（单次下载可在界面上另行选择）。
    #[serde(default)]
    pub default_cbz_mode: CbzMode,
    /// 阅读方向：决定点击区域、键盘、滑动与翻页按钮的语义。
    #[serde(default)]
    pub reading_direction: ReadingDirection,
    /// 阅读时保持屏幕常亮（移动端与桌面端均生效）。
    #[serde(default = "default_true")]
    pub keep_screen_on: bool,
    /// Android 音量键翻页（仅移动端有意义，桌面端忽略）。
    #[serde(default)]
    pub volume_key_paging: bool,
    /// 仅在非计费网络（通常为 Wi-Fi）下允许下载。桌面端无网络计费概念，恒放行。
    #[serde(default)]
    pub download_only_wifi: bool,
}

impl Config {
    pub fn apply_storage_policy(&mut self) {
        self.export_dir.clone_from(&self.download_dir);
    }
    pub fn apply_download_policy(&mut self) {
        // 原始格式策略：未混淆图片按源格式原样落盘，重组图片使用无损 WebP，
        // 相比统一转 PNG 显著降低体积与 CPU 开销，同时不引入有损压缩。
        self.download_format = DownloadFormat::Original;
        self.chapter_download_interval_sec = 0;
        self.img_download_interval_sec = 0;
        self.download_all_favorites_interval_sec = 0;
        self.update_downloaded_comics_interval_sec = 0;
    }
    pub fn new(storage: &ConfigStorage) -> anyhow::Result<Self> {
        let app_data_dir = storage.app_data_dir();
        let base_dir = storage.base_dir();
        let config_path = storage.config_path();

        atomic_fs::recover_file_backup(config_path)?;

        let (mut config, repaired_config) = if config_path.exists() {
            let config_string = std::fs::read_to_string(config_path)?;
            let (mut cfg, repaired) = match deserialize_config_from_storage(&config_string) {
                Ok(config) if Config::validate(&config).is_ok() => (config, false),
                // 缺字段、字段类型错误或值越界时逐字段恢复，保留其余有效配置。
                Ok(_) | Err(_) => (Config::merge_config(&config_string, base_dir), true),
            };
            if cfg.download_dir == app_data_dir.join("漫画下载") {
                cfg.download_dir = base_dir.join("漫画下载");
            }
            if cfg.export_dir == app_data_dir.join("漫画导出") {
                cfg.export_dir = base_dir.join("漫画导出");
            }
            (cfg, repaired)
        } else {
            (Config::default(base_dir), false)
        };

        if repaired_config {
            let backup_path = config_path
                .parent()
                .context(format!("配置`{}`没有父目录", config_path.display()))?
                .join(format!("config.invalid-{}.json", uuid::Uuid::new_v4()));
            let config_string = std::fs::read_to_string(config_path)?;
            atomic_fs::write(&backup_path, config_string).context(format!(
                "备份需要修复的配置到`{}`失败",
                backup_path.display()
            ))?;
            tracing::warn!(
                backup_path = %backup_path.display(),
                "检测到无效或旧版配置，已逐字段恢复并保留原文件备份"
            );
        }
        config.apply_download_policy();
        config.apply_storage_policy();
        config.validate()?;
        // 启动期回写失败（只读目录、磁盘满、杀软占用）不应让应用无法打开：
        // 内存中已有可用配置，下次保存或启动时会再次尝试。
        if let Err(error) = config.save_to(config_path) {
            tracing::warn!(error = %error, "启动时保存配置失败，本次使用内存中的配置");
        }
        Ok(config)
    }

    pub fn save(&self, app: &AppHandle) -> anyhow::Result<()> {
        let storage = app.state::<ConfigStorage>();
        self.save_to(storage.config_path())
    }

    fn save_to(&self, config_path: &Path) -> anyhow::Result<()> {
        self.validate()?;
        let config_string = serialize_config_for_storage(self)?;
        atomic_fs::write(config_path, config_string)
            .context(format!("原子写入配置`{}`失败", config_path.display()))?;
        Ok(())
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        ensure!(
            !self.download_dir.as_os_str().is_empty(),
            "下载目录不能为空"
        );
        ensure!(!self.export_dir.as_os_str().is_empty(), "导出目录不能为空");
        ensure!(
            (1..=10).contains(&self.chapter_concurrency),
            "章节并发数必须在1到10之间（移动端建议1~3）"
        );
        ensure!(
            (1..=50).contains(&self.img_concurrency),
            "图片并发数必须在1到50之间（移动端建议3~10）"
        );

        for (name, value) in [
            ("章节下载间隔", self.chapter_download_interval_sec),
            ("图片下载间隔", self.img_download_interval_sec),
            (
                "下载整个收藏夹间隔",
                self.download_all_favorites_interval_sec,
            ),
            (
                "更新本地库存间隔",
                self.update_downloaded_comics_interval_sec,
            ),
        ] {
            ensure!(value <= 3_600, "{name}不能超过3600秒（1小时）");
        }

        let dir_fmt = self.dir_fmt.trim();
        ensure!(!dir_fmt.is_empty(), "下载目录格式不能为空");
        ensure!(
            dir_fmt
                .split('/')
                .filter(|part| !part.trim().is_empty())
                .count()
                >= 2,
            "下载目录格式至少需要两个层级"
        );
        ensure!(
            ["{comic_id}", "{comic_title}"]
                .iter()
                .any(|key| dir_fmt.contains(key)),
            "下载目录格式必须包含comic_id或comic_title"
        );
        ensure!(
            ["{chapter_id}", "{chapter_title}", "{order}"]
                .iter()
                .any(|key| dir_fmt.contains(key)),
            "下载目录格式必须包含chapter_id、chapter_title或order"
        );
        utils::validate_chapter_directory_format(dir_fmt)?;

        if self.api_domain_mode == ApiDomainMode::Custom {
            validate_custom_domain(&self.custom_api_domain, "自定义API域名")?;
        }

        if self.image_domain_mode == ImageDomainMode::Custom {
            validate_custom_domain(&self.custom_image_domain, "自定义图片域名")?;
        }

        if self.proxy_mode == ProxyMode::Custom {
            ensure!(!self.proxy_host.trim().is_empty(), "代理主机不能为空");
            ensure!(self.proxy_port > 0, "代理端口必须大于0");
        }

        Ok(())
    }

    pub fn get_api_domain(&self) -> String {
        match self.api_domain_mode {
            ApiDomainMode::Domain1 => API_DOMAIN_1.to_string(),
            ApiDomainMode::Domain2 => API_DOMAIN_2.to_string(),
            ApiDomainMode::Domain3 => API_DOMAIN_3.to_string(),
            ApiDomainMode::Domain4 => API_DOMAIN_4.to_string(),
            ApiDomainMode::Domain5 => API_DOMAIN_5.to_string(),
            ApiDomainMode::Custom => resolve_custom_domain(&self.custom_api_domain, API_DOMAIN_2),
        }
    }

    pub fn get_image_domain(&self) -> String {
        match self.image_domain_mode {
            ImageDomainMode::Domain1 => IMAGE_DOMAIN_1.to_string(),
            ImageDomainMode::Domain2 => IMAGE_DOMAIN_2.to_string(),
            ImageDomainMode::Domain3 => IMAGE_DOMAIN_3.to_string(),
            ImageDomainMode::Domain4 => IMAGE_DOMAIN_4.to_string(),
            ImageDomainMode::Domain5 => IMAGE_DOMAIN_5.to_string(),
            ImageDomainMode::Domain6 => IMAGE_DOMAIN_6.to_string(),
            ImageDomainMode::Custom => {
                resolve_custom_domain(&self.custom_image_domain, IMAGE_DOMAIN_3)
            }
        }
    }

    pub(crate) fn merge_config(config_string: &str, base_dir: &Path) -> Config {
        let default_config = Config::default(base_dir);
        let Ok(mut user_value) = serde_json::from_str::<serde_json::Value>(config_string) else {
            return default_config;
        };
        migrate_config_schema(&mut user_value);
        let serde_json::Value::Object(user_map) = user_value else {
            return default_config;
        };
        let Ok(serde_json::Value::Object(mut accepted_map)) = serde_json::to_value(&default_config)
        else {
            return default_config;
        };

        for (key, value) in user_map {
            if !accepted_map.contains_key(&key) {
                continue;
            }

            let mut candidate_map = accepted_map.clone();
            candidate_map.insert(key, value);
            let candidate_value = serde_json::Value::Object(candidate_map.clone());
            let Ok(candidate) = serde_json::from_value::<Config>(candidate_value) else {
                continue;
            };
            if candidate.validate().is_ok() {
                accepted_map = candidate_map;
            }
        }

        serde_json::from_value(serde_json::Value::Object(accepted_map)).unwrap_or(default_config)
    }

    pub fn default(base_dir: &Path) -> Config {
        let (download_dir, export_dir) = if cfg!(target_os = "android") {
            (base_dir.join("library"), base_dir.join("library"))
        } else {
            (base_dir.join("漫画下载"), base_dir.join("漫画下载"))
        };

        let (chapter_concurrency, img_concurrency) = if cfg!(target_os = "android") {
            (1, 4)
        } else {
            (2, 15)
        };

        Config {
            revision: 0,
            schema_version: 1,
            api_domain_migration_pending: false,
            auto_login: false,
            android_storage_initialized: !cfg!(target_os = "android"),
            username: String::new(),
            password: String::new(),
            download_dir,
            export_dir,
            download_format: DownloadFormat::default(),
            dir_fmt: "{comic_title}/{chapter_title}".to_string(),
            proxy_mode: ProxyMode::default(),
            proxy_host: "127.0.0.1".to_string(),
            proxy_port: 7890,
            // Android 上文件日志持续写入会消耗电量和存储，默认关闭
            enable_file_logger: !cfg!(target_os = "android"),
            chapter_concurrency,
            chapter_download_interval_sec: 0,
            img_concurrency,
            img_download_interval_sec: 0,
            download_all_favorites_interval_sec: 0,
            update_downloaded_comics_interval_sec: 0,
            api_domain_mode: ApiDomainMode::Domain2,
            custom_api_domain: API_DOMAIN_2.to_string(),
            image_domain_mode: ImageDomainMode::Domain3,
            custom_image_domain: IMAGE_DOMAIN_3.to_string(),
            should_download_cover: true,
            default_cbz_mode: CbzMode::default(),
            reading_direction: ReadingDirection::default(),
            keep_screen_on: true,
            volume_key_paging: false,
            download_only_wifi: false,
        }
    }

    pub fn apply_proxy(
        &self,
        builder: reqwest::ClientBuilder,
    ) -> anyhow::Result<reqwest::ClientBuilder> {
        match self.proxy_mode {
            ProxyMode::NoProxy => Ok(builder.no_proxy()),
            ProxyMode::System => Ok(builder),
            ProxyMode::Custom => {
                let proxy_url = format!("http://{}:{}", self.proxy_host, self.proxy_port);
                let proxy = reqwest::Proxy::all(&proxy_url)
                    .context(format!("创建代理`{proxy_url}`失败"))?;
                Ok(builder.proxy(proxy))
            }
        }
    }

    pub fn apply_proxy_best_effort(
        &self,
        builder: reqwest::ClientBuilder,
        client_name: &str,
    ) -> reqwest::ClientBuilder {
        match self.proxy_mode {
            ProxyMode::NoProxy => builder.no_proxy(),
            ProxyMode::System => builder,
            ProxyMode::Custom => {
                let proxy_url = format!("http://{}:{}", self.proxy_host, self.proxy_port);
                match reqwest::Proxy::all(&proxy_url) {
                    Ok(proxy) => builder.proxy(proxy),
                    Err(err) => {
                        let err_title = format!("`{client_name}`设置代理`{proxy_url}`失败");
                        tracing::error!(err_title, message = %err);
                        builder
                    }
                }
            }
        }
    }
}

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
pub enum ApiDomainMode {
    Domain1,
    #[default]
    Domain2,
    Domain3,
    Domain4,
    Domain5,
    Custom,
}

impl ApiDomainMode {
    pub fn preset_domain(self) -> Option<&'static str> {
        match self {
            Self::Domain1 => Some(API_DOMAIN_1),
            Self::Domain2 => Some(API_DOMAIN_2),
            Self::Domain3 => Some(API_DOMAIN_3),
            Self::Domain4 => Some(API_DOMAIN_4),
            Self::Domain5 => Some(API_DOMAIN_5),
            Self::Custom => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Type)]
pub struct ApiDomainPreset {
    pub mode: ApiDomainMode,
    pub label: String,
    pub domain: String,
}

pub fn api_domain_presets() -> Vec<ApiDomainPreset> {
    [
        ApiDomainMode::Domain2,
        ApiDomainMode::Domain3,
        ApiDomainMode::Domain4,
        ApiDomainMode::Domain1,
        ApiDomainMode::Domain5,
    ]
    .into_iter()
    .enumerate()
    .map(|(i, mode)| ApiDomainPreset {
        mode,
        label: format!("线路{}", i + 1),
        domain: mode.preset_domain().unwrap_or_default().to_string(),
    })
    .collect()
}

#[derive(Debug, Clone, Serialize, Type)]
pub struct ImageDomainPreset {
    pub mode: ImageDomainMode,
    pub label: String,
    pub domain: String,
}
pub fn image_domain_presets() -> Vec<ImageDomainPreset> {
    [
        ImageDomainMode::Domain1,
        ImageDomainMode::Domain2,
        ImageDomainMode::Domain3,
        ImageDomainMode::Domain4,
        ImageDomainMode::Domain5,
        ImageDomainMode::Domain6,
    ]
    .into_iter()
    .enumerate()
    .map(|(i, mode)| {
        let mut config = Config::default(Path::new("."));
        config.image_domain_mode = mode;
        ImageDomainPreset {
            mode,
            label: format!("线路{}", i + 1),
            domain: config.get_image_domain(),
        }
    })
    .collect()
}

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
pub enum ImageDomainMode {
    Domain1,
    Domain2,
    #[default]
    Domain3,
    Domain4,
    Domain5,
    Domain6,
    Custom,
}

#[cfg(test)]
mod tests {
    use super::{
        deserialize_config_from_storage, select_config_path, select_logs_dir,
        select_webview_data_dir, serialize_config_for_storage, ApiDomainMode, Config,
        ConfigStorage, ImageDomainMode, API_DOMAIN_2, CONFIG_FILE_NAME, IMAGE_DOMAIN_3,
        LOGS_DIR_NAME, PORTABLE_CACHE_DIR_NAME, PORTABLE_LOGS_DIR_NAME, WEBVIEW_DATA_DIR_NAME,
    };
    use anyhow::anyhow;
    use serde_json::json;
    use std::path::{Path, PathBuf};

    fn test_dir(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("joymanga-config-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn non_portable_mode_uses_app_data_without_probing_executable_dir() {
        let root = test_dir("non-portable");
        let app_data_dir = root.join("app-data");
        let executable_path = root.join("bin").join("app.exe");

        let selected = select_config_path(&app_data_dir, &executable_path, false, |_| {
            panic!("非便携模式不应探测可执行文件目录")
        })
        .unwrap();

        assert_eq!(selected, app_data_dir.join(CONFIG_FILE_NAME));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn writable_windows_release_uses_config_beside_executable() {
        let root = test_dir("portable");
        let app_data_dir = root.join("app-data");
        let executable_dir = root.join("portable");
        std::fs::create_dir_all(&executable_dir).unwrap();
        let executable_path = executable_dir.join("app.exe");

        let selected =
            select_config_path(&app_data_dir, &executable_path, true, |_| Ok(())).unwrap();

        assert_eq!(selected, executable_dir.join(CONFIG_FILE_NAME));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unifying_existing_paths_keeps_library_and_old_export_files() {
        let root = test_dir("unified-library");
        let library = root.join("existing-library");
        let old_exports = root.join("old-exports");
        std::fs::create_dir_all(&library).unwrap();
        std::fs::create_dir_all(&old_exports).unwrap();
        std::fs::write(library.join("chapter.cbz"), b"source").unwrap();
        std::fs::write(old_exports.join("old.cbz"), b"old export").unwrap();
        let storage = ConfigStorage {
            config_path: root.join(CONFIG_FILE_NAME),
            logs_dir: root.join("logs"),
            webview_data_dir: root.join("webview"),
            app_data_dir: root.join("app-data"),
        };
        let mut config = Config::default(&root);
        config.download_dir = library.clone();
        config.export_dir = old_exports.clone();
        config.save_to(storage.config_path()).unwrap();
        let loaded = Config::new(&storage).unwrap();
        assert_eq!(loaded.download_dir, library);
        assert_eq!(loaded.export_dir, loaded.download_dir);
        assert_eq!(
            std::fs::read(library.join("chapter.cbz")).unwrap(),
            b"source"
        );
        assert_eq!(
            std::fs::read(old_exports.join("old.cbz")).unwrap(),
            b"old export"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn portable_mode_defaults_download_and_export_dir_beside_executable() {
        let root = test_dir("portable-defaults");
        let portable_dir = root.join("bin");
        std::fs::create_dir_all(&portable_dir).unwrap();
        let storage = ConfigStorage {
            config_path: portable_dir.join(CONFIG_FILE_NAME),
            logs_dir: portable_dir.join("logs"),
            webview_data_dir: portable_dir.join("webview"),
            app_data_dir: root.join("app-data"),
        };
        let config = Config::new(&storage).unwrap();
        assert_eq!(config.download_dir, portable_dir.join("漫画下载"));
        assert_eq!(config.export_dir, config.download_dir);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn config_startup_recovers_interrupted_replacement_before_loading_defaults() {
        let root = test_dir("restore-backup");
        let path = root.join(CONFIG_FILE_NAME);
        let storage = ConfigStorage {
            config_path: path.clone(),
            logs_dir: root.join("logs"),
            webview_data_dir: root.join("webview"),
            app_data_dir: root.clone(),
        };
        let mut config = super::Config::default(&root);
        config.img_concurrency = 7;
        config.save_to(&path).unwrap();
        let backup = root.join(format!(".config.json.backup-{}", uuid::Uuid::new_v4()));
        std::fs::rename(&path, &backup).unwrap();
        assert_eq!(super::Config::new(&storage).unwrap().img_concurrency, 7);
        assert!(!backup.exists());
        config.dir_fmt = "{comic_title}/{chapter_id}/images".into();
        assert!(config.validate().is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn config_logs_and_webview_data_switch_storage_mode_together() {
        let root = test_dir("paired-storage");
        let app_data_dir = root.join("app-data");
        let app_local_data_dir = root.join("app-local-data");
        let portable_dir = root.join("portable");
        let app_data_config = app_data_dir.join(CONFIG_FILE_NAME);
        let portable_config = portable_dir.join(CONFIG_FILE_NAME);

        assert_eq!(
            select_logs_dir(&app_data_dir, &app_data_config).unwrap(),
            app_data_dir.join(LOGS_DIR_NAME)
        );
        assert_eq!(
            select_logs_dir(&app_data_dir, &portable_config).unwrap(),
            portable_dir.join(PORTABLE_LOGS_DIR_NAME)
        );
        assert_eq!(
            select_webview_data_dir(&app_data_dir, &app_local_data_dir, &app_data_config).unwrap(),
            app_local_data_dir.clone()
        );
        assert_eq!(
            select_webview_data_dir(&app_data_dir, &app_local_data_dir, &portable_config).unwrap(),
            portable_dir
                .join(PORTABLE_CACHE_DIR_NAME)
                .join(WEBVIEW_DATA_DIR_NAME)
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn protected_install_directory_falls_back_to_app_data() {
        let root = test_dir("protected-install");
        let app_data_dir = root.join("app-data");
        let executable_path = root.join("program-files").join("app.exe");

        let selected = select_config_path(&app_data_dir, &executable_path, true, |_| {
            Err(anyhow!("access denied"))
        })
        .unwrap();

        assert_eq!(selected, app_data_dir.join(CONFIG_FILE_NAME));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn existing_portable_config_must_not_silently_fall_back_when_read_only() {
        let root = test_dir("read-only-portable");
        let app_data_dir = root.join("app-data");
        let executable_dir = root.join("portable");
        std::fs::create_dir_all(&executable_dir).unwrap();
        let executable_path = executable_dir.join("app.exe");
        std::fs::write(executable_dir.join(CONFIG_FILE_NAME), "{}").unwrap();

        let error = select_config_path(&app_data_dir, &executable_path, true, |_| {
            Err(anyhow!("access denied"))
        })
        .unwrap_err();

        assert!(error.to_string().contains("已存在，但所在目录不可写"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn credentials_are_stored_in_plaintext() {
        let app_data_dir = Path::new("test-app-data");
        let mut config = Config::default(app_data_dir);
        config.username = "alice".to_string();
        config.password = "plain-password".to_string();

        // 磁盘存储序列化为明文，便于用户自用和查看
        let stored = serialize_config_for_storage(&config).unwrap();
        assert!(stored.contains("alice"));
        assert!(stored.contains("plain-password"));
        assert!(!stored.contains("enc:"));

        // 磁盘存储反序列化
        let restored = deserialize_config_from_storage(&stored).unwrap();
        assert_eq!(restored.username, "alice");
        assert_eq!(restored.password, "plain-password");
    }

    #[test]
    fn custom_domain_falls_back_to_default_when_empty() {
        let app_data_dir = Path::new("test-app-data");
        let mut config = Config::default(app_data_dir);

        config.api_domain_mode = ApiDomainMode::Custom;
        config.custom_api_domain = String::new();
        assert_eq!(config.get_api_domain(), API_DOMAIN_2);

        config.image_domain_mode = ImageDomainMode::Custom;
        config.custom_image_domain = "   ".to_string();
        assert_eq!(config.get_image_domain(), IMAGE_DOMAIN_3);

        // 非空自定义域名应去除首尾空白后返回
        config.custom_image_domain = "  custom-cdn.example.com  ".to_string();
        assert_eq!(config.get_image_domain(), "custom-cdn.example.com");
    }

    #[test]
    fn merge_config_keeps_valid_fields_when_one_field_has_wrong_type() {
        let app_data_dir = Path::new("test-app-data");
        let mut value = serde_json::to_value(Config::default(app_data_dir)).unwrap();
        let object = value.as_object_mut().unwrap();
        object.insert("username".into(), json!("alice"));
        object.insert("chapterConcurrency".into(), json!("not-a-number"));

        let merged = Config::merge_config(&value.to_string(), app_data_dir);

        assert_eq!(merged.username, "alice");
        assert_eq!(merged.chapter_concurrency, 2);
        merged.validate().unwrap();
    }

    #[test]
    fn merge_config_replaces_only_values_that_fail_validation() {
        let app_data_dir = Path::new("test-app-data");
        let mut value = serde_json::to_value(Config::default(app_data_dir)).unwrap();
        let object = value.as_object_mut().unwrap();
        object.insert("username".into(), json!("bob"));
        object.insert("imgConcurrency".into(), json!(0));

        let merged = Config::merge_config(&value.to_string(), app_data_dir);

        assert_eq!(merged.username, "bob");
        assert_eq!(merged.img_concurrency, 15);
        merged.validate().unwrap();
    }
}

#[cfg(test)]
mod route_migration_tests {
    use super::*;
    #[test]
    fn unversioned_route_is_preserved_and_marked_for_confirmation() {
        let mut value = serde_json::json!({ "apiDomainMode": "Domain2", "username": "user", "password": " pass " });
        migrate_config_schema(&mut value);
        assert_eq!(value["apiDomainMode"], "Domain3");
        assert_eq!(
            ApiDomainMode::Domain3.preset_domain(),
            Some("www.cdnhth.net")
        );
        assert_eq!(value["apiDomainMigrationPending"], true);
        assert_eq!(value["autoLogin"], true);
        assert_eq!(value["password"], " pass ");
        let once = value.clone();
        migrate_config_schema(&mut value);
        assert_eq!(once, value);
    }
}

#[cfg(test)]
mod simplified_settings_tests {
    use super::*;
    #[test]
    fn default_and_loaded_download_policy_are_original_without_sleep_intervals() {
        let mut config = Config::default(Path::new("app"));
        assert_eq!(config.download_format, DownloadFormat::Original);
        config.download_format = DownloadFormat::Jpeg;
        config.img_download_interval_sec = 5;
        config.chapter_download_interval_sec = 10;
        config.apply_download_policy();
        assert_eq!(config.download_format, DownloadFormat::Original);
        assert_eq!(config.img_download_interval_sec, 0);
        assert_eq!(config.chapter_download_interval_sec, 0);
    }
}
