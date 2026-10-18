use serde::{Deserialize, Serialize};
use specta::Type;

use super::string_to_i64;

/// 登录/用户资料响应。
///
/// 除令牌 `s` 与账号标识 `uid` 外，其余字段都视为可选：不同客户端版本、封禁/游客
/// 账号返回的字段集合并不一致，缺失字段降级为空值，避免“一个字段缺失导致整次登录失败”。
#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(default, rename_all = "camelCase")]
pub struct GetUserProfileRespData {
    pub uid: String,
    pub username: String,
    pub email: String,
    pub emailverified: String,
    pub photo: String,
    pub fname: String,
    pub gender: String,
    pub message: Option<String>,
    #[serde(deserialize_with = "string_to_i64")]
    pub coin: i64,
    #[serde(rename = "album_favorites")]
    pub album_favorites: i64,
    pub s: String,
    #[serde(rename = "level_name")]
    pub level_name: String,
    pub level: i64,
    pub next_level_exp: i64,
    pub exp: String,
    pub exp_percent: f64,
    #[serde(rename = "album_favorites_max")]
    pub album_favorites_max: i64,
    #[serde(rename = "ad_free")]
    pub ad_free: bool,
    pub charge: String,
    pub jar: String,
    #[serde(rename = "invitation_qrcode")]
    pub invitation_qrcode: String,
    #[serde(rename = "invitation_url")]
    pub invitation_url: String,
    #[serde(rename = "invited_cnt")]
    pub invited_cnt: String,
}

#[cfg(test)]
mod tests {
    use super::GetUserProfileRespData;

    #[test]
    fn profile_tolerates_missing_optional_fields() {
        // 只带核心字段的响应（例如部分客户端版本或受限账号）也应能解析
        let parsed: GetUserProfileRespData =
            serde_json::from_str(r#"{"uid":"42","username":"tester","s":"token"}"#).unwrap();
        assert_eq!(parsed.uid, "42");
        assert_eq!(parsed.s, "token");
        assert!(parsed.photo.is_empty());
        assert_eq!(parsed.coin, 0);
    }

    #[test]
    fn profile_still_parses_string_coin() {
        let parsed: GetUserProfileRespData =
            serde_json::from_str(r#"{"uid":"1","s":"t","coin":"100"}"#).unwrap();
        assert_eq!(parsed.coin, 100);
    }
}
