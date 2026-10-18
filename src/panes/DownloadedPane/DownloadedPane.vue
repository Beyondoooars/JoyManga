<script setup lang="tsx">
import { Comic, commands } from '../../bindings.ts'
import { computed, onMounted, onUnmounted, ref, watch, watchEffect } from 'vue'
import DownloadedComicCard from './components/DownloadedComicCard.vue'
import { open } from '@tauri-apps/plugin-dialog'
import { getCurrentWebview } from '@tauri-apps/api/webview'
import { useStore } from '../../store.ts'
import { DropdownOption, NCheckbox, NIcon } from 'naive-ui'
import { SelectionArea, SelectionEvent } from '@viselect/vue'
import {
  PhArchive,
  PhBooks,
  PhCheck,
  PhChecks,
  PhDownloadSimple,
  PhFilePdf,
  PhFileZip,
  PhTrash,
  PhX,
  PhPlus,
} from '@phosphor-icons/vue'
import ProgressesPane from '../ProgressesPane/ProgressesPane.vue'
import UpdateDownloadedComicsButton from './components/UpdateDownloadedComicsButton.vue'
import PathField from '../../components/PathField.vue'
import PathEditDialog from '../../dialogs/PathEditDialog.vue'
import { useDialog, useMessage } from 'naive-ui'
import { getErrorMessage } from '../../utils/errors.ts'
import { useDropdown } from '../../composables/useDropdown.ts'
import { useIsMobile } from '../../composables/useIsMobile.ts'
import { withAndroidSelection } from '../../utils/androidStorage.ts'
import { filterLibrary } from '../../utils/librarySearch.ts'
import { libraryApi } from '../../libraryApi.ts'
import { isMergeableComic } from '../../utils/mergeableComics.ts'

const store = useStore()
const message = useMessage()
const dialog = useDialog()

const { isMobile, isAndroid } = useIsMobile()

function toggleSelectAll() {
  if (checkedIds.value.size >= currentPageComics.value.length && currentPageComics.value.length > 0) {
    checkedIds.value.clear()
  } else {
    currentPageComics.value.forEach((c) => checkedIds.value.add(c.id))
  }
}

const selectedIds = ref<Set<number>>(new Set())
const checkedIds = ref<Set<number>>(new Set())
const { dropdownX, dropdownY, dropdownShowing, showDropdown, closeDropdown } = useDropdown()

const baseDropdownOptions: DropdownOption[] = [
  {
    label: '勾选',
    key: 'check',
    icon: () => (
      <NIcon size="20">
        <PhCheck />
      </NIcon>
    ),
    props: {
      onClick: () => {
        selectedIds.value.forEach((id) => checkedIds.value.add(id))
        dropdownShowing.value = false
      },
    },
  },
  {
    label: '取消勾选',
    key: 'uncheck',
    icon: () => (
      <NIcon size="20">
        <PhX />
      </NIcon>
    ),
    props: {
      onClick: () => {
        selectedIds.value.forEach((id) => checkedIds.value.delete(id))
        dropdownShowing.value = false
      },
    },
  },
  {
    label: '全选',
    key: 'select-all',
    icon: () => (
      <NIcon size="20">
        <PhChecks />
      </NIcon>
    ),
    props: {
      onClick: () => {
        currentPageComics.value.forEach((comic) => selectedIds.value.add(comic.id))
        dropdownShowing.value = false
      },
    },
  },
  {
    label: '整理所选漫画为整本 CBZ',
    key: 'merge-cbz',
    icon: () => (
      <NIcon size="20">
        <PhArchive />
      </NIcon>
    ),
    props: {
      onClick: () => {
        dropdownShowing.value = false
        void mergeSelectedComics(selectedMergeableComics.value)
      },
    },
  },
  {
    label: '删除库存',
    key: 'delete',
    icon: () => (
      <NIcon size="20">
        <PhTrash />
      </NIcon>
    ),
    props: {
      onClick: () => {
        // 框选项并入勾选项后执行删除
        selectedIds.value.forEach((id) => checkedIds.value.add(id))
        dropdownShowing.value = false
        void deleteDownloadedComics()
      },
    },
  },
]
const selectionAreaRef = ref<InstanceType<typeof SelectionArea>>()
const loading = ref(false)

const PAGE_SIZE = 20
const libraryFilter = ref('all')
const sortOrder = ref('added')
const searchQuery = ref('')
const importing = ref(false)
const recentComics = computed(() => store.downloadedComics.filter((comic) => store.readingState.progress[String(comic.id)])
  .sort((a, b) => store.readingState.progress[String(b.id)].updatedAt - store.readingState.progress[String(a.id)].updatedAt).slice(0, 3))
const mergeableComics = computed(() => store.downloadedComics.filter(isMergeableComic))
const selectedMergeableComics = computed(() => currentPageComics.value.filter((comic) => selectedIds.value.has(comic.id) && isMergeableComic(comic)))
const checkedMergeableComics = computed(() => currentPageComics.value.filter((comic) => checkedIds.value.has(comic.id) && isMergeableComic(comic)))
const dropdownOptions = computed(() => baseDropdownOptions.filter((option) => option.key !== 'merge-cbz' || selectedMergeableComics.value.length > 0))
const filterOptions = computed(() => [ { label: '全部漫画', value: 'all' }, { label: `可合并多话（${mergeableComics.value.length} 本）`, value: 'mergeable' }, { label: '正在阅读', value: 'reading' },
  { label: '有书签', value: 'bookmarks' }, { label: '本地导入', value: 'imported' } ])
const sortOptions = [ { label: '最近加入', value: 'added' }, { label: '最近阅读', value: 'read' }, { label: '标题排序', value: 'title' } ]
const filteredComics = computed(() => {
  const comics = store.downloadedComics.filter((comic) => {
    if (libraryFilter.value === 'mergeable') return isMergeableComic(comic)
    if (libraryFilter.value === 'reading') return !!store.readingState.progress[String(comic.id)]
    if (libraryFilter.value === 'bookmarks') return store.readingState.bookmarks.some((bookmark) => bookmark.comicId === comic.id)
    if (libraryFilter.value === 'imported') return comic.id < 0
    return true
  })
  if (sortOrder.value === 'title') comics.sort((a, b) => a.name.localeCompare(b.name, 'zh-CN', { numeric: true }))
  if (sortOrder.value === 'read') comics.sort((a, b) => (store.readingState.progress[String(b.id)]?.updatedAt ?? 0) - (store.readingState.progress[String(a.id)]?.updatedAt ?? 0))
  return filterLibrary(comics, searchQuery.value)
})

const pathFieldRef = ref<InstanceType<typeof PathField>>()

const showImportModal = ref(false)
const importMode = ref<'directory' | 'file'>('file')

async function importComics(directory = false) {
  if (importing.value) return
  if (isAndroid) {
    try { await withAndroidSelection(directory ? 'directory' : 'cbz', executeImport) }
    catch (error) { message.error(`选择文件失败：${getErrorMessage(error)}`) }
    return
  }
  if (isMobile.value) {
    importMode.value = directory ? 'directory' : 'file'
    showImportModal.value = true
    return
  }
  try {
    const selected = await open(directory ? { directory: true } : { multiple: true, filters: [{ name: '漫画压缩包', extensions: ['cbz'] }] })
    if (!selected) return
    await executeImport(Array.isArray(selected) ? selected : [selected])
  } catch (error) { message.error(`导入失败：${getErrorMessage(error)}`) }
}

async function executeImport(paths: string[]) {
  if (!paths || paths.length === 0) return
  importing.value = true
  try {
    const report = await libraryApi.importComics(paths)
    await store.refreshDownloadedComics()
    message.success(`导入 ${report.imported} 本，跳过 ${report.skipped} 项`)
    if (report.errors.length) dialog.warning({ title: '部分文件未能导入', content: report.errors.join('\n'), positiveText: '知道了' })
  } catch (error) {
    message.error(`导入失败：${getErrorMessage(error)}`)
  } finally {
    importing.value = false
  }
}

// ── 桌面端拖放导入（Fluent 习惯）：拖入 .cbz 文件松手即导入 ──
const dragActive = ref(false)
let unlistenDragDrop: (() => void) | undefined
// 注册是异步的：组件可能在 await 完成前就被卸载，此时不能再挂监听
let dragDropDisposed = false

onMounted(async () => {
  if (isMobile.value) return
  try {
    const unlisten = await getCurrentWebview().onDragDropEvent((event) => {
      const payload = event.payload
      if (payload.type === 'enter' || payload.type === 'over') {
        dragActive.value = true
      } else if (payload.type === 'leave') {
        dragActive.value = false
      } else if (payload.type === 'drop') {
        dragActive.value = false
        const paths = payload.paths ?? []
        // 只接受 .cbz 与目录（过滤掉 txt 等无关文件）；目录是否存在交给后端校验
        const toImport = paths.filter((path) => {
          if (path.toLowerCase().endsWith('.cbz')) return true
          return !/[\\/][^\\/]*\.[^\\/]*$/.test(path)
        })
        if (toImport.length === 0) {
          message.info('请拖入 .cbz 文件或漫画文件夹')
          return
        }
        void executeImport(toImport)
      }
    })
    if (dragDropDisposed) {
      unlisten()
    } else {
      unlistenDragDrop = unlisten
    }
  } catch (error) {
    console.error('注册拖放导入失败', error)
  }
})

onUnmounted(() => {
  dragDropDisposed = true
  unlistenDragDrop?.()
})

async function refreshLibrary() {
  loading.value = true
  try { await store.refreshDownloadedComics() }
  catch (error) { message.error(`刷新书库失败：${getErrorMessage(error)}`) }
  finally { loading.value = false }
}

watch(() => store.authoritativeConfig?.downloadDir, (path, previous) => {
  if (!previous || path === previous) return
  selectedIds.value.clear()
  checkedIds.value.clear()
  store.downloadedComics = []
  void refreshLibrary()
})

// 当前页码
const currentPage = ref<number>(1)
// 总页数
const pageCount = computed<number>(() => {
  if (filteredComics.value.length === 0) {
    return 1
  }
  return Math.ceil(filteredComics.value.length / PAGE_SIZE)
})
// 当前页的漫画
const currentPageComics = computed<Comic[]>(() => {
  const start = (currentPage.value - 1) * PAGE_SIZE
  const end = start + PAGE_SIZE
  return filteredComics.value.slice(start, end)
})
// 确保当前页码不超过总页数
watchEffect(() => {
  if (currentPage.value > pageCount.value) {
    currentPage.value = pageCount.value
  }
})

watch([libraryFilter, sortOrder, searchQuery], () => {
  currentPage.value = 1
  selectedIds.value.clear()
  checkedIds.value.clear()
  selectionAreaRef.value?.selection?.clearSelection()
})

watch(currentPage, () => {
  selectedIds.value.clear()
  checkedIds.value.clear()
  selectionAreaRef.value?.selection?.clearSelection()
  selectionAreaRef.value?.$el.scrollTo({ top: 0, behavior: 'instant' })
})

// 监听标签页变化，更新下载的漫画列表
watch(
  () => store.currentTabName,
  async () => {
    if (store.currentTabName !== 'downloaded') {
      return
    }

    loading.value = true
    try {
      await store.refreshDownloadedComics()
    } catch (error) {
      console.error(error)
      message.error(`读取本地库存失败：${getErrorMessage(error)}`)
    } finally {
      loading.value = false
    }
  },
  { immediate: true },
)

async function selectLibraryDir() {
  if (store.config === undefined) {
    return
  }

  if (isMobile.value) {
    pathFieldRef.value?.openModal()
    return
  }

  try {
    const selectedDirPath = await open({ directory: true })
    if (selectedDirPath === null) {
      return
    }

    store.config.downloadDir = selectedDirPath
    await store.saveConfigNow()
    await store.refreshDownloadedComics()
    message.success('书库路径已更新，导出同位置')
  } catch (e) {
    console.warn('Native folder picker failed or unsupported, opening modal:', e)
    pathFieldRef.value?.openModal()
  }
}

async function showLibraryDirInFileManager() {
  if (store.config === undefined) {
    return
  }
  const result = await commands.showPathInFileManager(store.config.downloadDir)
  if (result.status === 'error') {
    console.error(result.error)
  }
}

function extractIds(elements: Element[]): number[] {
  return elements
    .map((element) => element.getAttribute('data-key'))
    .filter(Boolean)
    .map(Number)
}

function updateSelectedIds({
  store: {
    changed: { added, removed },
  },
}: SelectionEvent) {
  extractIds(added).forEach((id) => selectedIds.value.add(id))
  extractIds(removed).forEach((id) => selectedIds.value.delete(id))
}

function unselectAll({ event, selection }: SelectionEvent) {
  if (!event?.ctrlKey && !event?.metaKey) {
    selection.clearSelection()
    selectedIds.value.clear()
  }
}

function handleCheckboxClick(comic: Comic) {
  if (checkedIds.value.has(comic.id)) {
    checkedIds.value.delete(comic.id)
  } else {
    checkedIds.value.add(comic.id)
  }
}

function handleContextMenu(comic: Comic) {
  if (selectedIds.value.has(comic.id)) {
    return
  }

  selectedIds.value.clear()
  selectedIds.value.add(comic.id)
}

const exporting = ref(false)

// 批量导出：失败不中断，结束后汇总提示；整个过程中禁用按钮防止重复提交
async function runBatchExport(exportType: 'cbz' | 'pdf') {
  if (checkedIds.value.size === 0 || exporting.value) {
    return
  }
  try {
    await store.saveConfigNow()
  } catch (error) {
    message.error(getErrorMessage(error))
    return
  }
  const comics = currentPageComics.value.filter((comic) => checkedIds.value.has(comic.id))
  if (comics.length === 0) return
  // 切到传输页，让用户看到进度
  store.progressesPaneTabName = 'export'
  store.downloadedSubTab = 'transfers'
  exporting.value = true
  const failures: string[] = []
  try {
    for (const comic of comics) {
      try {
        const result = exportType === 'cbz'
          ? await commands.exportCbz(comic)
          : await commands.exportPdf(comic)
        if (result.status === 'error') {
          failures.push(`${comic.name}：${result.error.err_message}`)
        }
      } catch (error) {
        failures.push(`${comic.name}：${getErrorMessage(error)}`)
      }
    }
  } finally {
    exporting.value = false
  }
  if (failures.length === 0) {
    message.success(`已提交 ${comics.length} 本导出`)
  } else {
    message.warning(`${failures.length}/${comics.length} 本导出失败：${failures.slice(0, 3).join('；')}${failures.length > 3 ? '…' : ''}`)
  }
}

async function exportCbz() {
  await runBatchExport('cbz')
}

async function exportPdf() {
  await runBatchExport('pdf')
}

async function mergeSelectedComics(comics: Comic[] = checkedMergeableComics.value) {
  if (comics.length === 0) {
    return
  }

  let submitted = 0
  for (const comic of comics) {
    if (store.isArchiving(comic.id)) continue
    const result = await commands.promoteComicArchive(comic.id, 'merge')
    if (result.status === 'error') {
      console.error(result.error)
      message.error(`${comic.name} 整理失败：${result.error.err_message}`)
    } else {
      submitted += 1
    }
  }
  if (submitted > 0) {
    message.success(`已开始整理 ${submitted} 本；完成后分话文件自动清理`)
  }
}

async function deleteDownloadedComics() {
  if (checkedIds.value.size === 0) {
    return
  }

  const comics = currentPageComics.value.filter((comic) => checkedIds.value.has(comic.id))
  const deleteFiles = ref(false)
  dialog.warning({
    title: '删除库存',
    content: () => <div>
      <p>删除选中的 {comics.length} 本漫画记录？</p>
      <NCheckbox checked={deleteFiles.value} onUpdateChecked={(value) => { deleteFiles.value = value }}>同时删除本地文件</NCheckbox>
      {deleteFiles.value && <p style="color: #d03050">本地文件删除后无法恢复</p>}
    </div>,
    positiveText: '删除',
    negativeText: '取消',
    onPositiveClick: async () => {
      let failedCount = 0
      for (const comic of comics) {
        try {
          await libraryApi.removeComic(comic.id, deleteFiles.value)
          if (store.activeReaderTarget?.comicId === comic.id) store.closeReader()
          // 清理这本漫画的阅读进度与书签，避免书签列表仍能点到已删除的漫画
          store.forgetComicReadingState(comic.id)
        } catch (error) {
          message.error(getErrorMessage(error))
          failedCount += 1
        }
      }
      checkedIds.value.clear()
      selectedIds.value.clear()
      await store.refreshDownloadedComics()
      if (failedCount > 0) {
        message.warning(`${failedCount} 本未能删除`)
      } else {
        message.success('删除库存成功')
      }
    },
  })
}
</script>

<template>
  <div v-if="store.config !== undefined" class="library-page h-full flex flex-col" :aria-busy="loading">
    <!-- Sub-tab Navigation (Segmented Control): [已入库] vs [下载与传输] -->
    <div class="library-subtabs-bar select-none">
      <div class="subtabs-segmented">
        <button
          type="button"
          :class="['subtab-btn', { active: store.downloadedSubTab === 'library' }]"
          @click="store.downloadedSubTab = 'library'">
          <PhBooks :size="16" weight="duotone" />
          <span>已入库 ({{ store.downloadedComics.length }})</span>
        </button>
        <button
          type="button"
          :class="['subtab-btn', { active: store.downloadedSubTab === 'transfers', 'is-transfer': store.downloadedSubTab === 'transfers' }]"
          @click="store.downloadedSubTab = 'transfers'">
          <div class="relative flex items-center gap-1.5">
            <PhDownloadSimple :size="16" weight="bold" />
            <span>下载与传输</span>
            <span v-if="store.activeDownloadCount > 0" class="subtab-active-dot"></span>
            <span v-if="store.activeDownloadCount > 0" class="subtab-badge">{{ store.activeDownloadCount }}</span>
          </div>
        </button>
      </div>

      <span v-if="store.activeDownloadCount > 0 && store.downloadSpeed" class="subtabs-speed-pill">
        ⚡ {{ store.downloadSpeed }}
      </span>
    </div>

    <!-- Subview 1: Transfers & Downloads -->
    <div v-if="store.downloadedSubTab === 'transfers'" class="transfers-container flex-1 overflow-auto flex flex-col">
      <ProgressesPane />
    </div>

    <!-- Subview 2: Comics Library -->
    <div v-else class="library-body flex-1 overflow-auto flex flex-col">
      <!-- Active Download Toast Banner in Library view -->
      <div
        v-if="store.activeDownloadCount > 0"
        class="active-download-banner"
        @click="store.downloadedSubTab = 'transfers'">
        <div class="banner-left">
          <span class="banner-pulse-dot"></span>
          <span class="banner-text">后台正在下载 <strong>{{ store.activeDownloadCount }}</strong> 个章节</span>
          <span v-if="store.downloadSpeed" class="banner-speed">{{ store.downloadSpeed }}</span>
        </div>
        <span class="banner-arrow">查看进度 ›</span>
      </div>

      <section class="library-hero">
        <div>
          <h1>本地书库<span>{{ store.downloadedComics.length }} 本</span></h1>
        </div>
        <div class="hero-import">
          <n-button type="primary" :loading="importing" @click="importComics(false)"><template #icon><PhPlus :size="18" /></template>导入 CBZ</n-button>
          <n-button :loading="importing" @click="importComics(true)">导入文件夹</n-button>
        </div>
      </section>
      <section v-if="recentComics.length" class="continue-section">
        <div class="section-caption">继续阅读</div>
        <div class="continue-grid">
          <button v-for="comic in recentComics" :key="comic.id" @click="store.continueReading(comic)">
            <span class="continue-dot">↗</span><span><strong>{{ comic.name }}</strong><small>{{ store.readingState.progress[String(comic.id)].chapterTitle }} · 第 {{ store.readingState.progress[String(comic.id)].page }} 页</small></span>
          </button>
        </div>
      </section>
      <div class="library-toolbar">
        <PathField
          ref="pathFieldRef"
          v-model:value="store.config.downloadDir"
          label="漫画书库"
          @choose="selectLibraryDir"
          @open="showLibraryDirInFileManager" />
        <div class="library-toolbar-buttons"><n-button size="small" :loading="loading" @click="refreshLibrary">刷新书库</n-button><update-downloaded-comics-button /></div>
      </div>
      <div class="library-actions select-none">
        <div class="library-filters">
        <n-input
          v-model:value="searchQuery"
          class="library-search"
          size="small"
          clearable
          placeholder="搜索标题 / 作者 / 标签 / ID"
          aria-label="搜索书库" />
        <n-select v-model:value="libraryFilter" :options="filterOptions" class="library-select" aria-label="筛选书库" />
        <n-select v-model:value="sortOrder" :options="sortOptions" class="library-select" aria-label="书库排序" />
        </div>
        <div class="export-format-actions">
          <n-button
            v-if="isMobile"
            class="export-format-button"
            size="small"
            @click="toggleSelectAll">
            {{ checkedIds.size >= currentPageComics.length && currentPageComics.length > 0 ? '全不选' : '全选' }}
          </n-button>
          <n-button
            class="export-format-button"
            type="primary"
            secondary
            size="small"
            :disabled="checkedIds.size === 0"
            :loading="exporting"
            aria-label="导出为 CBZ"
            @click="exportCbz">
            <template #icon><PhFileZip :size="17" /></template>
            CBZ
          </n-button>
          <n-button
            class="export-format-button"
            type="primary"
            secondary
            size="small"
            v-if="checkedMergeableComics.length > 0"
            aria-label="整理已选漫画为整本 CBZ"
            title="多话合并为整本 CBZ，并清理分话文件"
            @click="mergeSelectedComics()">
            <template #icon><PhArchive :size="17" /></template>
            整理为整本（{{ checkedMergeableComics.length }} 本）
          </n-button>
          <n-button
            class="export-format-button"
            type="primary"
            secondary
            size="small"
            :disabled="checkedIds.size === 0"
            :loading="exporting"
            aria-label="导出为 PDF"
            @click="exportPdf">
            <template #icon><PhFilePdf :size="17" /></template>
            PDF
          </n-button>
          <n-button
            class="export-format-button delete-stock-button"
            type="error"
            size="small"
            :disabled="checkedIds.size === 0"
            aria-label="删除库存"
            @click="deleteDownloadedComics">
            <template #icon><PhTrash :size="17" /></template>
            删除
          </n-button>
        </div>
      </div>
      <SelectionArea
        class="flex flex-col overflow-auto box-border px-2 selection-container mb-2"
        ref="selectionAreaRef"
        :options="{ selectables: isMobile ? '' : '.selectable', features: { deselectOnBlur: true } }"
        @contextmenu="showDropdown"
        @move="updateSelectedIds"
        @start="unselectAll">
        <div v-if="loading && currentPageComics.length === 0" class="library-empty">
          <n-spin size="small" />
          <strong>正在读取本地库存…</strong>
        </div>
        <div v-else-if="currentPageComics.length === 0" class="library-empty">
          <PhArchive :size="30" weight="duotone" />
          <strong v-if="searchQuery.trim()">没有匹配「{{ searchQuery.trim() }}」的漫画</strong>
          <strong v-else>{{ libraryFilter === 'mergeable' ? '暂无可合并的多话漫画' : '暂无漫画' }}</strong>
          <span v-if="searchQuery.trim()">可尝试更短的关键词，或清空搜索框。</span>
          <span v-else-if="libraryFilter === 'mergeable'">需要同一本漫画至少两话已下载。</span>
        </div>
        <div class="comic-grid">
        <DownloadedComicCard
          v-for="comic in currentPageComics"
          :key="comic.id"
          :data-key="comic.id"
          :class="['selectable mb-2', selectedIds.has(comic.id) ? 'selected shadow-md' : 'hover:bg-gray-1']"
          :comic="comic"
          :checked="checkedIds.has(comic.id)"
          @toggle-check="handleCheckboxClick"
          @card-contextmenu="handleContextMenu" />
        </div>
      </SelectionArea>

      <n-pagination
        class="box-border p-2 pt-0 mt-auto"
        :page-count="pageCount"
        :page="currentPage"
        @update:page="currentPage = $event" />
    </div>

    <n-dropdown
      placement="bottom-start"
      trigger="manual"
      :x="dropdownX"
      :y="dropdownY"
      :options="dropdownOptions"
      :show="dropdownShowing"
      :on-clickoutside="closeDropdown" />

    <!-- 桌面端拖放导入提示层 -->
    <div v-if="dragActive" class="drop-overlay">
      <div class="drop-overlay-card">
        <PhPlus :size="36" weight="bold" />
        <strong>松手导入 CBZ</strong>
        <span>支持 .cbz 文件与漫画文件夹</span>
      </div>
    </div>

    <!-- 移动端导入 CBZ / 目录文件选择弹窗 -->
    <PathEditDialog
      v-model:show="showImportModal"
      :title="importMode === 'file' ? '选择漫画压缩包 (.cbz)' : '选择漫画文件夹'"
      :current-path="store.config?.downloadDir || ''"
      :mode="importMode"
      :create-on-confirm="false"
      :file-extensions="importMode === 'file' ? ['.cbz'] : []"
      @confirm="(path) => executeImport([path])" />
  </div>
</template>

<style scoped>
.library-page { background: #fff; }
.library-hero { display: flex; flex-shrink: 0; align-items: center; justify-content: space-between; gap: 16px; padding: 16px 20px; border-bottom: 1px solid #f0edf3; background: #fff; }
.library-hero h1 { margin: 0; color: #282739; font-size: 22px; }
.library-hero h1 span { margin-left: 12px; padding: 4px 9px; color: #ab657d; background: #fcebf2; font-size: 12px; border-radius: 7px; vertical-align: middle; letter-spacing: 0; }
.hero-import { display: flex; gap: 8px; flex-wrap: wrap; justify-content: flex-end; }
.continue-section { padding: 16px 20px 2px; }
.section-caption { color: #6b6170; font-size: 12px; font-weight: 700; margin-bottom: 8px; }
.continue-grid { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: 10px; }
.continue-grid button { display: flex; gap: 10px; align-items: center; min-width: 0; text-align: left; padding: 12px; background: #fff7fa; border: 1px solid #f4e3eb; border-radius: 12px; cursor: pointer; }
.continue-grid button > span:last-child { min-width: 0; }
.continue-grid strong { display: block; font-size: 12px; color: #514553; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.continue-grid small { display: block; color: #a28495; font-size: 10px; margin-top: 5px; }
.continue-dot { color: #c65e87; background: #f9dfeb; padding: 6px 10px; border-radius: 8px; }
.library-select { width: 125px; flex-shrink: 0; }
.library-search { width: 210px; flex-shrink: 1; min-width: 140px; }
.library-toolbar-buttons { display: flex; align-items: center; gap: 6px; }
.comic-grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(330px, 1fr)); gap: 12px; padding: 4px 6px 12px; }
@media (max-width: 760px) { .library-hero { padding: 14px; flex-wrap: wrap; } .hero-import { justify-content: flex-start; } .continue-grid { grid-template-columns: 1fr; } .continue-grid button:nth-child(n+2) { display: none; } .comic-grid { grid-template-columns: 1fr; } }
@media (max-height: 680px) { .library-hero { padding: 12px 20px; } .continue-section { display: none; } }
.library-filters {
  display: flex;
  align-items: center;
  gap: 10px;
}

.library-toolbar {
  display: grid;
  flex: 0 0 auto;
  grid-template-columns: minmax(0, 1fr) auto;
  gap: 8px;
  padding: 13px 13px 8px;
}

.library-actions {
  display: flex;
  min-height: 42px;
  flex: 0 0 auto;
  align-items: center;
  gap: 10px;
  padding: 1px 13px 8px;
}

.export-format-actions {
  display: flex;
  align-items: center;
  gap: 6px;
  margin-left: auto;
  flex-wrap: wrap;
}

.export-format-button {
  min-width: 60px;
  height: 32px;
  padding: 0 8px;
  font-size: 11px;
  font-weight: 700;
}

@media (max-width: 768px) {
  .library-toolbar {
    grid-template-columns: 1fr;
    gap: 8px;
  }

  .library-toolbar-buttons {
    display: flex;
    justify-content: flex-end;
  }

  .library-actions {
    flex-wrap: wrap;
    gap: 6px;
  }

  .export-format-actions {
    width: 100%;
    justify-content: flex-end;
  }

  /* 触屏：工具栏与批量操作按钮统一 44dp，避免小目标连点误触 */
  .library-toolbar-buttons :deep(.n-button),
  .export-format-actions :deep(.n-button),
  .library-filters :deep(.n-base-selection),
  .library-filters :deep(.n-input) {
    min-height: 44px;
  }

  /* 移动端搜索框独占一行，筛选与排序并排，避免宽度挤压 */
  .library-filters {
    width: 100%;
    flex-wrap: wrap;
  }

  .library-search {
    width: 100%;
    flex-basis: 100%;
  }

  .library-select {
    flex: 1 1 0;
    width: auto;
  }

  .subtab-btn {
    min-height: 44px;
  }

  .hero-import :deep(.n-button) {
    min-height: 44px;
  }
}

.selection-container {
  @apply select-none overflow-auto;
  flex: 1;
  min-height: 0;
}

.selection-container .selected {
  background-color: rgba(251, 114, 153, 0.12) !important;
  border-color: #fb7299 !important;
}

:global(.selection-area) {
  background-color: rgba(251, 114, 153, 0.18) !important;
  border: 1px solid #fb7299 !important;
}

.library-empty {
  display: flex;
  min-height: 240px;
  flex: 1;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  color: #9499a0;
  text-align: center;
}

.library-empty svg {
  margin-bottom: 12px;
  color: #fb7299;
}

.library-subtabs-bar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 10px 16px 8px;
  background: #ffffff;
  border-bottom: 1px solid #e3e5e7;
  gap: 12px;
}

.subtabs-segmented {
  display: flex;
  align-items: center;
  background: #f1f2f3;
  padding: 3px;
  border-radius: var(--radius-card);
  gap: 4px;
}

.subtab-btn {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 6px 14px;
  border-radius: var(--radius-control);
  border: 0;
  font-size: 13px;
  font-weight: 600;
  color: #61666d;
  background: transparent;
  cursor: pointer;
  transition: all 180ms ease;
}

.subtab-btn:hover {
  color: #18191c;
}

.subtab-btn.active {
  background: #ffffff;
  color: #fb7299;
  box-shadow: 0 2px 6px rgba(0, 0, 0, 0.06);
}

.subtab-btn.active.is-transfer {
  color: #00aeec;
}

.subtab-badge {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  min-width: 16px;
  height: 16px;
  padding: 0 4px;
  font-size: 10px;
  font-weight: 800;
  color: #ffffff;
  background: #00aeec;
  border-radius: 999px;
}

.subtab-active-dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: #00aeec;
  animation: pulse-dot 1.5s infinite;
}

.subtabs-speed-pill {
  font-size: 11px;
  font-weight: 700;
  color: #00aeec;
  background: rgba(0, 174, 236, 0.08);
  padding: 4px 10px;
  border-radius: 999px;
  font-family: monospace;
}

/* Active Download Toast Banner in Library view */
.active-download-banner {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin: 10px 16px 2px;
  padding: 10px 14px;
  background: linear-gradient(90deg, rgba(0, 174, 236, 0.08) 0%, rgba(251, 114, 153, 0.08) 100%);
  border: 1px solid rgba(0, 174, 236, 0.25);
  border-radius: 12px;
  cursor: pointer;
  transition: transform 150ms ease, background 150ms ease;
}

.active-download-banner:hover {
  background: linear-gradient(90deg, rgba(0, 174, 236, 0.12) 0%, rgba(251, 114, 153, 0.12) 100%);
}

.active-download-banner:active {
  transform: scale(0.99);
}

.banner-left {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 12px;
  color: #18191c;
}

.banner-pulse-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: #00aeec;
  animation: pulse-dot 1.5s infinite;
}

.banner-text strong {
  color: #00aeec;
}

.banner-speed {
  font-size: 11px;
  color: #61666d;
  font-family: monospace;
  background: #ffffff;
  padding: 1px 6px;
  border-radius: 4px;
}

.banner-arrow {
  font-size: 12px;
  font-weight: 600;
  color: #00aeec;
}

@keyframes pulse-dot {
  0% { transform: scale(0.9); opacity: 0.7; }
  50% { transform: scale(1.2); opacity: 1; }
  100% { transform: scale(0.9); opacity: 0.7; }
}

.drop-overlay {
  position: fixed;
  inset: 0;
  z-index: 3000;
  display: flex;
  align-items: center;
  justify-content: center;
  background: rgba(251, 114, 153, 0.08);
  backdrop-filter: blur(1px);
  pointer-events: none;
}

.drop-overlay-card {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 8px;
  padding: 36px 56px;
  color: #b73963;
  background: #ffffff;
  border: 2px dashed #fb7299;
  border-radius: 16px;
  box-shadow: 0 16px 48px rgba(0, 0, 0, 0.12);
}

.drop-overlay-card strong { font-size: 16px; color: #18191c; }
.drop-overlay-card span { font-size: 12px; color: #61666d; }

.transfers-container {
  min-height: 0;
  background: #ffffff;
}

</style>
