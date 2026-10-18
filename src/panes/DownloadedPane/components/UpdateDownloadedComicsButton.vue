<script setup lang="ts">
import { MessageReactive, useMessage } from 'naive-ui'
import { onMounted, onUnmounted, ref } from 'vue'
import { formatCommandError } from '../../../utils/errors.ts'
import { commands, events, UpdateDownloadedComicsEvent } from '../../../bindings.ts'
import { getErrorMessage } from '../../../utils/errors.ts'

const message = useMessage()


const running = ref(false)
let eventUnlisten: (() => void) | undefined
let disposed = false

type ProgressData = Extract<UpdateDownloadedComicsEvent, { event: 'CreateDownloadTasksStart' }>['data'] & {
  progressMessage: MessageReactive
}

const progresses = ref<Map<number, ProgressData>>(new Map())
let updateMessage: MessageReactive | undefined

onMounted(async () => {
  const unlisten = await events.updateDownloadedComicsEvent.listen(async ({ payload: updateEvent }) => {
    if (updateEvent.event === 'GetComicStart') {
      updateMessage = message.loading(`正在获取已下载漫画的最新数据`, { duration: 0 })
    } else if (updateEvent.event === 'GetComicProgress' && updateMessage !== undefined) {
      const { current, total } = updateEvent.data
      updateMessage.content = `正在获取已下载漫画的最新数据(${current}/${total})`
    } else if (updateEvent.event === 'CreateDownloadTasksStart') {
      const { comicId, comicTitle, current, total } = updateEvent.data
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
    } else if (updateEvent.event === 'CreateDownloadTaskProgress') {
      const { comicId, current } = updateEvent.data
      const progressData = progresses.value.get(comicId)
      if (progressData) {
        progressData.current = current
      }
    } else if (updateEvent.event === 'CreateDownloadTasksEnd' && updateMessage !== undefined) {
      const { comicId } = updateEvent.data
      const progressData = progresses.value.get(comicId)
      if (progressData) {
        progressData.progressMessage.type = 'success'
        progressData.progressMessage.content = `${progressData.comicTitle} 创建下载任务完成(${progressData.current}/${progressData.total})`
        setTimeout(() => {
          progressData.progressMessage.destroy()
          progresses.value.delete(comicId)
        }, 3000)
      }
    } else if (updateEvent.event === 'GetComicEnd' && updateMessage !== undefined) {
      updateMessage.type = 'success'
      updateMessage.content = '已检查更新，新章节已加入下载队列'
      setTimeout(() => {
        updateMessage?.destroy()
        updateMessage = undefined
      }, 5000)
    }
  })
  if (disposed) unlisten()
  else eventUnlisten = unlisten
})

onUnmounted(() => {
  disposed = true
  eventUnlisten?.()
  updateMessage?.destroy()
  progresses.value.forEach((progress) => progress.progressMessage.destroy())
})

function clearProgressMessages() {
  updateMessage?.destroy()
  updateMessage = undefined
  progresses.value.forEach((progress) => progress.progressMessage.destroy())
  progresses.value.clear()
}

async function run() {
  if (running.value) return

  running.value = true
  try {
    const result = await commands.updateDownloadedComics()
    if (result.status === 'error') {
      console.error(result.error)
      message.error(formatCommandError(result.error))
      clearProgressMessages()
    }
  } catch (error) {
    console.error(error)
    message.error(`更新库存失败：${getErrorMessage(error)}`)
    clearProgressMessages()
  } finally {
    running.value = false
  }
}

</script>

<template>
  <n-button size="small" :loading="running" :disabled="running" @click="run">更新书库</n-button>
</template>

<style scoped>
.update-library-button {
  height: 36px;
  padding-inline: 11px;
}
</style>
