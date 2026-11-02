use parking_lot::RwLock;
use tauri::{Manager, State};

use crate::{config::Config, download_manager::DownloadManager, jm_client::JmClient};

pub trait AnyhowErrorToStringChain {
    /// 将 `anyhow::Error` 转换为chain格式
    /// # Example
    /// 0: error message
    /// 1: error message
    /// 2: error message
    fn to_string_chain(&self) -> String;
}

impl AnyhowErrorToStringChain for anyhow::Error {
    fn to_string_chain(&self) -> String {
        use std::fmt::Write;
        self.chain()
            .enumerate()
            .fold(String::new(), |mut output, (i, e)| {
                let _ = writeln!(output, "{i}: {e}");
                output
            })
    }
}

pub trait PathIsCbz {
    /// 判断路径是否为 CBZ 归档（扩展名大小写不敏感）
    fn is_cbz(&self) -> bool;
}

impl PathIsCbz for std::path::Path {
    fn is_cbz(&self) -> bool {
        self.extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("cbz"))
    }
}

pub trait PathIsImg {
    /// 判断路径是否为图片(jpg/png/webp/gif)
    fn is_img(&self) -> bool;
}

impl PathIsImg for std::path::Path {
    fn is_img(&self) -> bool {
        self.extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| {
                ext.eq_ignore_ascii_case("jpg")
                    || ext.eq_ignore_ascii_case("jpeg")
                    || ext.eq_ignore_ascii_case("png")
                    || ext.eq_ignore_ascii_case("webp")
                    || ext.eq_ignore_ascii_case("gif")
            })
    }
}

/// 图片扩展名 → MIME；未知扩展名按 JPEG 处理（JM 的图片以 JPEG 为主）。
pub fn mime_for_image(path: &std::path::Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("jpeg");
    if ext.eq_ignore_ascii_case("png") {
        "image/png"
    } else if ext.eq_ignore_ascii_case("webp") {
        "image/webp"
    } else if ext.eq_ignore_ascii_case("gif") {
        "image/gif"
    } else {
        "image/jpeg"
    }
}

pub trait WalkDirEntryExt {
    fn is_comic_metadata(&self) -> bool;
}
impl WalkDirEntryExt for walkdir::DirEntry {
    fn is_comic_metadata(&self) -> bool {
        if !self.file_type().is_file() {
            return false;
        }
        if self.file_name() != "元数据.json" {
            return false;
        }

        true
    }
}

pub trait AppHandleExt {
    fn get_config(&self) -> State<'_, RwLock<Config>>;
    fn get_jm_client(&self) -> State<'_, JmClient>;
    fn get_download_manager(&self) -> State<'_, DownloadManager>;
}

impl AppHandleExt for tauri::AppHandle {
    fn get_config(&self) -> State<'_, RwLock<Config>> {
        self.state::<RwLock<Config>>()
    }
    fn get_jm_client(&self) -> State<'_, JmClient> {
        self.state::<JmClient>()
    }
    fn get_download_manager(&self) -> State<'_, DownloadManager> {
        self.state::<DownloadManager>()
    }
}
