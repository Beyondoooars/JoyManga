/**
 * 阅读器的方向与翻页决策。
 *
 * 这些判断被点击区、键盘、滑动共用；抽成纯函数后可以用 node --test 直接覆盖，
 * 避免"方向只改了一半"（例如点击改了、键盘没改）。
 */

export type ReadingDirection = 'LeftToRight' | 'RightToLeft'

export type ReadingMode = 'Paginated' | 'Scroll'

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

/**
 * 点击左/右翻页区：固定左侧上一页、右侧下一页，与阅读方向无关。
 *
 * 点击区不再跟随「翻页方向」设置：移动端最自然的手感是左退右进，
 * 而 RTL 用户改用滑动翻页（滑动仍按阅读方向判断）。
 */
export function pageActionForTapZone(zone: 'left' | 'right'): PageAction {
  return zone === 'left' ? 'prev' : 'next'
}

/**
 * 滚动模式下的"当前页"：取最早进入视口顶部的那一页。
 *
 * IntersectionObserver 一次可能回调多页，只按「回调顺序」取会跳页；
 * 按页面在文档中的位置排序才稳定。`tops` 为各页相对滚动容器的顶部偏移。
 */
export function currentPageFromVisiblePages(
  visible: Array<{ page: number; top: number }>,
): number | null {
  if (visible.length === 0) return null
  return visible.reduce((top, item) => (item.top < top.top ? item : top)).page
}

/**
 * 键盘映射。左右方向键与点击区一致（左退右进），不随阅读方向翻转；
 * 空格/PageDown/PageUp 表示"往后读/往前读"（与桌面阅读器的通行习惯一致）。
 */
export function pageActionForKey(key: string): PageAction | null {
  switch (key) {
    case 'ArrowLeft':
    case 'a':
    case 'A':
      return pageActionForTapZone('left')
    case 'ArrowRight':
    case 'd':
    case 'D':
      return pageActionForTapZone('right')
    case ' ':
    case 'PageDown':
      return 'next'
    case 'PageUp':
      return 'prev'
    default:
      return null
  }
}
