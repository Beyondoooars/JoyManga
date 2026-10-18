use serde::{Deserialize, Serialize};
use specta::Type;

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(default, rename_all = "camelCase")]
pub struct ToggleFavoriteRespData {
    pub status: String,
    pub msg: String,
    #[serde(rename = "type")]
    pub toggle_type: ToggleType,
}

/// 收藏变更类型。接口实际返回 `"add"` / `"remove"`（小写），
/// 这里同时兼容旧文档中的 `Add` / `Remove` 写法，避免大小写不一致导致解析失败。
#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "PascalCase")]
pub enum ToggleType {
    #[default]
    Add,
    Remove,
}
