<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { commands, type FavoriteSort } from '../bindings.ts'
import { useMessage } from 'naive-ui'
import ComicCard from '../components/ComicCard.vue'
import { useStore } from '../store.ts'
import DownloadAllFavoriteButton from '../components/DownloadAllFavoriteButton.vue'
import { PhArrowsClockwise, PhFolderSimple, PhHeart, PhSignIn, PhWarningCircle } from '@phosphor-icons/vue'
import { getErrorMessage, formatCommandError } from '../utils/errors.ts'

const store = useStore()
const message = useMessage()

const emit = defineEmits<{ (e: 'open-login'): void }>()

const sortOptions: Array<{ label: string; value: FavoriteSort }> = [
  { label: '收藏时间', value: 'FavoriteTime' },
  { label: '更新时间', value: 'UpdateTime' },
]

const sortSelected = ref<FavoriteSort>('FavoriteTime')
const pageSelected = ref<number>(1)
const folderIdSelected = ref<number>(0)
const loading = ref(false)
const errorMessage = ref('')
const folderMenuOpen = ref(false)
let favoriteRequestGeneration = 0

const folders = computed(() =>
  (store.getFavoriteResult?.folderList || [])
    // FID 可能是空串或非数字：parseInt 会得到 NaN，经 IPC 序列化为 null 让整个请求失败
    .map((folder) => ({ label: folder.name, value: Number.parseInt(folder.FID, 10) }))
    .filter((folder) => Number.isSafeInteger(folder.value)),
)

const activeFolderLabel = computed(
  () => folders.value.find((folder) => folder.value === folderIdSelected.value)?.label ?? '全部收藏',
)

const favoritePageCount = computed(() => {
  if (store.getFavoriteResult === undefined) return 0
  const { total, count } = store.getFavoriteResult
  return count > 0 ? Math.ceil(total / count) : 0
})

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
      errorMessage.value = result.error.err_message || result.error.err_title
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
      errorMessage.value = getErrorMessage(error)
      message.error(`获取收藏夹失败：${errorMessage.value}`)
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

function pickFolder(folderId: number) {
  folderMenuOpen.value = false
  if (folderId === folderIdSelected.value) return
  void getFavourite(folderId, 1, sortSelected.value)
}
</script>

<template>
  <div class="favorite-page">
    <!-- 未登录 -->
    <div v-if="store.userProfile === undefined" class="jm-empty favorite-guest">
      <div class="jm-empty-icon"><PhHeart :size="29" weight="duotone" /></div>
      <strong>登录后查看收藏</strong>
      <p>登录即可同步你在 JM 的收藏夹，收藏状态会自动保持同步。</p>
      <div class="jm-empty-actions">
        <button type="button" class="jm-btn is-primary" @click="emit('open-login')">
          <PhSignIn :size="17" weight="bold" />
          立即登录
        </button>
      </div>
      <span class="guest-note">账号信息只保存在本机</span>
    </div>

    <template v-else>
      <!-- 工具栏：收藏夹 + 排序 + 同步 -->
      <div class="favorite-toolbar">
        <button
          type="button"
          class="folder-trigger"
          :aria-expanded="folderMenuOpen"
          :aria-label="`收藏夹：${activeFolderLabel}`"
          @click="folderMenuOpen = !folderMenuOpen">
          <PhFolderSimple :size="17" weight="duotone" />
          <span class="jm-truncate">{{ activeFolderLabel }}</span>
          <span v-if="folders.length > 0" class="folder-count">{{ folders.length }}</span>
        </button>
        <button
          type="button"
          class="jm-icon-btn"
          :disabled="loading || syncing"
          aria-label="同步收藏夹"
          title="同步收藏夹"
          @click="syncFavoriteFolder">
          <PhArrowsClockwise :size="19" :class="{ 'jm-spin': syncing }" />
        </button>
      </div>

      <div v-if="folderMenuOpen && folders.length > 0" class="folder-sheet">
        <button
          type="button"
          class="folder-option"
          :class="{ 'is-active': folderIdSelected === 0 }"
          @click="pickFolder(0)">
          全部收藏
        </button>
        <button
          v-for="folder in folders"
          :key="folder.value"
          type="button"
          class="folder-option"
          :class="{ 'is-active': folderIdSelected === folder.value }"
          @click="pickFolder(folder.value)">
          {{ folder.label }}
        </button>
      </div>

      <div class="favorite-actions">
        <div class="sort-row" role="group" aria-label="排序方式">
          <button
            v-for="option in sortOptions"
            :key="option.value"
            type="button"
            class="sort-chip"
            :class="{ 'is-active': sortSelected === option.value }"
            :disabled="loading"
            @click="getFavourite(folderIdSelected, 1, option.value)">
            {{ option.label }}
          </button>
        </div>
        <DownloadAllFavoriteButton :folder-id="folderIdSelected" />
      </div>

      <div class="jm-scroll favorite-scroll">
        <div v-if="store.getFavoriteResult === undefined && loading" class="cover-grid">
          <div v-for="n in 6" :key="n" class="skeleton-card">
            <div class="jm-skeleton skeleton-cover"></div>
            <div class="jm-skeleton skeleton-line"></div>
          </div>
        </div>

        <div v-else-if="errorMessage" class="jm-empty">
          <div class="jm-empty-icon is-muted"><PhWarningCircle :size="28" weight="duotone" /></div>
          <strong>加载收藏失败</strong>
          <p>{{ errorMessage }}</p>
          <div class="jm-empty-actions">
            <button type="button" class="jm-btn" :disabled="loading" @click="getFavourite(folderIdSelected, pageSelected, sortSelected)">
              重试
            </button>
            <button
              v-if="errorMessage.includes('登入') || errorMessage.includes('401')"
              type="button"
              class="jm-btn is-ghost"
              @click="emit('open-login')">
              重新登录
            </button>
          </div>
        </div>

        <div v-else-if="store.getFavoriteResult && store.getFavoriteResult.list.length === 0" class="jm-empty">
          <div class="jm-empty-icon is-muted"><PhHeart :size="28" weight="duotone" /></div>
          <strong>这里还没有收藏</strong>
          <p>在漫画详情页点收藏，或到 JM 添加收藏后回来同步。</p>
          <div class="jm-empty-actions">
            <button type="button" class="jm-btn is-ghost" :loading="syncing" @click="syncFavoriteFolder">同步收藏夹</button>
          </div>
        </div>

        <template v-else-if="store.getFavoriteResult">
          <div class="result-head">
            <strong>{{ activeFolderLabel }}</strong>
            <span>{{ store.getFavoriteResult.total }} 部</span>
          </div>
          <div class="cover-grid">
            <ComicCard
              v-for="comic in store.getFavoriteResult.list"
              :key="comic.id"
              :comic-id="comic.id"
              :comic-favorite="true"
              :favorite-epoch="store.favoriteResultEpoch"
              :comic-title="comic.name"
              :comic-author="comic.author"
              :comic-category="comic.category"
              :comic-category-sub="comic.categorySub"
              :comic-downloaded="comic.isDownloaded"
              :comic-download-dir="comic.comicDownloadDir" />
          </div>

          <nav v-if="favoritePageCount > 1" class="pager" aria-label="收藏分页">
            <button
              type="button"
              class="pager-btn"
              :disabled="pageSelected <= 1 || loading"
              aria-label="上一页"
              @click="getFavourite(folderIdSelected, pageSelected - 1, sortSelected)">
              ‹
            </button>
            <span class="pager-info">{{ pageSelected }} / {{ favoritePageCount }}</span>
            <button
              type="button"
              class="pager-btn"
              :disabled="pageSelected >= favoritePageCount || loading"
              aria-label="下一页"
              @click="getFavourite(folderIdSelected, pageSelected + 1, sortSelected)">
              ›
            </button>
          </nav>
        </template>
      </div>
    </template>
  </div>
</template>

<style scoped>
.favorite-page {
  display: flex;
  height: 100%;
  min-height: 0;
  flex-direction: column;
  background: var(--jm-surface);
}

.favorite-guest {
  background: var(--jm-surface);
}

.guest-note {
  margin-top: 14px;
  color: var(--jm-text-4);
  font-size: var(--jm-fs-xs);
}

.favorite-toolbar {
  display: flex;
  flex: 0 0 auto;
  align-items: center;
  gap: 8px;
  padding: 10px var(--jm-gutter) 8px;
  background: var(--jm-surface);
}

.folder-trigger {
  display: flex;
  height: 44px;
  min-width: 0;
  flex: 1;
  align-items: center;
  gap: 8px;
  padding: 0 14px;
  color: var(--jm-text);
  border: 1px solid var(--jm-line);
  border-radius: var(--jm-r-md);
  background: var(--jm-surface);
  font-size: 14px;
  font-weight: 650;
  text-align: left;
  cursor: pointer;
}

.folder-trigger:hover {
  border-color: var(--jm-line-strong);
}

.folder-trigger svg {
  flex: 0 0 auto;
  color: var(--jm-brand);
}

.folder-count {
  margin-left: auto;
  padding: 2px 7px;
  color: var(--jm-text-3);
  border-radius: var(--jm-r-pill);
  background: var(--jm-sunken);
  font-size: 10.5px;
  font-weight: 700;
  font-variant-numeric: tabular-nums;
}

.folder-sheet {
  display: flex;
  flex: 0 0 auto;
  gap: 7px;
  padding: 0 var(--jm-gutter) 10px;
  overflow-x: auto;
  scrollbar-width: none;
}

.folder-sheet::-webkit-scrollbar {
  display: none;
}

.folder-option {
  height: 32px;
  flex: 0 0 auto;
  padding: 0 13px;
  color: var(--jm-text-2);
  border: 1px solid transparent;
  border-radius: var(--jm-r-pill);
  background: var(--jm-sunken);
  font-size: var(--jm-fs-sm);
  font-weight: 600;
  white-space: nowrap;
  cursor: pointer;
}

.folder-option.is-active {
  color: var(--jm-brand);
  border-color: rgba(251, 114, 153, 0.28);
  background: var(--jm-brand-soft);
}

.favorite-actions {
  display: flex;
  flex: 0 0 auto;
  align-items: center;
  gap: 8px;
  padding: 0 var(--jm-gutter) 10px;
  border-bottom: 1px solid var(--jm-line);
  background: var(--jm-surface);
}

.sort-row {
  display: flex;
  gap: 6px;
}

.sort-chip {
  height: 30px;
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

.favorite-actions > :last-child {
  margin-left: auto;
}

.favorite-scroll {
  background: var(--jm-bg);
  padding-bottom: 24px;
}

.result-head {
  display: flex;
  align-items: baseline;
  gap: 8px;
  padding: 12px var(--jm-gutter) 0;
}

.result-head strong {
  overflow: hidden;
  max-width: 70%;
  color: var(--jm-text);
  font-size: var(--jm-fs-title);
  font-weight: 750;
  letter-spacing: -0.015em;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.result-head span {
  color: var(--jm-brand);
  font-size: var(--jm-fs-sm);
  font-weight: 700;
  font-variant-numeric: tabular-nums;
}

.cover-grid {
  padding: 12px var(--jm-gutter) 8px;
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
