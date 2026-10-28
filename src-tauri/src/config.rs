use std::path::{Path, PathBuf};

use anyhow::{ensure, Context};

use crate::types::{CbzMode, LogLevel, ProxyMode, ReadingDirection, ReadingMode};
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

/// Android 公共下载目录下的默认书库：用户可在文件管理器直接查看，
/// 卸载应用不会随之删除。需要「所有文件访问」权限（首次启动引导授予）。
///
/// 运行时的位置由 Android 桥（`Environment.DIRECTORY_DOWNLOADS`）给出，不依赖该常量；
/// 这里只保留给测试校验 `is_public_library_dir` 的典型布局。
#[cfg(test)]
pub const ANDROID_PUBLIC_LIBRARY_DIR: &str = "/storage/emulated/0/Download/JoyManga";

/// 判断候选路径是否为公共默认书库：以 `Download/JoyManga` 结尾且位于
/// `/storage` 之下，兼容 SD 卡等不同存储卷的 `/storage/<volume>/Download/JoyManga` 布局。
///
/// 用 `Path` 组件比较而不是字符串后缀：`/storage/x/NotDownload/JoyManga` 不会被误判，
/// 尾随分隔符（`…/JoyManga/`）也能正常匹配。`initialize_android_storage` 与
/// `clear_all_downloads` 共用这一判定，避免两处接受范围不一致。
pub fn is_public_library_dir(candidate: &Path) -> bool {
    let Ok(relative) = candidate.strip_prefix("/storage") else {
        return false;
    };
    relative.ends_with(Path::new("Download").join("JoyManga")) && relative.components().count() >= 3
}
const LOGS_DIR_NAME: &str = "日志";

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

        // Android 应用数据目录固定，不存在便携版/安装版的分叉。
        let config_path = app_data_dir.join(CONFIG_FILE_NAME);
        let logs_dir = {
            let fallback = app_data_dir.join(LOGS_DIR_NAME);
            utils::create_dir_all_beneath(&app_data_dir, Path::new(LOGS_DIR_NAME)).unwrap_or_else(
                |err| {
                    tracing::warn!(error = %err, "无法创建日志目录，回退到应用数据目录");
                    let _ = std::fs::create_dir_all(&fallback);
                    fallback
                },
            )
        };
        // WebView 数据由系统按应用管理；保留该路径供清理缓存使用。
        let webview_data_dir = app_local_data_dir;
        std::fs::create_dir_all(&webview_data_dir).context(format!(
            "创建 WebView 数据目录`{}`失败",
            webview_data_dir.display()
        ))?;
        let webview_data_dir = std::fs::canonicalize(&webview_data_dir).unwrap_or(webview_data_dir);

        Ok(Self {
            config_path,
            logs_dir,
            webview_data_dir,
            app_data_dir,
        })
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

/// 书库是否仍位于应用专属目录（公共 `Download/JoyManga` 引入前的位置）。
///
/// 这类配置要继续迁移；用户手选的 SD 卡或自建目录不在此列，不能擅自改动。
fn is_app_private_library_path(path: &str) -> bool {
    // 不能用 `Path::is_absolute()`：配置里的路径来自 Android，在桌面/CI 上
    // （测试与交叉编译的单元测试）它会被判成相对路径。这里只按组件比对。
    let path = Path::new(path);
    if !path.file_name().is_some_and(|name| name == "library") {
        return false;
    }
    // 只比对路径组件，不依赖平台的分隔符解析细节。历史上出现过三种落点：
    //
    // 1. `/data/user/<id>/<pkg>/library`（或 `/data/data/<pkg>/library`）
    //    —— `Config::default` 的默认值：Tauri 的 `app_data_dir()` 在 Android 上取
    //    `Context.getDataDir()`（应用数据根目录，不含 `files`），再拼 `library`。
    //    配置没被改写过时就是这个位置；
    // 2. `/data/user/<id>/<pkg>/files/library`（或 `/data/data/<pkg>/files/library`）
    //    —— 原生 `filesDir` 下的落点（`getExternalFilesDir(null)` 为 null 时的兜底），
    //    也可能来自早期手动填写的路径；
    // 3. `/storage/emulated/0/Android/data/<pkg>/files/library`
    //    —— v0.2.4 起首启走原生 `libraryPath()`（`getExternalFilesDir/library`）。
    //
    // 三者的共同点：都在「应用专属目录」内。用户手选的 SD 卡或自建目录（例如
    // `/storage/1A2B/library`）不在应用专属目录下，不能擅自改动。
    let parts: Vec<String> = path
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(name) => Some(name.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    if parts.last().is_none_or(|part| part != "library") {
        return false;
    }
    let starts_with_data = parts.first().is_some_and(|part| part == "data");
    if starts_with_data {
        // 形态 1：`…/files/library`；形态 2：`…/<pkg>/library`
        let files_index = parts.iter().position(|part| part == "files");
        return match files_index {
            Some(index) => index == parts.len() - 2,
            // `<pkg>/library` 至少要有 data / user / <id> / <pkg> / library 五段
            None => parts.len() >= 5,
        };
    }
    // 形态 3：`/storage/<volume>/Android/data/<pkg>/files/library`
    //
    // 卷名长度不定（`emulated/0`、SD 卡 `1A2B-3C4D`），不能假定 `Android` 的固定下标；
    // 只要求「storage 开头 → 某处是 Android → 紧接 data/<pkg>/files/library」。
    if !parts.first().is_some_and(|part| part == "storage") {
        return false;
    }
    let Some(android_index) = parts.iter().position(|part| part == "Android") else {
        return false;
    };
    // `Android` 之后必须正好是 data / <pkg> / files / library 四段
    parts.len() == android_index + 5
        && parts[android_index + 1] == "data"
        && parts[android_index + 3] == "files"
}

/// 当前配置 schema 版本。每次需要在已发布版本之上改写旧配置时递增。
const CONFIG_SCHEMA_VERSION: u32 = 2;

fn migrate_config_schema(value: &mut serde_json::Value) {
    let Some(object) = value.as_object_mut() else {
        return;
    };
    let version = object
        .get("schemaVersion")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    if version >= u64::from(CONFIG_SCHEMA_VERSION) {
        return;
    }
    // v1：早期版本把书库放在应用私有目录，公共 Download/JoyManga 是后来才引入的。
    // 只把「仍指向应用私有目录」的配置标记为未初始化，让启动流程按当前系统能力
    // 重新选一次位置；用户手选到别处的书库（SD 卡、自建目录）不动。
    if version == 1 {
        let private_library = object
            .get("downloadDir")
            .and_then(serde_json::Value::as_str)
            .is_some_and(is_app_private_library_path);
        if private_library {
            object.insert(
                "androidStorageInitialized".into(),
                serde_json::Value::Bool(false),
            );
        }
        object.insert(
            "schemaVersion".into(),
            serde_json::Value::from(CONFIG_SCHEMA_VERSION),
        );
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
    // 无版本的配置（早于公共书库）一律交给启动流程重新初始化书库位置。
    object
        .entry("androidStorageInitialized")
        .or_insert(serde_json::Value::Bool(false));
    object.insert(
        "schemaVersion".into(),
        serde_json::Value::from(CONFIG_SCHEMA_VERSION),
    );
}

/// 将配置序列化为磁盘存储格式。
///
/// 登录账号与密码分别加密后写入，其余字段仍是明文 JSON（便于用户查看与自用）。
fn serialize_config_for_storage(config: &Config) -> anyhow::Result<String> {
    let mut value = serde_json::to_value(config).context("序列化配置失败")?;
    // 加密失败时不能退回明文：宁可这次保存失败，也不要让凭据以明文落盘。
    let encrypted_username = crate::credential_cipher::encrypt_for_this_machine(&config.username)
        .context("加密登录账号失败")?;
    let encrypted_password = crate::credential_cipher::encrypt_for_this_machine(&config.password)
        .context("加密登录密码失败")?;
    if let Some(object) = value.as_object_mut() {
        object.insert(
            "username".into(),
            serde_json::Value::String(encrypted_username),
        );
        object.insert(
            "password".into(),
            serde_json::Value::String(encrypted_password),
        );
    }
    serde_json::to_string_pretty(&value).context("序列化配置失败")
}

/// 解密对象里的一个凭据字段；缺失或非字符串时保持原样。
fn decrypt_stored_credential(
    value: &mut serde_json::Value,
    field: &str,
    label: &str,
) -> anyhow::Result<()> {
    let Some(stored) = value
        .get(field)
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
    else {
        return Ok(());
    };
    let decrypted = crate::credential_cipher::decrypt_for_this_machine(&stored)
        .with_context(|| format!("解密登录{label}失败"))?;
    value[field] = serde_json::Value::String(decrypted);
    Ok(())
}

/// 从磁盘存储格式反序列化配置。
///
/// 账号与密码先解密再交给 `Config`；历史明文凭据原样通过，下次保存时自动转为密文。
fn deserialize_config_from_storage(config_string: &str) -> anyhow::Result<Config> {
    let mut value: serde_json::Value =
        serde_json::from_str(config_string).context("解析配置JSON失败")?;
    migrate_config_schema(&mut value);
    decrypt_stored_credential(&mut value, "username", "账号")?;
    decrypt_stored_credential(&mut value, "password", "密码")?;
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
    pub proxy_mode: ProxyMode,
    pub proxy_host: String,
    pub proxy_port: u16,
    pub chapter_concurrency: usize,
    pub img_concurrency: usize,
    pub api_domain_mode: ApiDomainMode,
    pub custom_api_domain: String,
    pub image_domain_mode: ImageDomainMode,
    pub custom_image_domain: String,
    /// 下载完成后 CBZ 自动整合的默认方式（单次下载可在界面上另行选择）。
    #[serde(default)]
    pub default_cbz_mode: CbzMode,
    /// 阅读方向：决定点击区域、键盘、滑动与翻页按钮的语义。
    #[serde(default)]
    pub reading_direction: ReadingDirection,
    /// 阅读排版：单页翻页或上下滚动。
    #[serde(default)]
    pub reading_mode: ReadingMode,
    /// 阅读时保持屏幕常亮。
    #[serde(default = "default_true")]
    pub keep_screen_on: bool,
    /// 音量键翻页。
    #[serde(default)]
    pub volume_key_paging: bool,
    /// 仅在非计费网络（通常为 Wi-Fi）下允许下载。
    #[serde(default)]
    pub download_only_wifi: bool,
    /// 文件日志收集等级：写入应用数据目录的滚动日志；`Off` 完全不落盘。
    #[serde(default)]
    pub log_level: LogLevel,
}

impl Config {
    pub fn apply_storage_policy(&mut self) {
        self.export_dir.clone_from(&self.download_dir);
    }
    pub fn new(storage: &ConfigStorage) -> anyhow::Result<Self> {
        let base_dir = storage.base_dir();
        let config_path = storage.config_path();

        atomic_fs::recover_file_backup(config_path)?;

        let (mut config, repaired_config) = if config_path.exists() {
            let config_string = std::fs::read_to_string(config_path)?;
            let (cfg, repaired) = match deserialize_config_from_storage(&config_string) {
                Ok(config) if Config::validate(&config).is_ok() => (config, false),
                // 缺字段、字段类型错误或值越界时逐字段恢复，保留其余有效配置。
                Ok(_) | Err(_) => (Config::merge_config(&config_string, base_dir), true),
            };
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
        // 磁盘上账号与密码都是密文：逐字段恢复也要先解密，否则会把 `enc:v1:…`
        // 当成账号或密码发出去。解密失败（换过机器、文件被改）时清空该字段，
        // 让用户重新登录；`Config::new` 会先把原配置备份到 `config.invalid-*.json`。
        for (field, label) in [("username", "账号"), ("password", "密码")] {
            let Some(stored) = user_value
                .get(field)
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
            else {
                continue;
            };
            let decrypted = match crate::credential_cipher::decrypt_for_this_machine(&stored) {
                Ok(decrypted) => decrypted,
                Err(error) => {
                    tracing::warn!(
                        field = field,
                        label = label,
                        error = %error,
                        "保存的登录凭据无法解密，本次以空值启动"
                    );
                    String::new()
                }
            };
            user_value[field] = serde_json::Value::String(decrypted);
        }
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
        let library_dir = base_dir.join("library");
        let (download_dir, export_dir) = (library_dir.clone(), library_dir);
        // Android 设备的默认并发：移动端网络与内存都更保守
        let (chapter_concurrency, img_concurrency) = (1, 4);

        Config {
            revision: 0,
            schema_version: 1,
            api_domain_migration_pending: false,
            auto_login: false,
            android_storage_initialized: false,
            username: String::new(),
            password: String::new(),
            download_dir,
            export_dir,
            proxy_mode: ProxyMode::default(),
            proxy_host: "127.0.0.1".to_string(),
            proxy_port: 7890,
            chapter_concurrency,
            img_concurrency,
            api_domain_mode: ApiDomainMode::Domain2,
            custom_api_domain: API_DOMAIN_2.to_string(),
            image_domain_mode: ImageDomainMode::Domain3,
            custom_image_domain: IMAGE_DOMAIN_3.to_string(),
            default_cbz_mode: CbzMode::default(),
            reading_direction: ReadingDirection::default(),
            reading_mode: ReadingMode::default(),
            keep_screen_on: true,
            volume_key_paging: false,
            download_only_wifi: false,
            log_level: LogLevel::default(),
        }
    }

    /// 当前实际生效的手动代理地址；`None` 表示不额外设置代理。
    ///
    /// 系统 VPN 开启时跳过手动代理：VPN 已经在系统层接管了全部流量，再把手动
    /// 代理地址（通常是某代理客户端的 127.0.0.1:7890）套上去，一旦那个端口
    /// 并没有在监听（VPN 走的是 tun 模式，或客户端换了端口），所有请求都会
    /// 卡在连接本地代理上——表现就是「开 VPN 反而连不上网」。
    /// 设置页的说明「VPN 随系统生效；开启 VPN 时应用会自动跳过此代理」即指此处。
    pub fn effective_proxy_url(&self) -> Option<String> {
        let vpn_active = crate::network_state::is_vpn_active();
        match self.proxy_mode {
            ProxyMode::Custom if !vpn_active => {
                Some(format!("http://{}:{}", self.proxy_host, self.proxy_port))
            }
            ProxyMode::Custom if vpn_active => {
                // 开着 VPN 却仍连不上时，首先要能确认「代理到底有没有被跳过」。
                // 这条信息此前完全没有落盘，用户反馈「开 VPN 还是连不上」时无从判断
                // 是 VPN 状态没识别到，还是跳过代理后线路本身不通。
                tracing::info!(
                    proxy_mode = ?self.proxy_mode,
                    vpn_active,
                    "VPN 已接管流量，本次跳过手动代理"
                );
                None
            }
            _ => None,
        }
    }

    pub fn apply_proxy(
        &self,
        builder: reqwest::ClientBuilder,
    ) -> anyhow::Result<reqwest::ClientBuilder> {
        if self.proxy_mode == ProxyMode::NoProxy {
            return Ok(builder.no_proxy());
        }
        let Some(proxy_url) = self.effective_proxy_url() else {
            return Ok(builder);
        };
        let proxy =
            reqwest::Proxy::all(&proxy_url).context(format!("创建代理`{proxy_url}`失败"))?;
        Ok(builder.proxy(proxy))
    }

    pub fn apply_proxy_best_effort(
        &self,
        builder: reqwest::ClientBuilder,
        client_name: &str,
    ) -> reqwest::ClientBuilder {
        if self.proxy_mode == ProxyMode::NoProxy {
            return builder.no_proxy();
        }
        let Some(proxy_url) = self.effective_proxy_url() else {
            return builder;
        };
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
        deserialize_config_from_storage, serialize_config_for_storage, ApiDomainMode, Config,
        ConfigStorage, ImageDomainMode, API_DOMAIN_2, CONFIG_FILE_NAME, IMAGE_DOMAIN_3,
    };
    use serde_json::json;
    use std::path::{Path, PathBuf};

    fn test_dir(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("joymanga-config-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        path
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
        config.img_concurrency = 0;
        assert!(config.validate().is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn credentials_are_encrypted_on_disk_and_restored() {
        let app_data_dir = Path::new("test-app-data");
        let mut config = Config::default(app_data_dir);
        config.username = "alice".to_string();
        config.password = "plain-password".to_string();

        // 账号与密码都必须加密落盘：配置会被备份、被文件管理器看到，明文等于泄漏
        let stored = serialize_config_for_storage(&config).unwrap();
        assert!(!stored.contains("alice"), "账号不能以明文出现在配置里");
        assert!(
            !stored.contains("plain-password"),
            "密码不能以明文出现在配置里"
        );
        assert_eq!(stored.matches("enc:v1:").count(), 2);

        // 读回来必须还原成明文，否则登录会用密文当账号或密码
        let restored = deserialize_config_from_storage(&stored).unwrap();
        assert_eq!(restored.username, "alice");
        assert_eq!(restored.password, "plain-password");
    }

    #[test]
    fn legacy_plaintext_credentials_are_still_readable() {
        // 升级前落盘的是明文账号与密码：必须能读出来，并在下次保存时转成密文。
        let app_data_dir = Path::new("test-app-data");
        let mut config = Config::default(app_data_dir);
        config.username = "alice".to_string();
        config.password = "plain-password".to_string();
        let mut value = serde_json::to_value(&config).unwrap();
        value["password"] = serde_json::json!("plain-password");
        let legacy = serde_json::to_string(&value).unwrap();

        let restored = deserialize_config_from_storage(&legacy).unwrap();
        assert_eq!(restored.username, "alice");
        assert_eq!(restored.password, "plain-password");

        let resaved = serialize_config_for_storage(&restored).unwrap();
        assert!(!resaved.contains("alice"));
        assert!(!resaved.contains("plain-password"));
    }

    #[test]
    fn merge_config_decrypts_encrypted_credentials() {
        let app_data_dir = Path::new("test-app-data");
        let mut config = Config::default(app_data_dir);
        config.username = "alice".to_string();
        config.password = "plain-password".to_string();
        let stored = serialize_config_for_storage(&config).unwrap();

        let merged = Config::merge_config(&stored, app_data_dir);

        assert_eq!(merged.username, "alice");
        assert_eq!(merged.password, "plain-password");
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
        assert_eq!(merged.chapter_concurrency, 1);
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
        assert_eq!(merged.img_concurrency, 4);
        merged.validate().unwrap();
    }
}

#[cfg(test)]
mod proxy_selection_tests {
    use super::*;

    fn custom_proxy_config() -> Config {
        let mut config = Config::default(Path::new("test-app-data"));
        config.proxy_mode = ProxyMode::Custom;
        config.proxy_host = "127.0.0.1".into();
        config.proxy_port = 7890;
        config
    }

    #[test]
    fn manual_proxy_is_used_only_without_a_vpn() {
        let _guard = crate::network_state::STATE_TEST_LOCK.lock();
        let config = custom_proxy_config();
        crate::network_state::set_vpn(false);
        assert_eq!(
            config.effective_proxy_url().as_deref(),
            Some("http://127.0.0.1:7890")
        );

        // VPN 已在系统层接管流量：再套手动代理只会连到本机端口，开 VPN 反而断网
        crate::network_state::set_vpn(true);
        assert_eq!(config.effective_proxy_url(), None);
        crate::network_state::set_vpn(false);
    }

    #[test]
    fn direct_and_system_modes_never_add_a_manual_proxy() {
        let _guard = crate::network_state::STATE_TEST_LOCK.lock();
        let mut config = custom_proxy_config();
        crate::network_state::set_vpn(false);
        config.proxy_mode = ProxyMode::System;
        assert_eq!(config.effective_proxy_url(), None);
        config.proxy_mode = ProxyMode::NoProxy;
        assert_eq!(config.effective_proxy_url(), None);
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

    #[test]
    fn legacy_private_library_is_reinitialized_instead_of_kept() {
        // 0.2.4~0.2.20 写出的配置是 schemaVersion 1 + 应用私有书库。
        // 若迁移时保持「已初始化」，启动流程就不会把它换到公共 Download/JoyManga，
        // 下载写进私有目录、书库扫描另一个目录，表现为「下载完成却看不到本子」。
        let mut value = serde_json::json!({
            "schemaVersion": 1,
            "androidStorageInitialized": true,
            "downloadDir": "/data/user/0/com.joymanga.app/files/library",
            "exportDir": "/data/user/0/com.joymanga.app/files/library",
        });
        migrate_config_schema(&mut value);
        assert_eq!(value["androidStorageInitialized"], false);
        assert_eq!(value["schemaVersion"], CONFIG_SCHEMA_VERSION);

        // 用户手选的书库（公共下载 / SD 卡）不能被改写
        let mut chosen = serde_json::json!({
            "schemaVersion": 1,
            "androidStorageInitialized": true,
            "downloadDir": "/storage/emulated/0/Download/JoyManga",
        });
        migrate_config_schema(&mut chosen);
        assert_eq!(chosen["androidStorageInitialized"], true);

        // 已经是当前 schema 的配置不再重复迁移
        let mut current = serde_json::json!({
            "schemaVersion": CONFIG_SCHEMA_VERSION,
            "androidStorageInitialized": true,
            "downloadDir": "/data/user/0/com.joymanga.app/files/library",
        });
        let before = current.clone();
        migrate_config_schema(&mut current);
        assert_eq!(before, current);
    }

    #[test]
    fn private_library_detection_matches_android_app_dirs_only() {
        // 形态 1：应用内部数据目录下的 files/library
        assert!(is_app_private_library_path(
            "/data/user/0/com.joymanga.app/files/library"
        ));
        assert!(is_app_private_library_path(
            "/data/data/com.joymanga.app/files/library"
        ));
        // 形态 2：更早版本的 `<app_data_dir>/library`
        assert!(is_app_private_library_path(
            "/data/user/0/com.joymanga.app/library"
        ));
        // 形态 3：v0.2.4 起首启写入的外部应用专属目录
        assert!(is_app_private_library_path(
            "/storage/emulated/0/Android/data/com.joymanga.app/files/library"
        ));
        assert!(!is_app_private_library_path(
            "/storage/emulated/0/Download/JoyManga"
        ));
        // 用户把书库放在 SD 卡名为 library 的深层目录，不能被当成应用私有目录
        assert!(!is_app_private_library_path("/storage/1A2B/library"));
        // 形似外部专属目录但层级不对：不能误判
        assert!(!is_app_private_library_path(
            "/storage/emulated/0/Android/data/com.joymanga.app/library"
        ));
        assert!(!is_app_private_library_path("/data/library"));
    }

    #[test]
    fn external_app_specific_library_is_also_migrated() {
        // v0.2.4 起首启走原生 libraryPath()，落点是外部应用专属目录。
        // 这类配置同样要重新初始化，否则书库仍停在一个不会随卸载保留的位置。
        let mut value = serde_json::json!({
            "schemaVersion": 1,
            "androidStorageInitialized": true,
            "downloadDir": "/storage/emulated/0/Android/data/com.joymanga.app/files/library",
        });
        migrate_config_schema(&mut value);
        assert_eq!(value["androidStorageInitialized"], false);

        let mut older = serde_json::json!({
            "schemaVersion": 1,
            "androidStorageInitialized": true,
            "downloadDir": "/data/user/0/com.joymanga.app/library",
        });
        migrate_config_schema(&mut older);
        assert_eq!(older["androidStorageInitialized"], false);
    }
}
