<script setup lang="ts">
import { LogLevel, commands } from '../bindings.ts'
import { useNotification } from 'naive-ui'
import { ref, watch, computed } from 'vue'
import { useStore, type LogRecord } from '../store.ts'
import { darkTheme } from 'naive-ui'
import { getErrorMessage } from '../utils/errors.ts'

const store = useStore()

const notification = useNotification()

const showing = defineModel<boolean>('showing', { required: true })

const searchText = ref<string>('')
const selectedLevel = ref<LogLevel>('INFO')
const logsDirSize = ref<number>(0)

const formatedLogsDirSize = computed<string>(() => {
  const units = ['B', 'KB', 'MB']
  let size = logsDirSize.value
  let unitIndex = 0

  while (size >= 1024 && unitIndex < 2) {
    size /= 1024
    unitIndex++
  }

  // 保留两位小数
  return `${size.toFixed(2)} ${units[unitIndex]}`
})
const filteredLogs = computed<LogRecord[]>(() => {
  return store.logRecords.filter(({ level, formatedLog }) => {
    // 定义日志等级的优先级顺序
    const logLevelPriority: Record<LogLevel, number> = {
      TRACE: 0,
      DEBUG: 1,
      INFO: 2,
      WARN: 3,
      ERROR: 4,
    }
    // 首先按日志等级筛选
    if (logLevelPriority[level] < logLevelPriority[selectedLevel.value]) {
      return false
    }
    // 然后按搜索文本筛选
    if (searchText.value === '') {
      return true
    }

    return formatedLog.toLowerCase().includes(searchText.value.toLowerCase())
  })
})

watch(showing, async () => {
  if (showing.value) {
    const result = await commands.getLogsDirSize()
    if (result.status === 'error') {
      console.error(result.error)
      return
    }
    logsDirSize.value = result.data
  }
})

function getLevelStyles(level: LogLevel) {
  switch (level) {
    case 'TRACE':
      return 'text-gray-400'
    case 'DEBUG':
      return 'text-green-400'
    case 'INFO':
      return 'text-blue-400'
    case 'WARN':
      return 'text-yellow-400'
    case 'ERROR':
      return 'text-red-400'
  }
}

const logLevelOptions = [
  { value: 'TRACE', label: 'TRACE' },
  { value: 'DEBUG', label: 'DEBUG' },
  { value: 'INFO', label: 'INFO' },
  { value: 'WARN', label: 'WARN' },
  { value: 'ERROR', label: 'ERROR' },
]

function clearLogRecords() {
  store.clearLogRecords()
}

const exporting = ref(false)
const exportedFilePath = ref('')
const showExportModal = ref(false)

async function handleExportLogs() {
  exporting.value = true
  try {
    const memoryLogs = store.logRecords.slice(-1000).map(r => `[${r.level}] [${r.timestamp}] ${r.formatedLog}`)
    const result = await commands.exportLogs(memoryLogs)
    if (result.status === 'ok') {
      exportedFilePath.value = result.data.filePath
      showExportModal.value = true
      notification.success({
        title: '导出日志成功',
        description: `日志已保存至：${result.data.filePath}`,
        duration: 8000,
      })
    } else {
      notification.error({
        title: result.error.err_title,
        description: result.error.err_message,
      })
    }
  } catch (error) {
    console.error(error)
    notification.error({
      title: '导出日志失败',
      description: getErrorMessage(error),
    })
  } finally {
    exporting.value = false
  }
}

async function copyExportedPath() {
  try {
    await navigator.clipboard.writeText(exportedFilePath.value)
    notification.success({
      title: '复制成功',
      description: '已复制到剪贴板',
    })
  } catch (e) {
    notification.error({
      title: '复制失败',
      description: getErrorMessage(e),
    })
  }
}

async function showLogsDirInFileManager() {
  try {
    const result = await commands.showLogsInFileManager()
    if (result.status === 'error') {
      console.error(result.error)
      notification.error({
        title: result.error.err_title,
        description: result.error.err_message,
      })
    }
  } catch (error) {
    console.error(error)
    notification.error({
      title: '打开日志目录失败',
      description: getErrorMessage(error),
    })
  }
}
</script>

<template>
  <n-modal v-model:show="showing" v-if="store.config !== undefined">
    <n-dialog
      :showIcon="false"
      :title="`日志目录总大小：${formatedLogsDirSize}`"
      @close="showing = false"
      style="width: 95%">
      <div class="mb-2 flex flex-wrap gap-2">
        <n-input-group class="w-100">
          <n-input size="small" v-model:value="searchText" placeholder="搜索日志…" clearable />
          <n-select size="small" v-model:value="selectedLevel" :options="logLevelOptions" style="width: 120px" />
        </n-input-group>

        <div class="flex flex-wrap gap-2 ml-auto items-center">
          <n-button size="small" type="primary" secondary :loading="exporting" @click="handleExportLogs">导出日志</n-button>
          <n-button size="small" @click="showLogsDirInFileManager">打开日志目录</n-button>
          <n-checkbox v-model:checked="store.config.enableFileLogger">输出文件日志</n-checkbox>
        </div>
      </div>

      <n-config-provider :theme="darkTheme" :theme-overrides="{ Scrollbar: { width: '8px' } }">
        <n-virtual-list
          class="h-[calc(100vh-300px)] overflow-hidden bg-gray-900"
          :item-size="42"
          item-resizable
          :hoverable="false"
          :items="filteredLogs"
          :scrollbar-props="{ trigger: 'none' }">
          <template #default="{ item: { level, formatedLog } }: { item: LogRecord }">
            <div :class="['py-1 px-3 hover:bg-white/10 whitespace-pre-wrap mr-4', getLevelStyles(level)]">
              {{ formatedLog }}
            </div>
          </template>
        </n-virtual-list>
      </n-config-provider>
      <div class="pt-1 flex">
        <n-button ghost class="ml-auto" size="small" type="error" @click="clearLogRecords">清空日志</n-button>
      </div>
    </n-dialog>
  </n-modal>

  <n-modal v-model:show="showExportModal">
    <n-card title="日志导出成功" style="width: min(520px, 92vw)" :bordered="false" size="medium" role="dialog" aria-modal="true">
      <p class="text-sm text-gray-300 mb-2">日志已导出至：</p>
      <div class="p-2.5 rounded bg-black/40 border border-white/10 font-mono text-xs text-[#FB7299] break-all select-all mb-4">
        {{ exportedFilePath }}
      </div>
      <div class="flex justify-end gap-2">
        <n-button size="small" type="primary" @click="copyExportedPath">复制完整路径</n-button>
        <n-button size="small" @click="showExportModal = false">完成</n-button>
      </div>
    </n-card>
  </n-modal>
</template>
