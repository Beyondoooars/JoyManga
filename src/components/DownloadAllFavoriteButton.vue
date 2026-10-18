<script setup lang="ts">
import { onMounted, onUnmounted, ref } from 'vue'
import { commands, DownloadAllFavoritesEvent, events } from '../bindings.ts'
import { MessageReactive, useMessage } from 'naive-ui'
import { getErrorMessage, formatCommandError } from '../utils/errors.ts'

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
  <n-button size="small" :loading="running" :disabled="running" @click="run">{{ folderId === 0 ? '下载全部收藏' : '下载当前收藏夹' }}</n-button>
</template>
