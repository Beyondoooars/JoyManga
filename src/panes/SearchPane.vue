<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import {
  commands,
  type ComicInSearch,
  type SearchResultVariant,
  type SearchSort,
} from '../bindings.ts'
import { useMessage } from 'naive-ui'
import ComicCard from '../components/ComicCard.vue'
import WeeklyPane from './WeeklyPane.vue'
import ContinueReadingRow from '../components/ContinueReadingRow.vue'
import {
  PhArrowLeft,
  PhArrowRight,
  PhClockCounterClockwise,
  PhMagnifyingGlass,
  PhSparkle,
  PhX,
} from '@phosphor-icons/vue'
import { useStore } from '../store.ts'
import { getErrorMessage } from '../utils/errors.ts'
import {
  SearchSession,
  SEARCH_PAGE_SIZE,
  type DualSearchRequest,
  type DualSearchResponse,
  type SearchVariantPage,
} from '../utils/searchSession.ts'
import { LANGUAGE_FILTER_LABELS, type SearchFilter } from '../utils/searchLanguage.ts'

const store = useStore()
const message = useMessage()

type DiscoverView = 'weekly' | 'search'
// 发现页默认展示每周必看；搜索时临时切到结果，清空搜索后回到榜单
const view = ref<DiscoverView>('weekly')

const sortOptions: Array<{ label: string; value: SearchSort }> = [
  { label: '最新', value: 'Latest' },
  { label: '最多点击', value: 'View' },
  { label: '最多图片', value: 'Picture' },
  { label: '最多爱心', value: 'Like' },
]

const searchInput = ref<string>(store.searchResult?.searchQuery ?? '')
const searching = ref(false)
const sortSelected = ref<SearchSort>('Latest')
const searchPage = ref<number>(1)
const committedKeyword = ref(store.searchResult?.searchQuery ?? '')
const committedSort = ref<SearchSort>('Latest')
const languageFilter = ref<SearchFilter>('all')
// SearchSession 是普通对象（不依赖 Vue 响应式），每次取页/筛选后递增版本号触发重算
const sessionRevision = ref(0)
let activeSession: SearchSession | null = null
let searchGeneration = 0

const languageFilters: Array<{ label: string; value: SearchFilter }> = [
  { label: '全部', value: 'all' },
  { label: '中文', value: 'zh' },
  { label: '日文', value: 'ja' },
  { label: '英文', value: 'en' },
  { label: '韩文', value: 'ko' },
  { label: '全彩', value: 'fullColor' },
]

// SearchSession 是普通对象：computed 通过读取版本号来建立依赖，会话更新后由 bump 触发重算
function sessionVersion(): number {
  return sessionRevision.value
}

const currentSession = computed(() => {
  sessionVersion()
  return activeSession
})
const pageItems = computed<ComicInSearch[]>(() => {
  sessionVersion()
  return activeSession?.pageItems(searchPage.value) ?? []
})
const visibleCount = computed(() => {
  sessionVersion()
  return activeSession?.visibleCount ?? 0
})
const hasNextPage = computed(() => {
  sessionVersion()
  return activeSession?.hasMoreAfter(searchPage.value) ?? false
})
const isExhausted = computed(() => {
  sessionVersion()
  return activeSession?.exhausted ?? false
})
const fatalError = computed(() => {
  sessionVersion()
  return activeSession?.fatalError ?? null
})
const resultWarnings = computed(() => {
  sessionVersion()
  return activeSession?.warningsList ?? []
})
const hiddenUnknownCount = computed(() => {
  sessionVersion()
  return activeSession?.hiddenUnknownCount ?? 0
})
const exactTotal = computed(() => {
  sessionVersion()
  return activeSession?.exactTotal ?? null
})
const pageCount = computed(() => Math.max(1, Math.ceil(visibleCount.value / SEARCH_PAGE_SIZE)))
const filterLabel = computed(() => LANGUAGE_FILTER_LABELS[languageFilter.value])
const resultSummary = computed(() => {
  if (activeSession === null) return ''
  if (languageFilter.value !== 'all') return `已筛选 ${visibleCount.value} 部`
  if (exactTotal.value !== null) return `${exactTotal.value} 部`
  return isExhausted.value ? `共 ${visibleCount.value} 部` : `至少 ${visibleCount.value} 部`
})
const pageIsPartial = computed(
  () =>
    pageItems.value.length > 0 &&
    pageItems.value.length < SEARCH_PAGE_SIZE &&
    hasNextPage.value,
)

const SEARCH_HISTORY_KEY = 'joymanga_search_history'
const searchHistory = ref<string[]>([])
const historyOpen = ref(false)

function loadHistory() {
  try {
    const raw = localStorage.getItem(SEARCH_HISTORY_KEY)
    if (raw) {
      const parsed: unknown = JSON.parse(raw)
      // 存储被写坏（非数组/含非字符串）时丢弃脏数据，避免后续渲染与保存直接抛错
      searchHistory.value = Array.isArray(parsed)
        ? parsed.filter((item): item is string => typeof item === 'string')
        : []
    }
  } catch (e) {
    console.error('加载搜索历史失败', e)
  }
}

function saveHistory(keyword: string) {
  const filtered = searchHistory.value.filter((k) => k !== keyword)
  filtered.unshift(keyword)
  if (filtered.length > 12) {
    filtered.length = 12
  }
  searchHistory.value = filtered
  try {
    localStorage.setItem(SEARCH_HISTORY_KEY, JSON.stringify(filtered))
  } catch (e) {
    console.error('保存搜索历史失败', e)
  }
}

function removeHistoryItem(keyword: string) {
  // 列表展示的是过滤后的匹配项，下标与完整历史不对应，必须按值删除
  searchHistory.value = searchHistory.value.filter((item) => item !== keyword)
  try {
    localStorage.setItem(SEARCH_HISTORY_KEY, JSON.stringify(searchHistory.value))
  } catch (e) {
    console.error('更新搜索历史失败', e)
  }
}

function clearHistory() {
  searchHistory.value = []
  try {
    localStorage.removeItem(SEARCH_HISTORY_KEY)
  } catch (e) {
    console.error('清空搜索历史失败', e)
  }
}

function handleHistoryClick(kw: string) {
  searchInput.value = kw
  historyOpen.value = false
  void startSearch(kw, sortSelected.value)
}

const matchingHistory = computed(() => {
  const kw = searchInput.value.trim().toLowerCase()
  if (!kw) return searchHistory.value
  return searchHistory.value.filter((h) => h.toLowerCase().includes(kw))
})

loadHistory()

watch(
  () => store.pendingSearchKeyword,
  (val) => {
    if (val) {
      searchInput.value = val
      void startSearch(val, sortSelected.value)
      store.pendingSearchKeyword = ''
    }
  },
  { immediate: true },
)

watch(() => store.sessionEpoch, () => {
  searchGeneration++
  searching.value = false
  if (committedKeyword.value !== '') {
    void startSearch(committedKeyword.value, committedSort.value, searchPage.value)
  }
})

function toVariantPage(
  variant: SearchResultVariant | null,
  error: string | null,
): SearchVariantPage | null {
  if (error !== null) return { kind: 'error', message: error }
  if (variant === null) return null
  if ('SearchResult' in variant) {
    const page = variant.SearchResult
    return { kind: 'result', content: page.content, total: page.total, hasMore: page.hasMore }
  }
  return { kind: 'comic', comic: variant.Comic }
}

async function fetchDual(
  keyword: string,
  sort: SearchSort,
  request: DualSearchRequest,
): Promise<DualSearchResponse> {
  const result = await commands.searchDual(
    keyword,
    sort,
    request.originalPage,
    request.convertedPage,
  )
  if (result.status === 'error') {
    throw new Error(`${result.error.err_title}：${result.error.err_message}`)
  }
  const data = result.data
  return {
    original: toVariantPage(data.original, data.originalError),
    converted: toVariantPage(data.converted, data.convertedError),
    convertedKeyword: data.convertedKeyword,
    warnings: data.warnings,
  }
}

/** 开始一次新的搜索：重建会话并加载第一页（或指定页） */
async function startSearch(keyword: string, sort: SearchSort, page = 1) {
  keyword = keyword.trim()
  if (keyword === '') {
    message.warning('请输入关键词或 JM 号')
    return
  }

  saveHistory(keyword)
  view.value = 'search'
  historyOpen.value = false

  const generation = ++searchGeneration
  const epoch = store.sessionEpoch
  const observedRevision = store.favoriteRevision
  const keywordChanged = keyword !== committedKeyword.value
  if (keywordChanged) languageFilter.value = 'all'

  committedKeyword.value = keyword
  committedSort.value = sort
  sortSelected.value = sort
  searchPage.value = page
  activeSession = new SearchSession({
    keyword,
    sort,
    fetcher: (request) => fetchDual(keyword, sort, request),
  })
  sessionRevision.value++

  await loadPage(page, { generation, epoch, observedRevision })
}

/** 加载指定逻辑页；已加载的结果不会重复请求 */
async function loadPage(
  page: number,
  context?: { generation: number; epoch: number; observedRevision: number },
) {
  const session = activeSession
  if (session === null) return

  const generation = context?.generation ?? searchGeneration
  const epoch = context?.epoch ?? store.sessionEpoch
  const observedRevision = context?.observedRevision ?? store.favoriteRevision

  searching.value = true
  try {
    await session.ensureVisible(page * SEARCH_PAGE_SIZE)
    if (
      generation !== searchGeneration ||
      epoch !== store.sessionEpoch ||
      session !== activeSession
    ) {
      return
    }
    searchPage.value = page

    if (session.redirect !== null) {
      store.showComic(session.redirect, true)
      return
    }

    const items = session.pageItems(page)
    store.searchResult = {
      searchQuery: committedKeyword.value,
      total: session.exactTotal ?? session.visibleCount,
      content: items,
      hasMore: session.hasMoreAfter(page),
    }
    store.searchResultEpoch = epoch
    for (const comic of items) {
      store.recordFavorite(comic.id, comic.isFavorite, epoch, observedRevision)
    }
    sessionRevision.value++
    if (session.fatalError !== null) message.error(session.fatalError)
  } catch (error) {
    if (generation === searchGeneration) {
      console.error(error)
      message.error(`搜索失败：${getErrorMessage(error)}`)
    }
  } finally {
    if (generation === searchGeneration) {
      searching.value = false
    }
  }
}

function clearSearch() {
  searchGeneration++
  searching.value = false
  searchInput.value = ''
  store.searchResult = undefined
  committedKeyword.value = ''
  activeSession = null
  languageFilter.value = 'all'
  sessionRevision.value++
  view.value = 'weekly'
}

function changeSort(sort: SearchSort) {
  if (sort === committedSort.value) return
  if (committedKeyword.value !== '') void startSearch(committedKeyword.value, sort)
}

function changeFilter(filter: SearchFilter) {
  if (filter === languageFilter.value || searching.value) return
  languageFilter.value = filter
  activeSession?.setFilter(filter)
  searchPage.value = 1
  sessionRevision.value++
  void loadPage(1)
}

function clearFilter() {
  changeFilter('all')
}

function goToPage(page: number) {
  if (searching.value || page < 1) return
  if (page > searchPage.value && !hasNextPage.value) return
  void loadPage(page)
}
</script>

<template>
  <div class="discover-page">
    <!-- 搜索区：常驻顶部，输入时浮出历史 -->
    <div class="search-zone">
      <div class="search-field">
        <PhMagnifyingGlass :size="19" class="search-icon" weight="bold" aria-hidden="true" />
        <input
          v-model="searchInput"
          class="search-input"
          type="search"
          inputmode="search"
          enterkeyhint="search"
          placeholder="搜索漫画、作者或 JM 编号"
          aria-label="搜索漫画、作者或 JM 编号"
          @focus="historyOpen = true"
          @blur="historyOpen = false"
          @keydown.enter="startSearch(searchInput, sortSelected)" />
        <button
          v-if="searchInput || currentSession"
          type="button"
          class="search-clear"
          aria-label="清空搜索"
          @click="clearSearch">
          <PhX :size="16" weight="bold" />
        </button>
      </div>

      <div v-if="historyOpen && matchingHistory.length > 0" class="history-panel">
        <div class="history-head">
          <span><PhClockCounterClockwise :size="14" weight="bold" /> 最近搜索</span>
          <button type="button" class="jm-link" @mousedown.prevent="clearHistory">清空</button>
        </div>
        <div class="history-chips">
          <button
            v-for="hist in matchingHistory"
            :key="hist"
            type="button"
            class="history-chip"
            @mousedown.prevent="handleHistoryClick(hist)">
            <span class="jm-truncate">{{ hist }}</span>
            <span class="history-remove" role="button" aria-label="删除该记录" @mousedown.stop.prevent="removeHistoryItem(hist)">×</span>
          </button>
        </div>
      </div>
    </div>

    <div class="jm-scroll discover-scroll">
      <ContinueReadingRow />

      <!-- 每周必看 -->
      <WeeklyPane v-if="view === 'weekly'" />

      <!-- 搜索结果 -->
      <template v-else>
        <div v-if="currentSession === null" class="jm-empty">
          <div class="jm-empty-icon"><PhSparkle :size="28" weight="duotone" /></div>
          <strong>找到下一部想看的漫画</strong>
          <p>输入标题、作者，或直接粘贴 JM 编号。</p>
        </div>

        <template v-else>
          <div
            v-if="visibleCount > 0 || languageFilter !== 'all' || resultWarnings.length > 0"
            class="result-head">
            <div class="result-copy">
              <strong>「{{ committedKeyword }}」</strong>
              <span>{{ resultSummary }}</span>
            </div>
            <div class="sort-row">
              <button
                v-for="option in sortOptions"
                :key="option.value"
                type="button"
                class="sort-chip"
                :class="{ 'is-active': sortSelected === option.value }"
                @click="changeSort(option.value)">
                {{ option.label }}
              </button>
            </div>
            <div class="filter-row" role="group" aria-label="语言筛选（本地按标题判断）">
              <button
                v-for="option in languageFilters"
                :key="option.value"
                type="button"
                class="sort-chip"
                :class="{ 'is-active': languageFilter === option.value }"
                :disabled="searching"
                :aria-pressed="languageFilter === option.value"
                @click="changeFilter(option.value)">
                {{ option.label }}
              </button>
              <span class="filter-hint">本地判断</span>
            </div>
            <p v-if="currentSession.streamCount > 1" class="result-warning">
              已合并「{{ committedKeyword }}」与「{{ currentSession.convertedKeywordValue }}」两种写法
            </p>
            <p v-for="warning in resultWarnings" :key="warning" class="result-warning">{{ warning }}</p>
          </div>

          <div v-if="fatalError !== null" class="jm-empty">
            <div class="jm-empty-icon is-muted"><PhMagnifyingGlass :size="28" weight="duotone" /></div>
            <strong>搜索失败</strong>
            <p>{{ fatalError }}</p>
            <div class="empty-actions">
              <button
                type="button"
                class="jm-link"
                @click="startSearch(committedKeyword, committedSort, searchPage)">
                重试
              </button>
            </div>
          </div>

          <template v-else>
            <div v-if="searching && pageItems.length === 0" class="cover-grid is-loading">
              <div v-for="n in 6" :key="n" class="skeleton-card">
                <div class="jm-skeleton skeleton-cover"></div>
                <div class="jm-skeleton skeleton-line"></div>
                <div class="jm-skeleton skeleton-line is-short"></div>
              </div>
            </div>

            <div v-else-if="pageItems.length > 0" class="cover-grid">
              <ComicCard
                v-for="comic in pageItems"
                :key="comic.id"
                :comic-id="comic.id"
                :comic-favorite="comic.isFavorite"
                :favorite-epoch="store.searchResultEpoch"
                :comic-title="comic.name"
                :comic-author="comic.author"
                :comic-category="comic.category"
                :comic-category-sub="comic.categorySub"
                :comic-downloaded="comic.isDownloaded"
                :comic-download-dir="comic.comicDownloadDir" />
            </div>

            <div v-else-if="languageFilter !== 'all'" class="jm-empty">
              <div class="jm-empty-icon is-muted"><PhMagnifyingGlass :size="28" weight="duotone" /></div>
              <strong v-if="isExhausted">没有符合「{{ filterLabel }}」的作品</strong>
              <strong v-else>本页暂无符合「{{ filterLabel }}」的作品</strong>
              <p v-if="isExhausted">「{{ committedKeyword }}」的搜索结果已到末尾，可以清除筛选查看全部。</p>
              <p v-else>筛选只作用于已加载的结果，可以继续加载下一页。</p>
              <p v-if="hiddenUnknownCount > 0">另有 {{ hiddenUnknownCount }} 部语言未识别，已隐藏。</p>
              <div class="empty-actions">
                <button
                  v-if="!isExhausted"
                  type="button"
                  class="jm-link"
                  @click="goToPage(searchPage + 1)">
                  查看下一页
                </button>
                <button type="button" class="jm-link" @click="clearFilter">清除筛选</button>
              </div>
            </div>

            <div v-else class="jm-empty">
              <div class="jm-empty-icon is-muted"><PhMagnifyingGlass :size="28" weight="duotone" /></div>
              <strong>没有匹配的结果</strong>
              <p>「{{ committedKeyword }}」没有找到漫画，试试更短的关键词或换个说法。</p>
            </div>

            <p v-if="pageIsPartial && languageFilter !== 'all'" class="page-hint">
              筛选结果较少，可继续加载下一页
            </p>

            <nav v-if="hasNextPage || searchPage > 1" class="pager" aria-label="搜索结果分页">
              <button
                type="button"
                class="pager-btn"
                :disabled="searchPage <= 1 || searching"
                aria-label="上一页"
                @click="goToPage(searchPage - 1)">
                <PhArrowLeft :size="17" weight="bold" />
              </button>
              <span class="pager-info">
                <template v-if="isExhausted">{{ searchPage }} / {{ pageCount }}</template>
                <template v-else>第 {{ searchPage }} 页</template>
              </span>
              <button
                type="button"
                class="pager-btn"
                :disabled="!hasNextPage || searching"
                aria-label="下一页"
                @click="goToPage(searchPage + 1)">
                <PhArrowRight :size="17" weight="bold" />
              </button>
            </nav>
          </template>
        </template>
      </template>
    </div>
  </div>
</template>

<style scoped>
.discover-page {
  display: flex;
  height: 100%;
  min-height: 0;
  flex-direction: column;
  background: var(--jm-surface);
}

.search-zone {
  position: relative;
  z-index: 12;
  flex: 0 0 auto;
  padding: 10px var(--jm-gutter) 8px;
  background: var(--jm-surface);
}

.search-field {
  display: flex;
  height: 44px;
  align-items: center;
  gap: 8px;
  padding: 0 12px;
  border: 1px solid transparent;
  border-radius: var(--jm-r-md);
  background: var(--jm-sunken);
  transition:
    background-color 180ms ease,
    border-color 180ms ease,
    box-shadow 180ms ease;
}

.search-field:focus-within {
  border-color: var(--jm-brand);
  background: var(--jm-surface);
  box-shadow: 0 0 0 3px rgba(251, 114, 153, 0.14);
}

.search-icon {
  flex: 0 0 auto;
  color: var(--jm-text-3);
}

.search-field:focus-within .search-icon {
  color: var(--jm-brand);
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
  font-size: 14.5px;
}

.search-input::placeholder {
  color: var(--jm-text-3);
}

.search-input::-webkit-search-cancel-button {
  display: none;
}

.search-clear {
  display: grid;
  width: 26px;
  height: 26px;
  flex: 0 0 auto;
  padding: 0;
  color: var(--jm-text-3);
  border: 0;
  border-radius: var(--jm-r-pill);
  background: rgba(23, 24, 28, 0.06);
  place-items: center;
  cursor: pointer;
}

.history-panel {
  position: absolute;
  top: calc(100% - 4px);
  right: var(--jm-gutter);
  left: var(--jm-gutter);
  padding: 12px;
  border: 1px solid var(--jm-line);
  border-radius: var(--jm-r-lg);
  background: var(--jm-surface);
  box-shadow: var(--jm-shadow-2);
}

.history-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding-bottom: 8px;
  color: var(--jm-text-3);
  font-size: var(--jm-fs-xs);
}

.history-head span {
  display: flex;
  align-items: center;
  gap: 5px;
  font-weight: 600;
}

.history-chips {
  display: flex;
  max-height: 168px;
  flex-wrap: wrap;
  gap: 7px;
  overflow-y: auto;
}

.history-chip {
  display: inline-flex;
  max-width: 100%;
  height: 30px;
  align-items: center;
  gap: 4px;
  padding: 0 6px 0 11px;
  color: var(--jm-text-2);
  border: 0;
  border-radius: var(--jm-r-pill);
  background: var(--jm-sunken);
  font-size: var(--jm-fs-sm);
  cursor: pointer;
}

.history-chip:active {
  background: var(--jm-brand-soft);
  color: var(--jm-brand);
}

.history-remove {
  display: grid;
  width: 18px;
  height: 18px;
  color: var(--jm-text-4);
  place-items: center;
  border-radius: var(--jm-r-pill);
  font-size: 14px;
  line-height: 1;
}

.history-remove:hover {
  color: var(--jm-brand);
  background: rgba(251, 114, 153, 0.14);
}

.discover-scroll {
  padding-bottom: 28px;
  background: var(--jm-bg);
}

.result-head {
  display: flex;
  flex-direction: column;
  gap: 10px;
  padding: 14px var(--jm-gutter) 12px;
  background: var(--jm-surface);
}

.result-copy {
  display: flex;
  align-items: baseline;
  gap: 8px;
}

.result-copy strong {
  overflow: hidden;
  max-width: 70%;
  color: var(--jm-text);
  font-size: var(--jm-fs-title);
  font-weight: 750;
  letter-spacing: -0.015em;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.result-copy span {
  color: var(--jm-brand);
  font-size: var(--jm-fs-sm);
  font-weight: 700;
  font-variant-numeric: tabular-nums;
}

.sort-row {
  display: flex;
  gap: 6px;
  overflow-x: auto;
  scrollbar-width: none;
}

.sort-row::-webkit-scrollbar {
  display: none;
}

.sort-chip {
  height: 30px;
  flex: 0 0 auto;
  padding: 0 12px;
  color: var(--jm-text-2);
  border: 1px solid transparent;
  border-radius: var(--jm-r-pill);
  background: var(--jm-sunken);
  font-size: var(--jm-fs-sm);
  font-weight: 600;
  cursor: pointer;
}

.sort-chip.is-active {
  color: var(--jm-brand);
  border-color: rgba(251, 114, 153, 0.28);
  background: var(--jm-brand-soft);
}

.filter-row {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 6px;
}

.filter-hint {
  margin-left: 2px;
  color: var(--jm-text-4);
  font-size: 10.5px;
}

.result-warning {
  margin: 0;
  color: var(--jm-text-3);
  font-size: var(--jm-fs-xs);
  line-height: 1.5;
}

.page-hint {
  padding: 0 var(--jm-gutter) 6px;
  color: var(--jm-text-3);
  font-size: var(--jm-fs-xs);
  text-align: center;
}

.empty-actions {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 16px;
  margin-top: 10px;
}

.cover-grid {
  padding: 14px var(--jm-gutter) 8px;
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

.pager {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 18px;
  padding: 10px var(--jm-gutter) 24px;
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
  cursor: pointer;
  transition:
    color 160ms ease,
    border-color 160ms ease;
}

.pager-btn:hover:not(:disabled) {
  color: var(--jm-brand);
  border-color: rgba(251, 114, 153, 0.4);
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
