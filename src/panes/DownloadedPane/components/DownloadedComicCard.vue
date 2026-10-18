<script setup lang="ts">
import { Comic, commands } from '../../../bindings.ts'
import { useStore } from '../../../store.ts'
import { PhArchive, PhBookOpen, PhCopy, PhFolderOpen, PhImage } from '@phosphor-icons/vue'
import { computed, ref } from 'vue'
import { useMessage } from 'naive-ui'
import IconButton from '../../../components/IconButton.vue'
import { getErrorMessage } from '../../../utils/errors.ts'
import { useIsMobile } from '../../../composables/useIsMobile.ts'
import { isMergeableComic, localChapters } from '../../../utils/mergeableComics.ts'

const store = useStore()
const message = useMessage()
const { isMobile } = useIsMobile()
const merging = ref(false)
const coverFailed = ref(false)
const progress = computed(() => store.readingState.progress[String(props.comic.id)])
const downloadedChapters = computed(() => localChapters(props.comic).length)

const props = defineProps<{
  comic: Comic
  checked: boolean
}>()

const emit = defineEmits<{
  'toggle-check': [comic: Comic]
  'card-contextmenu': [comic: Comic]
}>()

function pickComic() {
  if (props.comic.id < 0) { openReader(); return }
  store.showComic(props.comic)
}

// 卡片内的按钮/复选框按 Enter 时事件会冒泡到这里，不能同时触发“打开阅读器”
function onCardEnter(event: KeyboardEvent) {
  if (event.target !== event.currentTarget) return
  openReader()
}

function openReader() {
  if (store.isArchiving(props.comic.id)) {
    message.warning('正在整合 CBZ，完成后即可阅读')
    return
  }
  store.continueReading(props.comic)
}

async function showComicDownloadDirInFileManager() {
  if (store.config === undefined) {
    return
  }

  const comicDownloadDir = props.comic.comicDownloadDir

  if (comicDownloadDir === undefined || comicDownloadDir === null) {
    console.error('comicDownloadDir的值为undefined或null')
    return
  }

  if (isMobile.value) {
    try {
      await navigator.clipboard.writeText(comicDownloadDir)
      message.success('已复制到剪贴板')
    } catch {
      message.error('复制失败，请重试')
    }
    return
  }

  const result = await commands.showPathInFileManager(comicDownloadDir)
  if (result.status === 'error') {
    console.error(result.error)
  }
}

async function handleMergeCbz() {
  if (merging.value || !isMergeableComic(props.comic) || store.isArchiving(props.comic.id)) return
  merging.value = true
  try {
    const result = await commands.promoteComicArchive(props.comic.id, 'merge')
    if (result.status === 'error') {
      message.error(`整理为整本失败：${result.error.err_message}`)
    } else {
      message.success(`已开始整理《${props.comic.name}》；完成后分话文件自动清理`)
    }
  } catch (err) {
    message.error(`整理为整本失败：${getErrorMessage(err)}`)
  } finally {
    merging.value = false
  }
}
</script>

<template>
  <div
    class="downloaded-comic-card"
    tabindex="0"
    role="button"
    :aria-label="`打开《${comic.name}》`"
    @contextmenu="emit('card-contextmenu', comic)"
    @keydown.enter.prevent="onCardEnter">
    <n-checkbox
      size="large"
      :aria-label="`选择 ${comic.name}`"
      class="select-checkbox"
      :checked="checked"
      @click="emit('toggle-check', comic)" />
    <button class="cover-button" :aria-label="`阅读 ${comic.name}`" @click="openReader">
    <img v-if="!coverFailed && (comic.localCoverData || comic.id > 0)"
      class="comic-cover"
      :src="comic.localCoverData || `https://${store.imageDomain}/media/albums/${comic.id}_3x4.jpg`"
      :alt="comic.name"
      :draggable="false"
      loading="lazy"
      @error="coverFailed = true"
      referrerpolicy="no-referrer" />
    <div v-else class="cover-placeholder"><PhImage :size="26" /><span>{{ comic.name.slice(0, 8) }}</span></div>
    </button>
    <div class="comic-details">
      <div class="title-row">
        <span class="origin-badge">{{ comic.id < 0 ? '导入' : '离线' }}</span>
        <span
          class="comic-title"
          @click="pickComic">
          {{ comic.name }}
        </span>
      </div>
      <div class="author-row">
        <span>作者：</span>
        <span
          v-for="a in (Array.isArray(comic.author) ? comic.author : [comic.author])"
          :key="a"
          class="author-name"
          @click="comic.id > 0 && store.searchKeyword(a)">
          {{ a }}
        </span>
      </div>
      <div class="chapter-summary">
        {{ downloadedChapters }} 话已缓存<span v-if="progress"> · 上次读到第 {{ progress.page }} 页</span>
        <span v-if="store.isArchiving(comic.id)" class="archiving-badge">正在整合 CBZ…</span>
      </div>
      <div v-if="progress" class="reading-progress"><span :style="{ width: `${Math.min(100, progress.page / progress.totalPages * 100)}%` }"></span></div>
      <div class="card-bottom-actions">
        <IconButton
          :title="isMobile ? '复制下载目录到剪贴板' : '打开下载目录'"
          :aria-label="isMobile ? '复制下载目录' : '打开下载目录'"
          @click="showComicDownloadDirInFileManager">
          <PhCopy v-if="isMobile" :size="20" />
          <PhFolderOpen v-else :size="22" />
        </IconButton>

        <button
          v-if="isMergeableComic(comic)"
          type="button"
          class="merge-button"
          :aria-label="`整理《${comic.name}》已下载的 ${downloadedChapters} 话为整本 CBZ`"
          :disabled="merging || store.isArchiving(comic.id)"
          @click="handleMergeCbz">
          <PhArchive :size="16" aria-hidden="true" />{{ merging ? '正在发起…' : store.isArchiving(comic.id) ? '整合中…' : '整理为整本' }}
        </button>

        <button class="read-button" :disabled="store.isArchiving(comic.id)" @click="openReader"><PhBookOpen :size="15" />{{ progress ? '继续阅读' : '开始阅读' }}</button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.merge-button { display: inline-flex; align-items: center; gap: 6px; min-height: 40px; padding: 8px 10px; border: 1px solid #e3e5e7; border-radius: 8px; background: #fff; color: #b73963; font-size: 12px; cursor: pointer; }
.merge-button:hover { border-color: #b73963; background: #fff0f6; }
.merge-button:focus-visible { outline: 2px solid #b73963; outline-offset: 2px; }
.merge-button:disabled { opacity: .55; cursor: wait; }
@media (max-width: 600px) { .merge-button { min-height: 44px; } }
.cover-button { padding: 0; border: 0; background: none; cursor: pointer; margin-right: 14px; flex-shrink: 0; border-radius: 9px; overflow: hidden; }
.cover-button .comic-cover { margin: 0; display: block; }
.cover-placeholder { display: flex; align-items: center; justify-content: center; flex-direction: column; gap: 12px; width: 96px; height: 128px; color: #b8a1ba; background: linear-gradient(145deg, #f5ecf6, #f5f3fb); border-radius: 8px; }
.cover-placeholder span { color: #957d97; font-size: 11px; max-width: 80px; text-align: center; }
.origin-badge { flex-shrink: 0; font-size: 9px; color: #b67b91; background: #fcedf3; padding: 2px 5px; border-radius: 4px; margin-top: 2px; }
.chapter-summary { color: #aaa0ae; font-size: 10px; margin-bottom: 9px; }
.archiving-badge { margin-left: 6px; color: #d97706; background: #fef3c7; padding: 1px 6px; border-radius: 999px; font-weight: 600; }
.reading-progress { height: 3px; background: #f5edf2; border-radius: 10px; margin: 0 0 9px; overflow: hidden; }
.reading-progress span { height: 100%; display: block; background: linear-gradient(90deg, #f8b2c9, #d984ae); border-radius: inherit; }
.read-button { display: flex; align-items: center; gap: 5px; padding: 6px 9px; font-size: 10px; color: #b8597c; background: #fff0f6; border: 0; border-radius: 7px; cursor: pointer; white-space: nowrap; }
/* 触屏：主操作需可点、可读 —— 高度 44dp、字号不低于辅助级 11px */
@media (max-width: 600px) {
  .read-button { min-height: 44px; padding: 8px 14px; font-size: 12px; border-radius: 8px; }
  .read-button svg { width: 17px; height: 17px; }
}
.downloaded-comic-card {
  position: relative;
  display: flex;
  background-color: #ffffff;
  border: 1px solid #efebf1;
  border-radius: var(--radius-card);
  padding: var(--card-pad);
  transition: all 0.2s ease;
}

.downloaded-comic-card:hover {
  border-color: #fb7299;
  box-shadow: var(--shadow-card);
}

.downloaded-comic-card:focus-visible {
  outline: 2px solid #fb7299;
  outline-offset: 2px;
}

.select-checkbox {
  position: absolute;
  top: 12px;
  left: 12px;
  z-index: 2;
}

/* 触屏：18px 的勾选框实际可点区域扩到 44dp（视觉尺寸不变） */
@media (max-width: 600px) {
  .select-checkbox {
    top: 2px;
    left: 2px;
    padding: 13px;
    box-sizing: content-box;
  }
}

.comic-cover {
  width: 96px;
  height: 128px;
  object-fit: cover;
  margin-right: 14px;
  border-radius: 8px;
  background-color: #f1f2f3;
  flex-shrink: 0;
}

.comic-details {
  display: flex;
  flex-direction: column;
  flex: 1;
  min-width: 0;
}

.title-row {
  display: flex;
  align-items: flex-start;
  gap: 6px;
}

.bili-up-badge {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  font-size: 10px;
  font-weight: 700;
  color: #00aeec;
  border: 1px solid rgba(0, 174, 236, 0.4);
  background-color: rgba(0, 174, 236, 0.08);
  border-radius: 4px;
  padding: 1px 4px;
  line-height: 1.2;
  flex-shrink: 0;
  margin-top: 3px;
}

.comic-title {
  font-weight: 700;
  font-size: 15px;
  color: #18191c;
  line-height: 1.4;
  cursor: pointer;
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
  overflow: hidden;
  transition: color 0.2s;
}

.comic-title:hover {
  color: #fb7299;
}

.author-row {
  display: flex;
  align-items: center;
  gap: 4px;
  flex-wrap: wrap;
  font-size: 12px;
  margin: 6px 0 8px;
  color: #9499a0;
}

.author-name {
  color: #61666d;
  cursor: pointer;
  transition: color 0.2s;
}

.author-name:hover {
  color: #fb7299;
  text-decoration: underline;
}

.card-bottom-actions {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 4px;
  margin-top: auto;
}
</style>
