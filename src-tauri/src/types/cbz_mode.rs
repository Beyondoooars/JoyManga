use serde::{Deserialize, Serialize};
use specta::Type;

/// 下载完成后的 CBZ 归档方式（永久收敛为整本自动合并）。
#[derive(Debug, Default, Copy, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum CbzMode {
    /// 合并为该漫画已下载的全部话，产出单个以本子名命名的 CBZ（默认）。
    #[default]
    #[serde(alias = "perChapter")]
    Merge,
}
