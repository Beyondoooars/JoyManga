use std::{collections::HashMap, path::PathBuf};

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri_specta::Event;

use crate::{download_manager::DownloadTaskState, types::LogLevel};

#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
pub struct DownloadSpeedEvent {
    pub speed: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(tag = "event", content = "data")]
pub enum DownloadTaskEvent {
    Create(crate::download_queue::DownloadTaskSnapshot),

    #[serde(rename_all = "camelCase")]
    Update {
        chapter_id: i64,
        revision: u64,
        state: DownloadTaskState,
        downloaded_img_count: u32,
        total_img_count: u32,
        error_message: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(tag = "event", content = "data")]
pub enum DownloadAllFavoritesEvent {
    #[serde(rename_all = "camelCase")]
    GetFavoritesStart,

    #[serde(rename_all = "camelCase")]
    GetComicsProgress { current: i64, total: i64 },

    #[serde(rename_all = "camelCase")]
    StartCreateDownloadTasks {
        comic_id: i64,
        comic_title: String,
        current: i64,
        total: i64,
    },

    #[serde(rename_all = "camelCase")]
    CreatingDownloadTask { comic_id: i64, current: i64 },

    #[serde(rename_all = "camelCase")]
    EndCreateDownloadTasks { comic_id: i64 },

    #[serde(rename_all = "camelCase")]
    GetComicsEnd,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(tag = "event", content = "data")]
pub enum UpdateDownloadedComicsEvent {
    #[serde(rename_all = "camelCase")]
    GetComicStart { total: i64 },

    #[serde(rename_all = "camelCase")]
    GetComicProgress { current: i64, total: i64 },

    #[serde(rename_all = "camelCase")]
    CreateDownloadTasksStart {
        comic_id: i64,
        comic_title: String,
        current: i64,
        total: i64,
    },

    #[serde(rename_all = "camelCase")]
    CreateDownloadTaskProgress { comic_id: i64, current: i64 },

    #[serde(rename_all = "camelCase")]
    CreateDownloadTasksEnd { comic_id: i64 },

    #[serde(rename_all = "camelCase")]
    GetComicEnd,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(tag = "event", content = "data")]
pub enum ExportCbzEvent {
    #[serde(rename_all = "camelCase")]
    Start {
        uuid: String,
        comic_title: String,
        total: u32,
    },
    #[serde(rename_all = "camelCase")]
    Progress { uuid: String, current: u32 },
    #[serde(rename_all = "camelCase")]
    Error { uuid: String },
    #[serde(rename_all = "camelCase")]
    End {
        uuid: String,
        chapter_export_dir: PathBuf,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct LogEvent {
    pub timestamp: String,
    pub level: LogLevel,
    pub fields: HashMap<String, serde_json::Value>,
    pub target: String,
    pub filename: String,
    #[serde(rename = "line_number")]
    pub line_number: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
pub struct ConfigChangedEvent {
    pub config: crate::config::Config,
}

/// 下载完成后的 CBZ 自动整合（合并为单文件 / 逐话命名）过程事件。
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(tag = "event", content = "data")]
pub enum ComicArchiveEvent {
    #[serde(rename_all = "camelCase")]
    Start {
        comic_id: i64,
        comic_title: String,
        mode: crate::types::CbzMode,
    },
    #[serde(rename_all = "camelCase")]
    Progress {
        comic_id: i64,
        current: u32,
        total: u32,
    },
    #[serde(rename_all = "camelCase")]
    End {
        comic_id: i64,
        comic_title: String,
        mode: crate::types::CbzMode,
        archive_path: PathBuf,
    },
    #[serde(rename_all = "camelCase")]
    Error {
        comic_id: i64,
        comic_title: String,
        message: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct SessionChangedEvent {
    pub session_epoch: u64,
    pub profile: Option<crate::responses::GetUserProfileRespData>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct FavoriteChangedEvent {
    pub comic_id: i64,
    pub is_favorite: bool,
    pub session_epoch: u64,
}
