use serde::{Deserialize, Serialize};
use specta::Type;

use super::{string_to_i64, CategoryRespData, CategorySubRespData, GetComicRespData};

/// 搜索结果响应里存在有效的跳转漫画编号时返回它。
///
/// 不能再用“能否反序列化成 RedirectRespData”判断：普通搜索响应同样带 `total`，
/// 容器级 `#[serde(default)]` 会让它解析成功并得到 `redirect_aid = 0`，
/// 随后去请求一部不存在的漫画。必须显式检查字段存在且大于 0。
pub fn redirect_aid(value: &serde_json::Value) -> Option<i64> {
    let aid = match value.get("redirect_aid")? {
        serde_json::Value::Number(number) => number.as_i64(),
        // 接口曾返回字符串，现在返回数字，两种都要兼容
        serde_json::Value::String(text) => text.trim().parse::<i64>().ok(),
        _ => None,
    }?;
    (aid > 0).then_some(aid)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub enum SearchResp {
    SearchRespData(SearchRespData),
    // 用Box包装GetComicRespData，因为GetComicRespData比SearchRespData大得多
    // 如果不用Box包装，即使SearchResp的类型是SearchRespData，也会占用与GetComicRespData一样大的内存
    ComicRespData(Box<GetComicRespData>),
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(default, rename_all = "camelCase")]
pub struct SearchRespData {
    #[serde(rename = "search_query")]
    pub search_query: String,
    #[serde(deserialize_with = "string_to_i64")]
    pub total: i64,
    pub content: Vec<ComicInSearchRespData>,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(default, rename_all = "camelCase")]
pub struct ComicInSearchRespData {
    pub id: String,
    pub author: String,
    pub name: String,
    pub image: String,
    pub category: CategoryRespData,
    #[serde(rename = "category_sub")]
    pub category_sub: CategorySubRespData,
    pub liked: bool,
    #[serde(rename = "is_favorite")]
    pub is_favorite: bool,
    #[serde(rename = "update_at")]
    pub update_at: i64,
}

#[cfg(test)]
mod tests {
    use super::redirect_aid;

    fn parse(raw: &str) -> serde_json::Value {
        serde_json::from_str(raw).unwrap()
    }

    #[test]
    fn redirect_aid_accepts_numeric_and_string() {
        assert_eq!(
            redirect_aid(&parse(
                r#"{"search_query":"123456","total":1,"redirect_aid":123456}"#
            )),
            Some(123456)
        );
        assert_eq!(
            redirect_aid(&parse(
                r#"{"search_query":"123456","total":1,"redirect_aid":"123456"}"#
            )),
            Some(123456)
        );
    }

    #[test]
    fn redirect_aid_ignores_zero_missing_and_invalid() {
        assert_eq!(redirect_aid(&parse(r#"{"redirect_aid":0}"#)), None);
        assert_eq!(redirect_aid(&parse(r#"{"redirect_aid":-1}"#)), None);
        assert_eq!(redirect_aid(&parse(r#"{"redirect_aid":""}"#)), None);
        assert_eq!(redirect_aid(&parse(r#"{"redirect_aid":"abc"}"#)), None);
    }

    #[test]
    fn normal_search_response_is_not_a_redirect() {
        // 普通搜索响应的 total 是数字，旧实现会把它误判成 redirect
        let normal =
            parse(r#"{"search_query":"漫画","total":42,"content":[{"id":"1","name":"x"}]}"#);
        assert_eq!(redirect_aid(&normal), None);
    }
}
