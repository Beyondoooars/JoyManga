mod cbz_mode;
mod chapter_info;
mod comic;
mod comic_info;
mod daily_check_in;
mod failure;
mod favorite_sort;
mod get_favorite_result;
mod get_weekly_result;
mod log_level;
mod page_size;
mod proxy_mode;
mod reading_direction;
mod reading_mode;
mod search_dual_result;
mod search_result;
mod search_sort;

pub use cbz_mode::*;
pub use chapter_info::*;
pub use comic::*;
pub use comic_info::*;
pub use daily_check_in::*;
pub use failure::*;
pub use favorite_sort::*;
pub use get_favorite_result::*;
pub use get_weekly_result::*;
pub use log_level::*;
pub use page_size::*;
pub use proxy_mode::*;
pub use reading_direction::*;
pub use reading_mode::*;
pub use search_dual_result::*;
pub use search_result::*;
pub use search_sort::*;

use serde::{Deserialize, Serialize};
use specta::Type;

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct Category {
    pub id: Option<String>,
    pub title: Option<String>,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct CategorySub {
    pub id: Option<String>,
    pub title: Option<String>,
}
