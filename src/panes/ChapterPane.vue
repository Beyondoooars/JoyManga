<script setup lang="ts">
import FavoriteButton from '../components/FavoriteButton.vue'
import { computed, nextTick, onActivated, onDeactivated, onMounted, onUnmounted, ref, watch, watchEffect } from 'vue'
import { ChapterInfo, commands, DownloadTaskState } from '../bindings.ts'
import { useStore } from '../store.ts'
import { PhArrowClockwise, PhArrowLeft, PhBookOpen, PhDownloadSimple, PhFolderOpen } from '@phosphor-icons/vue'
import IconButton from '../components/IconButton.vue'
import { useMessage } from 'naive-ui'
import { getErrorMessage, formatCommandError } from '../utils/errors.ts'
import { failureKindLabels } from '../utils/downloadProgress.ts'
import { useBackHandler } from '../composables/useAndroidBack.ts'

const emit = defineEmits<{ (e: 'back'): void }>()
const store = useStore()
const message = useMessage()
// 整合失败只在本次会话内保留：成功或重新开始后由 store 清除
const archiveFailure = computed(() => store.pickedComic ? store.archiveFailure(store.pickedComic.id) : undefined)

function handleBack() {
  store.closeComicDetail()
  emit('back')
}

useBackHandler(() => {
  handleBack()
  return true
}, 20)

function onKeyDown(e: KeyboardEvent) {
  if (e.key === 'Escape') {
    handleBack()
  }
}

// 本页在 KeepAlive 中缓存：失活时必须摘掉全局键盘监听，否则返回其他页后 Esc 仍被吞掉
onMounted(() => {
  window.addEventListener('keydown', onKeyDown)
})

onActivated(() => {
  // 首次挂载与每次重新激活都会调用；先移除再添加，保证只注册一次
  window.removeEventListener('keydown', onKeyDown)
  window.addEventListener('keydown', onKeyDown)
})

onDeactivated(() => {
  window.removeEventListener('keydown', onKeyDown)
})

onUnmounted(() => {
  window.removeEventListener('keydown', onKeyDown)
})

/** 虚拟列表行高：与 `.chapter-item` 的 height 保持一致 */
const CHAPTER_ROW_HEIGHT = 40

const dropdownX = ref<number>(0)
const dropdownY = ref<number>(0)
const showDropdown = ref<boolean>(false)
const checkedIds = ref<number[]>([])
const downloadingSelected = ref(false)
const refreshing = ref(false)

const dropdownOptions = computed(() => [
  {
    label: '全选',
    key: 'check all',
    props: {
      onClick: () => {
        store.pickedComic?.chapterInfos
          ?.filter((c) => c.isDownloaded !== true && !checkedIds.value.includes(c.chapterId))
          .forEach((c) => checkedIds.value.push(c.chapterId))
        showDropdown.value = false
      },
    },
  },
  {
    label: '取消全选',
    key: 'uncheck all',
    props: {
      onClick: () => {
        checkedIds.value.length = 0
        showDropdown.value = false
      },
    },
  },
])

const hasDownloadedChapter = computed(() => {
  return store.pickedComic?.chapterInfos.some((c) => c.isDownloaded) ?? false
})

const hasReadingProgress = computed(() => {
  const comic = store.pickedComic
  if (!comic) return false
  return !!store.readingState.progress[String(comic.id)]
})

function openFirstDownloadedChapterReader() {
  if (!store.pickedComic) return
  if (store.isArchiving(store.pickedComic.id)) {
    message.warning('正在整合 CBZ，完成后即可阅读')
    return
  }
  if (hasReadingProgress.value) {
    store.continueReading(store.pickedComic)
    return
  }
  const target = store.pickedComic.chapterInfos.find((c) => c.isDownloaded) || store.pickedComic.chapterInfos[0]
  if (target) {
    store.openReader(store.pickedComic.id, target.chapterId, store.pickedComic.name)
  }
}

function handleChapterDoubleClick(chapterId: number, isDownloaded: boolean | null | undefined) {
  if (isDownloaded && store.pickedComic) {
    if (store.isArchiving(store.pickedComic.id)) {
      message.warning('正在整合 CBZ，完成后即可阅读')
      return
    }
    store.openReader(store.pickedComic.id, chapterId, store.pickedComic.name)
  }
}

type State = DownloadTaskState | 'Idle'
const chapterInfos = computed<(ChapterInfo & { state: State })[]>(() => {
  const pickedComic = store.pickedComic

  if (pickedComic === undefined) {
    return []
  }

  return pickedComic.chapterInfos.map((chapterInfo) => {
    const progressData = store.progresses.get(chapterInfo.chapterId)
    if (progressData === undefined) {
      return {
        ...chapterInfo,
        state: 'Idle',
      }
    }
    return {
      ...chapterInfo,
      state: progressData.state,
    }
  })
})

/** 可手动勾选章节：未下载、未在下载中（允许失败章节重新勾选下载） */
const selectableChapterIds = computed(() =>
  chapterInfos.value
    .filter((c) => c.isDownloaded !== true && !isDownloading(c.state))
    .map((c) => c.chapterId),
)
const selectableChapterIdSet = computed(() => new Set(selectableChapterIds.value))

/** 一键全选未下载：排除正在下载与失败的章节，避免批量误选刚失败的章节 */
const undownloadedIds = computed(() =>
  chapterInfos.value
    .filter((c) => c.isDownloaded !== true && !isDownloading(c.state) && c.state !== 'Failed')
    .map((c) => c.chapterId),
)

watch(
  // 只比较漫画 ID：刷新详情会整体替换 pickedComic 对象，引用变化不应清空用户已勾选的章节
  () => store.pickedComic?.id,
  () => {
    checkedIds.value = []
  },
)

watchEffect(() => {
  if (store.pickedComic === undefined) {
    return
  }
  const selectable = selectableChapterIdSet.value
  checkedIds.value = checkedIds.value.filter((id) => selectable.has(id))
})

const targetUndownloadedIds = computed(() =>
  undownloadedIds.value.length > 0 ? undownloadedIds.value : selectableChapterIds.value,
)

const allUndownloadedSelected = computed(() => {
  const targetIds = targetUndownloadedIds.value
  if (targetIds.length === 0) return false
  const checked = new Set(checkedIds.value)
  return targetIds.every((id) => checked.has(id))
})

function toggleSelectAllUndownloaded() {
  const targetIds = targetUndownloadedIds.value
  if (allUndownloadedSelected.value) {
    checkedIds.value = []
  } else {
    checkedIds.value = [...targetIds]
  }
}

async function onContextMenu(e: MouseEvent) {
  e.preventDefault()
  showDropdown.value = false
  await nextTick()
  showDropdown.value = true
  dropdownX.value = e.clientX
  dropdownY.value = e.clientY
}

const pendingChapterIds = ref<number[]>([])

function downloadChapters() {
  const comic = store.pickedComic
  if (comic === undefined || downloadingSelected.value) {
    return
  }
  // 下载勾选的章节（默认自动封包为 CBZ，无需弹窗询问）
  const checked = new Set(checkedIds.value)
  const chapterIdsToDownload = comic.chapterInfos
    .filter((c) => c.isDownloaded !== true && checked.has(c.chapterId))
    .map((c) => c.chapterId)
  if (chapterIdsToDownload.length === 0) {
    return
  }
  pendingChapterIds.value = chapterIdsToDownload
  void runDownloadChapters()
}

async function runDownloadChapters() {
  const comic = store.pickedComic
  if (comic === undefined || downloadingSelected.value) {
    return
  }
  const chapterIdsToDownload = pendingChapterIds.value
  pendingChapterIds.value = []
  downloadingSelected.value = true
  let failedCount = 0
  let submittedCount = 0
  try {
    for (const chapterId of chapterIdsToDownload) {
      try {
        const result = await commands.createDownloadTask(comic, chapterId, 'merge')
        if (result.status === 'error') {
          failedCount += 1
          console.error(result.error)
        } else {
          submittedCount += 1
        }
      } catch (error) {
        failedCount += 1
        console.error(error)
      }
    }

    if (failedCount > 0) {
      message.warning(`有 ${failedCount} 个章节未能加入下载队列，请查看日志`)
    } else if (submittedCount > 0) {
      message.success(`已将 ${submittedCount} 个章节加入下载队列`)
    }
  } finally {
    downloadingSelected.value = false
  }
}

async function refreshChapters() {
  if (store.pickedComic === undefined || refreshing.value) {
    return
  }

  refreshing.value = true
  try {
    const result = await store.refreshPickedComic()
    if (result.status === 'error') {
      console.error(result.error)
      message.error(formatCommandError(result.error))
    }
  } catch (error) {
    console.error(error)
    message.error(`刷新章节失败：${getErrorMessage(error)}`)
  } finally {
    refreshing.value = false
  }
  // TODO: 如果pickedComic已下载，则更新元数据
}

const isAndroid = /Android/i.test(navigator.userAgent)

async function showComicDownloadDirInFileManager() {
  if (store.pickedComic === undefined) {
    return
  }

  const comicDownloadDir = store.pickedComic.comicDownloadDir
  if (comicDownloadDir === undefined || comicDownloadDir === null) {
    console.error('comicDownloadDir的值为undefined或null')
    return
  }

  if (!isAndroid) {
    const result = await commands.showPathInFileManager(comicDownloadDir)
    if (result.status === 'ok') return
  }

  try {
    await navigator.clipboard.writeText(comicDownloadDir)
    message.success('已复制下载目录到剪贴板')
  } catch {
    const result = await commands.showPathInFileManager(comicDownloadDir)
    if (result.status === 'error') {
      console.error(result.error)
      message.error(formatCommandError(result.error))
    }
  }
}

function isDownloading(state: State) {
  return state === 'Pending' || state === 'Downloading' || state === 'Paused'
}
</script>

<template>
  <div class="chapter-pane-container">
    <div v-if="store.pickedComic === undefined" class="empty-chapter">
      <n-empty description="未选择漫画" />
      <n-button class="mt-4" type="primary" @click="handleBack">返回发现</n-button>
    </div>

    <template v-else>
      <!-- 悬浮返回按键：彻底移除冗余实体顶栏，轻量通透 -->
      <button
        type="button"
        class="hero-floating-back"
        title="返回列表 (Esc)"
        aria-label="返回列表"
        @click="handleBack">
        <PhArrowLeft :size="20" weight="bold" />
      </button>

      <!-- 作品信息卡 -->
      <section class="comic-meta-card">
        <img
          class="comic-meta-cover"
          :src="store.pickedComic.localCoverData || `https://${store.imageDomain}/media/albums/${store.pickedComic.id}_3x4.jpg`"
          alt=""
          referrerpolicy="no-referrer" />
        <div class="comic-meta-info">
          <div class="comic-name-row">
            <h2 class="comic-meta-name">{{ store.pickedComic.name }}</h2>
            <FavoriteButton
              class="hero-fav-btn"
              :comic-id="store.pickedComic.id"
              :initial-state="store.pickedComic.is_favorite"
              :initial-epoch="store.pickedComicFavoriteEpoch" />
          </div>
          <div class="comic-id-text">JM{{ store.pickedComic.id }}</div>

          <div class="meta-tags">
            <button
              v-for="a in (Array.isArray(store.pickedComic.author) ? store.pickedComic.author : [store.pickedComic.author])"
              :key="a"
              type="button"
              class="tag-chip is-author"
              @click="store.searchKeyword(a)">
              {{ a }}
            </button>
            <button
              v-for="t in (store.pickedComic.tags && store.pickedComic.tags.length > 0 ? (Array.isArray(store.pickedComic.tags) ? store.pickedComic.tags : [store.pickedComic.tags]) : [])"
              :key="t"
              type="button"
              class="tag-chip"
              @click="store.searchKeyword(t)">
              {{ t }}
            </button>
          </div>

          <div class="meta-stats-row">
            <span class="stats-item">共 <strong>{{ chapterInfos.length }}</strong> 话</span>
            <span class="dot-divider" aria-hidden="true">·</span>
            <span class="stats-item">已下载 <strong>{{ chapterInfos.filter(c => c.isDownloaded).length }}</strong></span>
            <span class="dot-divider" aria-hidden="true">·</span>
            <span class="stats-item is-brand">已选 <strong>{{ checkedIds.length }}</strong></span>
            <n-tag v-if="store.isArchiving(store.pickedComic.id)" type="warning" size="small" round class="archiving-tag">
              正在整合 CBZ…
            </n-tag>
            <n-tag
              v-else-if="archiveFailure"
              type="error"
              size="small"
              round
              class="archiving-tag"
              :title="archiveFailure.message">
              整合失败 · {{ failureKindLabels[archiveFailure.kind] }}
            </n-tag>
          </div>

          <div class="meta-actions">
            <button
              v-if="hasDownloadedChapter"
              type="button"
              class="jm-btn is-primary meta-action"
              :disabled="store.isArchiving(store.pickedComic.id)"
              @click="openFirstDownloadedChapterReader">
              <PhBookOpen :size="17" weight="bold" />
              开始阅读
            </button>
            <button
              type="button"
              class="jm-btn is-download meta-action"
              :disabled="checkedIds.length === 0 || downloadingSelected"
              @click="downloadChapters">
              <PhDownloadSimple :size="17" weight="bold" />
              {{ downloadingSelected ? '正在加入…' : `下载勾选 ${checkedIds.length > 0 ? `(${checkedIds.length})` : ''}` }}
            </button>
          </div>
        </div>
      </section>

      <!-- 章节列表：虚拟滚动，几百话的漫画也只渲染可视区域 -->
      <section class="chapters-section">
        <header class="chapters-head">
          <div class="chapters-title">
            <strong>章节</strong>
            <span>{{ chapterInfos.length }} 话</span>
          </div>
          <div class="chapters-tools">
            <button type="button" class="select-all-chip" @click="toggleSelectAllUndownloaded">
              {{ allUndownloadedSelected ? '取消全选' : '全选未下载' }}
            </button>
          </div>
        </header>

        <div class="selection-container custom-scrollbar" @contextmenu="onContextMenu">
          <n-checkbox-group v-model:value="checkedIds" class="chapter-checkbox-group">
            <n-virtual-list
              class="chapter-list"
              :items="chapterInfos"
              :item-size="CHAPTER_ROW_HEIGHT"
              key-field="chapterId">
              <template #default="{ item }">
                <n-checkbox
                  class="chapter-item"
                  :value="item.chapterId"
                  :disabled="item.isDownloaded === true || isDownloading(item.state)"
                  :class="{
                    downloaded: item.isDownloaded,
                    downloading: !item.isDownloaded && isDownloading(item.state),
                  }"
                  @dblclick="handleChapterDoubleClick(item.chapterId, item.isDownloaded)">
                  <span class="chapter-name">{{ item.chapterTitle }}</span>
                  <span v-if="item.isDownloaded" class="chapter-state is-done">已下载</span>
                  <span v-else-if="isDownloading(item.state)" class="chapter-state is-active">{{ item.state === 'Downloading' ? '下载中' : item.state === 'Paused' ? '已暂停' : '排队中' }}</span>
                  <span v-else-if="item.state === 'Failed'" class="chapter-state is-failed">失败</span>
                </n-checkbox>
              </template>
            </n-virtual-list>
          </n-checkbox-group>
        </div>
      </section>

      <!-- 底部常驻悬浮下载/阅读操作坞：大拇指热区直达 -->
      <nav class="chapter-bottom-dock">
        <div class="dock-left-actions">
          <FavoriteButton
            variant="icon"
            :comic-id="store.pickedComic.id"
            :initial-state="store.pickedComic.is_favorite"
            :initial-epoch="store.pickedComicFavoriteEpoch" />
          <IconButton title="刷新章节" aria-label="刷新章节" :disabled="refreshing" @click="refreshChapters">
            <PhArrowClockwise :size="19" :class="{ 'jm-spin': refreshing }" />
          </IconButton>
          <IconButton
            v-if="store.pickedComic.isDownloaded"
            title="打开下载目录"
            aria-label="打开下载目录"
            @click="showComicDownloadDirInFileManager">
            <PhFolderOpen :size="19" />
          </IconButton>
        </div>

        <button
          v-if="checkedIds.length > 0"
          type="button"
          class="dock-cta-btn is-download"
          :disabled="downloadingSelected"
          @click="downloadChapters">
          <PhDownloadSimple :size="18" weight="bold" />
          <span>{{ downloadingSelected ? '正在加入…' : `下载勾选 (${checkedIds.length})` }}</span>
        </button>
        <button
          v-else-if="hasDownloadedChapter"
          type="button"
          class="dock-cta-btn is-read"
          :disabled="store.isArchiving(store.pickedComic.id)"
          @click="openFirstDownloadedChapterReader">
          <PhBookOpen :size="18" weight="bold" />
          <span>{{ hasReadingProgress ? '继续阅读' : '开始阅读' }}</span>
        </button>
        <button
          v-else
          type="button"
          class="dock-cta-btn is-download"
          @click="toggleSelectAllUndownloaded">
          <PhDownloadSimple :size="18" weight="bold" />
          <span>选择章节下载</span>
        </button>
      </nav>

      <n-dropdown
        placement="bottom-start"
        trigger="manual"
        :x="dropdownX"
        :y="dropdownY"
        :options="dropdownOptions"
        :show="showDropdown"
        :on-clickoutside="() => (showDropdown = false)" />
    </template>
  </div>
</template>

<style scoped>
.chapter-pane-container {
  display: flex;
  height: 100%;
  flex-direction: column;
  overflow: hidden;
  background: var(--jm-bg);
}

.empty-chapter {
  display: flex;
  height: 100%;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 14px;
  background: var(--jm-surface);
}

/* 悬浮轻量返回按键：替代实体顶栏，轻量通透 */
.hero-floating-back {
  position: absolute;
  top: max(var(--safe-top), 12px);
  left: 14px;
  z-index: 20;
  display: grid;
  width: 38px;
  height: 38px;
  place-items: center;
  padding: 0;
  color: var(--jm-text);
  border: 1px solid var(--jm-line);
  border-radius: var(--jm-r-pill);
  background: var(--jm-surface);
  backdrop-filter: blur(12px);
  -webkit-backdrop-filter: blur(12px);
  cursor: pointer;
  box-shadow: var(--jm-shadow-1);
  transition: all 160ms ease;
}

.hero-floating-back:hover {
  color: var(--jm-brand);
  border-color: rgba(251, 114, 153, 0.35);
  background: var(--jm-brand-soft);
}

.hero-floating-back:active {
  transform: scale(0.92);
}

.comic-name-row {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 8px;
}

.hero-fav-btn {
  flex-shrink: 0;
}

.comic-id-text {
  color: var(--jm-brand);
  font-size: 11px;
  font-weight: 700;
  font-variant-numeric: tabular-nums;
  letter-spacing: 0.02em;
}

/* 底部常驻悬浮下载/阅读操作坞 */
.chapter-bottom-dock {
  display: flex;
  flex: 0 0 auto;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 10px var(--jm-gutter) max(var(--safe-bottom), 10px);
  background: var(--jm-surface);
  backdrop-filter: blur(16px);
  -webkit-backdrop-filter: blur(16px);
  border-top: 1px solid var(--jm-line);
  box-shadow: 0 -4px 20px rgba(0, 0, 0, 0.25);
  z-index: 30;
}

.dock-left-actions {
  display: flex;
  align-items: center;
  gap: 8px;
}

.dock-cta-btn {
  flex: 1;
  height: 42px;
  border-radius: var(--jm-r-pill);
  border: 0;
  font-size: 14px;
  font-weight: 750;
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 6px;
  cursor: pointer;
  transition: all 160ms ease;
}

.dock-cta-btn.is-read {
  background: linear-gradient(135deg, var(--jm-brand) 0%, var(--jm-brand-hover) 100%);
  color: #fff;
  box-shadow: 0 4px 14px rgba(251, 114, 153, 0.4);
}

.dock-cta-btn.is-download {
  background: linear-gradient(135deg, var(--jm-accent) 0%, #0088CC 100%);
  color: #fff;
  box-shadow: 0 4px 14px rgba(0, 194, 255, 0.4);
}

.dock-cta-btn:disabled {
  opacity: 0.55;
  cursor: not-allowed;
  box-shadow: none;
}

/* 作品信息卡 */
.comic-meta-card {
  display: flex;
  flex: 0 0 auto;
  gap: 14px;
  margin: calc(var(--safe-top, 0px) + 12px) var(--jm-gutter) 0;
  padding: 14px;
  border: 1px solid var(--jm-line);
  border-radius: var(--jm-r-lg);
  background: var(--jm-surface);
  box-shadow: var(--jm-shadow-1);
}

.comic-meta-cover {
  width: 82px;
  flex: 0 0 auto;
  aspect-ratio: 3 / 4;
  border-radius: var(--jm-r-md);
  box-shadow: var(--jm-shadow-2);
  object-fit: cover;
}

.comic-meta-info {
  display: flex;
  min-width: 0;
  flex: 1;
  flex-direction: column;
  gap: 8px;
}

.comic-meta-name {
  display: -webkit-box;
  margin: 0;
  overflow: hidden;
  color: var(--jm-text);
  font-size: 15.5px;
  font-weight: 750;
  letter-spacing: -0.015em;
  line-height: 1.35;
  -webkit-box-orient: vertical;
  -webkit-line-clamp: 2;
}

.meta-tags {
  display: flex;
  max-height: 54px;
  flex-wrap: wrap;
  gap: 5px;
  overflow: hidden;
}

.tag-chip {
  height: 24px;
  padding: 0 9px;
  color: var(--jm-text-2);
  border: 0;
  border-radius: var(--jm-r-pill);
  background: var(--jm-sunken);
  font-size: 11px;
  font-weight: 550;
  white-space: nowrap;
  cursor: pointer;
}

.tag-chip.is-author {
  color: var(--jm-brand);
  background: var(--jm-brand-soft);
}

.meta-stats-row {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 5px;
  color: var(--jm-text-3);
  font-size: 11.5px;
}

.stats-item strong {
  color: var(--jm-text);
  font-weight: 700;
  font-variant-numeric: tabular-nums;
}

.stats-item.is-brand strong {
  color: var(--jm-brand);
}

.dot-divider {
  color: var(--jm-text-4);
}

.archiving-tag {
  margin-left: 2px;
}

.meta-actions {
  display: flex;
  gap: 8px;
  margin-top: auto;
}

/* 开始阅读 = 主操作（粉），下载 = 信息/传输（蓝） */
.meta-action {
  display: inline-flex;
  height: 38px;
  flex: 1;
  align-items: center;
  justify-content: center;
  gap: 5px;
  border: 0;
  border-radius: var(--jm-r-md);
  font-size: var(--jm-fs-sm);
  font-weight: 700;
  white-space: nowrap;
  cursor: pointer;
  transition:
    background-color 160ms ease,
    transform 120ms ease;
}

.meta-action:active:not(:disabled) {
  transform: scale(0.98);
}

.meta-action.is-primary {
  color: #fff;
  background: var(--jm-brand);
  box-shadow: 0 3px 12px rgba(251, 114, 153, 0.3);
}

.meta-action.is-primary:disabled {
  color: var(--jm-text-4);
  background: var(--jm-sunken);
  box-shadow: none;
  cursor: default;
}

.meta-action.is-download {
  color: var(--jm-accent);
  background: var(--jm-accent-soft);
}

.meta-action.is-download:disabled {
  color: var(--jm-text-4);
  background: var(--jm-sunken);
  cursor: default;
}

/* 章节列表 */
.chapters-section {
  display: flex;
  min-height: 0;
  flex: 1;
  flex-direction: column;
  margin: 12px var(--jm-gutter) 0;
  padding: 12px 10px 4px;
  border: 1px solid var(--jm-line);
  border-radius: var(--jm-r-lg) var(--jm-r-lg) 0 0;
  background: var(--jm-surface);
}

.chapters-head {
  display: flex;
  flex: 0 0 auto;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
  padding: 0 4px 10px;
}

.chapters-title {
  display: flex;
  align-items: baseline;
  gap: 7px;
}

.chapters-title strong {
  color: var(--jm-text);
  font-size: 14px;
  font-weight: 750;
}

.chapters-title span {
  color: var(--jm-text-3);
  font-size: 11px;
  font-variant-numeric: tabular-nums;
}

.chapters-tools {
  display: flex;
  align-items: center;
  gap: 8px;
}

.select-all-chip {
  height: 28px;
  padding: 0 11px;
  color: var(--jm-brand);
  border: 1px solid rgba(251, 114, 153, 0.28);
  border-radius: var(--jm-r-pill);
  background: var(--jm-brand-soft);
  font-size: 11.5px;
  font-weight: 650;
  white-space: nowrap;
  cursor: pointer;
}

.selection-container {
  min-height: 0;
  flex: 1;
  overflow-y: auto;
  padding-bottom: var(--safe-bottom);
  user-select: none;
}

.chapter-list {
  height: 100%;
}

.chapter-checkbox-group {
  display: block;
  height: 100%;
}

/* 行高与 CHAPTER_ROW_HEIGHT 保持一致 */
.chapter-item {
  display: flex;
  height: 40px;
  align-items: center;
  gap: 8px;
  padding: 0 10px;
  border: 1px solid transparent;
  border-radius: var(--jm-r-sm);
  transition:
    background-color 140ms ease,
    border-color 140ms ease;
}

.chapter-item:hover {
  border-color: var(--jm-line);
  background: var(--jm-sunken) !important;
}

/* 已下载用蓝色（信息/内容），避免与「选中」的粉色混淆 */
.selection-container .downloaded {
  background-color: var(--jm-accent-soft) !important;
  cursor: pointer;
}

.selection-container .downloaded :deep(.n-checkbox__label) {
  cursor: pointer;
}

.selection-container .downloading {
  background-color: rgba(0, 174, 236, 0.07) !important;
}

.chapter-name {
  min-width: 0;
  flex: 1;
  overflow: hidden;
  color: var(--jm-text);
  font-size: 12.5px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.chapter-state {
  flex: 0 0 auto;
  padding: 1px 7px;
  border-radius: var(--jm-r-pill);
  font-size: 9.5px;
  font-weight: 700;
  white-space: nowrap;
}

.chapter-state.is-done {
  color: var(--jm-accent);
  background: var(--jm-accent-soft);
}

.chapter-state.is-active {
  color: var(--jm-brand);
  background: var(--jm-brand-soft);
}

.chapter-state.is-failed {
  color: var(--jm-danger);
  background: var(--jm-danger-soft);
}

:deep(.n-checkbox) {
  width: 100%;
}

:deep(.n-checkbox__label) {
  display: flex;
  min-width: 0;
  flex: 1;
  align-items: center;
  gap: 8px;
  padding: 0 !important;
}

:deep(.n-checkbox-box-wrapper) {
  flex: 0 0 auto;
}

/* 整理方式弹窗 */
.cbz-mode-options {
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.cbz-mode-option {
  padding: 12px 14px;
  text-align: left;
  border: 1px solid var(--jm-line);
  border-radius: var(--jm-r-md);
  background: var(--jm-surface);
  cursor: pointer;
  transition:
    border-color 160ms ease,
    background-color 160ms ease;
}

.cbz-mode-option:hover {
  border-color: rgba(251, 114, 153, 0.4);
}

.cbz-mode-option.active {
  border-color: var(--jm-brand);
  background: var(--jm-brand-soft);
  box-shadow: 0 2px 10px rgba(251, 114, 153, 0.12);
}

.cbz-mode-title {
  color: var(--jm-text);
  font-size: var(--jm-fs-sm);
  font-weight: 700;
}

.cbz-mode-desc {
  margin-top: 4px;
  color: var(--jm-text-3);
  font-size: var(--jm-fs-xs);
  line-height: 1.5;
}

.cbz-mode-remember {
  margin-top: 12px;
}

.cbz-mode-actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
}

@media (pointer: coarse) {
  .back-circle {
    width: 44px;
    height: 44px;
  }
}

@media (max-width: 640px) {
  .comic-meta-card {
    padding: 12px;
    gap: 12px;
  }

  .comic-meta-cover {
    width: 70px;
  }

  .comic-meta-name {
    font-size: 14.5px;
  }

  .chapters-section {
    margin-top: 10px;
  }
}
</style>
