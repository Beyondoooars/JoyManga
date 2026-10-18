<script setup lang="ts">
import { ref } from 'vue'
import { CategoryRespData, CategorySubRespData, commands } from '../bindings.ts'
import { useStore } from '../store.ts'
import IconButton from './IconButton.vue'
import FavoriteButton from './FavoriteButton.vue'
import { PhCopy, PhDownloadSimple, PhFolderOpen, PhImageBroken } from '@phosphor-icons/vue'
import { useMessage } from 'naive-ui'
import { getErrorMessage, formatCommandError } from '../utils/errors.ts'
import { useIsMobile } from '../composables/useIsMobile.ts'

const store = useStore()
const message = useMessage()
const { isMobile } = useIsMobile()
const coverFailed = ref(false)
const opening = ref(false)
const downloading = ref(false)

const props = defineProps<{
  comicId: number
  comicTitle: string
  comicAuthor: string
  comicCategory: CategoryRespData
  comicCategorySub: CategorySubRespData
  comicFavorite?: boolean
  favoriteEpoch?: number
  comicDownloaded: boolean
  comicDownloadDir: string
}>()

async function pickComic() {
  if (opening.value) return
  opening.value = true
  try {
    const result = await store.openComic(props.comicId)
    if (result.status === 'error') {
      console.error(result.error)
      message.error(formatCommandError(result.error))
    }
  } catch (error) {
    console.error(error)
    message.error(`打开漫画失败：${getErrorMessage(error)}`)
  } finally {
    opening.value = false
  }
}

async function downloadComic() {
  if (downloading.value) return
  downloading.value = true
  try {
    const result = await commands.downloadComic(props.comicId, null)
    if (result.status === 'error') {
      console.error(result.error)
      message.error(formatCommandError(result.error))
    }
  } catch (error) {
    console.error(error)
    message.error(`创建下载任务失败：${getErrorMessage(error)}`)
  } finally {
    downloading.value = false
  }
}

async function showComicDownloadDirInFileManager() {
  if (store.config === undefined) {
    return
  }
  if (isMobile.value) {
    try {
      await navigator.clipboard.writeText(props.comicDownloadDir)
      message.success('已复制到剪贴板')
    } catch {
      message.error('复制失败，请重试')
    }
    return
  }
  const result = await commands.showPathInFileManager(props.comicDownloadDir)
  if (result.status === 'error') {
    console.error(result.error)
    message.error(formatCommandError(result.error))
  }
}
</script>

<template>
  <n-card class="comic-card" content-style="padding: 0;" :bordered="false">
    <div class="comic-card-layout">
      <div class="cover-wrap" :aria-busy="opening" @click="pickComic">
        <img
          v-if="!coverFailed"
          class="comic-cover"
          :src="`https://${store.imageDomain}/media/albums/${comicId}_3x4.jpg`"
          :alt="comicTitle"
          referrerpolicy="no-referrer"
          @error="coverFailed = true" />
        <div v-else class="cover-fallback"><PhImageBroken :size="25" weight="duotone" /></div>
        <span v-if="comicDownloaded" class="downloaded-badge">已入库</span>
      </div>
      <div class="comic-info">
        <div>
          <button type="button" class="comic-title" :title="comicTitle" :disabled="opening" @click="pickComic">
            {{ comicTitle }}
          </button>
          <div class="comic-author">
            <span class="bili-up-badge">作者</span>
            <span
              v-if="comicAuthor"
              class="cursor-pointer hover:underline hover:text-[#fb7299] transition-colors"
              @click.stop="store.searchKeyword(comicAuthor)">
              {{ comicAuthor }}
            </span>
            <span v-else>佚名作者</span>
          </div>
          <div class="comic-categories">
            <span
              v-if="comicCategory.title"
              class="cursor-pointer hover:text-[#fb7299] transition-colors"
              @click.stop="store.searchKeyword(comicCategory.title)">
              {{ comicCategory.title }}
            </span>
            <span
              v-if="comicCategorySub.title"
              class="cursor-pointer hover:text-[#fb7299] transition-colors"
              @click.stop="store.searchKeyword(comicCategorySub.title)">
              {{ comicCategorySub.title }}
            </span>
          </div>
        </div>
        <div class="comic-actions">
          <FavoriteButton :comic-id="comicId" :initial-state="comicFavorite" :initial-epoch="favoriteEpoch" />
          <IconButton
            v-if="comicDownloaded"
            :aria-label="isMobile ? '复制下载目录' : '打开下载目录'"
            :title="isMobile ? '复制下载目录到剪贴板' : '打开下载目录'"
            @click="showComicDownloadDirInFileManager">
            <PhCopy v-if="isMobile" :size="24" />
            <PhFolderOpen v-else :size="24" />
          </IconButton>
          <IconButton
            class="ml-auto"
            aria-label="一键下载所有章节"
            title="一键下载所有章节"
            :disabled="downloading"
            @click="downloadComic">
            <PhDownloadSimple :size="24" />
          </IconButton>
        </div>
      </div>
    </div>
  </n-card>
</template>

<style scoped>
.comic-card {
  flex: 0 0 auto;
  overflow: hidden;
  background: #ffffff;
  border: 1px solid #e3e5e7;
  border-radius: var(--radius-card);
  box-shadow: var(--shadow-surface);
  transition:
    transform 200ms ease,
    border-color 200ms ease,
    box-shadow 200ms ease;
}

.comic-card:hover {
  transform: translateY(-3px);
  border-color: #fb7299;
  box-shadow: 0 8px 20px rgba(251, 114, 153, 0.15);
}

.comic-card-layout {
  display: flex;
  min-height: 116px;
  padding: 10px;
}

.cover-wrap {
  position: relative;
  width: 82px;
  min-width: 82px;
  height: 108px;
  margin-right: 12px;
  overflow: hidden;
  cursor: pointer;
  border-radius: 8px;
  background: #f1f2f3;
  border: 1px solid #e3e5e7;
}

.comic-cover {
  width: 100%;
  height: 100%;
  object-fit: cover;
  transition: transform 260ms ease;
}

.cover-fallback {
  display: grid;
  width: 100%;
  height: 100%;
  color: #9499a0;
  place-items: center;
  background: #f1f2f3;
}

.cover-wrap:hover .comic-cover {
  transform: scale(1.06);
}

.downloaded-badge {
  position: absolute;
  right: 4px;
  bottom: 4px;
  padding: 2px 6px;
  color: #fff;
  border-radius: 4px;
  background: #00aeec;
  font-size: 9px;
  font-weight: 700;
  box-shadow: 0 1px 4px rgba(0, 0, 0, 0.2);
}

.comic-info {
  display: flex;
  min-width: 0;
  flex: 1;
  flex-direction: column;
  justify-content: space-between;
  padding: 2px 2px 0 0;
}

.comic-title {
  display: -webkit-box;
  width: 100%;
  padding: 0;
  overflow: hidden;
  color: #18191c;
  cursor: pointer;
  border: 0;
  background: transparent;
  font-size: 14px;
  font-weight: 600;
  line-height: 1.35;
  text-align: left;
  -webkit-box-orient: vertical;
  -webkit-line-clamp: 2;
  transition: color 150ms ease;
}

.comic-title:hover {
  color: #fb7299;
}

.comic-author {
  display: flex;
  align-items: center;
  gap: 5px;
  margin-top: 6px;
  overflow: hidden;
  color: #9499a0;
  font-size: 11px;
}

.bili-up-badge {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  padding: 1px 4px;
  border-radius: 4px;
  background: rgba(0, 174, 236, 0.12);
  color: #00aeec;
  font-size: 9px;
  font-weight: 800;
  line-height: 1;
  flex-shrink: 0;
}

.comic-author span {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.comic-categories {
  display: flex;
  gap: 5px;
  margin-top: 6px;
  overflow: hidden;
}

.comic-categories span {
  max-width: 46%;
  padding: 2px 7px;
  overflow: hidden;
  color: #61666d;
  border: 1px solid #e3e5e7;
  border-radius: 999px;
  background: #f1f2f3;
  font-size: 10px;
  line-height: 1.2;
  text-overflow: ellipsis;
  white-space: nowrap;
  transition: all 0.15s;
}

.comic-categories span:hover {
  background: #ffffff;
  border-color: #fb7299;
  color: #fb7299;
}

.comic-actions {
  display: flex;
  align-items: center;
  min-height: 32px;
  margin-top: 4px;
}

.comic-actions :deep(button) {
  color: #61666d;
}

.comic-actions :deep(button:hover) {
  color: #fb7299;
  background: rgba(251, 114, 153, 0.1);
}
</style>
