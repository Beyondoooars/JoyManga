<script setup lang="ts">
import { PhCircleNotch, PhFolderOpen } from '@phosphor-icons/vue'
import { ProgressData } from './ExportProgresses.vue'
import { commands } from '../../../bindings.ts'
import IconButton from '../../../components/IconButton.vue'
import { isAndroid } from '../../../composables/useIsMobile.ts'
import { exportToAndroidDownloads } from '../../../utils/androidStorage.ts'
import { useMessage } from 'naive-ui'
import { ref } from 'vue'
const message = useMessage()
const publishing = ref(false)
async function publish() {
  if (!props.p.chapterExportDir || publishing.value) return
  publishing.value = true
  try { const result = await exportToAndroidDownloads(props.p.chapterExportDir); if (!result.cancelled) message.success(`已保存 ${result.exported ?? 0} 个文件到公共目录`) }
  catch (error) { message.error(String(error)) }
  finally { publishing.value = false }
}

const props = defineProps<{
  p: ProgressData
  handleContextMenu: (p: ProgressData) => void
}>()

async function showChapterExportDirInFileManager() {
  if (props.p.chapterExportDir === undefined) {
    return
  }

  const result = await commands.showPathInFileManager(props.p.chapterExportDir)
  if (result.status === 'error') {
    console.error(result.error)
  }
}
</script>

<template>
  <div class="flex flex-col border border-solid rounded-md border-gray-2 p-1 mb-2" @contextmenu="handleContextMenu(p)">
    <div class="text-ellipsis whitespace-nowrap overflow-hidden" :title="p.comicTitle">
      {{ p.comicTitle }}
    </div>

    <div v-if="p.state === 'Processing'" class="flex">
      <n-icon class="text-blue-5 mr-2" :size="20">
        <PhCircleNotch class="animate-spin" />
      </n-icon>
      <n-progress class="text-blue-5" :percentage="p.percentage" processing>
        {{ p.indicator }}
      </n-progress>
    </div>
    <n-progress v-else-if="p.state === 'Error'" class="text-red-5" status="error" :percentage="p.percentage">
      {{ p.indicator }}
    </n-progress>
    <div v-else-if="p.state === 'End'" class="text-green-5 flex items-center ml-auto">
      <span>{{ p.indicator }}</span>
    </div>

    <n-button v-if="isAndroid && p.state === 'End' && p.chapterExportDir" size="small" :loading="publishing" @click="publish">保存到系统下载</n-button>
    <IconButton
      class="ml-auto"
      title="打开导出目录"
      v-if="p.chapterExportDir !== undefined"
      @click="showChapterExportDirInFileManager">
      <PhFolderOpen :size="24" />
    </IconButton>
  </div>
</template>
