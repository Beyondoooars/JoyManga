use serde::{Deserialize, Serialize};
use specta::Type;

/// 阅读器为单页预留高度所用的像素尺寸。
///
/// 图片解码前 `<img>` 的高度是 0，滚动模式下任意一页解码完成都会把下方内容推开，
/// 程序化定位（恢复阅读进度、跳页）就再也对不准。
///
/// `ChapterReaderData::page_sizes` 里与之对应的元素可能是 `None`：该页的文件头
/// 无法解析（多半是文件已损坏），此时不预留高度，退回按图片自身尺寸排版。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PageSize {
    pub width: u32,
    pub height: u32,
}
