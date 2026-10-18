/**
 * 阅读器的方向与翻页决策。
 *
 * 这些判断被点击区、键盘、滑动共用；抽成纯函数后可以用 node --test 直接覆盖，
 * 避免"方向只改了一半"（例如点击改了、键盘没改）。
 */

export type ReadingDirection = 'LeftToRight' | 'RightToLeft'

export type PageAction = 'next' | 'prev'

export function isRtl(direction: ReadingDirection): boolean {
  return direction === 'RightToLeft'
}

/** 页面内容随手指移动时，RTL 下"看下一页"对应向左滑 */
export function pageActionForHorizontalGesture(
  direction: ReadingDirection,
  gesture: 'left' | 'right',
): PageAction {
  const forward = gesture === 'left'
  return forward === !isRtl(direction) ? 'next' : 'prev'
}

/** 点击左/右翻页区：RTL 下左侧是下一页 */
export function pageActionForTapZone(
  direction: ReadingDirection,
  zone: 'left' | 'right',
): PageAction {
  const forward = zone === 'left'
  return forward === isRtl(direction) ? 'next' : 'prev'
}

/**
 * 键盘映射。左右方向键遵循阅读方向；空格/PageDown/PageUp 表示"往后读/往前读"，
 * 与阅读方向无关（与桌面阅读器的通行习惯一致）。
 */
export function pageActionForKey(
  direction: ReadingDirection,
  key: string,
): PageAction | null {
  switch (key) {
    case 'ArrowLeft':
    case 'a':
    case 'A':
      return pageActionForTapZone(direction, 'left')
    case 'ArrowRight':
    case 'd':
    case 'D':
      return pageActionForTapZone(direction, 'right')
    case ' ':
    case 'PageDown':
      return 'next'
    case 'PageUp':
      return 'prev'
    default:
      return null
  }
}
