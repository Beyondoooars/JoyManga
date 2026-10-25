<script setup lang="ts">
import { type ProgressData } from '../../../types.ts'
import { computed, ref } from 'vue'
import { useStore } from '../../../store.ts'
import { formatCommandError } from '../../../utils/errors.ts'
import { commands, type Comic } from '../../../bindings.ts'
import { PhBookOpen, PhCheckCircle, PhFileZip, PhFolderOpen, PhTrash } from '@phosphor-icons/vue'
import { useMessage } from 'naive-ui'
import { getErrorMessage } from '../../../utils/errors.ts'

const store = useStore()
const message = useMessage()

const completedProgresses = computed<[number, ProgressData][]>(() =>
  Array.from(store.progresses.entries())
    .filter(([, { state }]) => state === 'Completed')
    .sort((a, b) => {
      return b[1].totalImgCount - a[1].totalImgCount
    }),
)

function clearCompleted() {
  for (const [id, progress] of store.progresses.entries()) {
    if (progress.state === 'Completed') {
      store.progresses.delete(id)
    }
  }
}

function openReader(comicId: number, chapterId: number, comicTitle: string) {
  store.openReader(comicId, chapterId, comicTitle)
}

async function showComicDir(comic: Comic) {
  if (!comic.comicDownloadDir) return
  const result = await commands.showPathInFileManager(comic.comicDownloadDir)
  if (result.status === 'error') {
    console.error(result.error)
    message.error(formatCommandError(result.error))
  }
}

const exportingIds = ref<Set<number>>(new Set())

async function exportComic(comic: Comic) {
  // 防重：导出是长任务，连点会重复创建事件与文件
  if (exportingIds.value.has(comic.id)) return
  exportingIds.value.add(comic.id)
  store.progressesPaneTabName = 'export'
  try {
    const result = await commands.exportCbz(comic)
    if (result.status === 'error') {
      console.error(result.error)
      message.error(`${comic.name} 导出失败：${result.error.err_message}`)
    }
  } catch (error) {
    console.error(error)
    message.error(`${comic.name} 导出失败：${getErrorMessage(error)}`)
  } finally {
    exportingIds.value.delete(comic.id)
  }
}

async function exportCbz(comic: Comic) {
  await exportComic(comic)
}
</script>

<template>
  <div class="completed-pane">
    <div v-if="completedProgresses.length === 0" class="jm-empty transfer-empty">
      <div class="jm-empty-icon is-muted"><PhCheckCircle :size="28" weight="duotone" /></div>
      <strong>暂无已完成任务</strong>
      <p>下载完成的章节会保留在这里，方便回看与导出。</p>
    </div>

    <template v-else>
      <div class="completed-head">
        <span>共 {{ completedProgresses.length }} 个已完成章节</span>
        <button type="button" class="clear-btn" @click="clearCompleted">
          <PhTrash :size="13" />
          清空记录
        </button>
      </div>

      <div class="completed-list">
        <div
          v-for="[chapterId, { comic, chapterInfo }] in completedProgresses"
          :key="chapterId"
          class="completed-card">
          <div class="completed-copy">
            <strong :title="comic.name">{{ comic.name }}</strong>
            <span :title="chapterInfo.chapterTitle">{{ chapterInfo.chapterTitle }}</span>
          </div>
          <div class="completed-actions">
            <button type="button" class="row-action is-brand" :aria-label="`阅读 ${comic.name} ${chapterInfo.chapterTitle}`" @click="openReader(comic.id, chapterInfo.chapterId, comic.name)">
              <PhBookOpen :size="17" />
            </button>
            <button v-if="comic.comicDownloadDir" type="button" class="row-action" aria-label="打开下载目录" @click="showComicDir(comic)">
              <PhFolderOpen :size="17" />
            </button>
            <button type="button" class="row-action is-accent" :disabled="exportingIds.has(comic.id)" aria-label="导出 CBZ" @click="exportCbz(comic)">
              <PhFileZip :size="17" />
            </button>
          </div>
        </div>
      </div>
    </template>
  </div>
</template>

<style scoped>
.completed-pane {
  height: 100%;
  overflow-y: auto;
  padding: 12px var(--jm-gutter) calc(24px + var(--safe-bottom));
}

.transfer-empty {
  min-height: 260px;
}

.completed-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
  padding-bottom: 10px;
  color: var(--jm-text-3);
  font-size: var(--jm-fs-xs);
}

.clear-btn {
  display: inline-flex;
  height: 30px;
  align-items: center;
  gap: 4px;
  padding: 0 11px;
  color: var(--jm-text-2);
  border: 1px solid var(--jm-line);
  border-radius: var(--jm-r-pill);
  background: var(--jm-surface);
  font-size: 11px;
  font-weight: 600;
  cursor: pointer;
}

.completed-list {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.completed-card {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 10px 12px;
  border: 1px solid var(--jm-line);
  border-radius: var(--jm-r-md);
  background: var(--jm-surface);
}

.completed-copy {
  display: flex;
  min-width: 0;
  flex: 1;
  flex-direction: column;
  gap: 2px;
}

.completed-copy strong {
  overflow: hidden;
  color: var(--jm-text);
  font-size: 12.5px;
  font-weight: 650;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.completed-copy span {
  overflow: hidden;
  color: var(--jm-text-3);
  font-size: 10.5px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.completed-actions {
  display: flex;
  flex: 0 0 auto;
  gap: 3px;
}

.row-action {
  display: grid;
  width: 34px;
  height: 34px;
  place-items: center;
  padding: 0;
  color: var(--jm-text-2);
  border: 0;
  border-radius: var(--jm-r-sm);
  background: var(--jm-sunken);
  cursor: pointer;
}

.row-action.is-brand {
  color: var(--jm-brand);
  background: var(--jm-brand-soft);
}

.row-action.is-accent {
  color: var(--jm-accent);
  background: var(--jm-accent-soft);
}

.row-action:disabled {
  opacity: 0.55;
  cursor: wait;
}

@media (pointer: coarse) {
  .row-action {
    width: 40px;
    height: 40px;
  }
}
</style>
