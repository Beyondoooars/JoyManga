<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { commands, SearchSort } from '../bindings.ts'
import { useMessage } from 'naive-ui'
import ComicCard from '../components/ComicCard.vue'
import FloatLabelInput from '../components/FloatLabelInput.vue'
import WeeklyPane from './WeeklyPane.vue'
import { PhClockCounterClockwise, PhFire, PhHashStraight, PhMagnifyingGlass, PhSparkle } from '@phosphor-icons/vue'
import { SelectProps } from 'naive-ui'
import { useStore } from '../store.ts'
import { getErrorMessage } from '../utils/errors.ts'

const store = useStore()
const activeSubTab = ref<'weekly' | 'search'>(store.searchResult !== undefined ? 'search' : 'weekly')

const message = useMessage()

const sortOptions: SelectProps['options'] = [
  { label: '最新', value: 'Latest' },
  { label: '最多点击', value: 'View' },
  { label: '最多图片', value: 'Picture' },
  { label: '最多爱心', value: 'Like' },
]

const searchInput = ref<string>(store.searchResult?.searchQuery ?? '')
const searching = ref<boolean>(false)
const sortSelected = ref<SearchSort>('Latest')
const searchPage = ref<number>(1)
const committedKeyword = ref(store.searchResult?.searchQuery ?? '')
const committedSort = ref<SearchSort>('Latest')
let searchGeneration = 0

const SEARCH_HISTORY_KEY = 'joymanga_search_history'
const searchHistory = ref<string[]>([])

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
  if (filtered.length > 15) {
    filtered.length = 15
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
  search(kw, 1, sortSelected.value)
}

const isInputFocused = ref(false)
const matchingHistory = computed(() => {
  const kw = searchInput.value.trim().toLowerCase()
  if (!kw) return searchHistory.value
  return searchHistory.value.filter((h) => h.toLowerCase().includes(kw))
})
const showHistoryDropdown = computed(() => isInputFocused.value && matchingHistory.value.length > 0)

function handleBlur() {
  setTimeout(() => {
    isInputFocused.value = false
  }, 220)
}

loadHistory()

watch(
  () => store.pendingSearchKeyword,
  (val) => {
    if (val) {
      searchInput.value = val
      search(val, 1, sortSelected.value)
      store.pendingSearchKeyword = ''
    }
  },
  { immediate: true },
)

const searchPageCount = computed(() => {
  const PAGE_SIZE = 80
  if (store.searchResult === undefined) {
    return 0
  }
  const total = store.searchResult.total
  return Math.ceil(total / PAGE_SIZE)
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
  activeSubTab.value = 'search'

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
    const searchResultVariant = result.data
    if ('SearchResult' in searchResultVariant) {
      const respData = searchResultVariant.SearchResult
      store.searchResult = respData
      store.searchResultEpoch = epoch
      for (const comic of respData.content) store.recordFavorite(comic.id, comic.isFavorite, epoch, observedRevision)
      committedKeyword.value = keyword
      committedSort.value = sort
      searchPage.value = page
      sortSelected.value = sort
      if (respData.content.length === 0) {
        message.warning('没有找到结果，换个关键词试试')
        return
      }
    } else if ('Comic' in searchResultVariant) {
      const comic = searchResultVariant.Comic
      store.showComic(comic, true)
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
</script>

<template>
  <div class="search-pane">
    <div class="search-toolbar">
      <div class="search-controls">
        <div class="search-input-wrapper relative flex-1 min-w-0">
          <FloatLabelInput
            class="search-query w-full"
            label="搜索关键词或 JM 号"
            size="small"
            :theme-overrides="{ borderRadius: '999px', borderHover: '1px solid #fb7299', borderFocus: '1px solid #fb7299' }"
            v-model:value="searchInput"
            clearable
            @focus="isInputFocused = true"
            @blur="handleBlur"
            @keydown.enter="search(searchInput.trim(), 1, sortSelected)" />

          <!-- 仅在输入/聚焦时呈现浮层历史，支持快捷搜索与单条清除 -->
          <div
            v-if="showHistoryDropdown"
            class="history-dropdown absolute top-[calc(100%+6px)] left-0 right-0 z-30 p-3 rounded-2xl bg-white border border-[#e3e5e7] shadow-xl">
            <div class="flex items-center justify-between pb-2 border-b border-[#f1f2f3] text-[11px] text-[#61666d]">
              <span class="flex items-center gap-1 font-bold text-[#18191c]">
                <PhClockCounterClockwise :size="14" class="text-[#fb7299]" />
                搜索历史建议
              </span>
              <button
                type="button"
                class="text-[#9499a0] hover:text-[#fb7299] transition text-[11px] cursor-pointer"
                @mousedown.prevent="clearHistory">
                清空全部
              </button>
            </div>
            <div class="flex flex-wrap gap-1.5 pt-2.5 max-h-36 overflow-y-auto no-scrollbar">
              <span
                v-for="hist in matchingHistory"
                :key="hist"
                class="history-chip"
                @mousedown.prevent="handleHistoryClick(hist)">
                <span class="truncate max-w-[150px]">{{ hist }}</span>
                <span class="remove-chip" @mousedown.stop.prevent="removeHistoryItem(hist)">×</span>
              </span>
            </div>
          </div>
        </div>

        <n-select
          class="sort-select"
          v-model:value="sortSelected"
          :options="sortOptions"
          :show-checkmark="false"
          size="small"
          @update-value="search(searchInput.trim(), 1, $event)" />
        <n-button
          :loading="searching"
          type="primary"
          size="small"
          class="search-button"
          aria-label="搜索"
          @click="search(searchInput.trim(), 1, sortSelected)">
          <template #icon>
            <n-icon size="22">
              <PhMagnifyingGlass />
            </n-icon>
          </template>
        </n-button>
      </div>
    </div>

    <!-- Discovery Sub-tabs Switcher -->
    <div class="discovery-subtabs">
      <button
        type="button"
        class="subtab-pill"
        :class="{ active: activeSubTab === 'weekly' }"
        @click="activeSubTab = 'weekly'">
        <PhFire :size="16" weight="duotone" />
        <span>每周必看榜单</span>
      </button>
      <button
        type="button"
        class="subtab-pill"
        :class="{ active: activeSubTab === 'search' }"
        @click="activeSubTab = 'search'">
        <PhMagnifyingGlass :size="15" weight="bold" />
        <span>
          {{ store.searchResult ? `搜索结果 (${store.searchResult.total} 部)` : '搜索探索' }}
        </span>
      </button>
    </div>

    <!-- Mode 1: Weekly Ranking -->
    <div v-if="activeSubTab === 'weekly'" class="weekly-wrapper">
      <WeeklyPane />
    </div>

    <!-- Mode 2: Search Exploration & Results -->
    <template v-else>
      <div v-if="store.searchResult === undefined" class="search-empty">
        <div class="search-orbit" aria-hidden="true">
          <PhMagnifyingGlass :size="31" weight="bold" />
          <PhSparkle class="sparkle" :size="16" weight="fill" />
        </div>
        <h2>找到下一部想看的漫画</h2>
        <p>输入标题、作者，或粘贴 JM 编号</p>
        <div class="search-tips">
          <span>
            <PhHashStraight :size="14" />
            支持 JM 号
          </span>
          <span>
            <PhSparkle :size="14" />
            多种热度排序
          </span>
        </div>
      </div>

      <div v-else class="result-area">
        <div class="result-summary">
          <span>找到符合条件的漫画</span>
          <strong>{{ store.searchResult.total }} 部</strong>
        </div>
        <div class="comic-list">
          <ComicCard
            v-for="comicInSearch in store.searchResult.content"
            :key="comicInSearch.id"
            :comic-id="comicInSearch.id"
            :comic-favorite="comicInSearch.isFavorite"
            :favorite-epoch="store.searchResultEpoch"
            :comic-title="comicInSearch.name"
            :comic-author="comicInSearch.author"
            :comic-category="comicInSearch.category"
            :comic-category-sub="comicInSearch.categorySub"
            :comic-downloaded="comicInSearch.isDownloaded"
            :comic-download-dir="comicInSearch.comicDownloadDir" />
        </div>
      </div>

      <n-pagination
        v-if="searchPageCount > 0"
        class="search-pagination"
        :page-count="searchPageCount"
        :page="searchPage"
        @update:page="search(committedKeyword, $event, committedSort)" />
    </template>
  </div>
</template>

<style scoped>
.search-pane {
  display: flex;
  height: 100%;
  min-height: 0;
  flex-direction: column;
  background: #ffffff;
}

.search-toolbar {
  flex: 0 0 auto;
  padding: 14px 18px 12px;
  border-bottom: 1px solid #e3e5e7;
  background: #ffffff;
}

.discovery-subtabs {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 10px 18px 8px;
  background: #ffffff;
  border-bottom: 1px solid #f1f2f3;
  flex-shrink: 0;
}

.subtab-pill {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 6px 16px;
  border-radius: 999px;
  background: #f1f2f3;
  border: 1px solid #e3e5e7;
  color: #61666d;
  font-size: 12px;
  font-weight: 600;
  cursor: pointer;
  transition: all 0.2s;
}

.subtab-pill:hover {
  background: #ffffff;
  border-color: #fb7299;
  color: #fb7299;
}

.subtab-pill.active {
  background: #fb7299;
  border-color: #fb7299;
  color: #ffffff;
  box-shadow: 0 3px 12px rgba(251, 114, 153, 0.35);
}

.weekly-wrapper {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
}

.search-controls {
  display: grid;
  grid-template-columns: minmax(0, 1fr) 112px 42px;
  gap: 8px;
  align-items: center;
}

.search-query {
  min-width: 0;
  height: 38px;
  --n-height: 38px !important;
}

.sort-select {
  width: 112px;
  --n-height: 38px !important;
}

.sort-select :deep(.n-base-selection),
.sort-select :deep(.n-base-selection-label) {
  height: 38px;
  border-radius: var(--radius-card);
  background: #f1f2f3;
  border: 1px solid #e3e5e7;
}

.sort-select :deep(.n-base-selection-tags) {
  min-height: 38px;
}

.search-button {
  width: 42px;
  height: 38px;
  padding: 0;
  border-radius: var(--radius-card);
  box-shadow: 0 3px 10px rgba(251, 114, 153, 0.3);
}

.search-empty {
  display: flex;
  min-height: 0;
  flex: 1;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  padding: 32px;
  text-align: center;
  background: #ffffff;
}

.search-orbit {
  position: relative;
  display: grid;
  width: 76px;
  height: 76px;
  margin-bottom: 18px;
  color: #fb7299;
  place-items: center;
  border: 1px solid rgba(251, 114, 153, 0.35);
  border-radius: 50%;
  background: rgba(251, 114, 153, 0.08);
  box-shadow: 0 0 0 10px rgba(251, 114, 153, 0.04);
}

.sparkle {
  position: absolute;
  top: 4px;
  right: 4px;
  color: #00aeec;
}

.search-empty h2 {
  margin: 0;
  color: #18191c;
  font-size: 17px;
  font-weight: 700;
  letter-spacing: -0.02em;
}

.search-empty p {
  max-width: 320px;
  margin: 8px 0 18px;
  color: #61666d;
  font-size: 12px;
  line-height: 1.6;
}

.search-tips {
  display: flex;
  flex-wrap: wrap;
  justify-content: center;
  gap: 8px;
}

.search-tips span {
  display: flex;
  align-items: center;
  gap: 5px;
  padding: 6px 12px;
  color: #61666d;
  border: 1px solid #e3e5e7;
  border-radius: 999px;
  background: #f6f7f8;
  font-size: 11px;
  transition: all 0.2s;
}

.search-tips span:hover {
  background: #ffffff;
  border-color: #fb7299;
  color: #fb7299;
}

.result-area {
  display: flex;
  min-height: 0;
  flex: 1;
  flex-direction: column;
  background: #f6f7f8;
}

.result-summary {
  display: flex;
  align-items: center;
  gap: 7px;
  padding: 12px 18px 8px;
  color: #61666d;
  font-size: 12px;
  font-weight: 600;
}

.result-summary strong {
  padding: 2px 8px;
  color: #fb7299;
  border-radius: 999px;
  background: rgba(251, 114, 153, 0.12);
  font-size: 11px;
}

.comic-list {
  display: flex;
  min-height: 0;
  flex: 1;
  flex-direction: column;
  gap: 10px;
  overflow: auto;
  padding: 0 16px 16px;
}

.search-pagination {
  flex: 0 0 auto;
  padding: 12px 18px;
  border-top: 1px solid #e3e5e7;
  background: #ffffff;
}

.history-chip {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 4px 10px;
  border-radius: 999px;
  background: #f1f2f3;
  border: 1px solid #e3e5e7;
  color: #18191c;
  font-size: 11px;
  cursor: pointer;
  transition: all 150ms ease;
}

.history-chip:hover {
  background: #ffffff;
  border-color: #fb7299;
  color: #fb7299;
}

.remove-chip {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 14px;
  height: 14px;
  border-radius: 50%;
  color: #9499a0;
  font-size: 13px;
  line-height: 1;
  transition: all 120ms ease;
}

.remove-chip:hover {
  background: rgba(251, 114, 153, 0.2);
  color: #fb7299;
}

@media (max-width: 768px) {
  .search-bar-header {
    padding: 10px 12px;
  }

  .search-controls {
    grid-template-columns: minmax(0, 1fr) 92px 38px;
    gap: 6px;
  }

  .sort-select {
    width: 92px;
  }

  .search-button {
    width: 38px;
  }

  .discovery-subtabs {
    padding: 6px 12px;
  }

  .comic-list {
    padding: 0 10px 12px;
  }
}
</style>
