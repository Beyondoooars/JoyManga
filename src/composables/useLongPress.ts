import { getCurrentInstance, onBeforeUnmount, onDeactivated } from 'vue'

export interface UseLongPressOptions<T = void> {
  delayMs?: number
  moveTolerancePx?: number
  vibrateMs?: number
  shouldStart?: (event: PointerEvent) => boolean
  onLongPress: (payload: T) => void
}

/**
 * 封装移动端长按进入选择模式的手势识别：
 * 按下超过阈值移动即取消（避免滑动列表误触），触发后吞掉紧随其后的 click。
 */
export function useLongPress<T = void>(options: UseLongPressOptions<T>) {
  const delayMs = options.delayMs ?? 480
  const moveTolerancePx = options.moveTolerancePx ?? 10
  const vibrateMs = options.vibrateMs ?? 12

  let pressTimer: ReturnType<typeof setTimeout> | undefined
  let pressStart: { x: number; y: number } | undefined
  let longPressed = false

  function clearPress() {
    if (pressTimer !== undefined) {
      clearTimeout(pressTimer)
      pressTimer = undefined
    }
    pressStart = undefined
  }

  function startPress(event: PointerEvent, ...args: T extends void ? [] : [payload: T]) {
    if (event.button !== 0) return
    if (options.shouldStart && !options.shouldStart(event)) return
    clearPress()
    longPressed = false
    pressStart = { x: event.clientX, y: event.clientY }
    const payload = args[0] as T
    pressTimer = setTimeout(() => {
      pressTimer = undefined
      longPressed = true
      if (vibrateMs > 0 && navigator.vibrate) navigator.vibrate(vibrateMs)
      options.onLongPress(payload)
    }, delayMs)
  }

  function movePress(event: PointerEvent) {
    if (pressStart === undefined) return
    if (
      Math.abs(event.clientX - pressStart.x) > moveTolerancePx
      || Math.abs(event.clientY - pressStart.y) > moveTolerancePx
    ) {
      clearPress()
    }
  }

  function endPress() {
    clearPress()
  }

  function consumeLongPressClick(event: MouseEvent): boolean {
    if (!longPressed) return false
    longPressed = false
    event.preventDefault()
    event.stopPropagation()
    return true
  }

  if (getCurrentInstance()) {
    onBeforeUnmount(clearPress)
    onDeactivated(clearPress)
  }

  return {
    startPress,
    movePress,
    endPress,
    clearPress,
    consumeLongPressClick,
  }
}
