/**
 * 阅读器缩放与适配模式的纯计算部分。
 *
 * 抽出来的原因：缩放涉及"双击放大到多少、以哪个点为中心、边界怎么夹紧"，
 * 这些规则最容易写错，且和 DOM 无关，可以单测覆盖。
 */

export type FitMode = 'width' | 'height' | 'screen' | 'original'

export const FIT_MODES: FitMode[] = ['width', 'height', 'screen', 'original']

export const FIT_MODE_LABELS: Record<FitMode, string> = {
  width: '适合宽度',
  height: '适合高度',
  screen: '适合屏幕',
  original: '原始尺寸',
}

/** 点击适配按钮时循环切换的顺序（与 Mihon 的心智一致：宽 → 高 → 屏幕 → 原始） */
export function nextFitMode(current: FitMode): FitMode {
  const index = FIT_MODES.indexOf(current)
  return FIT_MODES[(index + 1) % FIT_MODES.length]
}

export interface Size {
  width: number
  height: number
}

export interface Point {
  x: number
  y: number
}

export interface Transform {
  scale: number
  /** 平移量（相对居中位置的像素偏移） */
  offsetX: number
  offsetY: number
}

export const IDENTITY_TRANSFORM: Transform = { scale: 1, offsetX: 0, offsetY: 0 }

export const MIN_SCALE = 1
export const MAX_SCALE = 5

/**
 * 双击某个点时的新变换：未放大 → 放大到 2x 并让该点居中；
 * 已放大 → 复位。触点用相对容器中心的坐标。
 */
export function toggleZoom(current: Transform, point: Point, container: Size): Transform {
  if (current.scale > 1.01) return { ...IDENTITY_TRANSFORM }
  const scale = 2
  // 让点击位置保持在视野中心：平移量 = -(触点到中心的偏移) * (scale - 1)
  const dx = point.x - container.width / 2
  const dy = point.y - container.height / 2
  return clampTransform({
    scale,
    offsetX: -dx * (scale - 1),
    offsetY: -dy * (scale - 1),
  }, container)
}

/** 把缩放与平移限制在合法范围，避免把图片拖出视野留下大片空白。 */
export function clampTransform(transform: Transform, container: Size): Transform {
  const scale = Math.min(MAX_SCALE, Math.max(MIN_SCALE, transform.scale))
  if (scale <= 1.01) return { ...IDENTITY_TRANSFORM, scale: Math.min(scale, 1) }
  // 放大后可平移的余量 = (内容尺寸 - 容器尺寸) / 2；这里按容器尺寸近似
  const maxX = (container.width * (scale - 1)) / 2
  const maxY = (container.height * (scale - 1)) / 2
  return {
    scale,
    offsetX: Math.min(maxX, Math.max(-maxX, transform.offsetX)),
    offsetY: Math.min(maxY, Math.max(-maxY, transform.offsetY)),
  }
}

/** 双指缩放：以两指中点为锚点，按比例缩放并夹紧。 */
export function pinchTransform(
  start: Transform,
  anchor: Point,
  scaleRatio: number,
  container: Size,
): Transform {
  const scale = Math.min(MAX_SCALE, Math.max(MIN_SCALE, start.scale * scaleRatio))
  const effectiveRatio = start.scale > 0 ? scale / start.scale : 1
  const dx = anchor.x - container.width / 2
  const dy = anchor.y - container.height / 2
  return clampTransform({
    scale,
    offsetX: start.offsetX * effectiveRatio - dx * (effectiveRatio - 1),
    offsetY: start.offsetY * effectiveRatio - dy * (effectiveRatio - 1),
  }, container)
}

/** 放大状态下的单指拖动平移，边界始终被 clampTransform 夹紧。 */
export function panTransform(
  start: Transform,
  deltaX: number,
  deltaY: number,
  container: Size,
): Transform {
  if (start.scale <= 1.01) return { ...IDENTITY_TRANSFORM }
  return clampTransform({
    scale: start.scale,
    offsetX: start.offsetX + deltaX,
    offsetY: start.offsetY + deltaY,
  }, container)
}

/** 滚动模式下当前挂载图片的页码窗口（含首尾页，1-based）。 */
export function computeScrollWindow(
  currentPage: number,
  totalPages: number,
  radius = 8,
): { start: number; end: number } {
  if (totalPages <= 0) return { start: 1, end: 0 }
  const center = Math.min(totalPages, Math.max(1, Math.trunc(currentPage) || 1))
  const r = Math.max(0, Math.trunc(radius))
  return {
    start: Math.max(1, center - r),
    end: Math.min(totalPages, center + r),
  }
}

/** 翻页/换章时必须复位，否则新页面会带着上一页的放大状态。 */
export function resetForPageChange(): Transform {
  return { ...IDENTITY_TRANSFORM }
}
