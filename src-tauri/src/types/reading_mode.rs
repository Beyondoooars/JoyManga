use serde::{Deserialize, Serialize};
use specta::Type;

/// 阅读时的翻页排版方式。
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
pub enum ReadingMode {
    /// 单页翻页：一次显示一页，点击/滑动/键盘翻页。日漫默认。
    #[default]
    Paginated,
    /// 上下滚动：整章纵向排列，一屏可看多页，适合条漫和快速浏览。
    Scroll,
}
