use serde::{Deserialize, Serialize};
use specta::Type;
use yaserde::{YaDeserialize, YaSerialize};

use super::{ChapterInfo, Comic};

/// ComicInfo.xml 字段说明（anansi 规范）：<https://anansi-project.github.io/docs/comicinfo/documentation>
/// 兼容性参考：Kavita `ComicInfo.cs`、Komga `ComicInfoProvider.kt`
#[derive(
    Default, Debug, Clone, PartialEq, Serialize, Deserialize, Type, YaSerialize, YaDeserialize,
)]
#[serde(rename_all = "camelCase")]
pub struct ComicInfo {
    /// 漫画标记。`YesAndRightToLeft` 才同时表示"是漫画 + 从右往左"，
    /// Komga 只把该值映射为 RTL，`Yes` 会被当成方向未知（默认 LTR）。
    #[yaserde(rename = "Manga")]
    pub manga: String,
    /// 漫画名
    #[yaserde(rename = "Series")]
    pub series: String,
    /// 出版社
    #[yaserde(rename = "Publisher")]
    pub publisher: String,
    /// 作者
    #[yaserde(rename = "Writer")]
    pub writer: String,
    /// 漫画标签
    #[yaserde(rename = "Tags")]
    pub tags: String,
    /// 漫画类型（分类 / 子分类）
    #[yaserde(rename = "Genre", default = "default_empty")]
    pub genre: String,
    #[yaserde(rename = "Summary")]
    pub summary: String,
    /// 章节名
    #[yaserde(rename = "Title")]
    pub title: String,
    /// 普通章节序号
    #[yaserde(rename = "Number")]
    pub number: Option<String>,
    /// 该章节有多少页
    #[yaserde(rename = "PageCount", default = "default_page_count")]
    pub page_count: i64,
    /// 语言（BCP 47，如 `zh`），供 Komga/Kavita 做语言筛选
    #[yaserde(rename = "LanguageISO", default = "default_empty")]
    pub language_iso: String,
    /// 回源地址（JM 作品页），多个用空格分隔
    #[yaserde(rename = "Web", default = "default_empty")]
    pub web: String,
    /// 章节总数
    /// - `0` => Ongoing
    /// - `非零`且与`Number`或`Volume`一致 => Completed
    /// - `其他非零值` => Ended
    #[yaserde(rename = "Count")]
    pub count: i64,
}

impl ComicInfo {
    /// 默认语言：JM 内容以中文（简体/繁体混排）为主。
    pub const DEFAULT_LANGUAGE: &'static str = "zh";

    #[allow(clippy::cast_possible_wrap)]
    pub fn from(comic: &Comic, chapter_info: &ChapterInfo) -> ComicInfo {
        let number = Some(chapter_info.order.to_string());
        let count = comic.chapter_infos.len() as i64;

        ComicInfo {
            manga: "YesAndRightToLeft".to_string(),
            series: comic.name.clone(),
            publisher: "JoyManga".to_string(),
            writer: comic.author.join(", "),
            tags: comic.tags.join(", "),
            genre: String::new(),
            summary: comic.description.clone(),
            title: chapter_info.chapter_title.clone(),
            number,
            page_count: 0,
            language_iso: Self::DEFAULT_LANGUAGE.to_string(),
            web: comic_source_url(comic.id),
            count,
        }
    }
}

/// JM 作品页地址。导出时只用于元数据回源，不参与任何网络请求。
pub fn comic_source_url(comic_id: i64) -> String {
    format!("https://18comic.vip/album/{comic_id}")
}

/// 反序列化默认值：本版本之前导出的 CBZ 不含这些新元素，缺失时必须能正常解析，
/// 否则扫描旧归档会整本读不出来。
fn default_empty() -> String {
    String::new()
}

fn default_page_count() -> i64 {
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ChapterInfo;

    fn sample_comic() -> Comic {
        Comic {
            id: 123456,
            name: "测试漫画".into(),
            addtime: String::new(),
            description: "简介".into(),
            total_views: String::new(),
            likes: String::new(),
            series_id: String::new(),
            comment_total: String::new(),
            author: vec!["作者甲".into(), "作者乙".into()],
            tags: vec!["标签一".into(), "标签二".into()],
            works: Vec::new(),
            actors: Vec::new(),
            related_list: Vec::new(),
            liked: false,
            is_favorite: false,
            is_aids: false,
            is_downloaded: None,
            comic_download_dir: None,
            local_cover_data: None,
            chapter_infos: vec![
                ChapterInfo {
                    chapter_id: 1,
                    chapter_title: "第1话".into(),
                    order: 1,
                    is_downloaded: None,
                    chapter_download_dir: None,
                },
                ChapterInfo {
                    chapter_id: 2,
                    chapter_title: "第2话".into(),
                    order: 2,
                    is_downloaded: None,
                    chapter_download_dir: None,
                },
            ],
        }
    }

    #[test]
    fn manga_flag_marks_right_to_left() {
        let comic = sample_comic();
        let info = ComicInfo::from(&comic, &comic.chapter_infos[0]);
        // Komga 仅把 YesAndRightToLeft 映射为 RTL
        assert_eq!(info.manga, "YesAndRightToLeft");
    }

    #[test]
    fn metadata_fields_are_filled_for_third_party_readers() {
        let comic = sample_comic();
        let info = ComicInfo::from(&comic, &comic.chapter_infos[1]);
        assert_eq!(info.language_iso, "zh");
        assert_eq!(info.web, "https://18comic.vip/album/123456");
        assert_eq!(info.count, 2);
        assert_eq!(info.number.as_deref(), Some("2"));
        // 页数由调用方在打包时按实际图片数回填，默认 0 表示未知
        assert_eq!(info.page_count, 0);
    }

    #[test]
    fn serialized_xml_contains_new_elements() {
        let comic = sample_comic();
        let mut info = ComicInfo::from(&comic, &comic.chapter_infos[0]);
        info.page_count = 12;
        info.genre = "同人".to_string();
        let xml = yaserde::ser::to_string(&info).unwrap();
        for tag in [
            "<Manga>YesAndRightToLeft</Manga>",
            "<LanguageISO>zh</LanguageISO>",
            "<Web>https://18comic.vip/album/123456</Web>",
            "<PageCount>12</PageCount>",
            "<Genre>同人</Genre>",
        ] {
            assert!(xml.contains(tag), "缺少元素 {tag}: {xml}");
        }

        // 取默认值的字段不写出元素：旧阅读器读到空元素反而可能误判
        let info = ComicInfo::from(&comic, &comic.chapter_infos[0]);
        let xml = yaserde::ser::to_string(&info).unwrap();
        assert!(
            !xml.contains("<PageCount"),
            "PageCount 为 0 时不应写出: {xml}"
        );
        assert!(!xml.contains("<Genre"), "Genre 为空时不应写出: {xml}");
    }

    #[test]
    fn metadata_round_trips_through_old_and_new_xml() {
        let comic = sample_comic();
        let mut info = ComicInfo::from(&comic, &comic.chapter_infos[0]);
        info.page_count = 24;
        let xml = yaserde::ser::to_string(&info).unwrap();
        let parsed: ComicInfo = yaserde::de::from_str(&xml).unwrap();
        assert_eq!(parsed, info);

        // 缺少可选字段的 XML 反序列化时用默认值兜底（不能整本解析失败）
        let minimal = r#"<ComicInfo>
            <Manga>Yes</Manga><Series>示例包</Series><Publisher>JoyManga</Publisher>
            <Writer>作者</Writer><Tags>标签</Tags><Summary>简介</Summary>
            <Title>第1话</Title><Number>1</Number><Count>3</Count>
        </ComicInfo>"#;
        let parsed: ComicInfo = yaserde::de::from_str(minimal).unwrap();
        assert_eq!(parsed.manga, "Yes");
        assert_eq!(parsed.page_count, 0);
        assert_eq!(parsed.language_iso, "");
    }
}
