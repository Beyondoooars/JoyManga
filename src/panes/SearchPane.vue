<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { commands, type SearchSort } from '../bindings.ts'
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

const store = useStore()
const message = useMessage()

type DiscoverView = 'weekly' | 'search'
const view = ref<DiscoverView>(store.searchResult !== undefined ? 'search' : 'weekly')

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
let searchGeneration = 0

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
  void search(kw, 1, sortSelected.value)
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
      void search(val, 1, sortSelected.value)
      store.pendingSearchKeyword = ''
    }
  },
  { immediate: true },
)

const PAGE_SIZE = 80
const searchPageCount = computed(() => {
  if (store.searchResult === undefined) return 0
  return Math.ceil(store.searchResult.total / PAGE_SIZE)
})

watch(() => store.sessionEpoch, () => {
  searchGeneration++
  searching.value = false
  if (committedKeyword.value) void search(committedKeyword.value, searchPage.value, committedSort.value)
})

async function search(keyword: string, page: number, sort: SearchSort) {
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
  searching.value = true

  try {
    const result = await commands.search(keyword, page, sort)
    if (generation !== searchGeneration || epoch !== store.sessionEpoch) return
    if (result.status === 'error') {
      console.error(result.error)
      message.error(`${result.error.err_title}：${result.error.err_message}`)
      return
    }
    const variant = result.data
    if ('SearchResult' in variant) {
      const respData = variant.SearchResult
      store.searchResult = respData
      store.searchResultEpoch = epoch
      for (const comic of respData.content) store.recordFavorite(comic.id, comic.isFavorite, epoch, observedRevision)
      committedKeyword.value = keyword
      committedSort.value = sort
      searchPage.value = page
      sortSelected.value = sort
      if (respData.content.length === 0) {
        message.warning('没有找到结果，换个关键词试试')
      }
    } else if ('Comic' in variant) {
      store.showComic(variant.Comic, true)
    }
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
  view.value = 'weekly'
}

function changeSort(sort: SearchSort) {
  sortSelected.value = sort
  if (committedKeyword.value) void search(committedKeyword.value, 1, sort)
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
          @keydown.enter="search(searchInput, 1, sortSelected)" />
        <button
          v-if="searchInput || store.searchResult"
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

    <!-- 视图切换：搜索中显示"结果 / 每周必看" -->
    <div class="view-switch">
      <button
        type="button"
        class="switch-item"
        :class="{ 'is-active': view === 'weekly' }"
        @click="view = 'weekly'">
        榜单
      </button>
      <button
        type="button"
        class="switch-item"
        :class="{ 'is-active': view === 'search' }"
        :disabled="store.searchResult === undefined"
        @click="view = 'search'">
        搜索结果
        <span v-if="store.searchResult" class="switch-count">{{ store.searchResult.total }}</span>
      </button>
    </div>

    <div class="jm-scroll discover-scroll">
      <ContinueReadingRow />

      <!-- 每周必看 -->
      <WeeklyPane v-if="view === 'weekly'" />

      <!-- 搜索结果 -->
      <template v-else>
        <div v-if="store.searchResult === undefined" class="jm-empty">
          <div class="jm-empty-icon"><PhSparkle :size="28" weight="duotone" /></div>
          <strong>找到下一部想看的漫画</strong>
          <p>输入标题、作者，或直接粘贴 JM 编号。</p>
        </div>

        <template v-else-if="store.searchResult.content.length === 0">
          <div class="jm-empty">
            <div class="jm-empty-icon is-muted"><PhMagnifyingGlass :size="28" weight="duotone" /></div>
            <strong>没有匹配的结果</strong>
            <p>「{{ committedKeyword }}」没有找到漫画，试试更短的关键词或换个说法。</p>
          </div>
        </template>

        <template v-else>
          <div class="result-head">
            <div class="result-copy">
              <strong>「{{ committedKeyword }}」</strong>
              <span>{{ store.searchResult.total }} 部</span>
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
          </div>

          <div v-if="searching" class="cover-grid is-loading">
            <div v-for="n in 6" :key="n" class="skeleton-card">
              <div class="jm-skeleton skeleton-cover"></div>
              <div class="jm-skeleton skeleton-line"></div>
              <div class="jm-skeleton skeleton-line is-short"></div>
            </div>
          </div>
          <div v-else class="cover-grid">
            <ComicCard
              v-for="comic in store.searchResult.content"
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

          <nav v-if="searchPageCount > 1" class="pager" aria-label="搜索结果分页">
            <button
              type="button"
              class="pager-btn"
              :disabled="searchPage <= 1 || searching"
              aria-label="上一页"
              @click="search(committedKeyword, searchPage - 1, committedSort)">
              <PhArrowLeft :size="17" weight="bold" />
            </button>
            <span class="pager-info">{{ searchPage }} / {{ searchPageCount }}</span>
            <button
              type="button"
              class="pager-btn"
              :disabled="searchPage >= searchPageCount || searching"
              aria-label="下一页"
              @click="search(committedKeyword, searchPage + 1, committedSort)">
              <PhArrowRight :size="17" weight="bold" />
            </button>
          </nav>
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

/* 视图切换：下划线式，克制 */
.view-switch {
  display: flex;
  flex: 0 0 auto;
  gap: 22px;
  padding: 0 var(--jm-gutter);
  border-bottom: 1px solid var(--jm-line);
  background: var(--jm-surface);
}

.switch-item {
  position: relative;
  display: inline-flex;
  height: 42px;
  align-items: center;
  gap: 6px;
  padding: 0;
  color: var(--jm-text-3);
  border: 0;
  background: none;
  font-size: var(--jm-fs-body);
  font-weight: 600;
  cursor: pointer;
  transition: color 160ms ease;
}

.switch-item::after {
  position: absolute;
  right: 0;
  bottom: -1px;
  left: 0;
  height: 2.5px;
  border-radius: 2px;
  background: var(--jm-brand);
  opacity: 0;
  transform: scaleX(0.4);
  transition:
    opacity 180ms ease,
    transform 180ms ease;
  content: '';
}

.switch-item.is-active {
  color: var(--jm-text);
  font-weight: 700;
}

.switch-item.is-active::after {
  opacity: 1;
  transform: scaleX(1);
}

.switch-item:disabled {
  color: var(--jm-text-4);
  cursor: default;
}

.switch-count {
  padding: 1px 7px;
  color: var(--jm-brand);
  border-radius: var(--jm-r-pill);
  background: var(--jm-brand-soft);
  font-size: 10.5px;
  font-weight: 700;
  font-variant-numeric: tabular-nums;
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
