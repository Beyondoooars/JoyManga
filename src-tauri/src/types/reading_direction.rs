use serde::{Deserialize, Serialize};
use specta::Type;

/// 阅读方向：决定点击区域、键盘、滑动与"上一页/下一页"按钮的语义。
///
/// JM 内容以日式排版为主，默认从右往左读（右滑看下一页）。
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
pub enum ReadingDirection {
    /// 从左往右：左侧点击/左滑为上一页
    LeftToRight,
    /// 从右往左：右侧点击/右滑为上一页（日漫默认）
    #[default]
    RightToLeft,
}
