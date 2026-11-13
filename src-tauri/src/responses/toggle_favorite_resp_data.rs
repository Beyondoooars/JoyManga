use serde::{Deserialize, Serialize};
use specta::Type;

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(default, rename_all = "camelCase")]
pub struct ToggleFavoriteRespData {
    pub status: String,
    pub msg: String,
    #[serde(rename = "type")]
    pub toggle_type: Option<ToggleType>,
}

impl ToggleFavoriteRespData {
    pub fn into_is_favorite(self) -> anyhow::Result<bool> {
        anyhow::ensure!(
            self.status.trim().eq_ignore_ascii_case("ok"),
            "收藏操作失败：{}",
            if self.msg.trim().is_empty() {
                "服务端未确认操作成功"
            } else {
                self.msg.trim()
            }
        );
        match self.toggle_type {
            Some(ToggleType::Add) => Ok(true),
            Some(ToggleType::Remove) => Ok(false),
            None => anyhow::bail!("收藏响应缺少type字段，无法确认服务端状态"),
        }
    }
}

/// 收藏变更类型。接口实际返回 `"add"` / `"remove"`（小写），
/// 这里同时兼容旧文档中的 `Add` / `Remove` 写法，避免大小写不一致导致解析失败。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "PascalCase")]
pub enum ToggleType {
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
        assert_eq!(add_lower.toggle_type, Some(ToggleType::Add));

        let remove_lower: ToggleFavoriteRespData =
            serde_json::from_str(r#"{"status":"ok","msg":"ok","type":"remove"}"#).unwrap();
        assert_eq!(remove_lower.toggle_type, Some(ToggleType::Remove));

        let remove_pascal: ToggleFavoriteRespData =
            serde_json::from_str(r#"{"status":"ok","msg":"ok","type":"Remove"}"#).unwrap();
        assert_eq!(remove_pascal.toggle_type, Some(ToggleType::Remove));
    }

    #[test]
    fn business_failures_and_missing_type_are_not_successes() {
        let missing: ToggleFavoriteRespData =
            serde_json::from_str(r#"{"status":"fail","msg":"收藏已满"}"#).unwrap();
        assert!(missing.into_is_favorite().is_err());

        let missing_type: ToggleFavoriteRespData =
            serde_json::from_str(r#"{"status":"ok","msg":"ok"}"#).unwrap();
        assert!(missing_type.into_is_favorite().is_err());
    }
}
