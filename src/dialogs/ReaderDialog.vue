<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue'
import { commands, type ChapterReaderData, type ChapterNavInfo, type ReadingDirection } from '../bindings.ts'
import { useStore } from '../store.ts'
import { getErrorMessage } from '../utils/errors.ts'
import { ImageByteCache, imageDataToBlob } from '../utils/imageCache.ts'
import { bookmarkId, type ReadingProgress } from '../utils/readingState.ts'
import { useMessage } from 'naive-ui'
import {
  PhX,
  PhCornersOut,
  PhCornersIn,
  PhCaretLeft,
  PhCaretRight,
  PhCaretDoubleLeft,
  PhCaretDoubleRight,
  PhArrowsHorizontal,
  PhBookmarkSimple,
  PhListBullets,
} from '@phosphor-icons/vue'
import { useBackHandler } from '../composables/useAndroidBack.ts'
import { useIsMobile } from '../composables/useIsMobile.ts'
import {
  onVolumeKeyPage,
  setKeepScreenOn,
  setVolumeKeyPaging,
} from '../utils/androidStorage.ts'
import {
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

const { isMobile, isAndroid } = useIsMobile()
const hudVisible = ref(true)

function toggleHud() {
  hudVisible.value = !hudVisible.value
}

useBackHandler(() => {
  store.closeReader()
  return true
}, 100)

interface WinRect {
  x: number
  y: number
  w: number
  h: number
}

const STORAGE_RECT_KEY = 'joymanga_reader_win_rect'
const isMaximized = ref(false)
const isResizing = ref(false)
const isDragging = ref(false)
const effectiveMaximized = computed(() => isMobile.value || isMaximized.value)

const winRect = ref<WinRect>({
  x: 50,
  y: 50,
  w: 1000,
  h: 750,
})

function loadWinRect() {
  try {
    const raw = localStorage.getItem(STORAGE_RECT_KEY)
    if (raw) {
      const parsed = JSON.parse(raw)
      if (parsed && typeof parsed.w === 'number' && typeof parsed.h === 'number') {
        const w = Math.max(500, Math.min(parsed.w, window.innerWidth))
        const h = Math.max(400, Math.min(parsed.h, window.innerHeight))
        const x = Math.max(0, Math.min(parsed.x ?? 50, window.innerWidth - w))
        const y = Math.max(0, Math.min(parsed.y ?? 50, window.innerHeight - h))
        winRect.value = { x, y, w, h }
        return
      }
    }
  } catch (e) {
    console.error('Failed to load window rect', e)
  }
  const w = Math.min(1100, Math.max(700, window.innerWidth * 0.88))
  const h = Math.min(850, Math.max(500, window.innerHeight * 0.88))
  winRect.value = {
    x: Math.max(0, (window.innerWidth - w) / 2),
    y: Math.max(0, (window.innerHeight - h) / 2),
    w,
    h,
  }
}

function saveWinRect() {
  try {
    localStorage.setItem(STORAGE_RECT_KEY, JSON.stringify(winRect.value))
  } catch (e) {
    console.error('Failed to save window rect', e)
  }
}

type Direction = 'n' | 's' | 'e' | 'w' | 'ne' | 'nw' | 'se' | 'sw'

function startResize(e: MouseEvent, dir: Direction) {
  if (isMaximized.value) return
  e.preventDefault()
  e.stopPropagation()

  isResizing.value = true
  const startX = e.clientX
  const startY = e.clientY
  const { x: initX, y: initY, w: initW, h: initH } = winRect.value
  const minW = 500
  const minH = 400

  const onMouseMove = (moveEvent: MouseEvent) => {
    const dx = moveEvent.clientX - startX
    const dy = moveEvent.clientY - startY
    let newX = initX
    let newY = initY
    let newW = initW
    let newH = initH

    if (dir.includes('e')) {
      newW = Math.max(minW, Math.min(initW + dx, window.innerWidth - initX))
    }
    if (dir.includes('s')) {
      newH = Math.max(minH, Math.min(initH + dy, window.innerHeight - initY))
    }
    if (dir.includes('w')) {
      const candidateW = initW - dx
      if (candidateW >= minW) {
        newW = candidateW
        newX = initX + dx
      } else {
        newW = minW
        newX = initX + (initW - minW)
      }
    }
    if (dir.includes('n')) {
      const candidateH = initH - dy
      if (candidateH >= minH) {
        newH = candidateH
        newY = initY + dy
      } else {
        newH = minH
        newY = initY + (initH - minH)
      }
    }

    winRect.value = { x: newX, y: newY, w: newW, h: newH }
  }

  const onMouseUp = () => {
    isResizing.value = false
    window.removeEventListener('mousemove', onMouseMove)
    window.removeEventListener('mouseup', onMouseUp)
    saveWinRect()
  }

  window.addEventListener('mousemove', onMouseMove)
  window.addEventListener('mouseup', onMouseUp)
}

function startHeaderDrag(e: MouseEvent) {
  if (isMaximized.value) return
  if ((e.target as HTMLElement).closest('button') || (e.target as HTMLElement).closest('.no-drag')) {
    return
  }
  e.preventDefault()
  isDragging.value = true

  const startX = e.clientX
  const startY = e.clientY
  const { x: initX, y: initY, w, h } = winRect.value

  const onMouseMove = (moveEvent: MouseEvent) => {
    const dx = moveEvent.clientX - startX
    const dy = moveEvent.clientY - startY
    const newX = Math.max(0, Math.min(initX + dx, window.innerWidth - 100))
    const newY = Math.max(0, Math.min(initY + dy, window.innerHeight - 60))
    winRect.value = { x: newX, y: newY, w, h }
  }

  const onMouseUp = () => {
    isDragging.value = false
    window.removeEventListener('mousemove', onMouseMove)
    window.removeEventListener('mouseup', onMouseUp)
    saveWinRect()
  }

  window.addEventListener('mousemove', onMouseMove)
  window.addEventListener('mouseup', onMouseUp)
}

function toggleMaximize() {
  isMaximized.value = !isMaximized.value
}

const loading = ref(false)
const error = ref<string | null>(null)
const chapterData = ref<ChapterReaderData | null>(null)
const imageCache = ref<Map<string, string>>(new Map())
const byteCache = new ImageByteCache(isAndroid ? 64 * 1024 * 1024 : 128 * 1024 * 1024)
let activeImageReads = 0
const imageReadWaiters: Array<() => void> = []
async function acquireImageRead() {
  if (activeImageReads < (isAndroid ? 2 : 3)) activeImageReads++
  else await new Promise<void>((resolve) => imageReadWaiters.push(resolve))
}
function releaseImageRead() {
  const next = imageReadWaiters.shift()
  if (next) next()
  else activeImageReads--
}
function pinnedPages() {
  const paths = chapterData.value?.imagePaths ?? []
  return new Set(paths.slice(Math.max(0, currentPage.value - 2), currentPage.value + (isAndroid ? 1 : 2)))
}
const bookmarksShowing = ref(false)
const loadingImages = ref<Set<string>>(new Set())
const imageErrors = ref<Map<string, string>>(new Map())
let chapterGeneration = 0

const fitMode = ref<FitMode>('width')
const zoom = ref<Transform>({ ...IDENTITY_TRANSFORM })
const viewerBody = ref<HTMLElement | null>(null)
const currentPage = ref(1)

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
  if (progress) store.saveReadingProgress(progress)
}

function imageLoaded(path: string, event: Event) {
  const image = event.target as HTMLImageElement
  for (const evicted of byteCache.resize(path, image.naturalWidth * image.naturalHeight * 4, pinnedPages())) imageCache.value.delete(evicted)
}

function toggleCurrentBookmark() {
  const progress = progressSnapshot()
  if (progress) store.toggleReadingBookmark(progress)
}

watch(currentPage, saveProgress)

async function fetchImage(filePath: string, retry = false) {
  byteCache.touch(filePath)
  if (imageCache.value.has(filePath) || loadingImages.value.has(filePath)
    || (!retry && imageErrors.value.has(filePath))) {
    return
  }
  const generation = chapterGeneration
  imageErrors.value.delete(filePath)
  loadingImages.value.add(filePath)
  await acquireImageRead()
  try {
    if (generation !== chapterGeneration) return
    const res = await commands.getLocalImageDataUrl(filePath)
    if (generation !== chapterGeneration) return
    if (res.status === 'ok') {
      const blob = imageDataToBlob(res.data)
      const url = URL.createObjectURL(blob)
      const pinned = pinnedPages()
      const evicted = byteCache.put(filePath, url, blob.size, pinned)
      for (const key of evicted) imageCache.value.delete(key)
      if (!evicted.includes(filePath)) imageCache.value.set(filePath, url)
    } else {
      imageErrors.value.set(filePath, `${res.error.err_title}：${res.error.err_message}`)
    }
  } catch (e) {
    console.error(`Failed to load image ${filePath}`, e)
    if (generation === chapterGeneration) imageErrors.value.set(filePath, getErrorMessage(e))
  } finally {
    releaseImageRead()
    if (generation === chapterGeneration) loadingImages.value.delete(filePath)
  }
}

function markImageError(filePath: string) {
  if (!chapterData.value?.imagePaths.includes(filePath)) return
  byteCache.remove(filePath)
  imageCache.value.delete(filePath)
  imageErrors.value.set(filePath, '图片无法解码，文件可能已损坏')
}

function retryImage(filePath: string) {
  void fetchImage(filePath, true)
}

function preloadCurrentRange() {
  if (!chapterData.value) return
  const paths = chapterData.value.imagePaths
  const idx = currentPage.value - 1
  const start = Math.max(0, idx - 1)
  const end = Math.min(paths.length, idx + (isAndroid ? 2 : 3))
  for (let i = start; i < end; i++) {
    fetchImage(paths[i])
  }
}

async function loadChapter(comicId: number, chapterId: number, page = 1) {
  saveProgress()
  const generation = ++chapterGeneration
  loading.value = true
  error.value = null
  currentPage.value = 1
  imageCache.value.clear()
  byteCache.clear()
  loadingImages.value.clear()
  imageErrors.value.clear()
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
      if (generation === chapterGeneration) saveProgress()
    }
  }
}

function goToPage(page: number) {
  if (!chapterData.value) return
  currentPage.value = Math.max(1, Math.min(page, chapterData.value.totalImages))
  preloadCurrentRange()
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
  if (!isMobile.value) return
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
  if (isMobile.value) {
    toggleHud()
  }
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
  if (action === 'next') nextPage()
  else prevPage()
}

function tapLeft(): void {
  runPageAction(pageActionForTapZone(readingDirection.value, 'left'))
}

function tapRight(): void {
  runPageAction(pageActionForTapZone(readingDirection.value, 'right'))
}

// 桌面端滚轮翻页（累积一定位移后翻一页，避免触控板误触）
let wheelAccum = 0
let wheelResetTimer: ReturnType<typeof setTimeout> | undefined
function onWheel(e: WheelEvent) {
  if (Math.abs(e.deltaY) < 1) return
  wheelAccum += e.deltaY
  if (wheelResetTimer) clearTimeout(wheelResetTimer)
  wheelResetTimer = setTimeout(() => {
    wheelAccum = 0
  }, 240)
  const threshold = 36
  if (wheelAccum >= threshold) {
    wheelAccum = 0
    // 滚轮向下 = 往后读；方向设置只影响左右切换，不影响纵向滚轮
    nextPage()
  } else if (wheelAccum <= -threshold) {
    wheelAccum = 0
    prevPage()
  }
}

function handleKeyDown(e: KeyboardEvent) {
  if (e.key === 'Escape') {
    store.closeReader()
    return
  }
  if (e.key === 'f' || e.key === 'F') {
    toggleMaximize()
    return
  }
  const action = pageActionForKey(readingDirection.value, e.key)
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
      saveProgress()
      chapterGeneration++
      imageCache.value.clear()
      byteCache.clear()
      loadingImages.value.clear()
      imageErrors.value.clear()
      chapterData.value = null
    }
  },
  { immediate: true },
)

onMounted(() => {
  loadWinRect()
  window.addEventListener('keydown', handleKeyDown)
  // 阅读器打开即保持常亮（可在设置关闭）；音量键仅 Android 生效
  keepScreenOnWatch = watch(
    () => [store.config?.keepScreenOn ?? true, store.activeReaderTarget !== undefined] as const,
    ([keepScreenOn, reading]) => setKeepScreenOn(keepScreenOn && reading),
    { immediate: true },
  )
  setVolumeKeyPaging(store.config?.volumeKeyPaging ?? false)
  unsubscribeVolumeKeys = onVolumeKeyPage((action) => runPageAction(action))
})

onUnmounted(() => {
  saveProgress()
  keepScreenOnWatch?.()
  keepScreenOnWatch = undefined
  setKeepScreenOn(false)
  setVolumeKeyPaging(false)
  unsubscribeVolumeKeys?.()
  unsubscribeVolumeKeys = undefined
  chapterGeneration++
  // 触摸/手势与滚轮计时器可能在组件销毁后仍触发，先清掉
  if (wheelResetTimer !== undefined) {
    clearTimeout(wheelResetTimer)
    wheelResetTimer = undefined
  }
  if (swipeResetTimer !== undefined) {
    clearTimeout(swipeResetTimer)
    swipeResetTimer = undefined
  }
  if (justSwipedResetTimer !== undefined) {
    clearTimeout(justSwipedResetTimer)
    justSwipedResetTimer = undefined
  }
  window.removeEventListener('keydown', handleKeyDown)
  imageCache.value.clear()
  byteCache.clear()
  loadingImages.value.clear()
  imageErrors.value.clear()
  chapterData.value = null
})
</script>

<template>
  <div class="reader-backdrop" @click="store.closeReader">
    <div
      class="reader-window"
      :class="{ maximized: effectiveMaximized, 'is-mobile': isMobile }"
      :style="effectiveMaximized ? undefined : {
        top: `${winRect.y}px`,
        left: `${winRect.x}px`,
        width: `${winRect.w}px`,
        height: `${winRect.h}px`,
      }"
      @click.stop>
      <!-- 8 direction resize handles -->
      <template v-if="!effectiveMaximized">
        <div class="resize-handle handle-n" @mousedown="startResize($event, 'n')"></div>
        <div class="resize-handle handle-s" @mousedown="startResize($event, 's')"></div>
        <div class="resize-handle handle-e" @mousedown="startResize($event, 'e')"></div>
        <div class="resize-handle handle-w" @mousedown="startResize($event, 'w')"></div>
        <div class="resize-handle handle-ne" @mousedown="startResize($event, 'ne')"></div>
        <div class="resize-handle handle-nw" @mousedown="startResize($event, 'nw')"></div>
        <div class="resize-handle handle-se" @mousedown="startResize($event, 'se')"></div>
        <div class="resize-handle handle-sw" @mousedown="startResize($event, 'sw')"></div>
      </template>

      <!-- Window Header -->
      <header
        v-show="!isMobile || hudVisible"
        class="reader-header"
        :class="{ 'mobile-header': isMobile }"
        @mousedown="startHeaderDrag"
        @dblclick="toggleMaximize">
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
            type="button"
            :class="['action-btn', { active: fitMode !== 'width' }]"
            :title="`适配：${FIT_MODE_LABELS[fitMode]}（点击切换）`"
            @click="cycleFitMode">
            <PhArrowsHorizontal :size="18" />
          </button>

          <button
            v-if="!isMobile"
            type="button"
            class="action-btn"
            :title="isMaximized ? '还原窗口' : '最大化窗口'"
            @click="toggleMaximize">
            <PhCornersIn v-if="isMaximized" :size="18" />
            <PhCornersOut v-else :size="18" />
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
          <!-- 单页翻页模式（滑动 / 点击区域 / 滚轮 / 键盘） -->
          <div
            class="paginated-viewer"
            ref="viewerBody"
            @wheel="onWheel"
            @touchstart="onTouchStart"
            @touchmove="onTouchMove"
            @touchend="onTouchEnd"
            @touchcancel="onTouchCancel">
            <div class="click-zone click-prev" @click="!justSwiped && tapLeft()" :title="pageActionForTapZone(readingDirection, 'left') === 'next' ? '下一页 (← / A)' : '上一页 (← / A)'">
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
                <img
                  v-if="currentImagePath && imageCache.has(currentImagePath)"
                  :src="imageCache.get(currentImagePath)"
                  :alt="`第 ${currentPage} 页`"
                  class="reader-img"
                  @load="currentImagePath && imageLoaded(currentImagePath, $event)"
                  @error="currentImagePath && markImageError(currentImagePath)"
                  :draggable="false" />
                <div v-else-if="currentImagePath && imageErrors.has(currentImagePath)" class="image-skeleton image-error">
                  <span>第 {{ currentPage }} 页读取失败</span>
                  <small>{{ imageErrors.get(currentImagePath) }}</small>
                  <n-button size="small" @click.stop="retryImage(currentImagePath)">重新读取</n-button>
                </div>
                <div v-else class="image-skeleton">
                  <span>第 {{ currentPage }} 页 正在加载...</span>
                </div>
              </div>
            </div>

            <div class="click-zone click-next" @click="!justSwiped && tapRight()" :title="pageActionForTapZone(readingDirection, 'right') === 'next' ? '下一页 (→ / D / 空格)' : '上一页 (→ / D)'">
              <PhCaretRight :size="38" class="zone-icon" />
            </div>
          </div>
        </template>
      </main>

      <!-- Window Footer -->
      <footer
        v-show="!isMobile || hudVisible"
        class="reader-footer"
        :class="{ 'mobile-footer': isMobile }">
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

        <div v-if="!isMobile" class="shortcuts-hint">
          <span>拖拽边框可自由调节窗口大小</span>
          <span class="sep">·</span>
          <span>A / D 或 ← / → 翻页</span>
          <span class="sep">·</span>
          <span>Esc 退出</span>
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

/* 8-direction resize handles */
.resize-handle {
  position: absolute;
  z-index: 100;
}
.handle-n {
  top: 0;
  left: 10px;
  right: 10px;
  height: 6px;
  cursor: ns-resize;
}
.handle-s {
  bottom: 0;
  left: 10px;
  right: 10px;
  height: 6px;
  cursor: ns-resize;
}
.handle-w {
  left: 0;
  top: 10px;
  bottom: 10px;
  width: 6px;
  cursor: ew-resize;
}
.handle-e {
  right: 0;
  top: 10px;
  bottom: 10px;
  width: 6px;
  cursor: ew-resize;
}
.handle-nw {
  top: 0;
  left: 0;
  width: 12px;
  height: 12px;
  cursor: nwse-resize;
}
.handle-ne {
  top: 0;
  right: 0;
  width: 12px;
  height: 12px;
  cursor: nesw-resize;
}
.handle-sw {
  bottom: 0;
  left: 0;
  width: 12px;
  height: 12px;
  cursor: nesw-resize;
}
.handle-se {
  bottom: 0;
  right: 0;
  width: 12px;
  height: 12px;
  cursor: nwse-resize;
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

.shortcuts-hint {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 8px;
  font-size: 11px;
  color: #61666D;
}

.shortcuts-hint .sep {
  opacity: 0.5;
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
