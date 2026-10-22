<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue'
import { commands, type ChapterReaderData, type ChapterNavInfo, type ReadingDirection, type ReadingMode } from '../bindings.ts'
import { useStore } from '../store.ts'
import { getErrorMessage } from '../utils/errors.ts'
import { imageUrlFor } from '../utils/imageCache.ts'
import { bookmarkId, type ReadingProgress } from '../utils/readingState.ts'
import { useMessage } from 'naive-ui'
import {
  PhX,
  PhCaretLeft,
  PhCaretRight,
  PhCaretDoubleLeft,
  PhCaretDoubleRight,
  PhArrowsHorizontal,
  PhArrowsVertical,
  PhBookmarkSimple,
  PhListBullets,
  PhCornersOut,
} from '@phosphor-icons/vue'
import { useBackHandler } from '../composables/useAndroidBack.ts'
import { useIsMobile } from '../composables/useIsMobile.ts'
import {
  onVolumeKeyPage,
  setImmersiveMode,
  setKeepScreenOn,
  setVolumeKeyPaging,
} from '../utils/androidStorage.ts'
import {
  currentPageFromViewport,
  isTapGesture,
  pageActionForHorizontalGesture,
  pageActionForTapZone,
  pageActionForKey,
  type PageAction,
} from '../utils/readerNav.ts'
import {
  FIT_MODE_LABELS,
  IDENTITY_TRANSFORM,
  nextFitMode,
  pinchTransform,
  resetForPageChange,
  toggleZoom,
  type FitMode,
  type Transform,
} from '../utils/readerZoom.ts'

const store = useStore()
const message = useMessage()

const { isAndroid } = useIsMobile()
const hudVisible = ref(true)

// 全屏：进入阅读器默认沉浸式（隐藏状态栏/导航栏），头部按钮可切回
const fullscreen = ref(true)

function applyFullscreen() {
  setImmersiveMode(fullscreen.value)
}

function toggleFullscreen() {
  fullscreen.value = !fullscreen.value
  applyFullscreen()
}

function toggleHud() {
  hudVisible.value = !hudVisible.value
}

useBackHandler(() => {
  store.closeReader()
  return true
}, 100)

const loading = ref(false)
const error = ref<string | null>(null)
const chapterData = ref<ChapterReaderData | null>(null)
// 浏览器的图片解码缓存负责内存占用，这里只需记录哪些页编码失败，用于显示重试入口。
const imageErrors = ref<Map<string, string>>(new Map())
// 阅读进度写入防抖（见 saveProgress / flushProgress）
let progressSaveTimer: ReturnType<typeof setTimeout> | undefined
let pendingProgress: ReadingProgress | undefined
const bookmarksShowing = ref(false)
let chapterGeneration = 0

const fitMode = ref<FitMode>('width')
const zoom = ref<Transform>({ ...IDENTITY_TRANSFORM })
const viewerBody = ref<HTMLElement | null>(null)
const currentPage = ref(1)

// 阅读排版：单页翻页 / 上下滚动。偏好来自设置，可在阅读器头部临时切换。
const readingMode = ref<ReadingMode>(store.config?.readingMode ?? 'Paginated')
const scrollViewer = ref<HTMLElement | null>(null)
// 滚动模式下当前可见页，用于进度保存与页码显示；由 IntersectionObserver 维护。
let scrollObserver: IntersectionObserver | undefined
// 程序化定位的目标页。非 null 表示正在等上方图片解码、持续校正位置（见 scrollToPage）。
let pendingScrollPage: number | null = null
let scrollResizeObserver: ResizeObserver | undefined
let scrollPollTimer: ReturnType<typeof setTimeout> | undefined
let scrollDeadlineTimer: ReturnType<typeof setTimeout> | undefined
/** 校正期间检查位置与上方图片解码情况的间隔 */
const SCROLL_ALIGN_POLL_MS = 150
/** 校正的兜底时限：上方图片迟迟不返回时也不能一直压着页码观察器 */
const SCROLL_SETTLE_MAX_MS = 10000

// 设置页改动阅读方式后立刻同步（阅读器可能还开着）
watch(
  () => store.config?.readingMode,
  (mode) => {
    if (mode && mode !== readingMode.value) {
      readingMode.value = mode
      if (mode === 'Scroll') enterScrollMode()
      else leaveScrollMode()
    }
  },
)

function toggleReadingMode() {
  readingMode.value = readingMode.value === 'Paginated' ? 'Scroll' : 'Paginated'
  if (store.config) store.config.readingMode = readingMode.value
  resetZoom()
  if (readingMode.value === 'Scroll') enterScrollMode()
  else leaveScrollMode()
}

/** 切到滚动：等模板渲染出容器后建立观察器，并把当前页滚进视野，避免从第一页重新开始 */
function enterScrollMode() {
  void nextTick(() => {
    observeScrollPages()
    scrollToPage(currentPage.value)
  })
}

function leaveScrollMode() {
  scrollObserver?.disconnect()
  scrollObserver = undefined
  // 收尾会按当前布局补算一次当前页，切回单页时正好落在正在读的那一页
  finishScrollSettle()
}

/**
 * 滚动模式：把指定页对齐到视口顶部。
 *
 * 只滚一次是不够的：`loading="lazy"` 的图片在解码前高度为 0，目标页上方任何一张
 * 图加载完成都会把它往下推，一次 scrollTo 之后位置就漂了。所以先粗定位，再持续
 * 把目标页贴回顶部，直到上方每页都拿到确定高度（或到达兜底时限）。
 * 用户一旦自己触摸/滚轮接管，就立即停止校正，不抢滚动条。
 */
function scrollToPage(page: number) {
  const container = scrollViewer.value
  // 新的定位开始前先收掉上一次的校正，避免两套目标互相拉扯
  cancelScrollSettle()
  if (!container || !container.querySelector(`[data-page="${page}"]`)) return
  pendingScrollPage = page
  scrollToPendingTarget()
  // 图片解码会改变页高（ResizeObserver），缓存命中或已解码完的则未必再触发观察器，
  // 所以再用低频轮询兜底。
  scrollResizeObserver = new ResizeObserver(() => alignToPendingTarget())
  container.querySelectorAll<HTMLElement>('[data-page]').forEach((el) => scrollResizeObserver?.observe(el))
  scrollDeadlineTimer = setTimeout(finishScrollSettle, SCROLL_SETTLE_MAX_MS)
  scheduleAlignPoll()
}

function scrollToPendingTarget() {
  const container = scrollViewer.value
  if (!container || pendingScrollPage === null) return
  const target = container.querySelector<HTMLElement>(`[data-page="${pendingScrollPage}"]`)
  if (!target) return
  // 不能用 offsetTop：它的参照是 offsetParent（这里是定位的 reader-body），
  // 不是滚动容器；两者一旦有内边距或偏移就会滚错位置。
  const top = target.getBoundingClientRect().top - container.getBoundingClientRect().top + container.scrollTop
  // 一律瞬时定位：平滑滚动期间目标还会被图片推走，动画落点必然是错的。
  container.scrollTo({ top, behavior: 'auto' })
}

/**
 * 定位校正：把目标页贴回顶部；上方图片都已解码且已贴住，就结束校正。
 *
 * 判据用「上方每一页是否都有确定高度」，而不是 load 事件或固定时长的静置：
 * 图片可能命中缓存、在监听建立前就解码完；也可能很慢，慢到静置计时早就到期，
 * 之后再加载又把位置推走。只有上方高度确定下来，位置才算稳定。
 */
function alignToPendingTarget() {
  const container = scrollViewer.value
  if (!container || pendingScrollPage === null) return
  const target = container.querySelector<HTMLElement>(`[data-page="${pendingScrollPage}"]`)
  // 目标页已不在（换章/退出滚动模式）：必须收掉校正，否则观察器会被一直挡着，
  // 页码和阅读进度就再也不更新了
  if (!target) {
    finishScrollSettle()
    return
  }
  scrollToPendingTarget()
  if (!pagesAboveAreDecoded(container, pendingScrollPage)) return
  const offset = target.getBoundingClientRect().top - container.getBoundingClientRect().top
  // 已经贴住，或者滚到底了也贴不住（最后几页不足一屏）——上方高度已确定，位置不会再变。
  // 「到底」必须以内容真的超出容器为前提：章节还没撑开时 scrollTop 也是 0，
  // 若认成到底就会提前收尾，让观察器把当前页改回第一页并写坏阅读进度。
  if (Math.abs(offset) <= 1 || (container.scrollHeight > container.clientHeight + 1 && scrollContainerAtEnd(container))) {
    finishScrollSettle()
  }
}

/**
 * 轮询兜底。ResizeObserver 覆盖图片解码引起的尺寸变化，但缓存命中、解码发生在
 * 观察器建立之前的图片不会再触发它，所以用低频轮询补上。
 */
function scheduleAlignPoll() {
  scrollPollTimer = setTimeout(() => {
    scrollPollTimer = undefined
    if (pendingScrollPage === null) return
    alignToPendingTarget()
    if (pendingScrollPage !== null) scheduleAlignPoll()
  }, SCROLL_ALIGN_POLL_MS)
}

/** 目标页上方的每一页是否都已拿到确定高度（图片解码完成或已失败换成骨架） */
function pagesAboveAreDecoded(container: HTMLElement, page: number): boolean {
  for (const el of container.querySelectorAll<HTMLElement>('[data-page]')) {
    if (Number(el.dataset.page) >= page) continue
    const img = el.querySelector('img')
    if (img && !img.complete) return false
    if (el.getBoundingClientRect().height < 1) return false
  }
  return true
}

/**
 * 结束定位校正：位置已稳定，或用户/退出滚动模式接手了滚动。
 *
 * 校正期间可见性反映的是布局中间态，观察器结果被压着不用；结束后必须按当前
 * 布局补算一次当前页，否则页码与阅读进度会停在校正前的位置。
 */
function finishScrollSettle(syncPage = true) {
  if (scrollDeadlineTimer !== undefined) {
    clearTimeout(scrollDeadlineTimer)
    scrollDeadlineTimer = undefined
  }
  if (scrollPollTimer !== undefined) {
    clearTimeout(scrollPollTimer)
    scrollPollTimer = undefined
  }
  scrollResizeObserver?.disconnect()
  scrollResizeObserver = undefined
  pendingScrollPage = null
  const container = scrollViewer.value
  // 卸载时不再回写页码：此时进度已经 flush 过，改出来的新进度没有机会落盘
  if (syncPage && container) syncCurrentPageFromLayout(container)
}

/** 滚动容器是否已到底（末尾不足一屏时，最后一页永远贴不到顶部） */
function scrollContainerAtEnd(container: HTMLElement): boolean {
  return container.scrollHeight > container.clientHeight
    && container.scrollTop >= container.scrollHeight - container.clientHeight - 2
}

/** 用一份可见页列表更新当前页；号码没变就什么都不做 */
function applyScrollCurrentPage(container: HTMLElement, visible: Array<{ page: number; top: number }>) {
  const page = currentPageFromViewport(visible, scrollContainerAtEnd(container))
  if (page !== null && page !== currentPage.value) {
    currentPage.value = page
    preloadCurrentRange()
  }
}

/** 按几何位置取可见页，规则与 IntersectionObserver 的回调一致 */
function syncCurrentPageFromLayout(container: HTMLElement) {
  const containerRect = container.getBoundingClientRect()
  const visible: Array<{ page: number; top: number }> = []
  for (const el of container.querySelectorAll<HTMLElement>('[data-page]')) {
    const rect = el.getBoundingClientRect()
    const overlap = Math.min(rect.bottom, containerRect.bottom) - Math.max(rect.top, containerRect.top)
    // 阈值与观察器的 0.1 对齐：只露一条边不算这一页可见
    if (rect.height <= 0 || overlap / rect.height < 0.1) continue
    visible.push({ page: Number(el.dataset.page), top: rect.top - containerRect.top })
  }
  applyScrollCurrentPage(container, visible)
}

/**
 * 中断校正（用户接管、开始新的定位）。不回写当前页：调用方刚设好的目标页
 * 才是此刻的正确值，按旧布局补算反而会把它覆盖掉。
 */
function cancelScrollSettle() {
  if (pendingScrollPage !== null) finishScrollSettle(false)
}

/** 滚动模式：监听各页可见比例，把「最靠上的可见页」作为当前页 */
function observeScrollPages() {
  scrollObserver?.disconnect()
  const container = scrollViewer.value
  if (!container) return
  const pages = container.querySelectorAll<HTMLElement>('[data-page]')
  scrollObserver = new IntersectionObserver(
    (entries) => {
      // 定位校正期间页面高度还在变，此时的可见性反映的是布局中间态，
      // 用它覆盖 currentPage 会把刚恢复的进度改回第一页。
      if (pendingScrollPage !== null) return
      const containerTop = container.getBoundingClientRect().top
      const visible = entries
        .filter((entry) => entry.isIntersecting)
        .map((entry) => ({
          page: Number((entry.target as HTMLElement).dataset.page),
          top: entry.boundingClientRect.top - containerTop,
        }))
        .filter((item) => Number.isFinite(item.page))
      applyScrollCurrentPage(container, visible)
    },
    { root: container, threshold: [0.1, 0.5] },
  )
  pages.forEach((page) => scrollObserver?.observe(page))
}

const currentImagePath = computed(() => {
  if (!chapterData.value) return null
  return chapterData.value.imagePaths[currentPage.value - 1] ?? null
})

const currentBookmarkId = computed(() => chapterData.value
  ? bookmarkId(chapterData.value.comicId, chapterData.value.chapterId, currentPage.value) : '')
const currentIsBookmarked = computed(() => store.readingState.bookmarks.some((item) => item.id === currentBookmarkId.value))
const comicBookmarks = computed(() => store.readingState.bookmarks.filter((item) => item.comicId === chapterData.value?.comicId))

function progressSnapshot(): ReadingProgress | undefined {
  const chapter = chapterData.value
  if (!chapter || loading.value) return
  return { comicId: chapter.comicId, chapterId: chapter.chapterId, comicTitle: chapter.comicTitle,
    chapterTitle: chapter.chapterTitle, page: currentPage.value, totalPages: chapter.totalImages, updatedAt: Date.now() }
}

function saveProgress() {
  const progress = progressSnapshot()
  if (!progress) return
  // 每次翻页都写会做「排序 1000 条 + 全量 JSON.stringify + 同步 localStorage」，
  // 长阅读时会拖慢翻页。这里防抖 300ms，并在关闭阅读器/切章时立即落盘。
  pendingProgress = progress
  if (progressSaveTimer !== undefined) clearTimeout(progressSaveTimer)
  progressSaveTimer = setTimeout(flushProgress, 300)
}

/** 立即写入尚未落盘的阅读进度（防抖窗口内的最后一次改动）。 */
function flushProgress() {
  if (progressSaveTimer !== undefined) {
    clearTimeout(progressSaveTimer)
    progressSaveTimer = undefined
  }
  if (pendingProgress) {
    store.saveReadingProgress(pendingProgress)
    pendingProgress = undefined
  }
}

function toggleCurrentBookmark() {
  const progress = progressSnapshot()
  if (progress) store.toggleReadingBookmark(progress)
}

watch(currentPage, saveProgress)

/**
 * 页面图片地址：直接交给浏览器按 `jmimg://` 协议加载，由原生/后端返回原始字节。
 *
 * 之前是先 base64 传输再转 Blob URL，一张 2MB 的图在峰值时会同时占用
 * 字符串、二进制和 Blob 三份内存；协议直取只保留浏览器解码后的位图。
 */
function imageSrc(filePath: string): string {
  return imageUrlFor(filePath, imageRetryToken.value.get(filePath) ?? 0)
}

/** 读取失败后点「重新读取」时递增，让 `<img>` 换一个地址重新发起请求。 */
const imageRetryToken = ref<Map<string, number>>(new Map())

function markImageError(filePath: string) {
  if (!chapterData.value?.imagePaths.includes(filePath)) return
  imageErrors.value.set(filePath, '图片无法解码，文件可能已损坏')
}

function retryImage(filePath: string) {
  imageErrors.value.delete(filePath)
  imageRetryToken.value.set(filePath, (imageRetryToken.value.get(filePath) ?? 0) + 1)
}

/** 相邻页由浏览器在空闲时预取，翻页时通常已经在缓存里。 */
function preloadCurrentRange() {
  const chapter = chapterData.value
  if (!chapter) return
  const paths = chapter.imagePaths
  const idx = currentPage.value - 1
  const start = Math.max(0, idx - 1)
  const end = Math.min(paths.length, idx + (isAndroid ? 2 : 3))
  for (let i = start; i < end; i++) {
    const image = new Image()
    image.src = imageSrc(paths[i])
  }
}

async function loadChapter(comicId: number, chapterId: number, page = 1) {
  flushProgress()
  const generation = ++chapterGeneration
  loading.value = true
  error.value = null
  currentPage.value = 1
  imageErrors.value.clear()
  imageRetryToken.value.clear()
  chapterData.value = null

  try {
    const res = await commands.getChapterReaderData(comicId, chapterId)
    if (generation !== chapterGeneration) return
    if (res.status === 'ok') {
      chapterData.value = res.data
      currentPage.value = Math.max(1, Math.min(page, res.data.totalImages))
      if (res.data.imagePaths.length === 0) {
        // 空章节给出明确提示，避免阅读区一片空白且无任何说明
        error.value = '本章没有可显示的图片，文件可能不完整'
      } else {
        preloadCurrentRange()
      }
    } else {
      error.value = `${res.error.err_title}：${res.error.err_message}`
    }
  } catch (err) {
    if (generation === chapterGeneration) error.value = `加载章节数据失败：${getErrorMessage(err)}`
  } finally {
    if (generation === chapterGeneration) {
      loading.value = false
      await nextTick()
      if (generation === chapterGeneration) {
        saveProgress()
        // 滚动模式需要等所有页节点渲染完再建立观察器与定位
        if (readingMode.value === 'Scroll') {
          observeScrollPages()
          scrollToPage(currentPage.value)
        }
      }
    }
  }
}

function goToPage(page: number) {
  if (!chapterData.value) return
  currentPage.value = Math.max(1, Math.min(page, chapterData.value.totalImages))
  preloadCurrentRange()
  // 滚动模式下页码滑块/书签跳转要真的把对应页滚进视野
  if (readingMode.value === 'Scroll') scrollToPage(currentPage.value)
}

function onSliderUpdate(page: number) {
  goToPage(page)
}

/// 换页或换章都必须复位缩放，否则新页面会带着上一页的放大位置
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

/// 双击：未放大则放大到 2x 并把触点居中，已放大则复位
function onViewerDoubleClick(event: MouseEvent) {
  const rect = (event.currentTarget as HTMLElement).getBoundingClientRect()
  zoom.value = toggleZoom(
    zoom.value,
    { x: event.clientX - rect.left, y: event.clientY - rect.top },
    { width: rect.width, height: rect.height },
  )
}

watch(currentPage, resetZoom)

function prevPage() {
  if (currentPage.value > 1) {
    goToPage(currentPage.value - 1)
  } else if (chapterData.value?.prevChapter) {
    goToChapter(chapterData.value.prevChapter)
  }
}

function nextPage() {
  if (!chapterData.value) return
  if (currentPage.value + 1 <= chapterData.value.totalImages) {
    goToPage(currentPage.value + 1)
  } else if (chapterData.value.nextChapter) {
    goToChapter(chapterData.value.nextChapter)
  }
}

function handlePrevPageBtn() {
  if (!chapterData.value) return
  if (currentPage.value > 1) {
    prevPage()
  } else if (chapterData.value.prevChapter) {
    goToChapter(chapterData.value.prevChapter)
    message.info(`已切换至上一章：${chapterData.value.prevChapter.chapterTitle}`)
  } else {
    message.info('已经是第一章第一页了')
  }
}

function handleNextPageBtn() {
  if (!chapterData.value) return
  if (currentPage.value < chapterData.value.totalImages) {
    nextPage()
  } else if (chapterData.value.nextChapter) {
    goToChapter(chapterData.value.nextChapter)
    message.info(`已切换至下一章：${chapterData.value.nextChapter.chapterTitle}`)
  } else {
    message.info('已经是最后一页了')
  }
}

function handlePrevChapterBtn() {
  if (!chapterData.value) return
  if (chapterData.value.prevChapter) {
    goToChapter(chapterData.value.prevChapter)
  } else {
    message.info('已经是第一章了')
  }
}

function handleNextChapterBtn() {
  if (!chapterData.value) return
  if (chapterData.value.nextChapter) {
    goToChapter(chapterData.value.nextChapter)
  } else {
    message.info('已经是最后一章了')
  }
}

// 触摸手势控制（左右滑动翻页）
let touchStartX = 0
let touchStartY = 0
let touchStartTime = 0
let isTouching = false
let isHorizontalSwiping = false
let justSwiped = false
const swipeOffsetX = ref(0)
const isSwipingAnim = ref(false)

let pinchStartDistance = 0
let pinchStartZoom: Transform = { ...IDENTITY_TRANSFORM }
let pinchAnchor = { x: 0, y: 0 }

/// 两指间距，用于计算缩放比例
function touchDistance(touches: TouchList): number {
  const [a, b] = [touches[0], touches[1]]
  return Math.hypot(a.clientX - b.clientX, a.clientY - b.clientY)
}

function onTouchStart(e: TouchEvent) {
  if (e.touches.length === 2) {
    // 双指：进入缩放模式，取消单指滑动状态
    isTouching = false
    isHorizontalSwiping = false
    pinchStartDistance = touchDistance(e.touches)
    pinchStartZoom = { ...zoom.value }
    const rect = (e.currentTarget as HTMLElement).getBoundingClientRect()
    pinchAnchor = {
      x: (e.touches[0].clientX + e.touches[1].clientX) / 2 - rect.left,
      y: (e.touches[0].clientY + e.touches[1].clientY) / 2 - rect.top,
    }
    return
  }
  if (e.touches.length !== 1) {
    isTouching = false
    isHorizontalSwiping = false
    swipeOffsetX.value = 0
    return
  }
  const t = e.touches[0]
  touchStartX = t.clientX
  touchStartY = t.clientY
  touchStartTime = Date.now()
  isTouching = true
  isHorizontalSwiping = false
  isSwipingAnim.value = false
  swipeOffsetX.value = 0
}

function onTouchMove(e: TouchEvent) {
  if (e.touches.length === 2 && pinchStartDistance > 0) {
    const ratio = touchDistance(e.touches) / pinchStartDistance
    zoom.value = pinchTransform(pinchStartZoom, pinchAnchor, ratio, containerSize())
    return
  }
  if (!isTouching || e.touches.length !== 1) return
  const t = e.touches[0]
  const dx = t.clientX - touchStartX
  const dy = t.clientY - touchStartY
  const absDx = Math.abs(dx)
  const absDy = Math.abs(dy)

  if (!isHorizontalSwiping) {
    if (absDx > 10 && absDx > absDy * 1.2) {
      isHorizontalSwiping = true
    } else if (absDy > 10) {
      isTouching = false // 纵向滚动，取消横向手势
      return
    }
  }

  if (isHorizontalSwiping) {
    swipeOffsetX.value = dx * 0.45
  }
}

function onTouchEnd(e: TouchEvent) {
  if (!isTouching) {
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
  // 放大状态下横向拖动用于平移查看，不触发翻页
  if (zoom.value.scale > 1.01) {
    resetSwipeState()
    return
  }
  if (isHorizontalSwiping || (absDx > 40 && absDx > absDy * 1.2 && dt < 600)) {
    justSwiped = true
    if (justSwipedResetTimer !== undefined) clearTimeout(justSwipedResetTimer)
    justSwipedResetTimer = setTimeout(() => {
      justSwiped = false
      justSwipedResetTimer = undefined
    }, 250)

    if (dx < -40 || velocity < -0.3) {
      runPageAction(pageActionForHorizontalGesture(readingDirection.value, 'left'))
    } else if (dx > 40 || velocity > 0.3) {
      runPageAction(pageActionForHorizontalGesture(readingDirection.value, 'right'))
    }
  }

  resetSwipeState()
}

function onTouchCancel() {
  pinchStartDistance = 0
  resetSwipeState()
}

let justSwipedResetTimer: ReturnType<typeof setTimeout> | undefined
let swipeResetTimer: ReturnType<typeof setTimeout> | undefined

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

function handleViewerBodyClick() {
  if (justSwiped) return
  toggleHud()
}

/**
 * 滚动模式的轻点判定。`click` 在手指轻微移动后照样会派发，直接用它切 HUD
 * 会频繁误触，所以触摸设备上按触点位移判定，合成 click 一律忽略。
 */
let scrollTouchStart: { x: number; y: number; time: number } | null = null
let lastScrollTouchTime = 0

function onScrollTouchStart(e: TouchEvent) {
  lastScrollTouchTime = Date.now()
  // 用户接管滚动：停止程序化定位的校正，不跟手指抢滚动条
  cancelScrollSettle()
  // 落在按钮上的触摸交给按钮自己处理（如「重新读取」）：click 上的 .stop 拦不住
  // touch 事件，这里必须主动排除，否则点按钮会顺带把 HUD 也切了。
  if (e.touches.length !== 1 || (e.target instanceof Element && e.target.closest('button, a, input, select, textarea, [role="button"]'))) {
    scrollTouchStart = null
    return
  }
  const touch = e.touches[0]
  scrollTouchStart = { x: touch.clientX, y: touch.clientY, time: Date.now() }
}

function onScrollTouchEnd(e: TouchEvent) {
  // 合成 click 紧跟在 touchend 之后，用结束时刻计时才能覆盖长时间的拖动
  lastScrollTouchTime = Date.now()
  const start = scrollTouchStart
  scrollTouchStart = null
  const touch = e.changedTouches[0]
  if (!start || !touch) return
  if (isTapGesture(start, { x: touch.clientX, y: touch.clientY, time: Date.now() })) toggleHud()
}

function onScrollTouchCancel() {
  lastScrollTouchTime = Date.now()
  scrollTouchStart = null
}

function handleScrollViewerClick() {
  // 触摸设备会紧跟 touchend 派发合成 click；HUD 已在 onScrollTouchEnd 里判定过
  if (Date.now() - lastScrollTouchTime < 700) return
  toggleHud()
}

function onScrollWheel() {
  cancelScrollSettle()
}

function goToChapter(nav: ChapterNavInfo) {
  if (!chapterData.value) return
  store.openReader(chapterData.value.comicId, nav.chapterId, chapterData.value.comicTitle)
}

/// 阅读方向与翻页决策集中在 utils/readerNav.ts，这里只做状态绑定。
const readingDirection = computed<ReadingDirection>(() => store.config?.readingDirection ?? 'RightToLeft')

let keepScreenOnWatch: (() => void) | undefined
let unsubscribeVolumeKeys: (() => void) | undefined

function runPageAction(action: PageAction): void {
  // 滚动模式下「上一页/下一页」是滚动一屏，而不是切换单页显示
  if (readingMode.value === 'Scroll') {
    scrollByViewport(action === 'next' ? 1 : -1)
    return
  }
  if (action === 'next') nextPage()
  else prevPage()
}

/** 滚动模式：按容器高度的 85% 滚动，留一点重叠便于接续阅读 */
function scrollByViewport(direction: 1 | -1) {
  const container = scrollViewer.value
  if (!container) return
  // 键盘/音量键翻屏属于用户主动接管，先停掉定位校正
  cancelScrollSettle()
  const atTop = container.scrollTop <= 0
  const atBottom = container.scrollTop + container.clientHeight >= container.scrollHeight - 2
  // 已经到底还继续「下一页」时进入下一章，与单页模式的行为一致
  if (direction === 1 && atBottom && chapterData.value?.nextChapter) {
    goToChapter(chapterData.value.nextChapter)
    return
  }
  if (direction === -1 && atTop && chapterData.value?.prevChapter) {
    goToChapter(chapterData.value.prevChapter)
    return
  }
  container.scrollBy({ top: direction * container.clientHeight * 0.85, behavior: 'smooth' })
}

function tapLeft(): void {
  runPageAction(pageActionForTapZone('left'))
}

function tapRight(): void {
  runPageAction(pageActionForTapZone('right'))
}

function handleKeyDown(e: KeyboardEvent) {
  if (e.key === 'Escape') {
    store.closeReader()
    return
  }
  const action = pageActionForKey(e.key)
  if (!action) return
  // 空格/PageDown/PageUp 会滚动页面，必须阻止默认行为
  if (e.key === ' ' || e.key === 'PageDown' || e.key === 'PageUp') e.preventDefault()
  runPageAction(action)
}

watch(
  () => store.activeReaderTarget,
  (target) => {
    if (target) {
      loadChapter(target.comicId, target.chapterId, target.page ?? 1)
    } else {
      flushProgress()
      chapterGeneration++
      imageErrors.value.clear()
      imageRetryToken.value.clear()
      chapterData.value = null
    }
  },
  { immediate: true },
)

onMounted(() => {
  window.addEventListener('keydown', handleKeyDown)
  // 阅读器打开即保持常亮（可在设置关闭）；音量键仅 Android 生效
  keepScreenOnWatch = watch(
    () => [store.config?.keepScreenOn ?? true, store.activeReaderTarget !== undefined] as const,
    ([keepScreenOn, reading]) => setKeepScreenOn(keepScreenOn && reading),
    { immediate: true },
  )
  // 阅读器一打开就进入沉浸式；退出时必须恢复系统栏，否则状态栏会一直消失
  applyFullscreen()
  setVolumeKeyPaging(store.config?.volumeKeyPaging ?? false)
  unsubscribeVolumeKeys = onVolumeKeyPage((action) => runPageAction(action))
})

onUnmounted(() => {
  flushProgress()
  keepScreenOnWatch?.()
  keepScreenOnWatch = undefined
  setKeepScreenOn(false)
  // 退出阅读器恢复系统栏
  setImmersiveMode(false)
  scrollObserver?.disconnect()
  scrollObserver = undefined
  finishScrollSettle(false)
  setVolumeKeyPaging(false)
  unsubscribeVolumeKeys?.()
  unsubscribeVolumeKeys = undefined
  chapterGeneration++
  // 触摸/手势计时器可能在组件销毁后仍触发，先清掉
  if (swipeResetTimer !== undefined) {
    clearTimeout(swipeResetTimer)
    swipeResetTimer = undefined
  }
  if (justSwipedResetTimer !== undefined) {
    clearTimeout(justSwipedResetTimer)
    justSwipedResetTimer = undefined
  }
  window.removeEventListener('keydown', handleKeyDown)
  imageErrors.value.clear()
  imageRetryToken.value.clear()
  chapterData.value = null
})
</script>

<template>
  <div class="reader-backdrop" @click="store.closeReader">
    <div
      class="reader-window maximized is-mobile"
      @click.stop>
      <!-- Window Header -->
      <header
        v-show="hudVisible"
        class="reader-header mobile-header">
        <div class="header-title no-drag">
          <span class="comic-name" :title="chapterData?.comicTitle || store.activeReaderTarget?.comicTitle">
            {{ chapterData?.comicTitle || store.activeReaderTarget?.comicTitle || '漫画阅读器' }}
          </span>
          <span class="chapter-name" v-if="chapterData" :title="chapterData.chapterTitle">
            · {{ chapterData.chapterTitle }}
          </span>
        </div>

        <div class="header-actions no-drag">
          <button type="button" :class="['action-btn', { active: currentIsBookmarked }]"
            :title="currentIsBookmarked ? '移除当前页书签' : '添加当前页书签'" @click="toggleCurrentBookmark">
            <PhBookmarkSimple :size="18" :weight="currentIsBookmarked ? 'fill' : 'regular'" />
          </button>
          <button type="button" :class="['action-btn', { active: bookmarksShowing }]"
            title="查看本书书签" @click="bookmarksShowing = !bookmarksShowing"><PhListBullets :size="18" /></button>

          <button
            v-if="readingMode === 'Paginated'"
            type="button"
            :class="['action-btn', { active: fitMode !== 'width' }]"
            :title="`适配：${FIT_MODE_LABELS[fitMode]}（点击切换）`"
            @click="cycleFitMode">
            <PhArrowsHorizontal :size="18" />
          </button>

          <button
            type="button"
            :class="['action-btn', { active: readingMode === 'Scroll' }]"
            :title="readingMode === 'Scroll' ? '切换到单页翻页' : '切换到上下滚动'"
            @click="toggleReadingMode">
            <PhArrowsVertical :size="18" />
          </button>

          <button
            type="button"
            :class="['action-btn', { active: fullscreen }]"
            :title="fullscreen ? '退出全屏' : '全屏阅读'"
            @click="toggleFullscreen">
            <PhCornersOut :size="18" />
          </button>

          <button
            type="button"
            class="action-btn close-btn"
            title="关闭 (Esc)"
            @click="store.closeReader">
            <PhX :size="18" />
          </button>
        </div>
      </header>

      <!-- Viewer Body -->
      <div v-if="store.readingPersistenceError" class="reading-notice" role="alert">{{ store.readingPersistenceError }}</div>
      <aside v-if="bookmarksShowing" class="reader-bookmarks">
        <div class="bookmark-heading">本书书签 <span>{{ comicBookmarks.length }} 个</span></div>
        <p v-if="!comicBookmarks.length">点击顶部书签按钮保存当前页。</p>
        <div v-for="bookmark in comicBookmarks" :key="bookmark.id" class="bookmark-row">
          <button @click="store.openReader(bookmark.comicId, bookmark.chapterId, bookmark.comicTitle, bookmark.page)">
            {{ bookmark.chapterTitle }} · 第 {{ bookmark.page }} 页
          </button>
          <button title="删除书签" @click="store.removeReadingBookmark(bookmark.id)"><PhX :size="14" /></button>
        </div>
      </aside>
      <main class="reader-body">
        <div v-if="loading" class="reader-loading">
          <n-spin size="large" />
          <span class="mt-3 text-gray-400">正在载入章节内容...</span>
        </div>

        <div v-else-if="error" class="reader-error">
          <p class="text-red-400 mb-3">{{ error }}</p>
          <n-button
            size="small"
            type="primary"
            @click="store.activeReaderTarget && loadChapter(store.activeReaderTarget.comicId, store.activeReaderTarget.chapterId)">
            重试
          </n-button>
        </div>

        <template v-else-if="chapterData">
          <!-- 上下滚动模式：整章纵向排列，一次能看多页 -->
          <div
            v-if="readingMode === 'Scroll'"
            ref="scrollViewer"
            class="scroll-viewer"
            @touchstart.passive="onScrollTouchStart"
            @touchend="onScrollTouchEnd"
            @touchcancel="onScrollTouchCancel"
            @wheel.passive="onScrollWheel"
            @click="handleScrollViewerClick">
            <div
              v-for="(path, index) in chapterData.imagePaths"
              :key="path"
              :data-page="index + 1"
              class="scroll-page">
              <div v-if="imageErrors.has(path)" class="image-skeleton image-error">
                <span>第 {{ index + 1 }} 页读取失败</span>
                <small>{{ imageErrors.get(path) }}</small>
                <n-button size="small" @click.stop="retryImage(path)">重新读取</n-button>
              </div>
              <img
                v-else
                :src="imageSrc(path)"
                :alt="`第 ${index + 1} 页`"
                class="reader-img"
                loading="lazy"
                @error="markImageError(path)"
                :draggable="false" />
            </div>
          </div>

          <!-- 单页翻页模式（滑动 / 点击区域 / 键盘） -->
          <div
            v-else
            class="paginated-viewer"
            ref="viewerBody"
            @touchstart="onTouchStart"
            @touchmove="onTouchMove"
            @touchend="onTouchEnd"
            @touchcancel="onTouchCancel">
            <div class="click-zone click-prev" @click="!justSwiped && tapLeft()" title="上一页 (← / A)">
              <PhCaretLeft :size="38" class="zone-icon" />
            </div>

            <div
              class="paginated-content"
              :class="{ animating: isSwipingAnim }"
              :style="swipeOffsetX ? { transform: `translateX(${swipeOffsetX}px)` } : undefined"
              @click="handleViewerBodyClick">
              <div
                class="page-frame"
                :class="`fit-${fitMode}`"
                :style="zoom.scale !== 1
                  ? { transform: `translate(${zoom.offsetX}px, ${zoom.offsetY}px) scale(${zoom.scale})` }
                  : undefined"
                @dblclick.stop="onViewerDoubleClick">
                <div v-if="currentImagePath && imageErrors.has(currentImagePath)" class="image-skeleton image-error">
                  <span>第 {{ currentPage }} 页读取失败</span>
                  <small>{{ imageErrors.get(currentImagePath) }}</small>
                  <n-button size="small" @click.stop="retryImage(currentImagePath)">重新读取</n-button>
                </div>
                <img
                  v-else-if="currentImagePath"
                  :key="imageSrc(currentImagePath)"
                  :src="imageSrc(currentImagePath)"
                  :alt="`第 ${currentPage} 页`"
                  class="reader-img"
                  @error="markImageError(currentImagePath)"
                  :draggable="false" />
                <div v-else class="image-skeleton">
                  <span>第 {{ currentPage }} 页 正在加载...</span>
                </div>
              </div>
            </div>

            <div class="click-zone click-next" @click="!justSwiped && tapRight()" title="下一页 (→ / D / 空格)">
              <PhCaretRight :size="38" class="zone-icon" />
            </div>
          </div>
        </template>
      </main>

      <!-- Window Footer -->
      <footer
        v-show="hudVisible"
        class="reader-footer mobile-footer">
        <div class="footer-controls">
          <button
            type="button"
            class="nav-btn chapter-btn"
            :class="{ disabled: !chapterData?.prevChapter }"
            title="上一章"
            @click="handlePrevChapterBtn">
            <PhCaretDoubleLeft :size="16" />
            <span>上一章</span>
          </button>

          <button
            type="button"
            class="nav-btn page-arrow-btn"
            title="上一页 (← / A)"
            @click="handlePrevPageBtn">
            <PhCaretLeft :size="18" />
          </button>

          <div class="page-slider-area" v-if="chapterData && chapterData.totalImages > 0">
            <n-slider
              v-model:value="currentPage"
              :min="1"
              :max="chapterData.totalImages"
              :step="1"
              :tooltip="true"
              :format-tooltip="(val: number) => `第 ${val} / ${chapterData?.totalImages || 0} 页`"
              class="page-slider"
              @update-value="onSliderUpdate" />
            <span class="page-indicator">
              <strong>{{ currentPage }}</strong> / {{ chapterData.totalImages }}
            </span>
          </div>

          <button
            type="button"
            class="nav-btn page-arrow-btn"
            title="下一页 (→ / D)"
            @click="handleNextPageBtn">
            <PhCaretRight :size="18" />
          </button>

          <button
            type="button"
            class="nav-btn chapter-btn"
            :class="{ disabled: !chapterData?.nextChapter }"
            title="下一章"
            @click="handleNextChapterBtn">
            <span>下一章</span>
            <PhCaretDoubleRight :size="16" />
          </button>
        </div>
      </footer>
    </div>
  </div>
</template>

<style scoped>
.reader-bookmarks { position: absolute; top: 60px; right: 18px; width: min(320px, 85vw); max-height: 50vh; overflow: auto; padding: 18px; background: #252934; border: 1px solid #3c4250; border-radius: 14px; z-index: 20; box-shadow: 0 16px 40px #0006; }
.bookmark-heading { display: flex; justify-content: space-between; font-weight: 700; }
.bookmark-heading span, .reader-bookmarks p { color: #a9afbf; font-size: 12px; }
.bookmark-row { display: flex; gap: 8px; margin-top: 12px; }
.bookmark-row button { color: #e4e6ed; border: 0; background: #303645; border-radius: 8px; padding: 10px; cursor: pointer; }
.bookmark-row button:first-child { flex: 1; text-align: left; }
.reading-notice { color: #f4c391; background: #493921; padding: 8px 20px; font-size: 12px; }
.reader-backdrop {
  position: fixed;
  inset: 0;
  z-index: 9999;
  background-color: rgba(0, 0, 0, 0.55);
  backdrop-filter: blur(2px);
}

.reader-window {
  position: fixed;
  display: flex;
  flex-direction: column;
  background-color: #18191C;
  color: #ebebeb;
  border-radius: 12px;
  box-shadow: 0 20px 60px rgba(0, 0, 0, 0.7), 0 0 0 1px rgba(255, 255, 255, 0.12);
  overflow: hidden;
  user-select: none;
}

.reader-window.maximized {
  top: 0 !important;
  left: 0 !important;
  width: 100vw !important;
  height: 100vh !important;
  border-radius: 0;
  box-shadow: none;
}

/* Header */
.reader-header {
  height: 48px;
  flex: 0 0 48px;
  background-color: #18191C;
  border-bottom: 1px solid rgba(255, 255, 255, 0.08);
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 0 14px;
  cursor: move;
}

.header-title {
  display: flex;
  align-items: baseline;
  gap: 6px;
  font-size: 14px;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  max-width: 60%;
  min-width: 0;
}

.comic-name {
  font-weight: 600;
  color: #fff;
  overflow: hidden;
  text-overflow: ellipsis;
  min-width: 0;
}

.chapter-name {
  color: #9499A0;
  font-size: 13px;
  overflow: hidden;
  text-overflow: ellipsis;
  min-width: 0;
}

.header-actions {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-shrink: 0;
}

.action-btn {
  background: transparent;
  border: none;
  color: #9499A0;
  padding: 6px;
  border-radius: 6px;
  cursor: pointer;
  display: flex;
  align-items: center;
  justify-content: center;
  transition: all 120ms ease;
}

.action-btn:hover {
  color: #fff;
  background-color: rgba(255, 255, 255, 0.1);
}

.action-btn.active {
  color: #FB7299;
}

.action-btn.close-btn:hover {
  color: #fff;
  background-color: #e53935;
}

/* Body */
.reader-body {
  flex: 1 1 0;
  min-height: 0;
  position: relative;
  overflow: hidden;
  display: flex;
  align-items: center;
  justify-content: center;
  background-color: #18191C;
}

.reader-loading,
.reader-error {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  height: 100%;
}

/* Image states */
.reader-img {
  max-width: 100%;
  height: auto;
  object-fit: contain;
  box-shadow: 0 4px 16px rgba(0, 0, 0, 0.4);
}

.image-skeleton {
  width: 100%;
  max-width: 800px;
  height: 480px;
  background-color: #222427;
  border-radius: 6px;
  display: flex;
  align-items: center;
  justify-content: center;
  color: #61666D;
  font-size: 13px;
}

.image-error {
  flex-direction: column;
  gap: 12px;
  padding: 24px;
  box-sizing: border-box;
  color: #f0b5bb;
  text-align: center;
}

.image-error small {
  max-width: 100%;
  overflow-wrap: anywhere;
}

/* 上下滚动模式 */
.scroll-viewer {
  width: 100%;
  height: 100%;
  overflow-y: auto;
  overflow-x: hidden;
  background-color: #18191C;
  /* 惯性滚动与滚动链交给浏览器，避免整页跟着弹动 */
  overscroll-behavior: contain;
  -webkit-overflow-scrolling: touch;
}

.scroll-page {
  display: flex;
  align-items: center;
  justify-content: center;
  /* 页与页之间不留缝：日漫跨页图需要连续排列 */
  margin: 0;
  padding: 0;
}

/*
 * 滚动模式的图片宽度：始终铺满容器宽度，高度按原始比例。
 * 漫画页通常窄于屏幕（尤其是条漫），若按 contain 处理会左右留黑边、
 * 字变小；按宽度铺满才是滚动阅读的正确排版。
 */
.scroll-page .reader-img {
  display: block;
  width: 100%;
  max-width: 100%;
  height: auto;
  object-fit: contain;
  box-shadow: none;
}

.scroll-page .image-skeleton {
  width: 100%;
  max-width: none;
}

/* Paginated Viewer */
.paginated-viewer {
  width: 100%;
  height: 100%;
  display: flex;
  position: relative;
  align-items: center;
  justify-content: center;
}

.click-zone {
  position: absolute;
  top: 0;
  bottom: 0;
  width: 20%;
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 20;
  cursor: pointer;
  opacity: 0;
  transition: opacity 150ms ease;
}

.click-zone:hover {
  opacity: 1;
}

.click-prev {
  left: 0;
  background: linear-gradient(to right, rgba(0, 0, 0, 0.35), transparent);
}

.click-next {
  right: 0;
  background: linear-gradient(to left, rgba(0, 0, 0, 0.35), transparent);
}

.zone-icon {
  color: rgba(255, 255, 255, 0.85);
  background-color: rgba(0, 0, 0, 0.45);
  border-radius: 50%;
  padding: 6px;
}

.paginated-content {
  width: 100%;
  height: 100%;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 10px;
  gap: 8px;
  touch-action: pan-y;
  user-select: none;
}

.paginated-content.animating {
  transition: transform 220ms cubic-bezier(0.2, 0.8, 0.2, 1);
}

.page-frame {
  height: 100%;
  display: flex;
  align-items: center;
  justify-content: center;
}

.page-frame {
  transition: transform 140ms ease-out;
  will-change: transform;
}

.page-frame .reader-img {
  max-height: 100%;
  max-width: 100%;
  object-fit: contain;
}

/* 四种适配模式：模板里用 `fit-${fitMode}` 绑定 */
.page-frame.fit-width .reader-img {
  width: 100%;
  max-height: 100%;
}

.page-frame.fit-height .reader-img {
  height: 100%;
  width: auto;
  max-width: none;
}

.page-frame.fit-screen .reader-img {
  width: auto;
  height: auto;
}

.page-frame.fit-original .reader-img {
  width: auto;
  height: auto;
  max-width: none;
  max-height: none;
}

/* Footer */
.reader-footer {
  flex: 0 0 54px;
  background-color: #18191C;
  border-top: 1px solid rgba(255, 255, 255, 0.08);
  display: flex;
  flex-direction: column;
  justify-content: center;
  padding: 0 16px;
  gap: 2px;
}

.footer-controls {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
}

.nav-btn {
  background: transparent;
  border: 1px solid rgba(255, 255, 255, 0.12);
  color: #9499A0;
  padding: 4px 10px;
  border-radius: 6px;
  font-size: 12px;
  display: flex;
  align-items: center;
  gap: 4px;
  cursor: pointer;
  transition: all 120ms ease;
  white-space: nowrap;
}

.nav-btn:hover:not(:disabled) {
  color: #fff;
  border-color: #FB7299;
}

.nav-btn:disabled {
  opacity: 0.35;
  cursor: not-allowed;
}

.page-arrow-btn {
  padding: 4px 8px;
  min-width: 32px;
  justify-content: center;
}

.chapter-btn.disabled {
  opacity: 0.35;
}

.page-slider-area {
  flex: 1 1 0;
  max-width: 480px;
  display: flex;
  align-items: center;
  gap: 12px;
  margin: 0 auto;
}

.page-slider {
  flex: 1 1 0;
  touch-action: none;
}

.page-slider :deep(.n-slider-rail) {
  height: 6px;
  border-radius: 3px;
  background-color: rgba(255, 255, 255, 0.16);
}

.page-slider :deep(.n-slider-rail__fill) {
  background-color: #FB7299;
}

.page-slider :deep(.n-slider-handle) {
  width: 18px;
  height: 18px;
  border: 2px solid #fff;
  background-color: #FB7299;
  box-shadow: 0 0 6px rgba(0, 0, 0, 0.5);
  transition: transform 120ms ease, box-shadow 120ms ease;
}

.page-slider :deep(.n-slider-handle:hover),
.page-slider :deep(.n-slider-handle:active) {
  transform: scale(1.2);
  box-shadow: 0 0 10px rgba(251, 114, 153, 0.8);
}

.page-indicator {
  font-size: 13px;
  color: #9499A0;
  white-space: nowrap;
  font-family: monospace;
}

.page-indicator strong {
  color: #fff;
}

/* Mobile Fullscreen & Safe Area Overrides */
@media (max-width: 768px) {
  .reader-window {
    top: 0 !important;
    left: 0 !important;
    width: 100vw !important;
    height: 100vh !important;
    border-radius: 0 !important;
    box-shadow: none !important;
  }

  .reader-header {
    height: calc(48px + env(safe-area-inset-top, 0px)) !important;
    padding-top: max(env(safe-area-inset-top, 0px), 8px) !important;
    padding-left: 12px !important;
    padding-right: 12px !important;
  }

  .reader-footer {
    height: calc(56px + env(safe-area-inset-bottom, 0px)) !important;
    padding-bottom: max(env(safe-area-inset-bottom, 0px), 8px) !important;
    padding-left: 12px !important;
    padding-right: 12px !important;
  }

  .comic-name {
    max-width: 150px !important;
    font-size: 13px !important;
  }

  .chapter-name {
    font-size: 12px !important;
    max-width: 100px !important;
  }

  .page-slider-area {
    gap: 8px !important;
  }

  .chapter-btn span {
    display: none;
  }

  .page-arrow-btn {
    min-width: 34px;
    height: 34px;
    padding: 0;
  }

  .chapter-btn {
    min-width: 34px;
    height: 34px;
    padding: 0;
    justify-content: center;
  }

  .page-slider :deep(.n-slider-handle) {
    width: 22px;
    height: 22px;
  }

  .footer-controls {
    gap: 8px;
  }
}
</style>
