<script setup lang="ts">
import { onMounted, onUnmounted, ref, computed, watch } from 'vue'
import { GetWeeklyInfoRespData, commands } from '../bindings.ts'
import { SelectProps, useMessage } from 'naive-ui'
import ComicCard from '../components/ComicCard.vue'
import { useStore } from '../store.ts'
import { PhCalendarDots, PhWarningCircle, PhArrowClockwise } from '@phosphor-icons/vue'
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

const categoryOptions = computed<SelectProps['options']>(() =>
  weeklyInfo.value?.categories.map((category) => ({
    label: category.time,
    value: category.id,
  })),
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
  const filteredTypes = result.data.type.filter(
    (t) => t.id === 'manga' || t.id === 'hanman',
  )

  weeklyInfo.value = {
    ...result.data,
    type: filteredTypes,
  }
  infoLoading.value = false

  const firstCategory = result.data.categories[0]
  // 优先默认选中日漫，若无则选第一个
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
  requestGeneration++; loading.value = false; activeRequestKey = ''
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
  <div v-if="infoLoading" class="weekly-state">
    <n-spin size="small" />
    <span>正在获取每周精选…</span>
  </div>

  <div v-else-if="infoUnavailable" class="weekly-state">
    <div class="weekly-icon"><PhCalendarDots :size="30" weight="duotone" /></div>
    <strong>本周内容正在整理</strong>
    <span>新的精选内容稍后上线</span>
  </div>

  <div v-else-if="weeklyInfo !== undefined" class="weekly-pane" :aria-busy="loading">
    <div class="weekly-toolbar">
      <div class="weekly-filter-label">
        <span class="weekly-filter-icon"><PhCalendarDots :size="19" weight="duotone" /></span>
        <span>
          <strong>榜单周期</strong>
        </span>
      </div>
      <n-select
        v-if="categoryOptions !== undefined"
        class="weekly-category-select"
        v-model:value="selectedCategoryId"
        :options="categoryOptions"
        :show-checkmark="false"
        aria-label="榜单周期"
        size="small" />
    </div>

    <n-tabs class="weekly-tabs" v-model:value="currentWeeklyTypeId" type="line" size="small">
      <n-tab-pane
        class="weekly-tab-pane"
        v-for="weeklyType in weeklyInfo.type"
        :key="weeklyType.id"
        :name="weeklyType.id"
        :tab="weeklyType.title">
        <div v-if="loading && store.getWeeklyResult === undefined" class="weekly-content-state">
          <n-spin size="small" />
          <span>正在加载榜单…</span>
        </div>
        <div v-else-if="loadError && store.getWeeklyResult === undefined" class="weekly-error-state">
          <div class="weekly-error-content">
            <PhWarningCircle :size="36" weight="duotone" class="text-[#fb7299]" />
            <strong>获取榜单失败</strong>
            <span>{{ loadError }}</span>
            <n-button type="primary" size="small" class="mt-2" @click="getWeekly">
              <template #icon>
                <PhArrowClockwise />
              </template>
              点击重试
            </n-button>
          </div>
        </div>
        <div v-else-if="store.getWeeklyResult !== undefined" class="weekly-list">
          <n-empty
            v-if="store.getWeeklyResult.list.length === 0"
            class="weekly-list-empty"
            description="分类暂无内容" />
          <ComicCard
            v-for="comicInWeekly in store.getWeeklyResult.list"
            :key="comicInWeekly.id"
            :comic-id="comicInWeekly.id"
            :comic-favorite="comicInWeekly.is_favorite"
            :favorite-epoch="store.weeklyResultEpoch"
            :comic-title="comicInWeekly.name"
            :comic-author="comicInWeekly.author"
            :comic-category="comicInWeekly.category"
            :comic-category-sub="comicInWeekly.category_sub"
            :comic-downloaded="comicInWeekly.is_downloaded"
            :comic-download-dir="comicInWeekly.comic_download_dir" />
        </div>
      </n-tab-pane>
    </n-tabs>
  </div>
</template>

<style scoped>
.weekly-pane {
  display: flex;
  height: 100%;
  min-height: 0;
  flex-direction: column;
  background: #ffffff;
}

.weekly-toolbar {
  display: flex;
  min-height: 56px;
  flex: 0 0 auto;
  align-items: center;
  justify-content: space-between;
  gap: 14px;
  padding: 10px 18px;
  border-bottom: 1px solid #e3e5e7;
  background: #ffffff;
}

.weekly-filter-label {
  display: flex;
  min-width: 0;
  align-items: center;
  gap: 10px;
}

.weekly-filter-label > span:last-child {
  display: flex;
  min-width: 0;
  flex-direction: column;
}

.weekly-filter-label strong {
  color: #18191c;
  font-size: 13px;
  line-height: 1.25;
}

.weekly-filter-label small {
  margin-top: 2px;
  overflow: hidden;
  color: #9499a0;
  font-size: 11px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.weekly-filter-icon {
  display: grid;
  width: 34px;
  height: 34px;
  flex: 0 0 auto;
  color: #fb7299;
  place-items: center;
  border: 1px solid rgba(251, 114, 153, 0.3);
  border-radius: var(--radius-control);
  background: rgba(251, 114, 153, 0.08);
}

.weekly-category-select {
  width: clamp(120px, 36%, 160px);
  flex: 0 0 auto;
}

.weekly-category-select :deep(.n-base-selection) {
  border-radius: var(--radius-control);
  background: #f1f2f3;
  border: 1px solid #e3e5e7;
}

.weekly-tabs {
  display: flex;
  flex-direction: column;
  min-height: 0;
  flex: 1;
  overflow: hidden;
}

.weekly-tabs :deep(.n-tabs-nav) {
  flex: 0 0 auto;
  padding: 0 18px;
  border-bottom: 1px solid #f1f2f3;
  background: #ffffff;
}

.weekly-tabs :deep(.n-tabs-pane-wrapper) {
  flex: 1;
  min-height: 0;
  overflow: hidden;
}

.weekly-tab-pane {
  height: 100%;
  box-sizing: border-box;
  padding: 0 !important;
  overflow: hidden;
}

.weekly-list {
  display: flex;
  height: 100%;
  min-height: 0;
  flex-direction: column;
  gap: 10px;
  overflow: auto;
  box-sizing: border-box;
  padding: 16px;
  background: #f6f7f8;
}

.weekly-content-state {
  display: flex;
  height: 100%;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 8px;
  color: #9499a0;
  font-size: 12px;
}

.weekly-error-state {
  display: flex;
  height: 100%;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  padding: 24px;
}

.weekly-error-content {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 8px;
  text-align: center;
  color: #61666d;
}

.weekly-error-content strong {
  font-size: 15px;
  color: #18191c;
}

.weekly-error-content span {
  font-size: 12px;
  color: #9499a0;
  max-width: 280px;
  line-height: 1.5;
}

.weekly-state {
  display: flex;
  height: 100%;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 8px;
  padding: 28px;
  color: #61666d;
  text-align: center;
  font-size: 12px;
  background: #ffffff;
}

.weekly-icon {
  display: grid;
  width: 72px;
  height: 72px;
  margin-bottom: 10px;
  color: #fb7299;
  place-items: center;
  border: 1px solid rgba(251, 114, 153, 0.35);
  border-radius: 50%;
  background: rgba(251, 114, 153, 0.08);
  box-shadow: 0 0 0 9px rgba(251, 114, 153, 0.04);
}

.weekly-state strong {
  color: #18191c;
  font-size: 16px;
}

.weekly-state > span:last-child {
  max-width: 270px;
  line-height: 1.6;
}

.weekly-list-empty {
  margin: auto;
}

@media (max-width: 768px) {
  .weekly-toolbar {
    padding: 10px 12px;
    gap: 8px;
  }

  .weekly-list {
    padding: 12px 10px;
    gap: 8px;
  }
}
</style>
