import { computed, getCurrentInstance, nextTick, onUnmounted, ref, type Ref } from 'vue'
import type { ChapterReaderData } from '../bindings.ts'
import { currentPageFromViewport, isTapGesture } from '../utils/readerNav.ts'
import { computeScrollWindow } from '../utils/readerZoom.ts'

export interface UseReaderScrollOptions {
  scrollViewer?: Readonly<Ref<HTMLElement | null>>
  chapterData: Ref<ChapterReaderData | null>
  currentPage: Ref<number>
  onPageChange: () => void
  onToggleHud: () => void
  onMarkImageError: (filePath: string) => void
  onBoundaryChapter?: (direction: 1 | -1) => boolean
}

const SCROLL_SETTLE_TIMEOUT_MS = 10_000

/**
 * 封装滚动阅读模式的窗口虚拟化、可见页追踪、定位沉降纠偏与轻点手势。
 */
export function useReaderScroll(options: UseReaderScrollOptions) {
  const { chapterData, currentPage, onPageChange, onToggleHud, onMarkImageError, onBoundaryChapter } = options

  const scrollViewer = options.scrollViewer ?? ref<HTMLElement | null>(null)
  let scrollObserver: IntersectionObserver | undefined
  const intersectingPages = new Set<number>()
  const observedPageEls = new Map<number, HTMLElement>()
  const measuredPageSizes = ref<Map<number, { width: number; height: number }>>(new Map())

  /** 纠偏中的目标页；非 null 时观察器静音、图片加载会重新贴顶 */
  let settleTarget: number | null = null
  let settleDeadline = 0
  let settleTimer: ReturnType<typeof setTimeout> | undefined
  let scrollTouchStart: { x: number; y: number; time: number } | null = null
  let lastScrollTouchTime = 0

  const scrollWindow = computed(() =>
    computeScrollWindow(currentPage.value, chapterData.value?.imagePaths.length ?? 0, 8),
  )

  function isScrollPageMounted(page: number): boolean {
    return page >= scrollWindow.value.start && page <= scrollWindow.value.end
  }

  function disconnectScrollObserver() {
    scrollObserver?.disconnect()
    scrollObserver = undefined
    intersectingPages.clear()
    observedPageEls.clear()
  }

  function enterScrollMode() {
    void nextTick(() => {
      observeScrollPages()
      scrollToPage(currentPage.value)
    })
  }

  function leaveScrollMode() {
    stopScrollSettle(false)
    disconnectScrollObserver()
    const container = scrollViewer.value
    if (container) syncCurrentPageFromLayout(container)
  }

  function scrollToPage(page: number) {
    const container = scrollViewer.value
    if (!container) return
    const target = container.querySelector<HTMLElement>(`[data-page="${page}"]`)
    if (!target) return
    stopScrollSettle(false)
    const top = target.getBoundingClientRect().top - container.getBoundingClientRect().top + container.scrollTop
    container.scrollTo({ top, behavior: 'auto' })
    if (!pagesAboveHaveHeight(container, page)) beginScrollSettle(page)
  }

  function beginScrollSettle(page: number) {
    settleTarget = page
    settleDeadline = Date.now() + SCROLL_SETTLE_TIMEOUT_MS
    if (settleTimer !== undefined) clearTimeout(settleTimer)
    settleTimer = setTimeout(() => stopScrollSettle(true), SCROLL_SETTLE_TIMEOUT_MS)
    requestAnimationFrame(pinScrollTarget)
  }

  function stopScrollSettle(resync: boolean) {
    if (settleTarget === null) return
    settleTarget = null
    if (settleTimer !== undefined) {
      clearTimeout(settleTimer)
      settleTimer = undefined
    }
    if (!resync) return
    const container = scrollViewer.value
    if (container) syncCurrentPageFromLayout(container)
  }

  /** 目标页之上是否每页都有确定高度：只有未预留高度且已挂载图片的页才需要等图片解码 */
  function pagesAboveHaveHeight(container: HTMLElement, page: number): boolean {
    for (const el of container.querySelectorAll<HTMLElement>('[data-page]:not(.page-sized)')) {
      if (Number(el.dataset.page) >= page) continue
      if (el.querySelector('.image-error')) continue
      const img = el.querySelector('img')
      if (!img) continue
      if (!img.complete || img.naturalHeight === 0) return false
    }
    return true
  }

  function pinScrollTarget() {
    const container = scrollViewer.value
    const page = settleTarget
    if (!container || page === null) return
    const target = container.querySelector<HTMLElement>(`[data-page="${page}"]`)
    if (!target) {
      stopScrollSettle(false)
      return
    }
    const top = target.getBoundingClientRect().top - container.getBoundingClientRect().top + container.scrollTop
    const maxTop = Math.max(0, container.scrollHeight - container.clientHeight)
    const desired = Math.min(Math.max(top, 0), maxTop)
    if (Math.abs(container.scrollTop - desired) > 1) container.scrollTo({ top: desired, behavior: 'auto' })
    if (pagesAboveHaveHeight(container, page) || Date.now() >= settleDeadline) stopScrollSettle(true)
  }

  function onScrollPageImageLoad(page: number, event: Event) {
    const known = chapterData.value?.pageSizes[page - 1]
    if (!known || known.width <= 0 || known.height <= 0) {
      const target = event.target
      if (target instanceof HTMLImageElement && target.naturalWidth > 0 && target.naturalHeight > 0) {
        const existing = measuredPageSizes.value.get(page)
        if (!existing || existing.width !== target.naturalWidth || existing.height !== target.naturalHeight) {
          const next = new Map(measuredPageSizes.value)
          next.set(page, { width: target.naturalWidth, height: target.naturalHeight })
          measuredPageSizes.value = next
        }
      }
    }
    if (settleTarget === null || page >= settleTarget) return
    pinScrollTarget()
  }

  function onScrollPageImageError(page: number, filePath: string) {
    onMarkImageError(filePath)
    if (settleTarget !== null && page < settleTarget) {
      pinScrollTarget()
    }
  }

  function cancelScrollSettle() {
    stopScrollSettle(false)
  }

  const scrollPageStyles = computed<Array<Record<string, string> | undefined>>(() => {
    const paths = chapterData.value?.imagePaths ?? []
    const sizes = chapterData.value?.pageSizes ?? []
    const measured = measuredPageSizes.value
    return paths.map((_, index) => {
      const size = sizes[index]
      const resolved = size && size.width > 0 && size.height > 0 ? size : measured.get(index + 1)
      if (!resolved || resolved.width <= 0 || resolved.height <= 0) return undefined
      return { aspectRatio: `${resolved.width} / ${resolved.height}` }
    })
  })

  function scrollContainerAtEnd(container: HTMLElement): boolean {
    return container.scrollHeight > container.clientHeight
      && container.scrollTop >= container.scrollHeight - container.clientHeight - 2
  }

  function applyScrollCurrentPage(container: HTMLElement, visible: Array<{ page: number; top: number }>) {
    const page = currentPageFromViewport(visible, scrollContainerAtEnd(container))
    if (page !== null && page !== currentPage.value) {
      currentPage.value = page
      onPageChange()
    }
  }

  function syncCurrentPageFromLayout(container: HTMLElement) {
    const containerRect = container.getBoundingClientRect()
    const visible: Array<{ page: number; top: number }> = []
    for (const el of container.querySelectorAll<HTMLElement>('[data-page]')) {
      const rect = el.getBoundingClientRect()
      const overlap = Math.min(rect.bottom, containerRect.bottom) - Math.max(rect.top, containerRect.top)
      if (rect.height <= 0 || overlap / rect.height < 0.1) continue
      visible.push({ page: Number(el.dataset.page), top: rect.top - containerRect.top })
    }
    applyScrollCurrentPage(container, visible)
  }

  function observeScrollPages() {
    disconnectScrollObserver()
    const container = scrollViewer.value
    if (!container) return
    const pages = container.querySelectorAll<HTMLElement>('[data-page]')
    for (const pageEl of pages) {
      const page = Number(pageEl.dataset.page)
      if (Number.isFinite(page)) observedPageEls.set(page, pageEl)
    }
    scrollObserver = new IntersectionObserver(
      (entries) => {
        for (const entry of entries) {
          const page = Number((entry.target as HTMLElement).dataset.page)
          if (!Number.isFinite(page)) continue
          if (entry.isIntersecting) intersectingPages.add(page)
          else intersectingPages.delete(page)
        }
        // 沉降期间布局还在变化，先把 intersectingPages 更新完再拦截可见页计算，避免恢复时丢状态。
        if (settleTarget !== null) return
        const containerTop = container.getBoundingClientRect().top
        const visible: Array<{ page: number; top: number }> = []
        for (const page of intersectingPages) {
          const el = observedPageEls.get(page)
          if (!el) continue
          visible.push({
            page,
            top: el.getBoundingClientRect().top - containerTop,
          })
        }
        applyScrollCurrentPage(container, visible)
      },
      { root: container, threshold: [0.1, 0.5] },
    )
    pages.forEach((page) => scrollObserver?.observe(page))
  }

  function onScrollTouchStart(e: TouchEvent) {
    cancelScrollSettle()
    lastScrollTouchTime = Date.now()
    if (e.touches.length !== 1 || (e.target instanceof Element && e.target.closest('button, a, input, select, textarea, [role="button"]'))) {
      scrollTouchStart = null
      return
    }
    const touch = e.touches[0]
    scrollTouchStart = { x: touch.clientX, y: touch.clientY, time: Date.now() }
  }

  function onScrollTouchEnd(e: TouchEvent) {
    lastScrollTouchTime = Date.now()
    const start = scrollTouchStart
    scrollTouchStart = null
    const touch = e.changedTouches[0]
    if (!start || !touch) return
    if (isTapGesture(start, { x: touch.clientX, y: touch.clientY, time: Date.now() })) onToggleHud()
  }

  function onScrollTouchCancel() {
    lastScrollTouchTime = Date.now()
    scrollTouchStart = null
  }

  function handleScrollViewerClick() {
    if (Date.now() - lastScrollTouchTime < 700) return
    onToggleHud()
  }

  function scrollByViewport(direction: 1 | -1) {
    const container = scrollViewer.value
    if (!container) return
    cancelScrollSettle()
    const atTop = container.scrollTop <= 0
    const atBottom = container.scrollTop + container.clientHeight >= container.scrollHeight - 2
    if (direction === 1 && atBottom && onBoundaryChapter?.(1)) return
    if (direction === -1 && atTop && onBoundaryChapter?.(-1)) return
    container.scrollBy({ top: direction * container.clientHeight * 0.85, behavior: 'smooth' })
  }

  function resetScrollState(clearMeasuredSizes = false) {
    stopScrollSettle(false)
    disconnectScrollObserver()
    scrollTouchStart = null
    if (clearMeasuredSizes) measuredPageSizes.value = new Map()
  }

  if (getCurrentInstance()) {
    onUnmounted(() => {
      resetScrollState(true)
    })
  }

  return {
    scrollViewer,
    scrollPageStyles,
    isScrollPageMounted,
    enterScrollMode,
    leaveScrollMode,
    observeScrollPages,
    scrollToPage,
    cancelScrollSettle,
    onScrollPageImageLoad,
    onScrollPageImageError,
    onScrollTouchStart,
    onScrollTouchEnd,
    onScrollTouchCancel,
    handleScrollViewerClick,
    scrollByViewport,
    resetScrollState,
  }
}
