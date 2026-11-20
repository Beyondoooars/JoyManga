import { getCurrentInstance, onUnmounted, ref, watch, type Ref } from 'vue'
import type { ReadingDirection } from '../bindings.ts'
import { pageActionForHorizontalGesture, type PageAction } from '../utils/readerNav.ts'
import {
  IDENTITY_TRANSFORM,
  nextFitMode,
  panTransform,
  pinchTransform,
  resetForPageChange,
  toggleZoom,
  type FitMode,
  type Transform,
} from '../utils/readerZoom.ts'

export interface UseReaderTransformOptions {
  viewerBody?: Readonly<Ref<HTMLElement | null>>
  currentPage: Ref<number>
  readingDirection: Ref<ReadingDirection>
  onPageAction: (action: PageAction) => void
  onToggleHud: () => void
}

/**
 * 封装单页阅读模式的适配方式、双指缩放、平移拖动与左右滑动翻页手势。
 */
export function useReaderTransform(options: UseReaderTransformOptions) {
  const { currentPage, readingDirection, onPageAction, onToggleHud } = options

  const fitMode = ref<FitMode>('width')
  const zoom = ref<Transform>({ ...IDENTITY_TRANSFORM })
  const viewerBody = options.viewerBody ?? ref<HTMLElement | null>(null)
  const isInteracting = ref(false)
  const swipeOffsetX = ref(0)
  const isSwipingAnim = ref(false)
  const justSwiped = ref(false)

  let touchStartX = 0
  let touchStartY = 0
  let touchStartTime = 0
  let isTouching = false
  let isHorizontalSwiping = false
  let isPanning = false
  let panStartZoom: Transform = { ...IDENTITY_TRANSFORM }
  let hasPanned = false
  let pinchStartDistance = 0
  let pinchStartZoom: Transform = { ...IDENTITY_TRANSFORM }
  let pinchAnchor = { x: 0, y: 0 }
  let justSwipedResetTimer: ReturnType<typeof setTimeout> | undefined
  let swipeResetTimer: ReturnType<typeof setTimeout> | undefined

  function resetZoom() {
    zoom.value = resetForPageChange()
  }

  function cycleFitMode() {
    fitMode.value = nextFitMode(fitMode.value)
    resetZoom()
  }

  function containerSize(): { width: number; height: number } {
    const element = viewerBody.value
    return { width: element?.clientWidth ?? window.innerWidth, height: element?.clientHeight ?? window.innerHeight }
  }

  function onViewerDoubleClick(event: MouseEvent) {
    const rect = (viewerBody.value ?? (event.currentTarget as HTMLElement)).getBoundingClientRect()
    zoom.value = toggleZoom(
      zoom.value,
      { x: event.clientX - rect.left, y: event.clientY - rect.top },
      { width: rect.width, height: rect.height },
    )
  }

  watch(currentPage, resetZoom)

  function touchDistance(touches: TouchList): number {
    const [a, b] = [touches[0], touches[1]]
    return Math.hypot(a.clientX - b.clientX, a.clientY - b.clientY)
  }

  function markJustSwiped() {
    justSwiped.value = true
    if (justSwipedResetTimer !== undefined) clearTimeout(justSwipedResetTimer)
    justSwipedResetTimer = setTimeout(() => {
      justSwiped.value = false
      justSwipedResetTimer = undefined
    }, 250)
  }

  function resetSwipeState() {
    isTouching = false
    isHorizontalSwiping = false
    isSwipingAnim.value = true
    swipeOffsetX.value = 0
    if (swipeResetTimer !== undefined) clearTimeout(swipeResetTimer)
    swipeResetTimer = setTimeout(() => {
      isSwipingAnim.value = false
      swipeResetTimer = undefined
    }, 220)
  }

  function resetGestureState() {
    pinchStartDistance = 0
    isPanning = false
    hasPanned = false
    isInteracting.value = false
    justSwiped.value = false
    if (justSwipedResetTimer !== undefined) {
      clearTimeout(justSwipedResetTimer)
      justSwipedResetTimer = undefined
    }
    isTouching = false
    isHorizontalSwiping = false
    isSwipingAnim.value = false
    swipeOffsetX.value = 0
    if (swipeResetTimer !== undefined) {
      clearTimeout(swipeResetTimer)
      swipeResetTimer = undefined
    }
  }

  function onTouchStart(e: TouchEvent) {
    if (e.touches.length === 2) {
      isTouching = false
      isHorizontalSwiping = false
      isPanning = false
      hasPanned = false
      isInteracting.value = true
      pinchStartDistance = touchDistance(e.touches)
      pinchStartZoom = { ...zoom.value }
      const rect = (viewerBody.value ?? (e.currentTarget as HTMLElement)).getBoundingClientRect()
      pinchAnchor = {
        x: (e.touches[0].clientX + e.touches[1].clientX) / 2 - rect.left,
        y: (e.touches[0].clientY + e.touches[1].clientY) / 2 - rect.top,
      }
      return
    }
    if (e.touches.length !== 1) {
      isTouching = false
      isHorizontalSwiping = false
      isPanning = false
      hasPanned = false
      isInteracting.value = false
      swipeOffsetX.value = 0
      return
    }
    const t = e.touches[0]
    touchStartX = t.clientX
    touchStartY = t.clientY
    touchStartTime = Date.now()
    isTouching = true
    isHorizontalSwiping = false
    isPanning = zoom.value.scale > 1.01
    panStartZoom = { ...zoom.value }
    hasPanned = false
    isInteracting.value = isPanning
    isSwipingAnim.value = false
    swipeOffsetX.value = 0
  }

  function onTouchMove(e: TouchEvent) {
    if (e.touches.length === 2 && pinchStartDistance > 0) {
      e.preventDefault()
      const ratio = touchDistance(e.touches) / pinchStartDistance
      zoom.value = pinchTransform(pinchStartZoom, pinchAnchor, ratio, containerSize())
      return
    }
    if (!isTouching || e.touches.length !== 1) return
    const t = e.touches[0]
    const dx = t.clientX - touchStartX
    const dy = t.clientY - touchStartY
    if (isPanning) {
      e.preventDefault()
      if (Math.abs(dx) > 4 || Math.abs(dy) > 4) hasPanned = true
      zoom.value = panTransform(panStartZoom, dx, dy, containerSize())
      return
    }
    const absDx = Math.abs(dx)
    const absDy = Math.abs(dy)

    if (!isHorizontalSwiping) {
      if (absDx > 10 && absDx > absDy * 1.2) {
        isHorizontalSwiping = true
      } else if (absDy > 10) {
        isTouching = false
        return
      }
    }

    if (isHorizontalSwiping) {
      swipeOffsetX.value = dx * 0.45
    }
  }

  function onTouchEnd(e: TouchEvent) {
    if (pinchStartDistance > 0 && e.touches.length < 2) {
      pinchStartDistance = 0
      isInteracting.value = false
      markJustSwiped()
      resetSwipeState()
      return
    }
    if (!isTouching) {
      isInteracting.value = false
      resetSwipeState()
      return
    }
    if (isPanning) {
      const panned = hasPanned
      isPanning = false
      hasPanned = false
      isInteracting.value = false
      if (panned) markJustSwiped()
      resetSwipeState()
      return
    }

    const dt = Math.max(1, Date.now() - touchStartTime)
    const touch = e.changedTouches[0]
    const dx = touch ? touch.clientX - touchStartX : 0
    const dy = touch ? touch.clientY - touchStartY : 0
    const absDx = Math.abs(dx)
    const absDy = Math.abs(dy)
    const velocity = dx / dt

    pinchStartDistance = 0
    isInteracting.value = false
    if (zoom.value.scale > 1.01) {
      resetSwipeState()
      return
    }
    if (isHorizontalSwiping || (absDx > 40 && absDx > absDy * 1.2 && dt < 600)) {
      markJustSwiped()

      if (dx < -40 || velocity < -0.3) {
        onPageAction(pageActionForHorizontalGesture(readingDirection.value, 'left'))
      } else if (dx > 40 || velocity > 0.3) {
        onPageAction(pageActionForHorizontalGesture(readingDirection.value, 'right'))
      }
    }

    resetSwipeState()
  }

  function onTouchCancel() {
    pinchStartDistance = 0
    isPanning = false
    hasPanned = false
    isInteracting.value = false
    resetSwipeState()
  }

  function handleViewerBodyClick() {
    if (justSwiped.value) return
    onToggleHud()
  }

  if (getCurrentInstance()) {
    onUnmounted(() => {
      resetGestureState()
    })
  }

  return {
    fitMode,
    zoom,
    viewerBody,
    isInteracting,
    swipeOffsetX,
    isSwipingAnim,
    justSwiped,
    resetZoom,
    cycleFitMode,
    onViewerDoubleClick,
    resetGestureState,
    onTouchStart,
    onTouchMove,
    onTouchEnd,
    onTouchCancel,
    handleViewerBodyClick,
  }
}
