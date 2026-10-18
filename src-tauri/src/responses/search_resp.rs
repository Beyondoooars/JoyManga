use serde::{Deserialize, Serialize};
use specta::Type;

use super::{string_to_i64, CategoryRespData, CategorySubRespData, GetComicRespData};

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(default, rename_all = "camelCase")]
pub struct RedirectRespData {
    #[serde(rename = "search_query")]
    pub search_query: String,
    pub total: i64,
    // 接口曾返回字符串，现在返回数字，两种都要兼容
    #[serde(rename = "redirect_aid", deserialize_with = "string_to_i64")]
    pub redirect_aid: i64,
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
    use super::RedirectRespData;

    #[test]
    fn redirect_resp_parses_numeric_and_string_aid() {
        // 接口现在返回数字
        let numeric = r#"{"search_query":"123456","total":1,"redirect_aid":123456}"#;
        let parsed = serde_json::from_str::<RedirectRespData>(numeric).unwrap();
        assert_eq!(parsed.redirect_aid, 123456);

        // 兼容旧版返回字符串
        let string = r#"{"search_query":"123456","total":1,"redirect_aid":"123456"}"#;
        let parsed = serde_json::from_str::<RedirectRespData>(string).unwrap();
        assert_eq!(parsed.redirect_aid, 123456);
    }
}
