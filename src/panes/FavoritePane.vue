<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { commands, FavoriteSort } from '../bindings.ts'
import { useMessage } from 'naive-ui'
import ComicCard from '../components/ComicCard.vue'
import { SelectProps } from 'naive-ui'
import { useStore } from '../store.ts'
import DownloadAllFavoriteButton from '../components/DownloadAllFavoriteButton.vue'
import { PhHeart, PhSignIn, PhWarningCircle } from '@phosphor-icons/vue'
import { getErrorMessage, formatCommandError } from '../utils/errors.ts'

const store = useStore()

const message = useMessage()

const emit = defineEmits<{
  (e: 'open-login'): void
}>()

const sortOptions: SelectProps['options'] = [
  { label: '收藏时间', value: 'FavoriteTime' },
  { label: '更新时间', value: 'UpdateTime' },
]

const sortSelected = ref<FavoriteSort>('FavoriteTime')
const pageSelected = ref<number>(1)
const folderIdSelected = ref<number>(0)
const loading = ref(false)
const errorMessage = ref('')
let favoriteRequestGeneration = 0

const favoritePageCount = computed(() => {
  if (store.getFavoriteResult === undefined) {
    return 0
  }
  const total = store.getFavoriteResult.total
  const pageSize = store.getFavoriteResult.count
  return pageSize > 0 ? Math.ceil(total / pageSize) : 0
})
const folderOptions = computed<SelectProps['options']>(() => [
  { label: '全部', value: 0 },
  ...(store.getFavoriteResult?.folderList || [])
    // FID 可能是空串或非数字：parseInt 会得到 NaN，经 IPC 序列化为 null 让整个请求失败
    .map((folder) => ({ label: folder.name, value: Number.parseInt(folder.FID, 10) }))
    .filter((folder) => Number.isSafeInteger(folder.value)),
])

watch(
  () => [store.userProfile?.uid, store.sessionEpoch] as const,
  async ([uid]) => {
    favoriteRequestGeneration += 1
    store.getFavoriteResult = undefined
    folderIdSelected.value = 0
    sortSelected.value = 'FavoriteTime'
    pageSelected.value = 1
    if (uid === undefined) {
      return
    }
    await getFavourite(0, 1, 'FavoriteTime')
  },
  { immediate: true },
)

async function getFavourite(folderId: number, page: number, sort: FavoriteSort) {
  const uid = store.userProfile?.uid
  if (uid === undefined) return

  const generation = ++favoriteRequestGeneration
  const epoch = store.sessionEpoch
  const observedRevision = store.favoriteRevision
  loading.value = true
  errorMessage.value = ''
  try {
    const result = await commands.getFavoriteFolder(folderId, page, sort)
    if (generation !== favoriteRequestGeneration || store.userProfile?.uid !== uid || epoch !== store.sessionEpoch) return
    if (result.status === 'error') {
      console.error(result.error)
      const errText = result.error.err_message || result.error.err_title
      errorMessage.value = errText
      message.error(formatCommandError(result.error))
      return
    }
    store.getFavoriteResult = result.data
    store.favoriteResultEpoch = epoch
    for (const comic of result.data.list) store.recordFavorite(comic.id, true, epoch, observedRevision)
    folderIdSelected.value = folderId
    sortSelected.value = sort
    pageSelected.value = page
    errorMessage.value = ''
  } catch (error) {
    if (generation === favoriteRequestGeneration && store.userProfile?.uid === uid) {
      console.error(error)
      const errText = getErrorMessage(error)
      errorMessage.value = errText
      message.error(`获取收藏夹失败：${errText}`)
    }
  } finally {
    if (generation === favoriteRequestGeneration) {
      loading.value = false
    }
  }
}

const syncing = ref(false)
async function syncFavoriteFolder() {
  // 防重：连点会重复发起同步 + 列表请求
  if (syncing.value) return
  syncing.value = true
  try {
    const result = await commands.syncFavoriteFolder()
    if (result.status === 'error') {
      console.error(result.error)
      message.error(formatCommandError(result.error))
      return
    }
    // 同步后只刷新当前视图：回到「全部/第 1 页/默认排序」会把用户正在看的
    // 收藏夹、页码和排序全部重置掉。
    await getFavourite(folderIdSelected.value, pageSelected.value, sortSelected.value)
    message.success('收藏夹已同步')
  } catch (error) {
    console.error(error)
    message.error(`同步收藏夹失败：${getErrorMessage(error)}`)
  } finally {
    syncing.value = false
  }
}
</script>

<template>
  <div class="favorite-pane">
    <div v-if="store.userProfile === undefined" class="favorite-empty">
      <div class="empty-icon"><PhHeart :size="31" weight="duotone" /></div>
      <h2>登录后查看收藏</h2>
      <p>登录后自动同步你的收藏夹。</p>
      <n-button type="primary" size="small" class="login-prompt-btn" @click="emit('open-login')">
        立即登录
      </n-button>
      <span>
        <PhSignIn :size="15" />
        账号信息只保存在本机
      </span>
    </div>

    <template v-else>
      <div v-if="store.getFavoriteResult !== undefined" class="favorite-toolbar">
        <n-select
          :value="folderIdSelected"
          :options="folderOptions"
          :show-checkmark="false"
          :loading="loading"
          size="small"
          @update-value="getFavourite($event, 1, sortSelected)" />
        <n-select
          :value="sortSelected"
          :options="sortOptions"
          :show-checkmark="false"
          :loading="loading"
          size="small"
          @update-value="getFavourite(folderIdSelected, 1, $event)" />
        <download-all-favorite-button :folder-id="folderIdSelected" />
      </div>

      <div v-if="store.getFavoriteResult !== undefined" class="favorite-actions">
        <n-button class="ml-auto" size="small" type="primary" secondary :loading="syncing" @click="syncFavoriteFolder">刷新收藏夹</n-button>
      </div>

      <div v-if="store.getFavoriteResult !== undefined && store.getFavoriteResult.list.length > 0" class="favorite-list">
        <ComicCard
          v-for="comicInFavorite in store.getFavoriteResult.list"
          :key="comicInFavorite.id"
          :comic-id="comicInFavorite.id"
          :comic-favorite="true"
          :favorite-epoch="store.favoriteResultEpoch"
          :comic-title="comicInFavorite.name"
          :comic-author="comicInFavorite.author"
          :comic-category="comicInFavorite.category"
          :comic-category-sub="comicInFavorite.categorySub"
          :comic-downloaded="comicInFavorite.isDownloaded"
          :comic-download-dir="comicInFavorite.comicDownloadDir" />
      </div>

      <div v-else-if="store.getFavoriteResult !== undefined && store.getFavoriteResult.list.length === 0" class="favorite-empty">
        <div class="empty-icon"><PhHeart :size="31" weight="duotone" /></div>
        <h2>暂无收藏漫画</h2>
        <p>换个分类，或到 JM 添加收藏。</p>
      </div>

      <div v-else-if="errorMessage" class="favorite-empty">
        <div class="empty-icon" style="color: #e03e2d; background: rgba(224, 62, 45, 0.1); border-color: rgba(224, 62, 45, 0.35);">
          <PhWarningCircle :size="31" weight="duotone" />
        </div>
        <h2>加载收藏失败</h2>
        <p>{{ errorMessage }}</p>
        <div class="flex items-center gap-2">
          <n-button type="primary" size="small" :loading="loading" @click="getFavourite(folderIdSelected, pageSelected, sortSelected)">
            点击重试
          </n-button>
          <n-button v-if="errorMessage.includes('登入') || errorMessage.includes('401')" size="small" secondary @click="emit('open-login')">
            重新登录
          </n-button>
        </div>
      </div>

      <div v-else class="favorite-loading">
        <n-spin size="small" />
        <span>正在同步收藏夹…</span>
      </div>

      <n-pagination
        v-if="store.getFavoriteResult !== undefined && favoritePageCount > 1"
        class="box-border p-2 pt-0 mt-auto"
        :page-count="favoritePageCount"
        :page="pageSelected"
        @update:page="getFavourite(folderIdSelected, $event, sortSelected)" />
    </template>
  </div>
</template>

<style scoped>
.favorite-pane {
  display: flex;
  height: 100%;
  min-height: 0;
  flex-direction: column;
}

.favorite-empty,
.favorite-loading {
  display: flex;
  flex: 1;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  padding: 30px;
  text-align: center;
}

.empty-icon {
  display: grid;
  width: 72px;
  height: 72px;
  margin-bottom: 18px;
  color: #fb7299;
  place-items: center;
  border: 1px solid rgba(251, 114, 153, 0.35);
  border-radius: 50%;
  background: rgba(251, 114, 153, 0.1);
  box-shadow: 0 0 0 8px rgba(251, 114, 153, 0.04);
}

.favorite-empty h2 {
  margin: 0;
  color: #18191c;
  font-size: 17px;
  font-weight: 600;
}

.favorite-empty p {
  max-width: 290px;
  margin: 8px 0 16px;
  color: #61666d;
  font-size: 12px;
  line-height: 1.65;
}

.favorite-empty > span {
  display: flex;
  align-items: center;
  gap: 5px;
  color: #9499a0;
  font-size: 11px;
}

.login-prompt-btn {
  margin: 4px 0 16px;
  border-radius: 999px;
  padding: 0 20px;
}

.favorite-toolbar,
.favorite-actions {
  display: flex;
  flex: 0 0 auto;
  gap: 7px;
  padding: 14px 14px 0;
}

.favorite-toolbar :deep(.n-select) {
  min-width: 0;
}

.favorite-actions {
  padding-top: 8px;
}

.favorite-list {
  display: flex;
  min-height: 0;
  flex: 1;
  flex-direction: column;
  gap: 9px;
  overflow: auto;
  padding: 10px 12px 12px;
}

.favorite-loading {
  gap: 10px;
  color: #9499a0;
  font-size: 12px;
}
</style>
