<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import { commands, type GetWeeklyInfoRespData } from '../bindings.ts'
import { useMessage } from 'naive-ui'
import ComicCard from '../components/ComicCard.vue'
import { useStore } from '../store.ts'
import { PhArrowClockwise, PhCaretDown, PhFire, PhWarningCircle } from '@phosphor-icons/vue'
import { getErrorMessage } from '../utils/errors.ts'

const store = useStore()
const message = useMessage()

const weeklyInfo = ref<GetWeeklyInfoRespData>()
const selectedCategoryId = ref<string>('')
const currentWeeklyTypeId = ref<string>('')
const loading = ref(false)
const infoLoading = ref(true)
const infoUnavailable = ref(false)
const loadError = ref<string | null>(null)
let initialized = false
let isMounted = false
let requestGeneration = 0
let activeRequestKey = ''

const categories = computed(() => weeklyInfo.value?.categories ?? [])
const types = computed(() => weeklyInfo.value?.type ?? [])
const activeCategoryLabel = computed(
  () => categories.value.find((category) => category.id === selectedCategoryId.value)?.time ?? '',
)
// 期数选择：小框点开后可在弹层里上下滑动，直接跳到任意一期
const categoryOptions = computed(() =>
  categories.value.map((category) => ({ label: category.time, value: category.id })),
)

onMounted(async () => {
  isMounted = true
  let result: Awaited<ReturnType<typeof commands.getWeeklyInfo>>
  try {
    result = await commands.getWeeklyInfo()
  } catch (error) {
    // IPC 被拒绝（后端异常、窗口销毁）时也要结束加载态，不能一直转圈
    if (!isMounted) return
    console.error(error)
    message.error(`获取每周必看失败：${getErrorMessage(error)}`)
    infoLoading.value = false
    infoUnavailable.value = true
    return
  }
  if (!isMounted) return
  if (result.status === 'error') {
    console.error(result.error)
    message.error(`${result.error.err_title}：${result.error.err_message}`)
    infoLoading.value = false
    infoUnavailable.value = true
    return
  }

  // 按照需求：仅保留韩漫和日漫，删除其他
  const filteredTypes = result.data.type.filter((t) => t.id === 'manga' || t.id === 'hanman')

  weeklyInfo.value = { ...result.data, type: filteredTypes }
  infoLoading.value = false

  const firstCategory = result.data.categories[0]
  const defaultType = filteredTypes.find((t) => t.id === 'manga') ?? filteredTypes[0]

  if (firstCategory === undefined || defaultType === undefined) {
    message.warning('每周必看暂时没有可用分类')
    infoUnavailable.value = true
    return
  }

  selectedCategoryId.value = firstCategory.id
  currentWeeklyTypeId.value = defaultType.id
  initialized = true
  await getWeekly()
})

onUnmounted(() => {
  isMounted = false
  requestGeneration++
})

watch(
  () => [selectedCategoryId.value, currentWeeklyTypeId.value],
  () => {
    if (!initialized) return
    store.getWeeklyResult = undefined
    loadError.value = null
    void getWeekly()
  },
)

watch(() => store.sessionEpoch, () => {
  requestGeneration++
  loading.value = false
  activeRequestKey = ''
  if (initialized) void getWeekly()
})

async function getWeekly() {
  if (!isMounted) return
  if (selectedCategoryId.value === '' || currentWeeklyTypeId.value === '') return
  const requestKey = `${selectedCategoryId.value}:${currentWeeklyTypeId.value}`
  if (loading.value && activeRequestKey === requestKey) return
  activeRequestKey = requestKey
  const generation = ++requestGeneration
  const epoch = store.sessionEpoch
  const observedRevision = store.favoriteRevision
  loading.value = true
  try {
    const result = await commands.getWeekly(selectedCategoryId.value, currentWeeklyTypeId.value)
    if (!isMounted || generation !== requestGeneration || epoch !== store.sessionEpoch) return
    if (result.status === 'error') {
      console.error(result.error)
      loadError.value = `${result.error.err_title}：${result.error.err_message}`
      message.error(`${result.error.err_title}：${result.error.err_message}`)
      return
    }

    store.getWeeklyResult = result.data
    store.weeklyResultEpoch = epoch
    for (const comic of result.data.list) store.recordFavorite(comic.id, comic.is_favorite, epoch, observedRevision)
  } catch (error) {
    if (!isMounted || generation !== requestGeneration) return
    console.error(error)
    loadError.value = getErrorMessage(error)
    message.error(`获取每周必看失败：${loadError.value}`)
  } finally {
    // 无论成功、失败还是被拒绝，都要结束本次请求的加载态
    if (generation === requestGeneration) {
      loading.value = false
    }
  }
}
</script>

<template>
  <section class="weekly" aria-labelledby="weekly-heading">
    <!-- 加载中 -->
    <div v-if="infoLoading" class="weekly-shell">
      <div class="weekly-head">
        <div class="weekly-title">
          <span class="title-icon"><PhFire :size="18" weight="fill" /></span>
          <h2 id="weekly-heading">每周必看</h2>
        </div>
      </div>
      <div class="cover-grid">
        <div v-for="n in 6" :key="n" class="skeleton-card">
          <div class="jm-skeleton skeleton-cover"></div>
          <div class="jm-skeleton skeleton-line"></div>
        </div>
      </div>
    </div>

    <!-- 不可用 -->
    <div v-else-if="infoUnavailable" class="jm-empty">
      <div class="jm-empty-icon is-muted"><PhFire :size="28" weight="duotone" /></div>
      <strong>本周榜单还没准备好</strong>
      <p>新的精选内容稍后上线，先去搜索想看的漫画吧。</p>
    </div>

    <div v-else-if="weeklyInfo !== undefined" class="weekly-shell">
      <header class="weekly-head">
        <div class="weekly-title">
          <span class="title-icon"><PhFire :size="18" weight="fill" /></span>
          <div class="title-copy">
            <h2 id="weekly-heading">每周必看</h2>
            <n-popselect
              v-model:value="selectedCategoryId"
              :options="categoryOptions"
              trigger="click"
              size="small"
              scrollable
              :disabled="categories.length === 0">
              <button
                type="button"
                class="issue-select"
                :disabled="categories.length === 0"
                aria-label="选择榜单期数">
                <span>{{ activeCategoryLabel || '选择期数' }}</span>
                <PhCaretDown :size="13" weight="bold" aria-hidden="true" />
              </button>
            </n-popselect>
          </div>
        </div>
        <button
          type="button"
          class="jm-icon-btn"
          :disabled="loading"
          aria-label="刷新榜单"
          title="刷新榜单"
          @click="getWeekly">
          <PhArrowClockwise :size="19" :class="{ 'jm-spin': loading }" />
        </button>
      </header>

      <!-- 筛选：顺序为周期，第二个为类型 -->
      <div class="filter-block">
        <div class="filter-row" role="group" aria-label="漫画类型">
          <button
            v-for="weeklyType in types"
            :key="weeklyType.id"
            type="button"
            class="filter-chip"
            :class="{ 'is-active': currentWeeklyTypeId === weeklyType.id }"
            @click="currentWeeklyTypeId = weeklyType.id">
            {{ weeklyType.title }}
          </button>
        </div>
      </div>

      <div v-if="loading && store.getWeeklyResult === undefined" class="cover-grid">
        <div v-for="n in 6" :key="n" class="skeleton-card">
          <div class="jm-skeleton skeleton-cover"></div>
          <div class="jm-skeleton skeleton-line"></div>
        </div>
      </div>

      <div v-else-if="loadError && store.getWeeklyResult === undefined" class="jm-empty">
        <div class="jm-empty-icon is-muted"><PhWarningCircle :size="28" weight="duotone" /></div>
        <strong>获取榜单失败</strong>
        <p>{{ loadError }}</p>
        <div class="jm-empty-actions">
          <button type="button" class="jm-btn is-primary" @click="getWeekly">重新加载</button>
        </div>
      </div>

      <div v-else-if="store.getWeeklyResult" class="weekly-body" :aria-busy="loading">
        <div v-if="store.getWeeklyResult.list.length === 0" class="jm-empty">
          <div class="jm-empty-icon is-muted"><PhFire :size="28" weight="duotone" /></div>
          <strong>该分类暂无内容</strong>
          <p>换一个周期或类型看看。</p>
        </div>
        <div v-else class="cover-grid">
          <ComicCard
            v-for="comic in store.getWeeklyResult.list"
            :key="comic.id"
            :comic-id="comic.id"
            :comic-favorite="comic.is_favorite"
            :favorite-epoch="store.weeklyResultEpoch"
            :comic-title="comic.name"
            :comic-author="comic.author"
            :comic-category="comic.category"
            :comic-category-sub="comic.category_sub"
            :comic-downloaded="comic.is_downloaded"
            :comic-download-dir="comic.comic_download_dir" />
        </div>
      </div>
    </div>
  </section>
</template>

<style scoped>
.weekly {
  flex: 0 0 auto;
  background: var(--jm-surface);
}

.weekly-shell {
  display: flex;
  flex-direction: column;
}

.weekly-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
  padding: 6px var(--jm-gutter) 4px;
}

.weekly-title {
  display: flex;
  min-width: 0;
  align-items: center;
  gap: 9px;
}

.title-icon {
  display: grid;
  width: 30px;
  height: 30px;
  flex: 0 0 auto;
  color: var(--jm-brand);
  place-items: center;
  border-radius: var(--jm-r-sm);
  background: var(--jm-brand-soft);
}

.title-copy {
  display: flex;
  min-width: 0;
  flex-direction: column;
  gap: 1px;
}

.weekly-title h2 {
  margin: 0;
  color: var(--jm-text);
  font-size: var(--jm-fs-title);
  font-weight: 750;
  letter-spacing: -0.015em;
  line-height: 1.25;
}

/* 期数选择：小框点开后弹层可上下滑动 */
.issue-select {
  display: inline-flex;
  max-width: 100%;
  align-items: center;
  align-self: flex-start;
  gap: 4px;
  padding: 1px 8px 1px 9px;
  color: var(--jm-text-3);
  border: 1px solid var(--jm-line);
  border-radius: var(--jm-r-pill);
  background: var(--jm-surface);
  font-size: var(--jm-fs-xs);
  font-weight: 600;
  line-height: 1.6;
  cursor: pointer;
  transition:
    color 160ms ease,
    border-color 160ms ease;
}

.issue-select:hover {
  color: var(--jm-brand);
  border-color: var(--jm-brand-ring);
}

.issue-select:disabled {
  opacity: 0.6;
  cursor: default;
}

.issue-select span {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.filter-block {
  display: flex;
  flex-direction: column;
  gap: 6px;
  padding: 8px var(--jm-gutter) 2px;
}

.filter-row {
  display: flex;
  gap: 6px;
  overflow-x: auto;
  scrollbar-width: none;
}

.filter-row::-webkit-scrollbar {
  display: none;
}

.filter-chip {
  height: 32px;
  flex: 0 0 auto;
  padding: 0 13px;
  color: var(--jm-text-2);
  border: 1px solid transparent;
  border-radius: var(--jm-r-pill);
  background: var(--jm-sunken);
  font-size: var(--jm-fs-sm);
  font-weight: 600;
  cursor: pointer;
  transition:
    color 160ms ease,
    background-color 160ms ease,
    border-color 160ms ease;
}

.filter-chip.is-active {
  color: var(--jm-brand);
  border-color: rgba(251, 114, 153, 0.28);
  background: var(--jm-brand-soft);
}

.weekly-body {
  display: flex;
  flex-direction: column;
}

.cover-grid {
  padding: 12px var(--jm-gutter) 4px;
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
</style>
