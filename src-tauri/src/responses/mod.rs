mod daily_check_in_resp_data;
mod get_chapter_resp_data;
mod get_comic_resp_data;
mod get_favorite_resp_data;
mod get_user_profile_resp_data;
mod get_weekly_info_resp_data;
mod get_weekly_resp_data;
mod search_resp;
mod toggle_favorite_resp_data;

pub use daily_check_in_resp_data::*;
pub use get_chapter_resp_data::*;
pub use get_comic_resp_data::*;
pub use get_favorite_resp_data::*;
pub use get_user_profile_resp_data::*;
pub use get_weekly_info_resp_data::*;
pub use get_weekly_resp_data::*;
pub use search_resp::*;
pub use toggle_favorite_resp_data::*;

use serde::{Deserialize, Serialize};
use specta::Type;

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JmResp {
    pub code: i64,
    #[serde(default)]
    pub data: serde_json::Value,
    #[serde(flatten)]
    pub messages: serde_json::Map<String, serde_json::Value>,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SeriesRespData {
    pub id: String,
    pub name: String,
    pub sort: String,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CategoryRespData {
    pub id: Option<String>,
    pub title: Option<String>,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CategorySubRespData {
    pub id: Option<String>,
    pub title: Option<String>,
}

pub fn string_to_i64<'de, D>(d: D) -> Result<i64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde_json::Value;
    let value: Value = serde::Deserialize::deserialize(d)?;

    match value {
        Value::Number(n) => n
            .as_i64()
            .ok_or_else(|| serde::de::Error::custom(format!("`{n}`无法表示为i64"))),
        Value::String(s) => {
            let s = s.trim();
            if s.is_empty() {
                // 接口偶尔返回空串表示"没有"，按 0 处理而不是让整次响应解析失败
                return Ok(0);
            }
            s.parse()
                .map_err(|err| serde::de::Error::custom(format!("`{s}`无法解析为i64: {err}")))
        }
        Value::Null => Ok(0),
        _ => Err(serde::de::Error::custom(
            "`string_to_i64` 失败，value类型不是 `Number` 或 `String`",
        )),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct LoginRespData {
    pub profile: GetUserProfileRespData,
    pub session_epoch: u64,
    pub switched_api_domain_mode: Option<crate::config::ApiDomainMode>,
}

#[cfg(test)]
mod tests {
    #[derive(serde::Deserialize)]
    struct Holder {
        #[serde(deserialize_with = "super::string_to_i64")]
        value: i64,
    }

    #[test]
    fn string_to_i64_treats_empty_and_null_as_zero() {
        let empty: Holder = serde_json::from_str(r#"{"value":""}"#).unwrap();
        assert_eq!(empty.value, 0);
        let null: Holder = serde_json::from_str(r#"{"value":null}"#).unwrap();
        assert_eq!(null.value, 0);
        let text: Holder = serde_json::from_str(r#"{"value":" 12 "}"#).unwrap();
        assert_eq!(text.value, 12);
        assert!(serde_json::from_str::<Holder>(r#"{"value":"abc"}"#).is_err());
    }
}
