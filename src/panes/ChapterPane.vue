<script setup lang="ts">
import FavoriteButton from '../components/FavoriteButton.vue'
import { SelectionArea, SelectionEvent } from '@viselect/vue'
import { computed, nextTick, onActivated, onDeactivated, onMounted, onUnmounted, ref, watch, watchEffect } from 'vue'
import { ChapterInfo, commands, DownloadTaskState, type CbzMode, type Config } from '../bindings.ts'
import { useStore } from '../store.ts'
import { PhArrowClockwise, PhArrowLeft, PhBookOpen, PhFolderOpen } from '@phosphor-icons/vue'
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
      <!-- Top Navigation & Control Header -->
      <div class="chapter-top-bar">
        <button
          type="button"
          class="back-button"
          title="返回列表 (快捷键: Esc)"
          @click="handleBack">
          <PhArrowLeft :size="18" weight="bold" />
          <span>返回列表</span>
          <span class="esc-badge">Esc</span>
        </button>

        <div class="comic-header-title">
          <span class="comic-id-badge">JM{{ store.pickedComic.id }}</span>
          <span class="comic-name-truncate">{{ store.pickedComic.name }}</span>
        </div>

        <div class="chapter-top-actions">
          <FavoriteButton :comic-id="store.pickedComic.id" :initial-state="store.pickedComic.is_favorite" :initial-epoch="store.pickedComicFavoriteEpoch" />
          <n-tag
            v-if="store.isArchiving(store.pickedComic.id)"
            type="warning"
            size="small"
            round
            class="archiving-tag">
            正在整合 CBZ…
          </n-tag>
          <n-button
            v-if="hasDownloadedChapter"
            size="small"
            type="primary"
            class="read-action-btn"
            :disabled="store.isArchiving(store.pickedComic.id)"
            @click="openFirstDownloadedChapterReader">
            <template #icon>
              <PhBookOpen :size="16" weight="bold" />
            </template>
            开始阅读
          </n-button>

          <n-button
            size="small"
            type="primary"
            class="download-action"
            :loading="downloadingSelected"
            :disabled="checkedIds.length === 0"
            @click="downloadChapters">
            下载勾选 ({{ checkedIds.length }})
          </n-button>

          <n-button size="small" class="refresh-action" :loading="refreshing" @click="refreshChapters">
            <template v-if="!isMobile" #default>刷新</template>
            <template v-else #icon><PhArrowClockwise :size="16" weight="bold" /></template>
            <span class="sr-only">刷新章节</span>
          </n-button>

          <IconButton
            v-if="store.pickedComic.isDownloaded"
            title="打开下载目录"
            @click="showComicDownloadDirInFileManager">
            <PhFolderOpen :size="20" />
          </IconButton>
        </div>
      </div>

      <!-- Comic Meta Hero Info Card -->
      <div class="comic-meta-card">
        <img
          class="comic-meta-cover"
          :src="store.pickedComic.localCoverData || `https://${store.imageDomain}/media/albums/${store.pickedComic.id}_3x4.jpg`"
          alt=""
          referrerpolicy="no-referrer" />
        <div class="comic-meta-info">
          <h2 class="comic-meta-name">{{ store.pickedComic.name }}</h2>

          <div class="meta-row">
            <span class="meta-label">作者：</span>
            <div class="tags-container">
              <n-tag
                v-for="a in (Array.isArray(store.pickedComic.author) ? store.pickedComic.author : [store.pickedComic.author])"
                :key="a"
                size="small"
                type="error"
                round
                class="clickable-tag"
                @click="store.searchKeyword(a)">
                {{ a }}
              </n-tag>
            </div>
          </div>

          <div v-if="store.pickedComic.tags && store.pickedComic.tags.length > 0" class="meta-row">
            <span class="meta-label">标签：</span>
            <div class="tags-container max-h-16 overflow-y-auto">
              <n-tag
                v-for="t in (Array.isArray(store.pickedComic.tags) ? store.pickedComic.tags : [store.pickedComic.tags])"
                :key="t"
                size="small"
                round
                class="clickable-tag tag-pill"
                @click="store.searchKeyword(t)">
                {{ t }}
              </n-tag>
            </div>
          </div>

          <div class="meta-stats-row">
            <span class="stats-item">共 <strong>{{ chapterInfos.length }}</strong> 话</span>
            <span class="dot-divider">•</span>
            <span class="stats-item">已下载 <strong>{{ chapterInfos.filter(c => c.isDownloaded).length }}</strong> 话</span>
            <span class="dot-divider">•</span>
            <span class="stats-item">已选 <strong class="text-[#fb7299]">{{ checkedIds.length }}</strong> 话</span>
            <span v-if="!isMobile" class="dot-divider">•</span>
            <span v-if="!isMobile" class="hint-text">框选批量勾选 · 右键全选 · 双击阅读</span>
            <div class="chapter-quick-actions ml-auto flex items-center gap-2">
              <n-button
                size="tiny"
                type="primary"
                secondary
                @click="toggleSelectAllUndownloaded">
                {{ checkedIds.length >= notDownloadedCount && notDownloadedCount > 0 ? '取消全选' : '全选未下载' }}
              </n-button>
            </div>
          </div>
        </div>
      </div>

      <!-- Chapter Selection List：虚拟滚动，几百话的漫画也只渲染可视区域 -->
      <div class="chapters-section">
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
                  :label="item.chapterTitle"
                  :disabled="item.isDownloaded === true || isDownloading(item.state)"
                  :class="{
                    selected: selectedIds.has(item.chapterId),
                    downloaded: item.isDownloaded,
                    downloading: !item.isDownloaded && isDownloading(item.state),
                  }"
                  @dblclick="handleChapterDoubleClick(item.chapterId, item.isDownloaded)" />
              </template>
            </n-virtual-list>
          </n-checkbox-group>
        </SelectionArea>
      </div>

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
/* 与书库/发现页一致：根节点保持白底，灰底与卡片边界交给内部区块自己表示。
   原先这里自绘 #f6f7f8 灰底 + 10px 内边距，去掉内容区边框后会形成「白框套灰块」，
   和顶栏看起来仍是断开的。 */
.chapter-pane-container {
  display: flex;
  flex-direction: column;
  height: 100%;
  gap: 12px;
  overflow: hidden;
  background: #ffffff;
  padding: 12px 14px;
}

.empty-chapter {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  height: 100%;
  background: #ffffff;
  border: 1px solid #e3e5e7;
  border-radius: var(--radius-panel);
}

.chapter-top-bar {
  display: flex;
  /* 桌面单行、窄屏自动换行：操作区永不越出视口 */
  flex-wrap: wrap;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 10px 18px;
  background: #ffffff;
  border: 1px solid #e3e5e7;
  border-radius: var(--radius-panel);
  box-shadow: 0 2px 8px rgba(0, 0, 0, 0.03);
  flex-shrink: 0;
}

.back-button {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 6px 14px;
  background: #f1f2f3;
  border: 1px solid #e3e5e7;
  color: #18191c;
  border-radius: 999px;
  font-size: 13px;
  font-weight: 600;
  cursor: pointer;
  transition: all 0.2s;
  flex-shrink: 0;
}

.back-button:hover {
  background: #fb7299;
  border-color: #fb7299;
  color: #ffffff;
  box-shadow: 0 3px 10px rgba(251, 114, 153, 0.3);
}

.esc-badge {
  font-size: 10px;
  padding: 1px 5px;
  border-radius: 4px;
  background: rgba(0, 0, 0, 0.08);
  color: #61666d;
  font-family: monospace;
}

.comic-header-title {
  display: flex;
  align-items: center;
  gap: 8px;
  min-width: 0;
  flex: 1;
}

.comic-id-badge {
  padding: 2px 7px;
  background: rgba(251, 114, 153, 0.1);
  border: 1px solid rgba(251, 114, 153, 0.3);
  color: #fb7299;
  border-radius: 6px;
  font-size: 11px;
  font-weight: 700;
  flex-shrink: 0;
}

.comic-name-truncate {
  color: #18191c;
  font-size: 14px;
  font-weight: 700;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.chapter-top-actions {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 8px;
  min-width: 0;
  margin-left: auto;
}

.read-action-btn {
  font-weight: 700;
  box-shadow: 0 2px 8px rgba(251, 114, 153, 0.3);
}

.comic-meta-card {
  display: flex;
  gap: 16px;
  padding: 16px 20px;
  background: #ffffff;
  border: 1px solid #e3e5e7;
  border-radius: var(--radius-panel);
  box-shadow: 0 2px 10px rgba(0, 0, 0, 0.03);
  flex-shrink: 0;
}

.comic-meta-cover {
  width: 90px;
  aspect-ratio: 3/4;
  object-fit: cover;
  border-radius: 10px;
  border: 1px solid #e3e5e7;
  box-shadow: 0 4px 12px rgba(0, 0, 0, 0.08);
  flex-shrink: 0;
}

.comic-meta-info {
  display: flex;
  flex-direction: column;
  gap: 6px;
  min-width: 0;
  flex: 1;
}

.comic-meta-name {
  margin: 0;
  font-size: 16px;
  font-weight: 700;
  color: #18191c;
  line-height: 1.35;
}

.meta-row {
  display: flex;
  align-items: center;
  gap: 6px;
}

.meta-label {
  font-size: 12px;
  color: #9499a0;
  flex-shrink: 0;
}

.tags-container {
  display: flex;
  flex-wrap: wrap;
  gap: 5px;
}

.clickable-tag {
  cursor: pointer;
  transition: opacity 0.2s;
}

.clickable-tag:hover {
  opacity: 0.8;
}

.tag-pill {
  background: #f1f2f3;
  color: #61666d;
  border: 1px solid #e3e5e7;
}

.meta-stats-row {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 12px;
  color: #61666d;
  margin-top: 4px;
}

.stats-item strong {
  color: #fb7299;
}

.dot-divider {
  opacity: 0.4;
}

.hint-text {
  font-size: 11px;
  color: #9499a0;
  margin-left: auto;
}

.chapters-section {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  background: #ffffff;
  border: 1px solid #e3e5e7;
  border-radius: var(--radius-panel);
  padding: 14px;
  box-shadow: 0 2px 10px rgba(0, 0, 0, 0.03);
}

.selection-container {
  flex: 1;
  overflow-y: auto;
  user-select: none;
}

.chapter-list {
  height: 100%;
}

.chapter-checkbox-group {
  display: block;
  height: 100%;
}

.chapter-item {
  height: 40px;
  padding: 8px 12px;
  background: #f6f7f8;
  border: 1px solid #e3e5e7;
  border-radius: 10px;
  transition: all 0.2s;
}

.chapter-item:hover {
  background: #ffffff !important;
  border-color: #fb7299;
  box-shadow: 0 2px 8px rgba(251, 114, 153, 0.15);
}

.selection-container .selected {
  background-color: rgba(251, 114, 153, 0.15) !important;
  border-color: #fb7299 !important;
}

.selection-container .downloaded {
  background-color: rgba(0, 174, 236, 0.1) !important;
  border-color: #00aeec !important;
}

.selection-container .downloading {
  background-color: rgba(251, 114, 153, 0.12) !important;
  border-color: #fb7299 !important;
}

:deep(.n-checkbox__label) {
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
  font-size: 12px;
  color: #18191c;
}

:global(.selection-area) {
  background-color: rgba(251, 114, 153, 0.2) !important;
  border: 1px solid #fb7299 !important;
}

.archiving-tag {
  font-weight: 600;
}

.cbz-mode-options {
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.cbz-mode-option {
  text-align: left;
  padding: 12px 14px;
  border-radius: var(--radius-card);
  border: 1px solid #e3e5e7;
  background: #f6f7f8;
  cursor: pointer;
  transition: all 0.2s;
}

.cbz-mode-option:hover {
  border-color: #fb7299;
}

.cbz-mode-option.active {
  border-color: #fb7299;
  background: rgba(251, 114, 153, 0.08);
  box-shadow: 0 2px 10px rgba(251, 114, 153, 0.15);
}

.cbz-mode-title {
  font-weight: 700;
  color: #18191c;
  font-size: 13px;
}

.cbz-mode-desc {
  margin-top: 4px;
  font-size: 12px;
  color: #61666d;
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

@media (max-width: 768px) {
  .chapter-top-bar {
    padding: 8px 12px;
    gap: 8px;
  }

  /* 窄屏：顶栏切成两行。第一行返回 + 收藏/刷新/目录，第二行两个主操作等宽 */
  .chapter-top-bar {
    display: grid;
    grid-template-columns: auto 1fr;
    grid-template-areas:
      'back meta'
      'actions actions';
    align-items: center;
    row-gap: 8px;
  }

  .back-button { grid-area: back; min-height: 44px; }

  .comic-header-title {
    grid-area: meta;
    justify-content: flex-end;
    overflow: hidden;
  }

  .chapter-top-actions {
    grid-area: actions;
    display: grid;
    grid-template-columns: repeat(6, minmax(0, 1fr));
    align-items: center;
    gap: 6px;
    width: 100%;
    margin-left: 0;
  }

  /* 第一行：收藏、刷新、打开目录（三个图标，各占一格、左对齐） */
  .chapter-top-actions :deep(.favorite-button) {
    grid-column: 1;
    grid-row: 1;
    justify-self: start;
    width: 44px;
    min-width: 44px;
    min-height: 44px;
    padding: 0;
    justify-content: center;
  }

  .chapter-top-actions :deep(.favorite-button span) {
    display: none;
  }

  .chapter-top-actions :deep(.refresh-action) {
    grid-column: 2;
    grid-row: 1;
    justify-self: start;
    width: 44px;
    min-width: 44px;
    min-height: 44px;
    padding: 0;
  }

  .chapter-top-actions :deep(.icon-button) {
    grid-column: 3;
    grid-row: 1;
    justify-self: start;
  }

  .chapter-top-actions :deep(.archiving-tag) {
    grid-column: 4 / 7;
    grid-row: 1;
    justify-self: end;
  }

  /* 第二行：两个主操作各占半行，文案完整可读 */
  .chapter-top-actions :deep(.read-action-btn) {
    grid-column: 1 / 4;
    grid-row: 2;
  }

  .chapter-top-actions :deep(.download-action) {
    grid-column: 4 / 7;
    grid-row: 2;
  }

  .chapter-top-actions :deep(.n-button) {
    min-height: 44px;
    width: 100%;
  }

  .chapter-top-actions :deep(.n-button__content) {
    white-space: nowrap;
  }

  .esc-badge {
    display: none;
  }

}

@media (max-width: 640px) {
  .comic-header-title {
    display: none;
  }

  .comic-meta-card {
    padding: 12px;
    gap: 12px;
  }

  .comic-meta-cover {
    width: 72px;
  }
}
</style>
