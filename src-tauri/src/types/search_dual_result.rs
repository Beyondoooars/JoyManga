use serde::{Deserialize, Serialize};
use specta::Type;

use super::SearchResultVariant;

/// 一次简繁双变体取页的结果。
///
/// 每个变体独立的 `Option` 字段而非一个混合枚举，避免空值语义含糊：
/// - 成功：`Some(variant)`，对应 error 为 `None`
/// - 失败：`variant` 为 `None`，对应 error 为 `Some(原因)`
/// - 本轮未请求：两者都为 `None`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SearchDualResult {
    pub original: Option<SearchResultVariant>,
    pub original_error: Option<String>,
    pub converted: Option<SearchResultVariant>,
    pub converted_error: Option<String>,
    /// 转换后的关键词；`None` 表示该关键词没有另一种写法（数字、英文、简繁同形）
    pub converted_keyword: Option<String>,
    /// 非致命提示（例如转换变体被识别为 JM 号而忽略）
    pub warnings: Vec<String>,
}

impl SearchDualResult {
    pub fn empty(converted_keyword: Option<String>) -> Self {
        Self {
            original: None,
            original_error: None,
            converted: None,
            converted_error: None,
            converted_keyword,
            warnings: Vec::new(),
        }
    }
}
