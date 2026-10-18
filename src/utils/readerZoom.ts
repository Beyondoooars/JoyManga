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

/** 图片在给定容器内按适配模式铺开后的显示尺寸（不放大超过原始尺寸，original 除外）。 */
export function displaySize(image: Size, container: Size, mode: FitMode): Size {
  if (image.width <= 0 || image.height <= 0) return { width: 0, height: 0 }
  const scaleX = container.width / image.width
  const scaleY = container.height / image.height
  switch (mode) {
    case 'width':
      return { width: container.width, height: image.height * scaleX }
    case 'height':
      return { width: image.width * scaleY, height: container.height }
    case 'screen': {
      // 完整显示且不裁剪：取较小的缩放比
      const scale = Math.min(scaleX, scaleY)
      return { width: image.width * scale, height: image.height * scale }
    }
    case 'original':
      return { width: image.width, height: image.height }
  }
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
  const dx = anchor.x - container.width / 2
  const dy = anchor.y - container.height / 2
  return clampTransform({
    scale,
    offsetX: start.offsetX - dx * (scaleRatio - 1),
    offsetY: start.offsetY - dy * (scaleRatio - 1),
  }, container)
}

/** 翻页/换章时必须复位，否则新页面会带着上一页的放大状态。 */
export function resetForPageChange(): Transform {
  return { ...IDENTITY_TRANSFORM }
}
