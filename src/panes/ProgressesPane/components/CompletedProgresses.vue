<script setup lang="ts">
import { ProgressData } from '../../../types.ts'
import { computed, ref } from 'vue'
import { useStore } from '../../../store.ts'
import { formatCommandError } from '../../../utils/errors.ts'
import { commands, Comic } from '../../../bindings.ts'
import { PhBookOpen, PhCheckCircle, PhFilePdf, PhFileZip, PhFolderOpen, PhTrash } from '@phosphor-icons/vue'
import IconButton from '../../../components/IconButton.vue'
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

async function exportComic(comic: Comic, exportType: 'cbz' | 'pdf') {
  // 防重：导出是长任务，连点会重复创建事件与文件
  if (exportingIds.value.has(comic.id)) return
  exportingIds.value.add(comic.id)
  store.progressesPaneTabName = 'export'
  try {
    const result = exportType === 'cbz'
      ? await commands.exportCbz(comic)
      : await commands.exportPdf(comic)
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
  await exportComic(comic, 'cbz')
}

async function exportPdf(comic: Comic) {
  await exportComic(comic, 'pdf')
}
</script>

<template>
  <div class="h-full flex flex-col gap-row-2 px-2 overflow-auto">
    <div v-if="completedProgresses.length === 0" class="completed-empty">
      <PhCheckCircle :size="28" weight="duotone" />
      <strong>暂无已完成任务</strong>
      <span>完成的章节会保留在这里</span>
    </div>
    <template v-else>
      <div class="flex items-center justify-between pb-1">
        <span class="text-xs text-gray-500">共 {{ completedProgresses.length }} 个已完成章节</span>
        <n-button size="tiny" secondary type="error" @click="clearCompleted">
          <template #icon>
            <PhTrash :size="14" />
          </template>
          清空记录
        </n-button>
      </div>
      <div
        class="flex flex-col py-2 px-3 bg-gray-50 hover:bg-gray-100 rounded-lg border border-gray-100 transition-colors"
        v-for="[chapterId, { comic, chapterInfo }] in completedProgresses"
        :key="chapterId">
        <div class="grid grid-cols-[minmax(0,1fr)_minmax(0,1fr)] gap-2 mb-1">
          <span class="text-ellipsis whitespace-nowrap overflow-hidden font-medium text-xs text-gray-700" :title="comic.name">
            {{ comic.name }}
          </span>
          <span class="text-ellipsis whitespace-nowrap overflow-hidden text-xs text-gray-500" :title="chapterInfo.chapterTitle">
            {{ chapterInfo.chapterTitle }}
          </span>
        </div>
        <div class="flex items-center gap-1 mt-1">
          <IconButton title="阅读章节" @click="openReader(comic.id, chapterInfo.chapterId, comic.name)">
            <PhBookOpen :size="18" />
          </IconButton>
          <IconButton v-if="comic.comicDownloadDir" title="打开下载目录" @click="showComicDir(comic)">
            <PhFolderOpen :size="18" />
          </IconButton>
          <IconButton class="ml-auto" title="导出cbz" @click="exportCbz(comic)">
            <PhFileZip :size="18" />
          </IconButton>
          <IconButton title="导出pdf" @click="exportPdf(comic)">
            <PhFilePdf :size="18" />
          </IconButton>
        </div>
      </div>
    </template>
  </div>
</template>

<style scoped>
.completed-empty {
  display: flex;
  min-height: 230px;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  color: #aaa59d;
  text-align: center;
}

.completed-empty svg {
  margin-bottom: 10px;
  color: #36a269;
}

.completed-empty strong {
  color: #4d4a45;
  font-size: 13px;
}

.completed-empty span {
  margin-top: 5px;
  font-size: 9px;
}
</style>
