use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use anyhow::{anyhow, Context};
use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::AppHandle;

use crate::{
    extensions::AppHandleExt,
    responses::{GetComicRespData, RelatedListRespData},
    utils,
};

use super::ChapterInfo;

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
#[allow(clippy::struct_field_names)]
pub struct Comic {
    pub id: i64,
    pub name: String,
    pub addtime: String,
    pub description: String,
    #[serde(rename = "total_views")]
    pub total_views: String,
    pub likes: String,
    pub chapter_infos: Vec<ChapterInfo>,
    #[serde(rename = "series_id")]
    pub series_id: String,
    #[serde(rename = "comment_total")]
    pub comment_total: String,
    pub author: Vec<String>,
    pub tags: Vec<String>,
    pub works: Vec<String>,
    pub actors: Vec<String>,
    #[serde(rename = "related_list")]
    pub related_list: Vec<RelatedListRespData>,
    pub liked: bool,
    #[serde(rename = "is_favorite")]
    pub is_favorite: bool,
    #[serde(rename = "is_aids")]
    pub is_aids: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_downloaded: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comic_download_dir: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_cover_data: Option<String>,
}

impl Comic {
    pub fn from_comic_resp_data(app: &AppHandle, comic: GetComicRespData) -> anyhow::Result<Comic> {
        let mut chapter_infos: Vec<ChapterInfo> = comic
            .series
            .into_iter()
            .enumerate()
            .map(|(index, s)| -> anyhow::Result<ChapterInfo> {
                let chapter_id = s.id.parse().context(format!(
                    "漫画`{}`的章节ID`{}`无法解析为整数",
                    comic.name, s.id
                ))?;
                #[allow(clippy::cast_possible_wrap)]
                let order = (index + 1) as i64;
                let mut chapter_title = format!("第{order}话");
                if !s.name.is_empty() {
                    #[allow(clippy::format_push_string)]
                    chapter_title.push_str(&format!(" {}", s.name));
                }
                let chapter_info = ChapterInfo {
                    chapter_id,
                    chapter_title,
                    order,
                    is_downloaded: None,
                    chapter_download_dir: None,
                };
                Ok(chapter_info)
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        // 如果没有章节信息，就添加一个默认的章节信息
        if chapter_infos.is_empty() {
            chapter_infos.push(ChapterInfo {
                chapter_id: comic.id,
                chapter_title: "第1话".to_owned(),
                order: 1,
                is_downloaded: None,
                chapter_download_dir: None,
            });
        }

        let mut comic = Comic {
            id: comic.id,
            name: comic.name,
            addtime: comic.addtime,
            description: comic.description,
            total_views: comic.total_views,
            likes: comic.likes,
            chapter_infos,
            series_id: comic.series_id,
            comment_total: comic.comment_total,
            author: comic.author,
            tags: comic.tags,
            works: comic.works,
            actors: comic.actors,
            related_list: comic.related_list,
            liked: comic.liked,
            is_favorite: comic.is_favorite,
            is_aids: comic.is_aids,
            is_downloaded: None,
            comic_download_dir: None,
            local_cover_data: None,
        };

        let id_to_dir_map =
            utils::create_id_to_dir_map(app).context("创建漫画ID到下载目录映射失败")?;

        comic
            .update_fields(&id_to_dir_map)
            .context(format!("`{}`更新Comic的字段失败", comic.name))?;

        Ok(comic)
    }

    pub fn update_fields(&mut self, id_to_dir_map: &HashMap<i64, PathBuf>) -> anyhow::Result<()> {
        self.comic_download_dir = None;
        self.is_downloaded = Some(false);
        self.local_cover_data = None;
        for chapter in &mut self.chapter_infos {
            chapter.chapter_download_dir = None;
            chapter.is_downloaded = Some(false);
        }

        if let Some(comic_download_dir) = id_to_dir_map.get(&self.id) {
            self.comic_download_dir = Some(comic_download_dir.clone());
            self.local_cover_data = Self::load_local_cover_data(comic_download_dir);

            self.update_chapter_infos_fields()
                .context("更新章节信息字段失败")?;
            self.is_downloaded = Some(
                self.chapter_infos
                    .iter()
                    .any(|chapter| chapter.is_downloaded == Some(true)),
            );
        }

        Ok(())
    }

    #[inline]
    pub fn load_local_cover_data(comic_download_dir: &Path) -> Option<String> {
        crate::comic_scanner::load_local_cover_data(comic_download_dir)
    }

    #[inline]
    pub fn from_metadata(metadata_path: &Path) -> anyhow::Result<Comic> {
        crate::comic_scanner::from_metadata(metadata_path)
    }

    #[inline]
    pub fn from_cbz(cbz_path: &Path) -> anyhow::Result<Comic> {
        crate::comic_scanner::from_cbz(cbz_path)
    }

    pub fn get_comic_export_dir(&self, app: &AppHandle) -> anyhow::Result<PathBuf> {
        let (download_dir, export_dir) = {
            let config = app.get_config();
            let config = config.read();
            (config.download_dir.clone(), config.export_dir.clone())
        };

        let Some(comic_download_dir) = self.comic_download_dir.clone() else {
            return Err(anyhow!("`comic_download_dir`字段为`None`"));
        };

        let _ = std::fs::create_dir_all(&download_dir);
        let _ = std::fs::create_dir_all(&export_dir);

        let canonical_download_dir = std::fs::canonicalize(&download_dir)
            .context(format!("规范化下载根目录`{}`失败", download_dir.display()))?;
        let canonical_comic_download_dir = std::fs::canonicalize(&comic_download_dir).context(
            format!("规范化漫画下载目录`{}`失败", comic_download_dir.display()),
        )?;

        if canonical_comic_download_dir.is_file()
            || canonical_comic_download_dir == canonical_download_dir
        {
            utils::create_dir_all_beneath(
                &export_dir,
                Path::new(&format!(
                    "{} [JM{}]",
                    utils::filename_filter(&self.name),
                    self.id
                )),
            )
        } else {
            let relative_dir =
                utils::relative_subpath(&canonical_download_dir, &canonical_comic_download_dir)?;
            utils::create_dir_all_beneath(&export_dir, &relative_dir)
        }
        .context(format!(
            "在导出根目录`{}`下安全创建漫画导出目录失败",
            export_dir.display()
        ))
    }

    #[inline]
    pub fn save_comic_metadata(&self) -> anyhow::Result<()> {
        crate::comic_scanner::save_comic_metadata(self)
    }

    pub fn get_cover_path(&self) -> anyhow::Result<PathBuf> {
        let comic_download_dir = self
            .comic_download_dir
            .as_ref()
            .context("`comic_download_dir`字段为`None`")?;

        let cover_path = comic_download_dir.join("cover.jpg");

        Ok(cover_path)
    }

    #[inline]
    pub(crate) fn update_chapter_infos_fields(&mut self) -> anyhow::Result<()> {
        crate::comic_scanner::update_chapter_infos_fields(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose;
    use base64::Engine as _;
    use std::io::Write;
    use zip::write::SimpleFileOptions;
    use zip::ZipWriter;

    struct TestDir(PathBuf);
    impl TestDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("joymanga-test-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn load_local_cover_data_extracts_first_image_from_cbz() {
        let temp = TestDir::new();
        let cbz_path = temp.0.join("test_comic.cbz");

        let file = std::fs::File::create(&cbz_path).unwrap();
        let mut zip = ZipWriter::new(file);

        zip.start_file("ComicInfo.xml", SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"<ComicInfo><Title>Test</Title></ComicInfo>")
            .unwrap();

        zip.start_file("0001.jpg", SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"fake_jpeg_image_data_cover").unwrap();

        zip.start_file("0002.jpg", SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"fake_jpeg_image_data_page_2").unwrap();

        zip.finish().unwrap();

        let cover_data = Comic::load_local_cover_data(&temp.0);
        assert!(cover_data.is_some());
        let cover_url = cover_data.unwrap();
        assert!(cover_url.starts_with("data:image/jpeg;base64,"));
        let b64_payload = cover_url.strip_prefix("data:image/jpeg;base64,").unwrap();
        let decoded = general_purpose::STANDARD.decode(b64_payload).unwrap();
        assert_eq!(decoded, b"fake_jpeg_image_data_cover");
    }

    #[test]
    fn update_chapter_infos_fields_matches_cbz_archive() {
        let temp = TestDir::new();
        let cbz_path = temp.0.join("测试漫画.cbz");
        std::fs::write(&cbz_path, b"dummy_cbz").unwrap();

        let mut comic = Comic {
            id: 12345,
            name: "测试漫画".to_string(),
            comic_download_dir: Some(temp.0.clone()),
            chapter_infos: vec![ChapterInfo {
                chapter_id: 101,
                chapter_title: "第1话".to_string(),
                order: 1,
                is_downloaded: None,
                chapter_download_dir: None,
            }],
            ..Default::default()
        };

        comic.update_chapter_infos_fields().unwrap();
        assert_eq!(comic.chapter_infos[0].is_downloaded, Some(true));
        assert_eq!(comic.chapter_infos[0].chapter_download_dir, Some(cbz_path));
    }

    #[test]
    fn from_cbz_reads_embedded_metadata() {
        let temp = TestDir::new();
        let cbz_path = temp.0.join("embedded_test.cbz");

        let file = std::fs::File::create(&cbz_path).unwrap();
        let mut zip = ZipWriter::new(file);

        let comic_meta = Comic {
            id: 99999,
            name: "内嵌元数据漫画".to_string(),
            author: vec!["测试作者".to_string()],
            chapter_infos: vec![ChapterInfo {
                chapter_id: 8888,
                chapter_title: "第1话".to_string(),
                order: 1,
                is_downloaded: None,
                chapter_download_dir: None,
            }],
            ..Default::default()
        };
        let json_bytes = serde_json::to_vec(&comic_meta).unwrap();
        zip.start_file("元数据.json", SimpleFileOptions::default())
            .unwrap();
        zip.write_all(&json_bytes).unwrap();

        zip.start_file("0001.jpg", SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"fake_cover_bytes").unwrap();

        zip.finish().unwrap();

        let loaded = Comic::from_cbz(&cbz_path).unwrap();
        assert_eq!(loaded.id, 99999);
        assert_eq!(loaded.name, "内嵌元数据漫画");
        assert_eq!(loaded.is_downloaded, Some(true));
        assert_eq!(loaded.chapter_infos[0].is_downloaded, Some(true));
        assert_eq!(
            loaded.chapter_infos[0].chapter_download_dir,
            Some(cbz_path.clone())
        );
        assert!(loaded.local_cover_data.is_some());
    }

    #[test]
    fn flat_mode_chapter_matching_prevents_cross_comic_collision() {
        let temp = TestDir::new();
        // 创建两个不同漫画的第1话CBZ，位于同一个平铺目录下
        let comic_a_cbz = temp.0.join("漫画A - 0001 - 第1话.cbz");
        let comic_b_cbz = temp.0.join("漫画B - 0001 - 第1话.cbz");
        std::fs::write(&comic_a_cbz, b"a").unwrap();
        std::fs::write(&comic_b_cbz, b"b").unwrap();

        // 构造漫画B对象，comic_download_dir指向单个CBZ文件（平铺模式）
        let mut comic_b = Comic {
            id: 200,
            name: "漫画B".to_string(),
            comic_download_dir: Some(comic_b_cbz.clone()),
            chapter_infos: vec![
                ChapterInfo {
                    chapter_id: 201,
                    chapter_title: "第1话".to_string(),
                    order: 1,
                    is_downloaded: None,
                    chapter_download_dir: None,
                },
                ChapterInfo {
                    chapter_id: 202,
                    chapter_title: "第2话".to_string(),
                    order: 2,
                    is_downloaded: None,
                    chapter_download_dir: None,
                },
            ],
            ..Default::default()
        };

        comic_b.update_chapter_infos_fields().unwrap();
        // 漫画B的第1话必须精确匹配漫画B自身，绝不能误匹配漫画A
        assert_eq!(comic_b.chapter_infos[0].is_downloaded, Some(true));
        assert_eq!(
            comic_b.chapter_infos[0].chapter_download_dir,
            Some(comic_b_cbz)
        );
        // 漫画B的第2话未下载，绝不能被误判
        assert_eq!(comic_b.chapter_infos[1].is_downloaded, None);
        assert_eq!(comic_b.chapter_infos[1].chapter_download_dir, None);
    }
}
