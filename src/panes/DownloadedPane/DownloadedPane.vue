<script setup lang="tsx">
import { type Comic, type LibraryDiagnosis, commands } from '../../bindings.ts'
import { computed, onActivated, onMounted, ref, watch, watchEffect } from 'vue'
import DownloadedComicCard from './components/DownloadedComicCard.vue'
import { useStore } from '../../store.ts'
import { NCheckbox } from 'naive-ui'
import {
  PhArchive,
  PhArrowClockwise,
  PhBookOpen,
  PhFileZip,
  PhFunnel,
  PhImage,
  PhMagnifyingGlass,
  PhPlus,
  PhTrash,
  PhWarningCircle,
  PhX,
} from '@phosphor-icons/vue'
import ProgressesPane from '../ProgressesPane/ProgressesPane.vue'
import UpdateDownloadedComicsButton from './components/UpdateDownloadedComicsButton.vue'
import PathField from '../../components/PathField.vue'
import { useDialog, useMessage } from 'naive-ui'
import { getErrorMessage } from '../../utils/errors.ts'
import { hasAllFilesAccess, requestAllFilesAccessAndWait, withAndroidSelection } from '../../utils/androidStorage.ts'
import { isAndroid } from '../../composables/useIsMobile.ts'
import { filterLibrary } from '../../utils/librarySearch.ts'
import { libraryApi } from '../../libraryApi.ts'
import { isMergeableComic } from '../../utils/mergeableComics.ts'

const store = useStore()
const message = useMessage()
const dialog = useDialog()

const checkedIds = ref<Set<number>>(new Set())
const loading = ref(false)
const importing = ref(false)
const exporting = ref(false)
const searchQuery = ref('')
const libraryFilter = ref<'all' | 'reading' | 'bookmarks' | 'imported' | 'mergeable'>('all')
const sortOrder = ref<'added' | 'read' | 'title'>('added')
const filterSheetOpen = ref(false)

const PAGE_SIZE = 24

const mergeableComics = computed(() => store.downloadedComics.filter(isMergeableComic))
const checkedComics = computed(() => currentPageComics.value.filter((comic) => checkedIds.value.has(comic.id)))
const checkedMergeableComics = computed(() => checkedComics.value.filter(isMergeableComic))

const filters = computed(() => [
  { value: 'all' as const, label: '全部', count: store.downloadedComics.length },
  { value: 'reading' as const, label: '在读', count: store.downloadedComics.filter((comic) => !!store.readingState.progress[String(comic.id)]).length },
  { value: 'bookmarks' as const, label: '有书签', count: store.downloadedComics.filter((comic) => store.readingState.bookmarks.some((bookmark) => bookmark.comicId === comic.id)).length },
  { value: 'imported' as const, label: '本地导入', count: store.downloadedComics.filter((comic) => comic.id < 0).length },
  { value: 'mergeable' as const, label: '可整理', count: mergeableComics.value.length },
])

const sortOptions = [
  { value: 'added' as const, label: '最近加入' },
  { value: 'read' as const, label: '最近阅读' },
  { value: 'title' as const, label: '标题' },
]

const activeFilterLabel = computed(() => filters.value.find((filter) => filter.value === libraryFilter.value)?.label ?? '全部')

const filteredComics = computed(() => {
  let comics = store.downloadedComics
  if (libraryFilter.value === 'mergeable') comics = comics.filter(isMergeableComic)
  else if (libraryFilter.value === 'reading') comics = comics.filter((comic) => !!store.readingState.progress[String(comic.id)])
  else if (libraryFilter.value === 'bookmarks') comics = comics.filter((comic) => store.readingState.bookmarks.some((bookmark) => bookmark.comicId === comic.id))
  else if (libraryFilter.value === 'imported') comics = comics.filter((comic) => comic.id < 0)

  comics = filterLibrary(comics, searchQuery.value)
  if (sortOrder.value === 'title') return [...comics].sort((a, b) => a.name.localeCompare(b.name, 'zh-CN', { numeric: true }))
  if (sortOrder.value === 'read') {
    return [...comics].sort(
      (a, b) => (store.readingState.progress[String(b.id)]?.updatedAt ?? 0) - (store.readingState.progress[String(a.id)]?.updatedAt ?? 0),
    )
  }
  return comics
})

const currentPage = ref(1)
const pageCount = computed(() => Math.max(1, Math.ceil(filteredComics.value.length / PAGE_SIZE)))
const currentPageComics = computed(() => {
  const start = (currentPage.value - 1) * PAGE_SIZE
  return filteredComics.value.slice(start, start + PAGE_SIZE)
})
const allChecked = computed(() => currentPageComics.value.length > 0 && currentPageComics.value.every((comic) => checkedIds.value.has(comic.id)))
const selectionMode = computed(() => checkedIds.value.size > 0)

watchEffect(() => {
  if (currentPage.value > pageCount.value) currentPage.value = pageCount.value
})

watch([libraryFilter, sortOrder, searchQuery], () => {
  currentPage.value = 1
  checkedIds.value.clear()
})

watch(currentPage, () => checkedIds.value.clear())

function toggleSelectAll() {
  if (allChecked.value) {
    currentPageComics.value.forEach((comic) => checkedIds.value.delete(comic.id))
  } else {
    currentPageComics.value.forEach((comic) => checkedIds.value.add(comic.id))
  }
}

function toggleCheck(comic: Comic) {
  if (checkedIds.value.has(comic.id)) checkedIds.value.delete(comic.id)
  else checkedIds.value.add(comic.id)
}

function clearSelection() {
  checkedIds.value.clear()
}

async function importComics(directory = false) {
  if (importing.value) return
  // Android 通过 SAF 选择文件/目录；应用只面向 Android，不再有桌面分支。
  try {
    await withAndroidSelection(directory ? 'directory' : 'cbz', executeImport)
  } catch (error) {
    message.error(`选择文件失败：${getErrorMessage(error)}`)
  }
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

// 书库为空或被跳过了文件时，后端能给出具体原因（目录不存在/不可读/文件损坏）。
// 只显示「暂无漫画」时用户无从判断是没下载好、位置不对还是权限丢了。
const diagnosis = ref<LibraryDiagnosis | null>(null)
const dismissedDiagnosis = ref('')

const partialWarning = computed(() => {
  const value = diagnosis.value
  if (value?.kind !== 'Partial' || value.message === dismissedDiagnosis.value) return null
  return value
})

async function updateDiagnosis() {
  try {
    diagnosis.value = await commands.diagnoseLibrary()
  } catch (error) {
    console.error('书库诊断失败', error)
    diagnosis.value = null
  }
}

async function refreshLibrary() {
  loading.value = true
  try {
    await store.refreshDownloadedComics()
    await updateDiagnosis()
  } catch (error) {
    message.error(`刷新书库失败：${getErrorMessage(error)}`)
  } finally {
    loading.value = false
  }
}

/** 诊断提示里的「授权后重试」：申请「所有文件访问」并重新扫描。 */
async function grantStorageAndRetry() {
  const granted = await requestAllFilesAccessAndWait()
  if (!granted) {
    message.warning('未获得「所有文件访问」权限，书库仍可能无法读取')
  }
  await refreshLibrary()
}

function openStorageSettings() {
  store.openSettings()
}

function dismissPartialWarning() {
  dismissedDiagnosis.value = diagnosis.value?.message ?? ''
}

watch(
  () => store.authoritativeConfig?.downloadDir,
  (path, previous) => {
    if (!previous || path === previous) return
    checkedIds.value.clear()
    store.downloadedComics = []
    void refreshLibrary()
  },
)

// 书库页在 KeepAlive 下常驻，onMounted 只会执行一次；若用户在其他页下载完再切回来，
// 列表会一直停在上次扫描的结果。每次进入书库页都重新扫描磁盘，保证与文件一致。
function refreshIfStale() {
  if (!store.downloadedComicsStale) return
  void refreshLibrary()
}

onMounted(() => {
  if (!store.downloadedComicsLoaded || store.downloadedComicsStale) void refreshLibrary()
})
onActivated(() => {
  refreshIfStale()
  if (store.downloadedSubTab === 'library' && !store.downloadedComicsLoaded) void refreshLibrary()
})

watch(
  () => store.currentTabName,
  async () => {
    if (store.currentTabName !== 'downloaded') return
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

// 批量导出：失败不中断，结束后汇总提示；整个过程中禁用按钮防止重复提交
async function runBatchExport() {
  if (checkedIds.value.size === 0 || exporting.value) return
  try {
    await store.saveConfigNow()
  } catch (error) {
    message.error(getErrorMessage(error))
    return
  }
  const comics = checkedComics.value
  if (comics.length === 0) return
  store.progressesPaneTabName = 'export'
  store.downloadedSubTab = 'transfers'
  exporting.value = true
  const failures: string[] = []
  try {
    for (const comic of comics) {
      try {
        const result = await commands.exportCbz(comic)
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
  checkedIds.value.clear()
  if (failures.length === 0) {
    message.success(`已提交 ${comics.length} 本导出`)
  } else {
    message.warning(`${failures.length}/${comics.length} 本导出失败：${failures.slice(0, 3).join('；')}${failures.length > 3 ? '…' : ''}`)
  }
}

async function mergeSelectedComics(comics: Comic[] = checkedMergeableComics.value) {
  if (comics.length === 0) return

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
  checkedIds.value.clear()
}

async function deleteDownloadedComics() {
  if (checkedIds.value.size === 0) return

  const comics = checkedComics.value
  const deleteFiles = ref(false)
  dialog.warning({
    title: `删除 ${comics.length} 本漫画`,
    content: () => (
      <div>
        <p>仅删除库存记录，阅读进度与书签会同步清理。</p>
        <NCheckbox checked={deleteFiles.value} onUpdateChecked={(value) => { deleteFiles.value = value }}>
          同时删除本地文件
        </NCheckbox>
        {deleteFiles.value && <p style="color: #DE3B3B">本地文件删除后无法恢复</p>}
      </div>
    ),
    positiveText: '删除',
    negativeText: '取消',
    onPositiveClick: async () => {
      let failedCount = 0
      for (const comic of comics) {
        try {
          await libraryApi.removeComic(comic.id, deleteFiles.value)
          if (store.activeReaderTarget?.comicId === comic.id) store.closeReader()
          store.forgetComicReadingState(comic.id)
        } catch (error) {
          message.error(getErrorMessage(error))
          failedCount += 1
        }
      }
      checkedIds.value.clear()
      await store.refreshDownloadedComics()
      if (failedCount > 0) message.warning(`${failedCount} 本未能删除`)
      else message.success('已删除')
    },
  })
}

function openTransfers() {
  store.openTransfers()
}

const pendingCount = computed(() => {
  let count = 0
  for (const progress of store.progresses.values()) {
    if (progress.state !== 'Completed' && progress.state !== 'Cancelled') count++
  }
  return count
})

const readingCount = computed(() => Object.keys(store.readingState.progress).length)

function percentOf(comic: Comic) {
  const progress = store.readingState.progress[String(comic.id)]
  if (!progress || progress.totalPages <= 0) return 0
  return Math.min(100, Math.max(1, Math.round((progress.page / progress.totalPages) * 100)))
}

function continueComic(comic: Comic) {
  store.continueReading(comic)
}
</script>

<template>
  <div v-if="store.config !== undefined" class="library-page">
    <!-- 分段控件：已入库 / 传输 -->
    <div class="library-segment-bar">
      <div class="segment" role="tablist" aria-label="书库视图">
        <button
          type="button"
          role="tab"
          class="segment-btn"
          :class="{ 'is-active': store.downloadedSubTab === 'library' }"
          :aria-selected="store.downloadedSubTab === 'library'"
          @click="store.downloadedSubTab = 'library'">
          已入库
          <span class="segment-count">{{ store.downloadedComics.length }}</span>
        </button>
        <button
          type="button"
          role="tab"
          class="segment-btn"
          :class="{ 'is-active': store.downloadedSubTab === 'transfers' }"
          :aria-selected="store.downloadedSubTab === 'transfers'"
          @click="store.downloadedSubTab = 'transfers'">
          传输
          <span v-if="pendingCount > 0" class="segment-badge">{{ pendingCount }}</span>
        </button>
      </div>
      <button
        v-if="store.downloadedSubTab === 'library'"
        type="button"
        class="jm-icon-btn"
        :disabled="loading"
        aria-label="刷新书库"
        title="刷新书库"
        @click="refreshLibrary">
        <PhArrowClockwise :size="19" :class="{ 'jm-spin': loading }" />
      </button>
    </div>

    <!-- 传输页 -->
    <div v-if="store.downloadedSubTab === 'transfers'" class="transfers-wrap">
      <ProgressesPane />
    </div>

    <template v-else>
      <!-- 正在下载提示条 -->
      <button v-if="store.activeDownloadCount > 0" type="button" class="live-strip" @click="openTransfers">
        <span class="live-dot" aria-hidden="true"></span>
        <span class="live-copy">
          正在下载 <strong>{{ store.activeDownloadCount }}</strong> 个章节
          <em v-if="store.downloadSpeed">{{ store.downloadSpeed }}</em>
        </span>
        <span class="live-more">查看 ›</span>
      </button>

      <div v-if="partialWarning" class="inline-warning" role="status">
        <PhWarningCircle :size="17" weight="fill" aria-hidden="true" />
        <div>
          <strong>{{ partialWarning.message }}</strong>
          <ul v-if="partialWarning.skipped.length">
            <li v-for="item in partialWarning.skipped" :key="item">{{ item }}</li>
          </ul>
        </div>
        <button type="button" class="warning-close" aria-label="关闭提示" @click="dismissPartialWarning">
          <PhX :size="15" weight="bold" />
        </button>
      </div>

      <!-- 继续阅读横幅 -->
      <button
        v-if="store.downloadedComics.length > 0 && readingCount > 0"
        type="button"
        class="resume-banner"
        @click="(() => { const next = store.downloadedComics.filter((c) => store.readingState.progress[String(c.id)]).sort((a, b) => (store.readingState.progress[String(b.id)]?.updatedAt ?? 0) - (store.readingState.progress[String(a.id)]?.updatedAt ?? 0))[0]; if (next) continueComic(next) })()">
        <span class="resume-icon"><PhBookOpen :size="20" weight="duotone" /></span>
        <span class="resume-copy">
          <strong>继续阅读</strong>
          <span>{{ (() => { const next = store.downloadedComics.filter((c) => store.readingState.progress[String(c.id)]).sort((a, b) => (store.readingState.progress[String(b.id)]?.updatedAt ?? 0) - (store.readingState.progress[String(a.id)]?.updatedAt ?? 0))[0]; return next ? `${next.name} · ${store.readingState.progress[String(next.id)]?.chapterTitle ?? ''} ${percentOf(next)}%` : '' })() }}</span>
        </span>
        <span class="resume-go">继续 ›</span>
      </button>

      <!-- 搜索 + 筛选 -->
      <div class="library-tools">
        <div class="search-box">
          <PhMagnifyingGlass :size="17" weight="bold" class="search-ico" aria-hidden="true" />
          <input
            v-model="searchQuery"
            class="search-input"
            type="search"
            placeholder="搜索书名、作者或 JM 号"
            aria-label="搜索书库" />
          <button v-if="searchQuery" type="button" class="search-clear" aria-label="清空搜索" @click="searchQuery = ''">
            <PhX :size="14" weight="bold" />
          </button>
        </div>
        <button
          type="button"
          class="filter-trigger"
          :class="{ 'is-active': libraryFilter !== 'all' }"
          :aria-expanded="filterSheetOpen"
          aria-label="筛选与排序"
          @click="filterSheetOpen = !filterSheetOpen">
          <PhFunnel :size="16" weight="bold" />
          <span>{{ activeFilterLabel }}</span>
        </button>
        <button type="button" class="jm-icon-btn add-btn" aria-label="导入漫画" title="导入漫画" :disabled="importing" @click="importComics(false)">
          <PhPlus :size="19" weight="bold" />
        </button>
      </div>

      <!-- 筛选与排序抽屉 -->
      <div v-if="filterSheetOpen" class="filter-sheet">
        <div class="filter-group">
          <span class="filter-label">筛选</span>
          <div class="filter-chips">
            <button
              v-for="filter in filters"
              :key="filter.value"
              type="button"
              class="filter-chip"
              :class="{ 'is-active': libraryFilter === filter.value }"
              @click="libraryFilter = filter.value">
              {{ filter.label }}
              <span class="chip-count">{{ filter.count }}</span>
            </button>
          </div>
        </div>
        <div class="filter-group">
          <span class="filter-label">排序</span>
          <div class="filter-chips">
            <button
              v-for="option in sortOptions"
              :key="option.value"
              type="button"
              class="filter-chip"
              :class="{ 'is-active': sortOrder === option.value }"
              @click="sortOrder = option.value">
              {{ option.label }}
            </button>
          </div>
        </div>
        <div class="filter-group">
          <span class="filter-label">书库位置</span>
          <PathField v-model:value="store.config.downloadDir" label="书库" />
          <div class="filter-extra">
            <UpdateDownloadedComicsButton />
            <button type="button" class="jm-link" @click="openStorageSettings">更改位置</button>
          </div>
        </div>
      </div>

      <!-- 选择模式工具条 -->
      <div v-if="selectionMode" class="selection-bar">
        <button type="button" class="sel-btn" @click="clearSelection">
          <PhX :size="16" weight="bold" />
          退出
        </button>
        <span class="sel-count">已选 {{ checkedIds.size }} 本</span>
        <button type="button" class="sel-icon" :aria-label="allChecked ? '取消全选' : '全选'" :title="allChecked ? '取消全选' : '全选'" @click="toggleSelectAll">
          {{ allChecked ? '取消全选' : '全选' }}
        </button>
        <button
          v-if="checkedMergeableComics.length > 0"
          type="button"
          class="sel-icon is-brand"
          title="多话合并为整本 CBZ，并清理分话文件"
          @click="mergeSelectedComics()">
          <PhArchive :size="17" />
          整理
        </button>
        <button type="button" class="sel-icon is-accent" :disabled="exporting" title="导出为 CBZ" @click="runBatchExport">
          <PhFileZip :size="17" />
          导出
        </button>
        <button type="button" class="sel-icon is-danger" title="删除库存" @click="deleteDownloadedComics">
          <PhTrash :size="17" />
        </button>
      </div>

      <div class="jm-scroll library-scroll">
        <!-- 空状态（带诊断） -->
        <div v-if="loading && currentPageComics.length === 0" class="library-grid is-loading">
          <div v-for="n in 6" :key="n" class="skeleton-card">
            <div class="jm-skeleton skeleton-cover"></div>
            <div class="jm-skeleton skeleton-line"></div>
            <div class="jm-skeleton skeleton-line is-short"></div>
          </div>
        </div>

        <div v-else-if="currentPageComics.length === 0" class="jm-empty">
          <div class="jm-empty-icon is-muted">
            <PhMagnifyingGlass v-if="searchQuery.trim()" :size="28" weight="duotone" />
            <PhArchive v-else :size="28" weight="duotone" />
          </div>
          <strong v-if="searchQuery.trim()">没有匹配「{{ searchQuery.trim() }}」的漫画</strong>
          <strong v-else-if="libraryFilter !== 'all'">这个筛选下还没有漫画</strong>
          <strong v-else>书库还是空的</strong>
          <p v-if="searchQuery.trim()">试试更短的关键词，或清空搜索框。</p>
          <p v-else-if="libraryFilter === 'mergeable'">同一本漫画至少下载两话后，才能整理为整本。</p>
          <template v-else-if="diagnosis">
            <p>{{ diagnosis.message }}</p>
            <ul v-if="diagnosis.skipped.length" class="diagnosis-list">
              <li v-for="item in diagnosis.skipped" :key="item">{{ item }}</li>
            </ul>
          </template>
          <p v-else>从「发现」下载漫画，或导入本地的 CBZ 文件。</p>
          <div class="jm-empty-actions">
            <template v-if="diagnosis && !searchQuery.trim() && libraryFilter === 'all'">
              <button v-if="isAndroid && !hasAllFilesAccess()" type="button" class="jm-btn is-primary" @click="grantStorageAndRetry">
                授予存储权限
              </button>
              <button type="button" class="jm-btn" :disabled="loading" @click="refreshLibrary">重新扫描</button>
              <button type="button" class="jm-btn" @click="openStorageSettings">更改位置</button>
            </template>
            <template v-else>
              <button type="button" class="jm-btn is-primary" @click="importComics(false)">
                <PhImage :size="17" />
                导入 CBZ
              </button>
              <button type="button" class="jm-btn" @click="importComics(true)">导入文件夹</button>
            </template>
          </div>
        </div>

        <!-- 封面墙 -->
        <div v-else class="library-grid">
          <DownloadedComicCard
            v-for="comic in currentPageComics"
            :key="comic.id"
            :comic="comic"
            :checked="checkedIds.has(comic.id)"
            :selection-mode="selectionMode"
            @toggle-check="toggleCheck"
            @long-press="toggleCheck" />
        </div>

        <nav v-if="pageCount > 1" class="pager" aria-label="书库分页">
          <button type="button" class="pager-btn" :disabled="currentPage <= 1" aria-label="上一页" @click="currentPage--">‹</button>
          <span class="pager-info">{{ currentPage }} / {{ pageCount }}</span>
          <button type="button" class="pager-btn" :disabled="currentPage >= pageCount" aria-label="下一页" @click="currentPage++">›</button>
        </nav>
      </div>
    </template>
  </div>
</template>

<style scoped>
.library-page {
  display: flex;
  height: 100%;
  min-height: 0;
  flex-direction: column;
  background: var(--jm-surface);
}

/* 顶部视图切换 */
.library-segment-bar {
  display: flex;
  flex: 0 0 auto;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
  padding: 10px var(--jm-gutter) 8px;
  background: var(--jm-surface);
}

.segment {
  display: flex;
  gap: 3px;
  padding: 3px;
  border-radius: var(--jm-r-md);
  background: var(--jm-sunken);
}

.segment-btn {
  display: inline-flex;
  height: 36px;
  align-items: center;
  gap: 6px;
  padding: 0 15px;
  color: var(--jm-text-2);
  border: 0;
  border-radius: var(--jm-r-sm);
  background: transparent;
  font-size: 13.5px;
  font-weight: 650;
  cursor: pointer;
  transition:
    color 160ms ease,
    background-color 160ms ease,
    box-shadow 160ms ease;
}

.segment-btn.is-active {
  color: var(--jm-brand);
  background: var(--jm-surface);
  box-shadow: 0 1px 4px rgba(23, 24, 28, 0.08);
}

.segment-count {
  color: var(--jm-text-3);
  font-size: 10.5px;
  font-weight: 700;
  font-variant-numeric: tabular-nums;
}

.segment-btn.is-active .segment-count {
  color: var(--jm-brand);
}

.segment-badge {
  display: inline-flex;
  min-width: 17px;
  height: 17px;
  align-items: center;
  justify-content: center;
  padding: 0 4px;
  color: #fff;
  border-radius: var(--jm-r-pill);
  background: var(--jm-accent);
  font-size: 9.5px;
  font-weight: 800;
  font-variant-numeric: tabular-nums;
}

.transfers-wrap {
  min-height: 0;
  flex: 1;
  overflow: hidden;
}

/* 下载中提示条 */
.live-strip {
  display: flex;
  flex: 0 0 auto;
  align-items: center;
  gap: 8px;
  margin: 0 var(--jm-gutter) 8px;
  padding: 10px 13px;
  text-align: left;
  border: 0;
  border-radius: var(--jm-r-md);
  background: linear-gradient(100deg, rgba(0, 174, 236, 0.1), rgba(251, 114, 153, 0.08));
  cursor: pointer;
}

.live-dot {
  width: 7px;
  height: 7px;
  flex: 0 0 auto;
  border-radius: 50%;
  background: var(--jm-accent);
  animation: live-pulse 1.6s ease-in-out infinite;
}

@keyframes live-pulse {
  0%,
  100% {
    opacity: 1;
    transform: scale(1);
  }
  50% {
    opacity: 0.5;
    transform: scale(0.8);
  }
}

.live-copy {
  display: flex;
  min-width: 0;
  flex: 1;
  align-items: baseline;
  gap: 4px;
  color: var(--jm-text-2);
  font-size: var(--jm-fs-sm);
}

.live-copy strong {
  color: var(--jm-accent);
  font-variant-numeric: tabular-nums;
}

.live-copy em {
  overflow: hidden;
  color: var(--jm-text-3);
  font-family: ui-monospace, 'SF Mono', Menlo, monospace;
  font-size: 10.5px;
  font-style: normal;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.live-more {
  flex: 0 0 auto;
  color: var(--jm-accent);
  font-size: var(--jm-fs-sm);
  font-weight: 700;
}

/* 诊断提示 */
.inline-warning {
  display: flex;
  flex: 0 0 auto;
  align-items: flex-start;
  gap: 8px;
  margin: 0 var(--jm-gutter) 8px;
  padding: 10px 12px;
  color: var(--jm-warning);
  border-radius: var(--jm-r-md);
  background: var(--jm-warning-soft);
  font-size: var(--jm-fs-sm);
}

.inline-warning > div {
  min-width: 0;
  flex: 1;
}

.inline-warning strong {
  color: var(--jm-text);
  font-weight: 650;
}

.inline-warning ul {
  margin: 6px 0 0;
  padding-left: 16px;
  color: var(--jm-text-2);
  font-size: var(--jm-fs-xs);
  line-height: 1.5;
}

.warning-close {
  display: grid;
  width: 24px;
  height: 24px;
  flex: 0 0 auto;
  padding: 0;
  color: var(--jm-text-3);
  border: 0;
  border-radius: var(--jm-r-pill);
  background: transparent;
  place-items: center;
  cursor: pointer;
}

/* 继续阅读横幅 */
.resume-banner {
  display: flex;
  flex: 0 0 auto;
  align-items: center;
  gap: 11px;
  margin: 0 var(--jm-gutter) 10px;
  padding: 11px 13px;
  text-align: left;
  border: 1px solid rgba(251, 114, 153, 0.22);
  border-radius: var(--jm-r-lg);
  background: linear-gradient(105deg, #FFF4F8 0%, #FFFFFF 72%);
  cursor: pointer;
}

.resume-banner:active {
  transform: scale(0.99);
}

.resume-icon {
  display: grid;
  width: 38px;
  height: 38px;
  flex: 0 0 auto;
  color: var(--jm-brand);
  place-items: center;
  border-radius: var(--jm-r-md);
  background: var(--jm-surface);
  box-shadow: var(--jm-shadow-1);
}

.resume-copy {
  display: flex;
  min-width: 0;
  flex: 1;
  flex-direction: column;
  gap: 2px;
}

.resume-copy strong {
  color: var(--jm-text);
  font-size: 13.5px;
  font-weight: 700;
}

.resume-copy span {
  overflow: hidden;
  color: var(--jm-text-3);
  font-size: 11px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.resume-go {
  flex: 0 0 auto;
  color: var(--jm-brand);
  font-size: var(--jm-fs-sm);
  font-weight: 700;
}

/* 搜索 / 筛选 */
.library-tools {
  display: flex;
  flex: 0 0 auto;
  align-items: center;
  gap: 7px;
  padding: 0 var(--jm-gutter) 10px;
  border-bottom: 1px solid var(--jm-line);
}

.search-box {
  display: flex;
  height: 40px;
  min-width: 0;
  flex: 1;
  align-items: center;
  gap: 7px;
  padding: 0 12px;
  border: 1px solid transparent;
  border-radius: var(--jm-r-md);
  background: var(--jm-sunken);
  transition:
    border-color 180ms ease,
    background-color 180ms ease;
}

.search-box:focus-within {
  border-color: var(--jm-brand);
  background: var(--jm-surface);
}

.search-ico {
  flex: 0 0 auto;
  color: var(--jm-text-3);
}

.search-input {
  min-width: 0;
  flex: 1;
  height: 100%;
  padding: 0;
  color: var(--jm-text);
  border: 0;
  outline: none;
  background: transparent;
  font-size: 13.5px;
}

.search-input::placeholder {
  color: var(--jm-text-3);
}

.search-input::-webkit-search-cancel-button {
  display: none;
}

.search-clear {
  display: grid;
  width: 20px;
  height: 20px;
  flex: 0 0 auto;
  padding: 0;
  color: var(--jm-text-3);
  border: 0;
  border-radius: var(--jm-r-pill);
  background: rgba(23, 24, 28, 0.07);
  place-items: center;
  cursor: pointer;
}

.filter-trigger {
  display: inline-flex;
  height: 40px;
  flex: 0 0 auto;
  align-items: center;
  gap: 5px;
  padding: 0 12px;
  color: var(--jm-text-2);
  border: 1px solid var(--jm-line);
  border-radius: var(--jm-r-md);
  background: var(--jm-surface);
  font-size: var(--jm-fs-sm);
  font-weight: 650;
  cursor: pointer;
}

.filter-trigger.is-active {
  color: var(--jm-brand);
  border-color: rgba(251, 114, 153, 0.35);
  background: var(--jm-brand-soft);
}

.add-btn {
  border: 1px solid var(--jm-line);
  border-radius: var(--jm-r-md);
}

.filter-sheet {
  display: flex;
  flex: 0 0 auto;
  flex-direction: column;
  gap: 12px;
  padding: 12px var(--jm-gutter);
  border-bottom: 1px solid var(--jm-line);
  background: var(--jm-surface);
}

.filter-group {
  display: flex;
  flex-direction: column;
  gap: 7px;
}

.filter-label {
  color: var(--jm-text-3);
  font-size: 10.5px;
  font-weight: 700;
  letter-spacing: 0.04em;
}

.filter-chips {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
}

.filter-chip {
  display: inline-flex;
  height: 32px;
  align-items: center;
  gap: 5px;
  padding: 0 12px;
  color: var(--jm-text-2);
  border: 1px solid transparent;
  border-radius: var(--jm-r-pill);
  background: var(--jm-sunken);
  font-size: var(--jm-fs-sm);
  font-weight: 600;
  cursor: pointer;
}

.filter-chip.is-active {
  color: var(--jm-brand);
  border-color: rgba(251, 114, 153, 0.28);
  background: var(--jm-brand-soft);
}

.chip-count {
  color: var(--jm-text-4);
  font-size: 10px;
  font-weight: 700;
  font-variant-numeric: tabular-nums;
}

.filter-chip.is-active .chip-count {
  color: rgba(251, 114, 153, 0.72);
}

.filter-extra {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
}

.filter-extra :deep(.n-button) {
  height: 34px;
  border-radius: var(--jm-r-pill);
}

/* 选择模式 */
.selection-bar {
  display: flex;
  flex: 0 0 auto;
  align-items: center;
  gap: 6px;
  padding: 8px var(--jm-gutter);
  border-bottom: 1px solid var(--jm-line);
  background: var(--jm-surface);
  overflow-x: auto;
  scrollbar-width: none;
}

.selection-bar::-webkit-scrollbar {
  display: none;
}

.sel-btn,
.sel-icon {
  display: inline-flex;
  height: 34px;
  flex: 0 0 auto;
  align-items: center;
  gap: 4px;
  padding: 0 11px;
  color: var(--jm-text-2);
  border: 1px solid var(--jm-line);
  border-radius: var(--jm-r-pill);
  background: var(--jm-surface);
  font-size: 12px;
  font-weight: 650;
  white-space: nowrap;
  cursor: pointer;
}

.sel-count {
  flex: 0 0 auto;
  padding: 0 4px;
  color: var(--jm-brand);
  font-size: 12px;
  font-weight: 700;
  font-variant-numeric: tabular-nums;
}

.sel-icon.is-brand {
  color: var(--jm-brand);
  border-color: rgba(251, 114, 153, 0.3);
  background: var(--jm-brand-soft);
}

.sel-icon.is-accent {
  color: var(--jm-accent);
  border-color: rgba(0, 174, 236, 0.26);
  background: var(--jm-accent-soft);
}

.sel-icon.is-danger {
  margin-left: auto;
  color: var(--jm-danger);
  border-color: rgba(222, 59, 59, 0.2);
  background: var(--jm-danger-soft);
}

.sel-icon:disabled {
  opacity: 0.5;
  cursor: wait;
}

/* 封面墙 */
.library-scroll {
  background: var(--jm-bg);
  padding-bottom: 24px;
}

.library-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(104px, 1fr));
  gap: 16px 10px;
  padding: 14px var(--jm-gutter) 8px;
}

@media (min-width: 720px) {
  .library-grid {
    grid-template-columns: repeat(auto-fill, minmax(126px, 1fr));
    gap: 20px 14px;
  }
}

.skeleton-card {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.skeleton-cover {
  aspect-ratio: 3 / 4;
  border-radius: var(--jm-r-md);
}

.skeleton-line {
  height: 12px;
  border-radius: 4px;
}

.skeleton-line.is-short {
  width: 55%;
}

.diagnosis-list {
  margin: 8px 0 0;
  padding: 0 0 0 16px;
  text-align: left;
  color: var(--jm-text-2);
  font-size: var(--jm-fs-xs);
  line-height: 1.55;
  word-break: break-all;
}

.pager {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 18px;
  padding: 8px var(--jm-gutter) 20px;
}

.pager-btn {
  display: grid;
  width: 42px;
  height: 42px;
  place-items: center;
  padding: 0;
  color: var(--jm-text-2);
  border: 1px solid var(--jm-line);
  border-radius: var(--jm-r-pill);
  background: var(--jm-surface);
  font-size: 20px;
  line-height: 1;
  cursor: pointer;
}

.pager-btn:disabled {
  color: var(--jm-text-4);
  cursor: default;
}

.pager-info {
  min-width: 58px;
  color: var(--jm-text-2);
  font-size: var(--jm-fs-sm);
  font-weight: 600;
  text-align: center;
  font-variant-numeric: tabular-nums;
}
</style>
