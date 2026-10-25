<script setup lang="ts">
import { ref } from 'vue'
import { CategoryRespData, CategorySubRespData, commands } from '../bindings.ts'
import { useStore } from '../store.ts'
import FavoriteButton from './FavoriteButton.vue'
import { PhCheckCircle, PhDownloadSimple, PhImageBroken } from '@phosphor-icons/vue'
import { useMessage } from 'naive-ui'
import { getErrorMessage, formatCommandError } from '../utils/errors.ts'

const store = useStore()
const message = useMessage()

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

const categoryLabel = () => props.comicCategorySub.title || props.comicCategory.title || ''
</script>

<template>
  <article class="comic-card">
    <div class="cover-wrap" :aria-busy="opening">
      <button type="button" class="cover-hit" :aria-label="`打开《${comicTitle}》`" @click="pickComic">
        <img
          v-if="!coverFailed"
          class="comic-cover"
          :src="`https://${store.imageDomain}/media/albums/${comicId}_3x4.jpg`"
          :alt="comicTitle"
          loading="lazy"
          referrerpolicy="no-referrer"
          @error="coverFailed = true" />
        <span v-else class="cover-fallback"><PhImageBroken :size="26" weight="duotone" /></span>
      </button>

      <span v-if="comicDownloaded" class="downloaded-mark" title="已入库">
        <PhCheckCircle :size="13" weight="fill" />
      </span>

      <FavoriteButton
        class="cover-favorite"
        variant="icon"
        :comic-id="comicId"
        :initial-state="comicFavorite"
        :initial-epoch="favoriteEpoch" />
    </div>

    <div class="card-meta">
      <button type="button" class="comic-title" :title="comicTitle" :disabled="opening" @click="pickComic">
        {{ comicTitle }}
      </button>
      <div class="meta-row">
        <button
          v-if="comicAuthor"
          type="button"
          class="author"
          :title="`搜索作者 ${comicAuthor}`"
          @click.stop="store.searchKeyword(comicAuthor)">
          {{ comicAuthor }}
        </button>
        <span v-else class="author is-muted">佚名</span>
        <span v-if="categoryLabel()" class="dot-sep" aria-hidden="true">·</span>
        <span v-if="categoryLabel()" class="category">{{ categoryLabel() }}</span>
        <button
          type="button"
          class="quick-download"
          :disabled="downloading || comicDownloaded"
          :title="comicDownloaded ? '已入库' : '下载全部章节'"
          :aria-label="comicDownloaded ? '已入库' : `下载《${comicTitle}》全部章节`"
          @click.stop="downloadComic">
          <PhDownloadSimple :size="17" weight="bold" />
        </button>
      </div>
    </div>
  </article>
</template>

<style scoped>
.comic-card {
  display: flex;
  min-width: 0;
  flex-direction: column;
  gap: 8px;
}

.cover-wrap {
  position: relative;
  overflow: hidden;
  aspect-ratio: 3 / 4;
  border-radius: var(--jm-r-md);
  background: var(--jm-sunken);
  box-shadow: var(--jm-shadow-1);
}

.cover-hit {
  display: block;
  width: 100%;
  height: 100%;
  padding: 0;
  border: 0;
  background: transparent;
  cursor: pointer;
}

.comic-cover {
  width: 100%;
  height: 100%;
  object-fit: cover;
  transition: transform 320ms cubic-bezier(0.2, 0.7, 0.3, 1);
}

.cover-wrap:active .comic-cover {
  transform: scale(0.985);
}

@media (hover: hover) {
  .comic-card:hover .comic-cover {
    transform: scale(1.04);
  }
}

.cover-fallback {
  display: grid;
  width: 100%;
  height: 100%;
  color: var(--jm-text-4);
  place-items: center;
  background: linear-gradient(150deg, #f3f0f5, #eef1f5);
}

/* 已入库：左下角小徽标，不用整块色带遮挡封面 */
.downloaded-mark {
  position: absolute;
  bottom: 6px;
  left: 6px;
  display: grid;
  width: 21px;
  height: 21px;
  color: #fff;
  place-items: center;
  border-radius: var(--jm-r-pill);
  background: var(--jm-accent);
  box-shadow: 0 2px 6px rgba(0, 0, 0, 0.22);
  pointer-events: none;
}

.cover-favorite {
  position: absolute;
  top: 6px;
  right: 6px;
}

.card-meta {
  display: flex;
  min-width: 0;
  flex-direction: column;
  gap: 3px;
  padding-inline: 1px;
}

.comic-title {
  display: -webkit-box;
  padding: 0;
  overflow: hidden;
  color: var(--jm-text);
  border: 0;
  background: transparent;
  font-size: 13.5px;
  font-weight: 650;
  line-height: 1.35;
  text-align: left;
  letter-spacing: -0.005em;
  -webkit-box-orient: vertical;
  -webkit-line-clamp: 2;
  cursor: pointer;
}

.comic-title:active {
  color: var(--jm-brand);
}

.meta-row {
  display: flex;
  min-width: 0;
  align-items: center;
  gap: 4px;
  color: var(--jm-text-3);
  font-size: 11px;
  line-height: 1.3;
}

.author {
  max-width: 55%;
  padding: 0;
  overflow: hidden;
  color: var(--jm-text-3);
  border: 0;
  background: none;
  font-size: inherit;
  font-weight: 500;
  text-align: left;
  text-overflow: ellipsis;
  white-space: nowrap;
  cursor: pointer;
}

.author.is-muted {
  cursor: default;
}

.dot-sep {
  opacity: 0.6;
}

.category {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.quick-download {
  display: grid;
  width: 30px;
  height: 30px;
  flex: 0 0 auto;
  margin-left: auto;
  margin-right: -4px;
  padding: 0;
  color: var(--jm-text-3);
  border: 0;
  border-radius: var(--jm-r-sm);
  background: transparent;
  place-items: center;
  cursor: pointer;
  transition:
    color 160ms ease,
    background-color 160ms ease;
}

.quick-download:hover:not(:disabled) {
  color: var(--jm-brand);
  background: var(--jm-brand-soft);
}

.quick-download:disabled {
  color: var(--jm-accent);
  cursor: default;
}
</style>
