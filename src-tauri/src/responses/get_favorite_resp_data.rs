use serde::{Deserialize, Serialize};
use specta::Type;

use super::{string_to_i64, CategoryRespData, CategorySubRespData};

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(default, rename_all = "camelCase")]
pub struct GetFavoriteRespData {
    #[serde(default)]
    pub list: Vec<ComicInFavoriteRespData>,
    #[serde(default, rename = "folder_list")]
    pub folder_list: Vec<FavoriteFolderRespData>,
    #[serde(deserialize_with = "string_to_i64")]
    pub total: i64,
    #[serde(default, deserialize_with = "string_to_i64")]
    pub count: i64,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ComicInFavoriteRespData {
    #[serde(deserialize_with = "string_to_i64")]
    pub id: i64,
    pub author: String,
    pub description: Option<String>,
    pub name: String,
    #[serde(rename = "latest_ep")]
    pub latest_ep: Option<String>,
    #[serde(rename = "latest_ep_aid")]
    pub latest_ep_aid: Option<String>,
    pub image: String,
    pub category: CategoryRespData,
    #[serde(rename = "category_sub")]
    pub category_sub: CategorySubRespData,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct FavoriteFolderRespData {
    #[serde(rename = "FID")]
    pub fid: String,
    #[serde(rename = "UID")]
    pub uid: String,
    pub name: String,
}

#[cfg(test)]
mod tests {
    use super::GetFavoriteRespData;

    #[test]
    fn get_favorite_resp_parses_numeric_and_string_values() {
        let json_str = r#"{
            "list": [
                {
                    "id": 12345,
                    "author": "测试作者",
                    "name": "测试漫画",
                    "image": "test.jpg",
                    "category": {},
                    "category_sub": {}
                },
                {
                    "id": "67890",
                    "author": "作者2",
                    "name": "漫画2",
                    "image": "test2.jpg",
                    "category": {},
                    "category_sub": {}
                }
            ],
            "folder_list": [
                {
                    "FID": "100",
                    "UID": "200",
                    "name": "我的分组"
                }
            ],
            "total": "2",
            "count": 20
        }"#;
        let parsed: GetFavoriteRespData = serde_json::from_str(json_str).unwrap();
        assert_eq!(parsed.total, 2);
        assert_eq!(parsed.count, 20);
        assert_eq!(parsed.list.len(), 2);
        assert_eq!(parsed.list[0].id, 12345);
        assert_eq!(parsed.list[1].id, 67890);
        assert_eq!(parsed.folder_list.len(), 1);

        // 测试全数字与缺少 folder_list 的情况
        let json_numeric = r#"{
            "total": 0,
            "count": 0
        }"#;
        let parsed_empty: GetFavoriteRespData = serde_json::from_str(json_numeric).unwrap();
        assert_eq!(parsed_empty.total, 0);
        assert_eq!(parsed_empty.count, 0);
        assert!(parsed_empty.list.is_empty());
        assert!(parsed_empty.folder_list.is_empty());
    }
}
