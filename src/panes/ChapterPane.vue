<script setup lang="ts">
import FavoriteButton from '../components/FavoriteButton.vue'
import { SelectionArea, SelectionEvent } from '@viselect/vue'
import { computed, nextTick, onActivated, onDeactivated, onMounted, onUnmounted, ref, watch, watchEffect } from 'vue'
import { ChapterInfo, commands, DownloadTaskState, type CbzMode, type Config } from '../bindings.ts'
import { useStore } from '../store.ts'
import { PhArrowClockwise, PhArrowLeft, PhBookOpen, PhDownloadSimple, PhFolderOpen } from '@phosphor-icons/vue'
import IconButton from '../components/IconButton.vue'
import { useMessage } from 'naive-ui'
import { getErrorMessage, formatCommandError } from '../utils/errors.ts'
import { useBackHandler } from '../composables/useAndroidBack.ts'
import { useIsMobile } from '../composables/useIsMobile.ts'

const emit = defineEmits<{ (e: 'back'): void }>()
const store = useStore()
const message = useMessage()
const { isMobile } = useIsMobile()

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
const selectedIds = ref<Set<number>>(new Set())
const selectionAreaRef = ref<InstanceType<typeof SelectionArea>>()
const downloadingSelected = ref(false)
const refreshing = ref(false)

const dropdownOptions = computed(() => {
  const options = [
    {
      label: '勾选',
      key: 'check',
      props: {
        onClick: () => {
          ;[...selectedIds.value]
            .filter((id) => !checkedIds.value.includes(id))
            .forEach((id) => checkedIds.value.push(id))
          showDropdown.value = false
        },
      },
    },
    {
      label: '取消勾选',
      key: 'uncheck',
      props: {
        onClick: () => {
          checkedIds.value = checkedIds.value.filter((id) => !selectedIds.value.has(id))
          showDropdown.value = false
        },
      },
    },
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
  ]

  if (selectedIds.value.size === 1) {
    const selectedId = Array.from(selectedIds.value)[0]
    const chapter = store.pickedComic?.chapterInfos.find((c) => c.chapterId === selectedId)
    if (chapter && chapter.isDownloaded) {
      options.unshift({
        label: `阅读「${chapter.chapterTitle}」`,
        key: 'read-chapter',
        props: {
          onClick: () => {
            if (store.pickedComic) {
              if (store.isArchiving(store.pickedComic.id)) {
                message.warning('正在整合 CBZ，完成后即可阅读')
              } else {
                store.openReader(store.pickedComic.id, chapter.chapterId, store.pickedComic.name)
              }
            }
            showDropdown.value = false
          },
        },
      })
    }
  }

  return options
})

const hasDownloadedChapter = computed(() => {
  return store.pickedComic?.chapterInfos.some((c) => c.isDownloaded) ?? false
})

function openFirstDownloadedChapterReader() {
  if (!store.pickedComic) return
  if (store.isArchiving(store.pickedComic.id)) {
    message.warning('正在整合 CBZ，完成后即可阅读')
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

watch(
  // 只比较漫画 ID：刷新详情会整体替换 pickedComic 对象，引用变化不应清空用户已勾选的章节
  () => store.pickedComic?.id,
  () => {
    checkedIds.value = []
    selectedIds.value.clear()
    selectionAreaRef.value?.selection?.clearSelection()
  },
)

watchEffect(() => {
  if (store.pickedComic === undefined) {
    return
  }
  // 只保留未下载且未在下载中的章节。
  // 失败(Failed)的章节虽然也"未下载"，但用户刚点过下载，不应自动重新出现在
  // 可勾选集合里——否则一次"全选未下载"会把刚失败的章节再次提交。
  const selectableChapterIds = chapterInfos.value
    .filter((c) => c.isDownloaded !== true && !isDownloading(c.state) && c.state !== 'Failed')
    .map((c) => c.chapterId)
  checkedIds.value = checkedIds.value.filter((id) => selectableChapterIds.includes(id))
})

const notDownloadedCount = computed(
  () => chapterInfos.value.filter((c) => c.isDownloaded !== true && !isDownloading(c.state) && c.state !== 'Failed').length,
)

function toggleSelectAllUndownloaded() {
  const notDownloadedChapterIds = chapterInfos.value
    .filter((c) => c.isDownloaded !== true && !isDownloading(c.state) && c.state !== 'Failed')
    .map((c) => c.chapterId)

  if (checkedIds.value.length >= notDownloadedChapterIds.length && notDownloadedChapterIds.length > 0) {
    checkedIds.value = []
  } else {
    checkedIds.value = [...notDownloadedChapterIds]
  }
}

function extractIds(elements: Element[]): number[] {
  return elements
    .map((element) => element.getAttribute('data-key'))
    .filter(Boolean)
    .map(Number)
    .filter((id) => {
      const chapterInfo = store.pickedComic?.chapterInfos.find((c) => c.chapterId === id)
      if (chapterInfo === undefined) {
        return false
      }
      return chapterInfo.isDownloaded !== true
    })
}

function unselectAll({ event, selection }: SelectionEvent) {
  if (!event?.ctrlKey && !event?.metaKey) {
    selection.clearSelection()
    selectedIds.value.clear()
  }
}

function updateSelectedIds({
  store: {
    changed: { added, removed },
  },
}: SelectionEvent) {
  extractIds(added).forEach((id) => selectedIds.value.add(id))
  extractIds(removed).forEach((id) => selectedIds.value.delete(id))
}

async function onContextMenu(e: MouseEvent) {
  showDropdown.value = false
  await nextTick()
  showDropdown.value = true
  dropdownX.value = e.clientX
  dropdownY.value = e.clientY
}

const showCbzModeDialog = ref(false)
const chosenCbzMode = ref<CbzMode>('merge')
const rememberCbzMode = ref(false)
const pendingChapterIds = ref<number[]>([])

function defaultCbzMode(): CbzMode {
  return store.config?.defaultCbzMode ?? 'merge'
}

function downloadChapters() {
  const comic = store.pickedComic
  if (comic === undefined || downloadingSelected.value) {
    return
  }
  // 下载勾选的章节
  const checked = new Set(checkedIds.value)
  const chapterIdsToDownload = comic.chapterInfos
    .filter((c) => c.isDownloaded !== true && checked.has(c.chapterId))
    .map((c) => c.chapterId)
  if (chapterIdsToDownload.length === 0) {
    return
  }
  pendingChapterIds.value = chapterIdsToDownload
  if (comic.chapterInfos.length > 1) {
    // 多话本子：提供「合并全部话」与「逐话」两种整理方式
    chosenCbzMode.value = defaultCbzMode()
    rememberCbzMode.value = false
    showCbzModeDialog.value = true
  } else {
    void runDownloadChapters(defaultCbzMode())
  }
}

async function confirmCbzModeDialog() {
  showCbzModeDialog.value = false
  if (rememberCbzMode.value && store.config && store.config.defaultCbzMode !== chosenCbzMode.value) {
    try {
      const snapshot = JSON.parse(JSON.stringify(store.config)) as Config
      snapshot.defaultCbzMode = chosenCbzMode.value
      await store.persistConfig(snapshot)
    } catch (error) {
      message.warning(`默认整理方式保存失败：${getErrorMessage(error)}`)
    }
  }
  await runDownloadChapters(chosenCbzMode.value)
}

async function runDownloadChapters(cbzMode: CbzMode) {
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
        const result = await commands.createDownloadTask(comic, chapterId, cbzMode)
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

async function showComicDownloadDirInFileManager() {
  if (store.pickedComic === undefined) {
    return
  }

  const comicDownloadDir = store.pickedComic.comicDownloadDir
  if (comicDownloadDir === undefined || comicDownloadDir === null) {
    console.error('comicDownloadDir的值为undefined或null')
    return
  }

  const result = await commands.showPathInFileManager(comicDownloadDir)
  if (result.status === 'error') {
    console.error(result.error)
    message.error(formatCommandError(result.error))
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
      <!-- 顶栏：返回 + 书名 + 图标操作 -->
      <header class="chapter-top-bar">
        <button type="button" class="back-circle" title="返回列表 (Esc)" aria-label="返回列表" @click="handleBack">
          <PhArrowLeft :size="20" weight="bold" />
        </button>

        <div class="comic-header-title">
          <strong class="comic-name-truncate">{{ store.pickedComic.name }}</strong>
          <span class="comic-header-sub">
            <span class="comic-id-badge">JM{{ store.pickedComic.id }}</span>
            <span v-if="!isMobile" class="esc-badge">Esc 返回</span>
          </span>
        </div>

        <div class="chapter-top-actions">
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
      </header>

      <!-- 作品信息卡 -->
      <section class="comic-meta-card">
        <img
          class="comic-meta-cover"
          :src="store.pickedComic.localCoverData || `https://${store.imageDomain}/media/albums/${store.pickedComic.id}_3x4.jpg`"
          alt=""
          referrerpolicy="no-referrer" />
        <div class="comic-meta-info">
          <h2 class="comic-meta-name">{{ store.pickedComic.name }}</h2>

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
            <span v-if="!isMobile" class="hint-text">框选批量勾选 · 右键更多</span>
            <button type="button" class="select-all-chip" @click="toggleSelectAllUndownloaded">
              {{ checkedIds.length >= notDownloadedCount && notDownloadedCount > 0 ? '取消全选' : '全选未下载' }}
            </button>
          </div>
        </header>

        <SelectionArea
          ref="selectionAreaRef"
          class="selection-container custom-scrollbar"
          :options="{ selectables: isMobile ? '' : '.selectable', features: { deselectOnBlur: true } }"
          @contextmenu="onContextMenu"
          @move="updateSelectedIds"
          @start="unselectAll">
          <n-checkbox-group v-model:value="checkedIds" class="chapter-checkbox-group">
            <n-virtual-list
              class="chapter-list"
              :items="chapterInfos"
              :item-size="CHAPTER_ROW_HEIGHT"
              key-field="chapterId">
              <template #default="{ item }">
                <n-checkbox
                  :data-key="item.chapterId"
                  class="selectable chapter-item"
                  :value="item.chapterId"
                  :disabled="item.isDownloaded === true || isDownloading(item.state)"
                  :class="{
                    selected: selectedIds.has(item.chapterId),
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
        </SelectionArea>
      </section>

      <n-dropdown
        placement="bottom-start"
        trigger="manual"
        :x="dropdownX"
        :y="dropdownY"
        :options="dropdownOptions"
        :show="showDropdown"
        :on-clickoutside="() => (showDropdown = false)" />

      <n-modal
        v-model:show="showCbzModeDialog"
        :mask-closable="false"
        preset="card"
        title="整理方式"
        class="cbz-mode-modal"
        style="max-width: 460px; width: 92vw">
        <div class="cbz-mode-options">
          <button
            type="button"
            class="cbz-mode-option"
            :class="{ active: chosenCbzMode === 'merge' }"
            @click="chosenCbzMode = 'merge'">
            <div class="cbz-mode-title">合并全部话（推荐）</div>
            <div class="cbz-mode-desc">整本一个文件，下载完自动合并分话。</div>
          </button>
          <button
            type="button"
            class="cbz-mode-option"
            :class="{ active: chosenCbzMode === 'perChapter' }"
            @click="chosenCbzMode = 'perChapter'">
            <div class="cbz-mode-title">逐话保留</div>
            <div class="cbz-mode-desc">每话一个文件，适合追更。</div>
          </button>
        </div>
        <n-checkbox v-model:checked="rememberCbzMode" class="cbz-mode-remember">
          设为默认
        </n-checkbox>
        <template #footer>
          <div class="cbz-mode-actions">
            <n-button @click="showCbzModeDialog = false">取消</n-button>
            <n-button type="primary" @click="confirmCbzModeDialog">
              开始下载（{{ pendingChapterIds.length }} 话）
            </n-button>
          </div>
        </template>
      </n-modal>
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

/* 顶栏：与一级页同一套 52px 高度 + 安全区 */
.chapter-top-bar {
  display: flex;
  height: calc(var(--jm-header-h) + var(--safe-top));
  flex: 0 0 auto;
  align-items: center;
  gap: 10px;
  padding: var(--safe-top) var(--jm-gutter) 0;
  border-bottom: 1px solid var(--jm-line);
  background: rgba(255, 255, 255, 0.96);
  backdrop-filter: saturate(180%) blur(12px);
}

.back-circle {
  display: grid;
  width: 40px;
  height: 40px;
  flex: 0 0 auto;
  place-items: center;
  padding: 0;
  color: var(--jm-text);
  border: 1px solid var(--jm-line);
  border-radius: var(--jm-r-pill);
  background: var(--jm-sunken);
  cursor: pointer;
  transition:
    color 160ms ease,
    border-color 160ms ease,
    background-color 160ms ease,
    transform 120ms ease;
}

.back-circle:hover {
  color: var(--jm-brand);
  border-color: rgba(251, 114, 153, 0.35);
  background: var(--jm-brand-soft);
}

.back-circle:active {
  transform: scale(0.94);
}

.comic-header-title {
  display: flex;
  min-width: 0;
  flex: 1;
  flex-direction: column;
  gap: 1px;
}

.comic-name-truncate {
  overflow: hidden;
  color: var(--jm-text);
  font-size: 15px;
  font-weight: 750;
  letter-spacing: -0.01em;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.comic-header-sub {
  display: flex;
  align-items: center;
  gap: 7px;
}

.comic-id-badge {
  color: var(--jm-brand);
  font-size: 10px;
  font-weight: 700;
  font-variant-numeric: tabular-nums;
  letter-spacing: 0.02em;
}

.esc-badge {
  color: var(--jm-text-4);
  font-size: 9.5px;
}

.chapter-top-actions {
  display: flex;
  flex: 0 0 auto;
  align-items: center;
  gap: 4px;
}

/* 作品信息卡 */
.comic-meta-card {
  display: flex;
  flex: 0 0 auto;
  gap: 14px;
  margin: 12px var(--jm-gutter) 0;
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

.hint-text {
  color: var(--jm-text-4);
  font-size: 10px;
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

.selection-container .selected {
  border-color: rgba(251, 114, 153, 0.4) !important;
  background-color: var(--jm-brand-soft) !important;
}

/* 已下载用蓝色（信息/内容），避免与「选中」的粉色混淆 */
.selection-container .downloaded {
  background-color: var(--jm-accent-soft) !important;
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

:global(.selection-area) {
  background-color: rgba(251, 114, 153, 0.14) !important;
  border: 1px solid var(--jm-brand) !important;
  border-radius: 2px;
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
