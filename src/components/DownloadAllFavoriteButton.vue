<script setup lang="ts">
import { onMounted, onUnmounted, ref } from 'vue'
import { commands, type DownloadAllFavoritesEvent, events } from '../bindings.ts'
import { type MessageReactive, useMessage } from 'naive-ui'
import { getErrorMessage, formatCommandError } from '../utils/errors.ts'
import { PhDownloadSimple } from '@phosphor-icons/vue'

const props = withDefaults(defineProps<{ folderId?: number }>(), { folderId: 0 })

const running = ref(false)
let eventUnlisten: (() => void) | undefined
let disposed = false

type ProgressData = Extract<DownloadAllFavoritesEvent, { event: 'StartCreateDownloadTasks' }>['data'] & {
  progressMessage: MessageReactive
}

const message = useMessage()

const progresses = ref<Map<number, ProgressData>>(new Map())
let prepareMessage: MessageReactive | undefined

onMounted(async () => {
  const unlisten = await events.downloadAllFavoritesEvent.listen(({ payload }) => {
    if (payload.event === 'GetFavoritesStart') {
      prepareMessage = message.loading('正在获取收藏夹', { duration: 0 })
    } else if (payload.event === 'GetComicsProgress' && prepareMessage !== undefined) {
      const { current, total } = payload.data
      prepareMessage.content = `正在获取收藏夹中的漫画(${current}/${total})`
    } else if (payload.event === 'StartCreateDownloadTasks') {
      const { comicId, comicTitle, current, total } = payload.data
      progresses.value.set(comicId, {
        comicId,
        comicTitle,
        current,
        total,
        progressMessage: message.loading(
          () => {
            const progressData = progresses.value.get(comicId)
            if (progressData === undefined) return ''
            return `${progressData.comicTitle} 正在创建下载任务(${progressData.current}/${progressData.total})`
          },
          { duration: 0 },
        ),
      })
    } else if (payload.event === 'CreatingDownloadTask') {
      const { comicId, current } = payload.data
      const progressData = progresses.value.get(comicId)
      if (progressData) {
        progressData.current = current
      }
    } else if (payload.event === 'EndCreateDownloadTasks') {
      const { comicId } = payload.data
      const progressData = progresses.value.get(comicId)
      if (progressData) {
        progressData.progressMessage.type = 'success'
        progressData.progressMessage.content = `${progressData.comicTitle} 创建下载任务完成(${progressData.current}/${progressData.total})`
        setTimeout(() => {
          progressData.progressMessage.destroy()
          progresses.value.delete(comicId)
        }, 3000)
      }
    } else if (payload.event === 'GetComicsEnd' && prepareMessage !== undefined) {
      prepareMessage.type = 'success'
      prepareMessage.content = '已获取收藏夹全部漫画'
      setTimeout(() => {
        prepareMessage?.destroy()
        prepareMessage = undefined
      }, 3000)
    }
  })
  if (disposed) unlisten()
  else eventUnlisten = unlisten
})

onUnmounted(() => {
  disposed = true
  eventUnlisten?.()
  prepareMessage?.destroy()
  progresses.value.forEach((progress) => progress.progressMessage.destroy())
})

function clearProgressMessages() {
  prepareMessage?.destroy()
  prepareMessage = undefined
  progresses.value.forEach((progress) => progress.progressMessage.destroy())
  progresses.value.clear()
}

async function run() {
  if (running.value) return

  running.value = true
  try {
    const result = await commands.downloadAllFavorites(props.folderId)
    if (result.status === 'error') {
      console.error(result.error)
      message.error(formatCommandError(result.error))
      clearProgressMessages()
    }
  } catch (error) {
    console.error(error)
    message.error(`下载收藏夹失败：${getErrorMessage(error)}`)
    clearProgressMessages()
  } finally {
    running.value = false
  }
}
</script>

<template>
  <button
    type="button"
    class="download-all"
    :disabled="running"
    :title="folderId === 0 ? '把全部收藏加入下载队列' : '把当前收藏夹加入下载队列'"
    @click="run">
    <PhDownloadSimple :size="16" weight="bold" :class="{ 'jm-spin': running }" />
    <span>{{ folderId === 0 ? '全部下载' : '下载本夹' }}</span>
  </button>
</template>

<style scoped>
.download-all {
  display: inline-flex;
  height: 34px;
  flex: 0 0 auto;
  align-items: center;
  gap: 5px;
  padding: 0 13px;
  color: var(--jm-accent);
  border: 1px solid rgba(0, 174, 236, 0.24);
  border-radius: var(--jm-r-pill);
  background: var(--jm-accent-soft);
  font-size: var(--jm-fs-sm);
  font-weight: 650;
  white-space: nowrap;
  cursor: pointer;
  transition: background-color 160ms ease;
}

.download-all:hover:not(:disabled) {
  background: rgba(0, 174, 236, 0.16);
}

.download-all:disabled {
  opacity: 0.55;
  cursor: wait;
}
</style>
