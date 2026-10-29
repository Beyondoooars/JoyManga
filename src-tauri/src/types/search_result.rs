use std::{collections::HashMap, path::PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::AppHandle;

use crate::responses::{
    CategoryRespData, CategorySubRespData, ComicInSearchRespData, SearchResp, SearchRespData,
};

use super::Comic;

/// 服务端搜索每页固定返回 80 条（与官方客户端一致）。
pub const SEARCH_PAGE_SIZE: i64 = 80;

/// 是否还有下一个服务端页。
///
/// 只有"整页返回"才可能还有更多；不足一页即最后一页。再结合 total 判断整页边界，
/// 避免总数正好是 80 的倍数时多取一个空页。total 不可信（<=0）时以整页为准。
pub fn compute_has_more(page: i64, total: i64, content_len: usize) -> bool {
    let full_page = content_len as i64 >= SEARCH_PAGE_SIZE;
    full_page && (total <= 0 || page.saturating_mul(SEARCH_PAGE_SIZE) < total)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub enum SearchResultVariant {
    SearchResult(SearchResult),
    // 用Box包装Comic，因为Comic比SearchResult大得多
    // 如果不用Box包装，即使SearchResult的类型是SearchResult，也会占用与Comic一样大的内存
    Comic(Box<Comic>),
}

impl SearchResultVariant {
    /// `keyword` 是本次请求实际使用的关键词（转换变体时是转换后的写法），
    /// 不作为服务端回显使用，保证界面和会话状态稳定。
    pub fn from_search_resp_with_map(
        app: &AppHandle,
        search_resp: SearchResp,
        page: i64,
        keyword: &str,
        id_to_dir_map: &HashMap<i64, PathBuf>,
    ) -> anyhow::Result<SearchResultVariant> {
        match search_resp {
            SearchResp::SearchRespData(search_resp_data) => {
                let search_result =
                    SearchResult::from_resp_data(search_resp_data, page, keyword, id_to_dir_map)?;
                Ok(SearchResultVariant::SearchResult(search_result))
            }
            SearchResp::ComicRespData(get_comic_resp) => {
                let comic = Comic::from_comic_resp_data(app, *get_comic_resp)?;
                Ok(SearchResultVariant::Comic(Box::new(comic)))
            }
        }
    }
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    pub search_query: String,
    pub total: i64,
    pub content: Vec<ComicInSearch>,
    /// 是否还有下一页；双变体合并后只代表"这个变体"是否还有页
    pub has_more: bool,
}

impl SearchResult {
    pub fn from_resp_data(
        search_resp_data: SearchRespData,
        page: i64,
        keyword: &str,
        id_to_dir_map: &HashMap<i64, PathBuf>,
    ) -> anyhow::Result<SearchResult> {
        let has_more =
            compute_has_more(page, search_resp_data.total, search_resp_data.content.len());
        let content = search_resp_data
            .content
            .into_iter()
            .map(|comic| ComicInSearch::from_resp_data(comic, id_to_dir_map))
            .collect::<anyhow::Result<_>>()?;

        Ok(SearchResult {
            search_query: keyword.to_string(),
            total: search_resp_data.total,
            content,
            has_more,
        })
    }
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ComicInSearch {
    pub id: i64,
    pub author: String,
    pub name: String,
    pub image: String,
    pub category: CategoryRespData,
    pub category_sub: CategorySubRespData,
    pub liked: bool,
    pub is_favorite: bool,
    pub update_at: i64,
    pub is_downloaded: bool,
    pub comic_download_dir: PathBuf,
}

impl ComicInSearch {
    pub fn from_resp_data(
        resp_data: ComicInSearchRespData,
        id_to_dir_map: &HashMap<i64, PathBuf>,
    ) -> anyhow::Result<ComicInSearch> {
        let id: i64 = resp_data.id.parse().context("将id解析为i64失败")?;

        let mut comic = ComicInSearch {
            id,
            author: resp_data.author,
            name: resp_data.name,
            image: resp_data.image,
            category: resp_data.category,
            category_sub: resp_data.category_sub,
            liked: resp_data.liked,
            is_favorite: resp_data.is_favorite,
            update_at: resp_data.update_at,
            is_downloaded: false,
            comic_download_dir: PathBuf::new(),
        };

        comic.update_fields(id_to_dir_map);

        Ok(comic)
    }

    pub fn update_fields(&mut self, id_to_dir_map: &HashMap<i64, PathBuf>) {
        self.comic_download_dir = PathBuf::new();
        self.is_downloaded = false;
        if let Some(comic_download_dir) = id_to_dir_map.get(&self.id) {
            self.comic_download_dir = comic_download_dir.clone();
            self.is_downloaded = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::compute_has_more;

    #[test]
    fn has_more_requires_full_page() {
        // 不足一页：最后一页
        assert!(!compute_has_more(2, 100, 20));
        assert!(!compute_has_more(1, 20, 20));
    }

    #[test]
    fn has_more_uses_total_for_full_page() {
        assert!(compute_has_more(1, 100, 80));
        assert!(!compute_has_more(2, 160, 80));
        // total 偏大时最多多取一个空页
        assert!(compute_has_more(2, 200, 80));
    }

    #[test]
    fn has_more_falls_back_to_page_size_when_total_unknown() {
        assert!(compute_has_more(1, 0, 80));
        assert!(!compute_has_more(1, 0, 79));
    }
}
