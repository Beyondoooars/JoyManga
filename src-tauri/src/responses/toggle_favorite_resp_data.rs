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
    #[serde(alias = "add", alias = "ADD")]
    Add,
    #[serde(alias = "remove", alias = "REMOVE", alias = "del", alias = "delete")]
    Remove,
}

#[cfg(test)]
mod tests {
    use super::{ToggleFavoriteRespData, ToggleType};

    #[test]
    fn toggle_type_accepts_lowercase_and_pascal_case() {
        let add_lower: ToggleFavoriteRespData =
            serde_json::from_str(r#"{"status":"ok","msg":"ok","type":"add"}"#).unwrap();
        assert_eq!(add_lower.toggle_type, ToggleType::Add);

        let remove_lower: ToggleFavoriteRespData =
            serde_json::from_str(r#"{"status":"ok","msg":"ok","type":"remove"}"#).unwrap();
        assert_eq!(remove_lower.toggle_type, ToggleType::Remove);

        let remove_pascal: ToggleFavoriteRespData =
            serde_json::from_str(r#"{"status":"ok","msg":"ok","type":"Remove"}"#).unwrap();
        assert_eq!(remove_pascal.toggle_type, ToggleType::Remove);
    }
}
