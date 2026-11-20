<script setup lang="ts">
import { computed, ref, toRaw, watch } from 'vue'
import { PhCaretRight } from '@phosphor-icons/vue'
import { useDialog, useMessage } from 'naive-ui'
import { commands, type LogLevel, type Value } from '../../bindings.ts'
import { useStore } from '../../store.ts'
import { openDialogWithBack } from '../../composables/useDialogBack.ts'
import {
  withAndroidSelection,
  exportToAndroidDownloads,
  isUnderPublicDownloads,
} from '../../utils/androidStorage.ts'
import { getErrorMessage } from '../../utils/errors.ts'
import { getFrontendErrorLogs } from '../../utils/frontendErrors.ts'

const store = useStore()
const message = useMessage()
const dialog = useDialog()

const advancedData = ref(false)
// 与后端 LogLevel 枚举保持一致：恢复备份或手工编辑配置可能带入 TRACE/DEBUG，
// 选项缺失时 naive-ui 只能回退显示原始字符串。
const logLevelOptions: Array<{ label: string; value: LogLevel }> = [
  { label: '最详细（TRACE）', value: 'TRACE' },
  { label: '调试（DEBUG）', value: 'DEBUG' },
  { label: '详细（INFO）', value: 'INFO' },
  { label: '仅警告（WARN）', value: 'WARN' },
  { label: '仅错误（ERROR）', value: 'ERROR' },
  { label: '关闭（OFF）', value: 'OFF' },
]
const logsDirSize = ref<number | undefined>(undefined)
const logsSizeLoading = ref(false)
const dataBusy = ref('')
const dataError = ref('')

const logLevel = computed({
  get: () => store.config?.logLevel ?? 'INFO',
  set: (value: LogLevel) => {
    if (store.config) store.config.logLevel = value
  },
})

async function dataAction(name: string, action: () => Promise<void>) {
  if (dataBusy.value) return
  dataBusy.value = name
  dataError.value = ''
  try {
    await action()
  } catch (error) {
    dataError.value = getErrorMessage(error)
    message.error(dataError.value)
  } finally {
    dataBusy.value = ''
  }
}

async function backup() {
  await dataAction('backup', async () => {
    await store.saveConfigNow()
    const readingState: Value = JSON.parse(JSON.stringify(toRaw(store.readingState)))
    const result = await commands.backupAppData(readingState)
    if (result.status === 'error') throw new Error(result.error.err_message)
    // 书库本身就在公共下载时，备份文件已经可见；再发布只会产生「xxx (1).json」副本
    if (isUnderPublicDownloads(result.data)) {
      message.success(`备份已保存到 ${result.data}`)
      return
    }
    const published = await exportToAndroidDownloads(result.data)
    if (!published.cancelled) message.success('备份已保存到系统下载')
  })
}

function restore() {
  openDialogWithBack(dialog, {
    title: '恢复备份',
    content: '将用备份覆盖当前设置、任务队列、阅读进度、书签和归档设置；已下载的漫画文件不受影响。',
    positiveText: '选择备份文件',
    negativeText: '取消',
    // 确认后先选文件：Android 的导入文件会在选择回调返回后清理，
    // 若把确认框放进回调里等用户点击，恢复时文件可能已经被删掉。
    onPositiveClick: () => {
      void withAndroidSelection('backup', (paths) =>
        dataAction('restore', async () => {
          await store.saveConfigNow()
          const result = await store.runConfigMaintenance(() => commands.restoreAppData(paths[0]))
          if (result.status === 'error') throw new Error(result.error.err_message)
          const outcome = result.data
          store.applyConfig(outcome.config, true)
          store.applySession(await commands.getSession())
          store.progresses.clear()
          let readingNotice = ''
          if (outcome.readingState !== null) {
            const imported = store.importReadingStateSnapshot(outcome.readingState)
            if (imported === 'invalid') readingNotice = '；阅读记录格式无效，已跳过'
            else if (imported === 'write-failed') readingNotice = '；阅读记录未能保存到本地'
          }
          await store.reloadTasks()
          store.invalidateDownloadedLibrary({ clear: true })
          message.success(readingNotice ? `备份已恢复${readingNotice}` : '备份已恢复')
        }),
      ).catch((error) => {
        dataError.value = getErrorMessage(error)
      })
    },
  })
}

async function clearCache() {
  await dataAction('cache', async () => {
    const result = await commands.clearCache()
    if (result.status === 'error') throw new Error(result.error.err_message)
    message.success('缓存已清理，漫画与阅读记录保留')
  })
}

function resetSettings() {
  openDialogWithBack(dialog, {
    title: '重置设置',
    content: '恢复默认设置并清除账号、任务队列与归档设置；已下载的漫画与阅读记录保留。',
    positiveText: '重置设置',
    negativeText: '取消',
    onPositiveClick: () =>
      dataAction('reset', async () => {
        const result = await store.runConfigMaintenance(() => commands.resetAppState())
        if (result.status === 'error') throw new Error(result.error.err_message)
        store.applyConfig(result.data, true)
        store.applySession(await commands.getSession())
        store.progresses.clear()
        store.invalidateDownloadedLibrary({ clear: true })
        message.success('设置已重置')
      }),
  })
}

async function exportLogs() {
  await dataAction('logs', async () => {
    const logs = getFrontendErrorLogs()
    const result = await commands.exportLogs(logs)
    if (result.status === 'error') throw new Error(result.error.err_message)
    // 固定保存在书库下的 logs 目录，不再另存一份到系统下载：位置只有这一个
    message.success(`诊断包已保存到 ${result.data.filePath}`)
  })
}

async function refreshLogsDirSize() {
  logsSizeLoading.value = true
  try {
    const result = await commands.getLogsDirSize()
    if (result.status === 'ok') logsDirSize.value = result.data
    else message.error(getErrorMessage(result.error))
  } catch (error) {
    message.error(getErrorMessage(error))
  } finally {
    logsSizeLoading.value = false
  }
}

/** 日志占用的展示文案（含加载态），只依赖上面两个状态，不再单独维护第三份。 */
const logsSizeText = computed(() => {
  if (logsSizeLoading.value) return '正在统计日志占用…'
  if (logsDirSize.value === undefined) return ''
  if (logsDirSize.value === 0) return '暂无日志'
  return `占用 ${(logsDirSize.value / 1048576).toFixed(1)} MB`
})

async function clearLogsAction() {
  await dataAction('logs-clear', async () => {
    // runConfigMaintenance 会取消待执行的防抖保存；先显式落盘，避免用户的设置改动被静默丢弃
    await store.saveConfigNow()
    const result = await store.runConfigMaintenance(() => commands.clearLogs())
    if (result.status === 'error') throw new Error(result.error.err_message)
    message.success('日志已清理')
    await refreshLogsDirSize()
  })
}

// 展开「更多维护」时统计一次；每次重新展开都刷新，避免展示过期的大小
watch(advancedData, (open) => {
  if (open) void refreshLogsDirSize()
})
</script>

<template>
  <div>
    <section class="group">
      <button type="button" class="nav-row" :disabled="!!dataBusy" @click="backup">
        <div class="nav-copy">
          <strong>导出备份</strong>
          <span>设置、任务、阅读进度与书签，不含漫画文件和账号密码</span>
        </div>
        <PhCaretRight v-if="dataBusy !== 'backup'" :size="16" aria-hidden="true" />
        <n-spin v-else :size="16" />
      </button>
      <button type="button" class="nav-row" :disabled="!!dataBusy" @click="restore">
        <div class="nav-copy">
          <strong>恢复备份</strong>
          <span>从备份还原设置、任务、阅读进度与归档设置</span>
        </div>
        <PhCaretRight v-if="dataBusy !== 'restore'" :size="16" aria-hidden="true" />
        <n-spin v-else :size="16" />
      </button>
      <button type="button" class="nav-row" :disabled="!!dataBusy" @click="clearCache">
        <div class="nav-copy">
          <strong>清理缓存</strong>
          <span>漫画与阅读记录保留</span>
        </div>
        <PhCaretRight v-if="dataBusy !== 'cache'" :size="16" aria-hidden="true" />
        <n-spin v-else :size="16" />
      </button>
      <button type="button" class="nav-row" :disabled="!!dataBusy" @click="exportLogs">
        <div class="nav-copy">
          <strong>导出诊断包</strong>
          <span>日志、任务与脱敏配置，保存到书库 logs</span>
        </div>
        <PhCaretRight v-if="dataBusy !== 'logs'" :size="16" aria-hidden="true" />
        <n-spin v-else :size="16" />
      </button>
    </section>

    <section class="group">
      <button
        type="button"
        class="nav-row inner"
        :aria-expanded="advancedData"
        @click="advancedData = !advancedData">
        <div class="nav-copy">
          <strong>更多维护</strong>
          <span>日志等级、清理日志、重置设置</span>
        </div>
        <PhCaretRight :size="16" class="chevron" :class="{ expanded: advancedData }" aria-hidden="true" />
      </button>
      <div v-if="advancedData" class="field-stack">
        <div class="field">
          <label class="field-label">文件日志等级</label>
          <n-select v-model:value="logLevel" :options="logLevelOptions" />
        </div>
        <div class="row">
          <span class="row-value">{{ logsSizeText }}</span>
          <n-button :disabled="!!dataBusy" :loading="dataBusy === 'logs-clear'" @click="clearLogsAction">
            清理日志
          </n-button>
        </div>
        <p class="note">滚动日志按天保存，保留 7 天、上限 20MB；「清理日志」同时清除已导出的诊断包。</p>
        <n-button type="error" secondary :disabled="!!dataBusy" @click="resetSettings">重置设置</n-button>
      </div>
    </section>
    <n-alert v-if="dataError" type="error" :show-icon="false">{{ dataError }}</n-alert>
  </div>
</template>

<style scoped src="./settingsShared.css"></style>
