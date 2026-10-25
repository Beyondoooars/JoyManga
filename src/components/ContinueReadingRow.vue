<script setup lang="ts">
import { computed, onMounted } from 'vue'
import { PhBookOpen, PhCaretRight } from '@phosphor-icons/vue'
import { useStore } from '../store.ts'
import type { Comic } from '../bindings.ts'

const store = useStore()

const reading = computed(() => {
  const byId = new Map(store.downloadedComics.map((comic) => [String(comic.id), comic]))
  return Object.values(store.readingState.progress)
    .slice()
    .sort((a, b) => b.updatedAt - a.updatedAt)
    .map((progress) => ({ progress, comic: byId.get(String(progress.comicId)) }))
    .filter((item): item is { progress: (typeof store.readingState.progress)[string]; comic: Comic } => item.comic !== undefined)
    .slice(0, 6)
})

function percentOf(page: number, total: number) {
  if (!Number.isFinite(total) || total <= 0) return 0
  return Math.min(100, Math.max(1, Math.round((page / total) * 100)))
}

function coverFor(comic: Comic) {
  return comic.localCoverData || `https://${store.imageDomain}/media/albums/${comic.id}_3x4.jpg`
}

onMounted(() => {
  if (!store.downloadedComicsLoaded && !store.loadingTasks) {
    void store.refreshDownloadedComics().catch((error) => console.error('加载继续阅读失败', error))
  }
})
</script>

<template>
  <section v-if="reading.length > 0" class="continue-section" aria-labelledby="continue-heading">
    <header class="continue-head">
      <h2 id="continue-heading">继续阅读</h2>
      <span class="continue-sub">接着上次的位置</span>
    </header>
    <div class="continue-rail">
      <button
        v-for="{ progress, comic } in reading"
        :key="progress.comicId"
        type="button"
        class="continue-card"
        :aria-label="`继续阅读《${comic.name}》第 ${progress.page} 页`"
        @click="store.continueReading(comic)">
        <img class="continue-cover" :src="coverFor(comic)" :alt="comic.name" loading="lazy" referrerpolicy="no-referrer" />
        <span class="continue-body">
          <span class="continue-title">{{ comic.name }}</span>
          <span class="continue-chapter">{{ progress.chapterTitle }} · 第 {{ progress.page }} 页</span>
          <span class="continue-bar" role="progressbar" :aria-valuenow="percentOf(progress.page, progress.totalPages)" aria-valuemin="0" aria-valuemax="100">
            <span class="continue-fill" :style="{ width: `${percentOf(progress.page, progress.totalPages)}%` }"></span>
          </span>
          <span class="continue-resume">
            <PhBookOpen :size="13" weight="bold" />
            继续阅读
            <PhCaretRight :size="12" weight="bold" />
          </span>
        </span>
      </button>
    </div>
  </section>
</template>

<style scoped>
.continue-section {
  flex: 0 0 auto;
  padding: 14px 0 4px;
  background: var(--jm-surface);
}

.continue-head {
  display: flex;
  align-items: baseline;
  gap: 8px;
  padding: 0 var(--jm-gutter) 10px;
}

.continue-head h2 {
  margin: 0;
  color: var(--jm-text);
  font-size: var(--jm-fs-title);
  font-weight: 750;
  letter-spacing: -0.015em;
}

.continue-sub {
  color: var(--jm-text-3);
  font-size: var(--jm-fs-xs);
}

.continue-rail {
  display: flex;
  gap: 10px;
  padding: 0 var(--jm-gutter) 6px;
  overflow-x: auto;
  scrollbar-width: none;
}

.continue-rail::-webkit-scrollbar {
  display: none;
}

.continue-card {
  display: flex;
  width: 250px;
  flex: 0 0 auto;
  align-items: stretch;
  gap: 10px;
  padding: 8px;
  text-align: left;
  border: 1px solid var(--jm-line);
  border-radius: var(--jm-r-lg);
  background: var(--jm-surface);
  cursor: pointer;
  transition:
    border-color 160ms ease,
    box-shadow 160ms ease,
    transform 120ms ease;
}

.continue-card:active {
  transform: scale(0.985);
  border-color: rgba(251, 114, 153, 0.4);
}

.continue-cover {
  width: 60px;
  height: 80px;
  flex: 0 0 auto;
  object-fit: cover;
  border-radius: var(--jm-r-sm);
  background: var(--jm-sunken);
}

.continue-body {
  display: flex;
  min-width: 0;
  flex: 1;
  flex-direction: column;
  gap: 4px;
  padding: 2px 0;
}

.continue-title {
  display: -webkit-box;
  overflow: hidden;
  color: var(--jm-text);
  font-size: 13px;
  font-weight: 650;
  line-height: 1.3;
  -webkit-box-orient: vertical;
  -webkit-line-clamp: 2;
}

.continue-chapter {
  overflow: hidden;
  color: var(--jm-text-3);
  font-size: 10.5px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.continue-bar {
  height: 3px;
  margin-top: 2px;
  overflow: hidden;
  border-radius: var(--jm-r-pill);
  background: var(--jm-sunken);
}

.continue-fill {
  display: block;
  height: 100%;
  border-radius: inherit;
  background: linear-gradient(90deg, #FFA3BF, var(--jm-brand));
}

.continue-resume {
  display: flex;
  align-items: center;
  gap: 3px;
  margin-top: auto;
  color: var(--jm-brand);
  font-size: 10.5px;
  font-weight: 700;
}
</style>
