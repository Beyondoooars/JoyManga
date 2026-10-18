use serde::{Deserialize, Serialize};
use specta::Type;

#[derive(Debug, Default, Copy, Clone, PartialEq, Serialize, Deserialize, Type)]
pub enum DownloadFormat {
    Jpeg,
    Png,
    Webp,
    /// 未混淆的图片直接保存网络原始字节（保持原格式）；
    /// 需要反混淆重组的图片编码为无损 WebP。
    #[default]
    Original,
}

impl DownloadFormat {
    pub fn extension(self) -> &'static str {
        match self {
            DownloadFormat::Jpeg => "jpg",
            DownloadFormat::Png => "png",
            DownloadFormat::Webp => "webp",
            // 原始模式没有固定扩展名，此处返回重组图片使用的名义扩展名；
            // 具体落盘扩展名由图片任务按源格式决定。
            DownloadFormat::Original => "webp",
        }
    }
}
