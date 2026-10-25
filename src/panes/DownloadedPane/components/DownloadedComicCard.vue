<script setup lang="ts">
import { type Comic, commands } from '../../../bindings.ts'
import { useStore } from '../../../store.ts'
import { PhArchive, PhCheck, PhDotsThree, PhFolderOpen, PhImage } from '@phosphor-icons/vue'
import { computed, h, onBeforeUnmount, ref } from 'vue'
import { NDropdown, useMessage } from 'naive-ui'
import { isMergeableComic, localChapters } from '../../../utils/mergeableComics.ts'
import { useIsMobile } from '../../../composables/useIsMobile.ts'
import { formatCommandError, getErrorMessage } from '../../../utils/errors.ts'

const store = useStore()
const message = useMessage()
const coverFailed = ref(false)
const progress = computed(() => store.readingState.progress[String(props.comic.id)])
const downloadedChapters = computed(() => localChapters(props.comic).length)
const percent = computed(() => {
  const value = progress.value
  if (!value || value.totalPages <= 0) return 0
  return Math.min(100, Math.max(1, Math.round((value.page / value.totalPages) * 100)))
})
const archiving = computed(() => store.isArchiving(props.comic.id))

const props = defineProps<{
  comic: Comic
  checked: boolean
  selectionMode: boolean
}>()

const emit = defineEmits<{
  'toggle-check': [comic: Comic]
  'long-press': [comic: Comic]
}>()

function pickComic() {
  if (props.comic.id < 0) {
    openReader()
    return
  }
  store.showComic(props.comic)
}

function openReader() {
  if (archiving.value) {
    message.warning('正在整合 CBZ，完成后即可阅读')
    return
  }
  store.continueReading(props.comic)
}

function onCoverClick() {
  if (props.selectionMode) {
    emit('toggle-check', props.comic)
    return
  }
  openReader()
}

// 长按进入选择：移动端没有右键，长按是进入批量操作的唯一入口。
// 按住期间一旦移动就视为滚动，取消计时，避免滑动列表时误选。
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

function startPress(event: PointerEvent) {
  if (event.button !== 0) return
  clearPress()
  longPressed = false
  pressStart = { x: event.clientX, y: event.clientY }
  pressTimer = setTimeout(() => {
    pressTimer = undefined
    longPressed = true
    if (navigator.vibrate) navigator.vibrate(12)
    emit('long-press', props.comic)
  }, 480)
}

function movePress(event: PointerEvent) {
  if (pressStart === undefined) return
  if (Math.abs(event.clientX - pressStart.x) > 10 || Math.abs(event.clientY - pressStart.y) > 10) clearPress()
}

function endPress() {
  clearPress()
}

// ── 更多菜单：桌面端没有长按习惯，键盘也无法长按；封面墙省掉了原来的整行按钮，
// 用右上角一个 ⋯ 把「打开目录 / 整理 / 选择」收进去。
const { isMobile } = useIsMobile()
const menuOpen = ref(false)
const menuX = ref(0)
const menuY = ref(0)
const merging = ref(false)

const menuOptions = computed(() => [
  { label: isMobile.value ? '复制下载目录' : '打开下载目录', key: 'dir', icon: () => h(PhFolderOpen, { size: 16 }) },
  ...(isMergeableComic(props.comic)
    ? [{ label: archiving.value ? '正在整理…' : '整理为整本 CBZ', key: 'merge', disabled: archiving.value || merging.value, icon: () => h(PhArchive, { size: 16 }) }]
    : []),
  { label: '选择', key: 'select', icon: () => h(PhCheck, { size: 16 }) },
])

function openMenu(event: MouseEvent) {
  event.stopPropagation()
  // bottom-end：x 是菜单右边缘的锚点，贴着 ⋯ 按钮右侧展开
  const rect = (event.currentTarget as HTMLElement).getBoundingClientRect()
  menuX.value = rect.right + 4
  menuY.value = rect.bottom + 6
  menuOpen.value = true
}

async function handleMenu(key: string) {
  menuOpen.value = false
  if (key === 'select') {
    emit('long-press', props.comic)
    return
  }
  if (key === 'merge') {
    await handleMergeCbz()
    return
  }
  await showComicDownloadDirInFileManager()
}

async function showComicDownloadDirInFileManager() {
  const dir = props.comic.comicDownloadDir
  if (!dir) return
  if (isMobile.value) {
    try {
      await navigator.clipboard.writeText(dir)
      message.success('已复制到剪贴板')
    } catch {
      message.error('复制失败，请重试')
    }
    return
  }
  const result = await commands.showPathInFileManager(dir)
  if (result.status === 'error') message.error(getErrorMessage(result.error))
}

async function handleMergeCbz() {
  if (merging.value || !isMergeableComic(props.comic) || archiving.value) return
  merging.value = true
  try {
    const result = await commands.promoteComicArchive(props.comic.id, 'merge')
    if (result.status === 'error') message.error(`整理失败：${formatCommandError(result.error)}`)
    else message.success('已开始整理；完成后分话文件自动清理')
  } catch (error) {
    message.error(`整理失败：${getErrorMessage(error)}`)
  } finally {
    merging.value = false
  }
}

// 长按已经进入选择时，抬起后紧跟的 click 不应再触发阅读/取消选择
function onClickCapture(event: MouseEvent) {
  if (!longPressed) return
  longPressed = false
  event.preventDefault()
  event.stopPropagation()
}

onBeforeUnmount(clearPress)
</script>

<template>
  <article class="library-tile" :class="{ 'is-selected': checked }">
    <button
      type="button"
      class="tile-cover"
      :aria-label="selectionMode ? `选择《${comic.name}》` : archiving ? `《${comic.name}》正在整合` : `阅读《${comic.name}》`"
      @click.capture="onClickCapture"
      @click="onCoverClick"
      @pointerdown="startPress"
      @pointermove="movePress"
      @pointerup="endPress"
      @pointerleave="endPress"
      @pointercancel="endPress">
      <img
        v-if="!coverFailed && (comic.localCoverData || comic.id > 0)"
        class="tile-img"
        :src="comic.localCoverData || `https://${store.imageDomain}/media/albums/${comic.id}_3x4.jpg`"
        :alt="comic.name"
        :draggable="false"
        loading="lazy"
        referrerpolicy="no-referrer"
        @error="coverFailed = true" />
      <span v-else class="tile-placeholder">
        <PhImage :size="24" weight="duotone" />
      </span>

      <span v-if="selectionMode" class="tile-check" :class="{ 'is-on': checked }" aria-hidden="true">
        <PhCheck v-if="checked" :size="13" weight="bold" />
      </span>

      <span v-if="comic.id < 0" class="tile-origin">导入</span>
      <span v-if="archiving" class="tile-archiving">整合中</span>

      <span v-if="progress" class="tile-progress" aria-hidden="true">
        <span :style="{ width: `${percent}%` }"></span>
      </span>
    </button>

    <div class="tile-foot">
      <div class="tile-copy">
        <button type="button" class="tile-title" :title="comic.name" @click="pickComic">{{ comic.name }}</button>
        <div class="tile-meta">
          <span>{{ downloadedChapters }} 话</span>
          <span v-if="progress" class="tile-read">· {{ percent }}%</span>
        </div>
      </div>
      <button v-if="!selectionMode" type="button" class="tile-more" aria-label="更多操作" title="更多操作" @click="openMenu">
        <PhDotsThree :size="17" weight="bold" />
      </button>
    </div>

    <n-dropdown
      :show="menuOpen"
      :options="menuOptions"
      placement="bottom-end"
      :x="menuX"
      :y="menuY"
      @select="handleMenu"
      @clickoutside="menuOpen = false" />
  </article>
</template>

<style scoped>
.library-tile {
  display: flex;
  min-width: 0;
  flex-direction: column;
  gap: 5px;
}

.tile-cover {
  position: relative;
  display: block;
  width: 100%;
  padding: 0;
  overflow: hidden;
  aspect-ratio: 3 / 4;
  border: 1.5px solid transparent;
  border-radius: var(--jm-r-md);
  background: var(--jm-sunken);
  cursor: pointer;
  transition:
    border-color 160ms ease,
    transform 160ms ease,
    box-shadow 160ms ease;
  -webkit-tap-highlight-color: transparent;
  touch-action: pan-y;
}

.tile-cover:active {
  transform: scale(0.985);
}

.library-tile.is-selected .tile-cover {
  border-color: var(--jm-brand);
  box-shadow: 0 0 0 2px rgba(251, 114, 153, 0.16);
}

.tile-img {
  display: block;
  width: 100%;
  height: 100%;
  object-fit: cover;
  user-select: none;
}

.tile-placeholder {
  display: grid;
  width: 100%;
  height: 100%;
  color: var(--jm-text-4);
  place-items: center;
  background: linear-gradient(145deg, #f7f8fa, #eef0f3);
}

.tile-check {
  position: absolute;
  top: 7px;
  left: 7px;
  display: grid;
  width: 21px;
  height: 21px;
  color: transparent;
  place-items: center;
  border: 1.5px solid rgba(255, 255, 255, 0.9);
  border-radius: var(--jm-r-pill);
  background: rgba(23, 24, 28, 0.28);
  backdrop-filter: blur(4px);
}

.tile-check.is-on {
  color: #fff;
  border-color: var(--jm-brand);
  background: var(--jm-brand);
}

.tile-origin,
.tile-archiving {
  position: absolute;
  top: 7px;
  right: 7px;
  padding: 1px 6px;
  color: #fff;
  border-radius: var(--jm-r-pill);
  font-size: 9px;
  font-weight: 700;
  backdrop-filter: blur(4px);
}

.tile-origin {
  background: rgba(0, 174, 236, 0.82);
}

.tile-archiving {
  background: rgba(217, 131, 0, 0.9);
}

/* 阅读进度：贴着封面底边，粉色只在有进度时出现 */
.tile-progress {
  position: absolute;
  right: 0;
  bottom: 0;
  left: 0;
  height: 3px;
  background: rgba(23, 24, 28, 0.28);
}

.tile-progress span {
  display: block;
  height: 100%;
  border-radius: 0 2px 2px 0;
  background: var(--jm-brand);
}

.tile-foot {
  display: flex;
  min-width: 0;
  align-items: flex-start;
  gap: 2px;
}

.tile-copy {
  display: flex;
  min-width: 0;
  flex: 1;
  flex-direction: column;
  gap: 3px;
}

/* 更多操作：24px 视觉、44px 触控热区，只在触屏上放大 */
.tile-more {
  display: grid;
  width: 24px;
  height: 24px;
  flex: 0 0 auto;
  margin: 1px -5px 0 0;
  place-items: center;
  color: var(--jm-text-4);
  border: 0;
  border-radius: var(--jm-r-sm);
  background: transparent;
  cursor: pointer;
  transition:
    color 150ms ease,
    background-color 150ms ease;
}

.tile-more:hover {
  color: var(--jm-text-2);
  background: var(--jm-sunken);
}

.tile-more:focus-visible {
  outline: 2px solid var(--jm-brand);
  outline-offset: -2px;
}

.tile-title {
  display: -webkit-box;
  padding: 0 1px;
  overflow: hidden;
  color: var(--jm-text);
  border: 0;
  background: none;
  font-size: 12.5px;
  font-weight: 600;
  line-height: 1.35;
  text-align: left;
  cursor: pointer;
  -webkit-box-orient: vertical;
  -webkit-line-clamp: 2;
}

.library-tile.is-selected .tile-title {
  color: var(--jm-brand);
}

.tile-meta {
  display: flex;
  align-items: center;
  gap: 3px;
  padding: 0 1px;
  overflow: hidden;
  color: var(--jm-text-3);
  font-size: 10.5px;
  white-space: nowrap;
}

.tile-read {
  color: var(--jm-brand);
  font-variant-numeric: tabular-nums;
  font-weight: 600;
}

@media (pointer: coarse) {
  .tile-check {
    width: 24px;
    height: 24px;
  }

  .tile-more {
    width: 36px;
    height: 36px;
    margin: -5px -7px -5px 0;
  }
}
</style>
