<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, toRaw, watch } from 'vue'
import { PhArrowClockwise, PhArrowLeft, PhCaretRight } from '@phosphor-icons/vue'
import { getVersion } from '@tauri-apps/api/app'
import { useDialog, useMessage } from 'naive-ui'
import { commands, type CbzMode, type ImageDomainPreset, type LogLevel, type NetworkCheckReport, type NetworkProbe, type ProxyMode, type ReadingDirection } from '../bindings.ts'
import { useStore } from '../store.ts'
import { useBackHandler } from '../composables/useAndroidBack.ts'
import { useIsMobile } from '../composables/useIsMobile.ts'
import { useAppUpdate } from '../composables/useAppUpdate.ts'
import { withAndroidSelection, exportToAndroidDownloads, isUnderPublicDownloads } from '../utils/androidStorage.ts'
import { getErrorMessage } from '../utils/errors.ts'
import PathField from '../components/PathField.vue'
import StorageLocationDialog from '../dialogs/StorageLocationDialog.vue'

const store = useStore()
const message = useMessage()
const dialog = useDialog()
const { isAndroid } = useIsMobile()
const emit = defineEmits<{ (e: 'back'): void }>()

const showStoragePicker = ref(false)
// 二级面板：设置主页只列分类，点进去才看到具体项，避免一屏塞满所有输入框。
const openPanel = ref<'storage' | 'network' | 'reading' | 'data' | 'about' | null>(null)
const manualNetwork = ref(false)
const advancedNetwork = ref(false)
const advancedData = ref(false)

// 输入框被清空时 naive-ui 会给出 null；null 进入配置会让 Rust 的 u16 反序列化失败，
// 导致此后每一次保存/检查连接都报“设置格式无效”。这里兜底为默认端口。
function updateProxyPort(value: number | null) {
  if (!store.config) return
  store.config.proxyPort = value === null || !Number.isFinite(value) ? 7890 : value
}

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
const imagePresets = ref<ImageDomainPreset[]>([])
const checking = ref(false)
const checkResult = ref<NetworkCheckReport>()
const networkError = ref('')
const dataBusy = ref('')
const dataError = ref('')
let generation = 0
let disposed = false

const domainStatus = ref<NetworkProbe>()
const appVersion = ref('')
const {
  checking: updateChecking,
  hasUpdate: updateHasUpdate,
  latestVersion: updateLatestVersion,
  errorMessage: updateError,
  showUpdateDialog,
  checkForUpdates,
} = useAppUpdate()

// 返回键：先退二级面板，再退出设置页
useBackHandler(() => {
  if (showStoragePicker.value) {
    showStoragePicker.value = false
    return true
  }
  if (openPanel.value !== null) {
    openPanel.value = null
    return true
  }
  if (store.closeSettings()) return true
  return false
}, 50)

onBeforeUnmount(() => {
  disposed = true
  generation++
})
onMounted(() => {
  void refreshDomainStatus()
  void getVersion().then((value) => {
    if (!disposed) appVersion.value = value
  })
})
void commands.getImageDomainPresets()
  .then((value) => {
    if (!disposed) imagePresets.value = value
  })
  .catch((error) => {
    networkError.value = getErrorMessage(error)
  })

async function refreshDomainStatus() {
  const result = await commands.getApiDomainStatus()
  if (disposed) return
  if (result.status === 'ok') domainStatus.value = result.data
}

const apiOptions = computed(() => [
  ...store.apiDomainPresets.map((preset) => ({ label: `${preset.label} · ${preset.domain}`, value: preset.mode })),
  { label: '自定义地址', value: 'Custom' as const },
])
const imageOptions = computed(() => [
  ...imagePresets.value.map((preset) => ({ label: `${preset.label} · ${preset.domain}`, value: preset.mode })),
  { label: '自定义地址', value: 'Custom' as const },
])
const proxyOptions = [
  { label: '使用系统网络', value: 'System' },
  { label: '直接连接', value: 'NoProxy' },
  { label: '手动 HTTP 代理', value: 'Custom' },
]
const proxyMode = computed({
  get: () => store.config?.proxyMode ?? 'System',
  set: (value: ProxyMode) => {
    if (store.config) store.config.proxyMode = value
  },
})
const cbzModeOptions = [
  { label: '合并为整本 CBZ', value: 'merge' },
  { label: '逐话保留 CBZ', value: 'perChapter' },
]
const cbzMode = computed({
  get: () => store.config?.defaultCbzMode ?? 'merge',
  set: (value: CbzMode) => {
    if (store.config) store.config.defaultCbzMode = value
  },
})
const logLevel = computed({
  get: () => store.config?.logLevel ?? 'INFO',
  set: (value: LogLevel) => {
    if (store.config) store.config.logLevel = value
  },
})
const readingDirectionOptions = [
  { label: '从右往左（日漫）', value: 'RightToLeft' },
  { label: '从左往右', value: 'LeftToRight' },
]
const readingDirection = computed({
  get: () => store.config?.readingDirection ?? 'RightToLeft',
  set: (value: ReadingDirection) => {
    if (store.config) store.config.readingDirection = value
  },
})
const keepScreenOn = computed({
  get: () => store.config?.keepScreenOn ?? true,
  set: (value: boolean) => {
    if (store.config) store.config.keepScreenOn = value
  },
})
const volumeKeyPaging = computed({
  get: () => store.config?.volumeKeyPaging ?? false,
  set: (value: boolean) => {
    if (store.config) store.config.volumeKeyPaging = value
  },
})
const downloadOnlyWifi = computed({
  get: () => store.config?.downloadOnlyWifi ?? false,
  set: (value: boolean) => {
    if (store.config) store.config.downloadOnlyWifi = value
  },
})
const networkKey = computed(() => {
  const config = store.config
  return config
    ? JSON.stringify([config.apiDomainMode, config.customApiDomain, config.imageDomainMode, config.customImageDomain, config.proxyMode, config.proxyHost, config.proxyPort])
    : ''
})
watch(networkKey, () => {
  generation++
  checking.value = false
  checkResult.value = undefined
  networkError.value = ''
  void refreshDomainStatus()
})

async function checkNetwork() {
  if (!store.config || checking.value) return
  const current = ++generation
  checking.value = true
  checkResult.value = undefined
  networkError.value = ''
  try {
    const sampleId = store.pickedComic?.id ?? store.searchResult?.content[0]?.id ?? store.getWeeklyResult?.list[0]?.id
    const result = await commands.checkNetwork(structuredClone(toRaw(store.config)), sampleId && sampleId > 0 ? sampleId : null)
    if (disposed || current !== generation) return
    if (result.status === 'error') networkError.value = result.error.err_message
    else checkResult.value = result.data
  } catch (error) {
    if (current === generation) networkError.value = getErrorMessage(error)
  } finally {
    if (current === generation) checking.value = false
  }
}

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
    const result = await commands.backupAppData()
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

async function restore() {
  const apply = async (paths: string[]) => {
    await dataAction('restore', async () => {
      await store.saveConfigNow()
      const result = await store.runConfigMaintenance(() => commands.restoreAppData(paths[0]))
      if (result.status === 'error') throw new Error(result.error.err_message)
      store.applyConfig(result.data, true)
      store.applySession(await commands.getSession())
      store.progresses.clear()
      await store.reloadTasks()
      await store.refreshDownloadedComics()
      message.success('备份已恢复')
    })
  }
  try {
    await withAndroidSelection('backup', apply)
  } catch (error) {
    dataError.value = getErrorMessage(error)
  }
}

async function clearCache() {
  await dataAction('cache', async () => {
    const result = await commands.clearCache()
    if (result.status === 'error') throw new Error(result.error.err_message)
    message.success('缓存已清理，漫画与阅读记录保留')
  })
}

function resetSettings() {
  dialog.warning({
    title: '重置设置',
    content: '恢复默认设置并清除账号与任务队列；已下载的漫画保留。',
    positiveText: '重置设置',
    negativeText: '取消',
    onPositiveClick: () =>
      dataAction('reset', async () => {
        const result = await store.runConfigMaintenance(() => commands.resetAppState())
        if (result.status === 'error') throw new Error(result.error.err_message)
        store.applyConfig(result.data, true)
        store.applySession(await commands.getSession())
        store.progresses.clear()
        await store.refreshDownloadedComics()
        message.success('设置已重置')
      }),
  })
}

async function exportLogs() {
  await dataAction('logs', async () => {
    const logs = store.logRecords.slice(-1000).map((record) => `[${record.level}] ${record.formatedLog}`)
    const result = await commands.exportLogs(logs)
    if (result.status === 'error') throw new Error(result.error.err_message)
    // 书库为公共下载时诊断日志写进 JoyManga/logs，文件管理器已可见，无需再复制
    if (isUnderPublicDownloads(result.data.filePath)) {
      message.success('诊断日志已保存到书库的 logs 目录')
      return
    }
    await exportToAndroidDownloads(result.data.filePath)
  })
}

/** 目录选择器确认后的书库位置变更；exportDir 由后端随配置自动同步 */
function chooseDownloadDir(path: string) {
  dialog.warning({
    title: '更改漫画书库位置',
    content: `新下载将写入 ${path}；已下载内容保留在原处，不会被移动。`,
    positiveText: '确认更改',
    negativeText: '取消',
    onPositiveClick: async () => {
      const config = store.config
      if (!config) return
      const previous = config.downloadDir
      config.downloadDir = path
      try {
        // 立即落盘：位置变更关系到之后所有下载的落点，不能等 400ms 防抖，
        // 否则保存失败时用户已经看到成功提示、前后端配置却已分叉。
        await store.saveConfigNow()
        message.success('书库位置已更新')
      } catch (error) {
        config.downloadDir = previous
        message.error(`书库位置未能保存：${getErrorMessage(error)}`)
      }
    },
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

const panelTitle = computed(() => {
  switch (openPanel.value) {
    case 'storage':
      return '下载与保存'
    case 'network':
      return '网络'
    case 'reading':
      return '阅读'
    case 'data':
      return '数据'
    case 'about':
      return '关于'
    default:
      return '设置'
  }
})

function goBack() {
  if (openPanel.value !== null) openPanel.value = null
  else emit('back')
}

const readingSummary = computed(() => (readingDirection.value === 'RightToLeft' ? '从右往左' : '从左往右'))
</script>

<template>
  <div v-if="store.config" class="settings-page">
    <header class="settings-hero">
      <button type="button" class="back-button" aria-label="返回" @click="goBack">
        <PhArrowLeft :size="18" weight="bold" aria-hidden="true" />
      </button>
      <h1>{{ panelTitle }}</h1>
    </header>

    <div class="settings-scroll">
      <!-- 一级：分类列表 -->
      <template v-if="openPanel === null">
        <div class="group">
          <button type="button" class="nav-row" @click="openPanel = 'storage'">
            <div class="nav-copy"><strong>下载与保存</strong><span>{{ store.config.downloadDir }}</span></div>
            <PhCaretRight :size="16" aria-hidden="true" />
          </button>
          <button type="button" class="nav-row" @click="openPanel = 'network'">
            <div class="nav-copy"><strong>网络</strong><span>{{ domainStatus ? domainStatus.message : '正在读取线路状态…' }}</span></div>
            <PhCaretRight :size="16" aria-hidden="true" />
          </button>
          <button type="button" class="nav-row" @click="openPanel = 'reading'">
            <div class="nav-copy"><strong>阅读</strong><span>{{ readingSummary }}</span></div>
            <PhCaretRight :size="16" aria-hidden="true" />
          </button>
        </div>

        <div class="group">
          <button type="button" class="nav-row" @click="openPanel = 'data'">
            <div class="nav-copy"><strong>数据</strong><span>备份、缓存与诊断日志</span></div>
            <PhCaretRight :size="16" aria-hidden="true" />
          </button>
          <button type="button" class="nav-row" @click="openPanel = 'about'">
            <div class="nav-copy"><strong>关于</strong><span>{{ appVersion ? 'v' + appVersion : 'JoyManga' }}</span></div>
            <PhCaretRight :size="16" aria-hidden="true" />
          </button>
        </div>
      </template>

      <!-- 二级：下载与保存 -->
      <template v-else-if="openPanel === 'storage'">
        <section class="group">
          <div class="field">
            <PathField v-model:value="store.config.downloadDir" label="漫画书库" />
            <n-button secondary size="small" block @click="showStoragePicker = true">更改位置</n-button>
          </div>
          <div class="field">
            <label class="field-label">下载整理</label>
            <n-select v-model:value="cbzMode" :options="cbzModeOptions" />
          </div>
          <p class="note">下载后自动整理；单次下载时可在章节页临时选择。更改后只影响新下载。</p>
          <p v-if="isAndroid" class="note">公共下载书库（Download/JoyManga）卸载后保留；应用专用书库会随卸载清理。</p>
          <StorageLocationDialog v-model:show="showStoragePicker" @select="chooseDownloadDir" />
        </section>
      </template>

      <!-- 二级：网络 -->
      <template v-else-if="openPanel === 'network'">
        <section class="group">
          <div class="row">
            <span>域名自动更新</span>
            <span class="row-value">{{ domainStatus ? domainStatus.message : '正在读取…' }}</span>
          </div>
          <button type="button" class="nav-row inner" :aria-expanded="manualNetwork" @click="manualNetwork = !manualNetwork">
            <div class="nav-copy"><strong>手动设置线路</strong><span>加载异常时更换</span></div>
            <PhCaretRight :size="16" class="chevron" :class="{ expanded: manualNetwork }" aria-hidden="true" />
          </button>
          <div v-if="manualNetwork" class="field-stack">
            <div class="field">
              <label class="field-label">漫画信息</label>
              <n-select v-model:value="store.config.apiDomainMode" :options="apiOptions" />
              <n-input v-if="store.config.apiDomainMode === 'Custom'" v-model:value="store.config.customApiDomain" placeholder="服务器域名，不含 https://" />
            </div>
            <div class="field">
              <label class="field-label">图片下载</label>
              <n-select v-model:value="store.config.imageDomainMode" :options="imageOptions" />
              <n-input v-if="store.config.imageDomainMode === 'Custom'" v-model:value="store.config.customImageDomain" placeholder="图片服务器域名，不含 https://" />
            </div>
            <n-button secondary :loading="checking" @click="checkNetwork">
              <template #icon><PhArrowClockwise :size="16" aria-hidden="true" /></template>
              检查连接
            </n-button>
            <n-alert v-if="networkError" type="error" :show-icon="false">{{ networkError }}</n-alert>
            <div v-if="checkResult" class="probe-list" aria-live="polite">
              <div v-for="probe in [checkResult.api, checkResult.image, checkResult.domain]" :key="probe.label" class="probe" :data-state="probe.state">
                <div><strong>{{ probe.label }}</strong><span>{{ probe.state === 'Passed' ? '正常' : probe.state === 'Skipped' ? '未检查' : '连接异常' }}<template v-if="probe.elapsedMs !== null"> · {{ probe.elapsedMs }} ms</template></span></div>
                <p>{{ probe.message }}</p>
              </div>
            </div>
          </div>
        </section>

        <section class="group">
          <button type="button" class="nav-row inner" :aria-expanded="advancedNetwork" @click="advancedNetwork = !advancedNetwork">
            <div class="nav-copy"><strong>代理</strong><span>{{ proxyMode === 'Custom' ? '手动 HTTP 代理' : proxyMode === 'NoProxy' ? '直接连接' : '跟随系统' }}</span></div>
            <PhCaretRight :size="16" class="chevron" :class="{ expanded: advancedNetwork }" aria-hidden="true" />
          </button>
          <div v-if="advancedNetwork" class="field-stack">
            <n-select v-model:value="proxyMode" :options="proxyOptions" />
            <div v-if="proxyMode === 'Custom'" class="proxy-grid">
              <div class="field"><label class="field-label">主机</label><n-input v-model:value="store.config.proxyHost" placeholder="127.0.0.1" /></div>
              <div class="field"><label class="field-label">端口</label><n-input-number :value="store.config.proxyPort" :min="1" :max="65535" :show-button="false" @update:value="updateProxyPort" /></div>
            </div>
            <p class="note">VPN 随系统生效；开启 VPN 时应用会自动跳过此代理。</p>
          </div>
        </section>
      </template>

      <!-- 二级：阅读 -->
      <template v-else-if="openPanel === 'reading'">
        <section class="group">
          <div class="field">
            <label class="field-label">翻页方向</label>
            <n-select v-model:value="readingDirection" :options="readingDirectionOptions" />
          </div>
          <div class="row">
            <div class="row-copy"><strong>阅读时屏幕常亮</strong></div>
            <n-switch v-model:value="keepScreenOn" aria-label="阅读时屏幕常亮" />
          </div>
          <div v-if="isAndroid" class="row">
            <div class="row-copy"><strong>音量键翻页</strong><span>音量减为下一页</span></div>
            <n-switch v-model:value="volumeKeyPaging" aria-label="音量键翻页" />
          </div>
          <div v-if="isAndroid" class="row">
            <div class="row-copy"><strong>仅 Wi-Fi 下载</strong><span>移动数据下无法新建下载</span></div>
            <n-switch v-model:value="downloadOnlyWifi" aria-label="仅 Wi-Fi 下载" />
          </div>
        </section>
      </template>

      <!-- 二级：数据 -->
      <template v-else-if="openPanel === 'data'">
        <section class="group">
          <button type="button" class="nav-row" :disabled="!!dataBusy" @click="backup">
            <div class="nav-copy"><strong>导出备份</strong><span>设置与任务，不含漫画文件和账号密码</span></div>
            <PhCaretRight v-if="dataBusy !== 'backup'" :size="16" aria-hidden="true" />
            <n-spin v-else :size="16" />
          </button>
          <button type="button" class="nav-row" :disabled="!!dataBusy" @click="restore">
            <div class="nav-copy"><strong>恢复备份</strong><span>从备份文件还原设置与任务</span></div>
            <PhCaretRight v-if="dataBusy !== 'restore'" :size="16" aria-hidden="true" />
            <n-spin v-else :size="16" />
          </button>
          <button type="button" class="nav-row" :disabled="!!dataBusy" @click="clearCache">
            <div class="nav-copy"><strong>清理缓存</strong><span>漫画与阅读记录保留</span></div>
            <PhCaretRight v-if="dataBusy !== 'cache'" :size="16" aria-hidden="true" />
            <n-spin v-else :size="16" />
          </button>
          <button type="button" class="nav-row" :disabled="!!dataBusy" @click="exportLogs">
            <div class="nav-copy"><strong>导出诊断日志</strong><span>出问题时导出给开发者定位</span></div>
            <PhCaretRight v-if="dataBusy !== 'logs'" :size="16" aria-hidden="true" />
            <n-spin v-else :size="16" />
          </button>
        </section>

        <section class="group">
          <button type="button" class="nav-row inner" :aria-expanded="advancedData" @click="advancedData = !advancedData">
            <div class="nav-copy"><strong>更多维护</strong><span>日志等级、清理日志、重置设置</span></div>
            <PhCaretRight :size="16" class="chevron" :class="{ expanded: advancedData }" aria-hidden="true" />
          </button>
          <div v-if="advancedData" class="field-stack">
            <div class="field">
              <label class="field-label">日志收集等级</label>
              <n-select v-model:value="logLevel" :options="logLevelOptions" />
            </div>
            <div class="row">
              <span class="row-value">{{ logsSizeText }}</span>
              <n-button :disabled="!!dataBusy" :loading="dataBusy === 'logs-clear'" @click="clearLogsAction">清理日志</n-button>
            </div>
            <p class="note">日志按天滚动保存，保留 7 天，总量上限 20MB。</p>
            <n-button type="error" secondary :disabled="!!dataBusy" @click="resetSettings">重置设置</n-button>
          </div>
        </section>
        <n-alert v-if="dataError" type="error" :show-icon="false">{{ dataError }}</n-alert>
      </template>

      <!-- 二级：关于 -->
      <template v-else-if="openPanel === 'about'">
        <section class="group">
          <div class="brand-row">
            <svg class="brand-mark" viewBox="0 0 48 48" aria-hidden="true" focusable="false">
              <rect width="48" height="48" rx="12" fill="#241C2B" />
              <rect x="24.2" y="5.4" width="11.6" height="11.6" rx="2.4" transform="rotate(45 30 11.2)" fill="#00AEEC" />
              <path d="M30 21.5 V30 A9 9 0 0 1 13 30" fill="none" stroke="#FB7299" stroke-width="10" stroke-linecap="round" />
            </svg>
            <div class="brand-copy"><strong>JoyManga</strong><span>下载 · 归档 · 阅读</span></div>
          </div>
          <div class="row">
            <span>当前版本</span>
            <span class="row-value">{{ appVersion ? 'v' + appVersion : '—' }}</span>
          </div>
          <div class="row">
            <span>版本更新</span>
            <div class="update-cell">
              <span v-if="updateHasUpdate" class="update-new">发现新版 v{{ updateLatestVersion }}</span>
              <span v-else-if="updateError" class="update-error">{{ updateError }}</span>
              <span v-else-if="!updateChecking && updateLatestVersion" class="update-ok">已是最新</span>
              <n-button v-if="updateHasUpdate" type="primary" size="small" @click="showUpdateDialog = true">查看新版</n-button>
              <n-button size="small" secondary :loading="updateChecking" @click="checkForUpdates(false)">检查更新</n-button>
            </div>
          </div>
        </section>
      </template>
    </div>
  </div>
</template>

<style scoped>
.settings-page {
  display: flex;
  height: 100%;
  min-height: 0;
  flex-direction: column;
  background: #ffffff;
}

/* 与书库、我的页同一套页头：22px 标题 + 底部细线，整页而不是浮层 */
.settings-hero {
  display: flex;
  flex: 0 0 auto;
  align-items: center;
  gap: 10px;
  padding: 12px 14px;
  border-bottom: 1px solid #f0edf3;
  background: #ffffff;
}

.settings-hero h1 {
  margin: 0;
  color: #282739;
  font-size: 22px;
  font-weight: 650;
}

.back-button {
  display: flex;
  flex-shrink: 0;
  align-items: center;
  justify-content: center;
  width: 40px;
  height: 40px;
  padding: 0;
  color: #18191c;
  border: 1px solid #e3e5e7;
  border-radius: 999px;
  background: #f1f2f3;
  cursor: pointer;
  transition: all 0.2s;
}

.back-button:hover,
.back-button:active {
  color: #ffffff;
  border-color: #fb7299;
  background: #fb7299;
}

.settings-scroll {
  flex: 1 1 auto;
  min-height: 0;
  overflow-y: auto;
  overscroll-behavior: contain;
  touch-action: pan-y;
  padding: 12px 14px 24px;
}

/* 分组卡片：同组设置共享一块白底，靠间距分组而不是靠分隔线 */
.group {
  display: flex;
  flex-direction: column;
  gap: 2px;
  margin-bottom: 12px;
  padding: 4px;
  border: 1px solid #e3e5e7;
  border-radius: 14px;
  background: #ffffff;
}

.nav-row {
  display: flex;
  width: 100%;
  min-height: 56px;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 10px 12px;
  color: #18191c;
  border: 0;
  border-radius: 10px;
  background: transparent;
  text-align: left;
  cursor: pointer;
}

.nav-row:hover,
.nav-row:active {
  background: #f6f7f8;
}

.nav-row:disabled {
  cursor: default;
  opacity: 0.6;
}

.nav-row.inner {
  padding-left: 12px;
}

.nav-row > svg {
  flex-shrink: 0;
  color: #9499a0;
}

.nav-copy {
  display: flex;
  min-width: 0;
  flex-direction: column;
  gap: 3px;
}

.nav-copy strong {
  font-size: 15px;
  font-weight: 600;
}

.nav-copy span {
  overflow: hidden;
  color: #9499a0;
  font-size: 13px;
  line-height: 1.4;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.chevron {
  transition: transform 160ms ease;
}

.chevron.expanded {
  transform: rotate(90deg);
}

.field {
  display: flex;
  min-width: 0;
  flex-direction: column;
  gap: 8px;
  padding: 10px 12px;
}

.field-label {
  color: #61666d;
  font-size: 13px;
  font-weight: 550;
}

.field-stack {
  display: flex;
  flex-direction: column;
  gap: 4px;
  padding: 2px 8px 10px;
}

.proxy-grid {
  display: grid;
  grid-template-columns: minmax(0, 1fr) 120px;
  gap: 8px;
}

.proxy-grid .field {
  padding: 0;
}

.row {
  display: flex;
  min-height: 52px;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 8px 12px;
  font-size: 15px;
}

.row-copy {
  display: flex;
  min-width: 0;
  flex-direction: column;
  gap: 3px;
}

.row-copy strong {
  font-size: 15px;
  font-weight: 600;
}

.row-copy span {
  color: #9499a0;
  font-size: 13px;
}

.row-value {
  min-width: 0;
  overflow: hidden;
  color: #9499a0;
  font-size: 13px;
  line-height: 1.5;
  text-align: right;
  text-overflow: ellipsis;
}

.note {
  margin: 4px 12px 8px;
  color: #9499a0;
  font-size: 13px;
  line-height: 1.6;
  overflow-wrap: anywhere;
}

.brand-row {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 14px 12px;
}

.brand-mark {
  width: 44px;
  height: 44px;
  flex: 0 0 auto;
  border-radius: 12px;
}

.brand-copy {
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.brand-copy strong {
  font-size: 17px;
  font-weight: 700;
}

.brand-copy span {
  color: #9499a0;
  font-size: 12px;
}

.update-cell {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
  justify-content: flex-end;
}

.update-new {
  color: #b73963;
  font-size: 13px;
  font-weight: 550;
}

.update-ok {
  color: #217346;
  font-size: 13px;
}

.update-error {
  color: #b42318;
  font-size: 13px;
}

.probe-list {
  display: grid;
  gap: 8px;
}

.probe {
  padding: 12px 14px;
  border: 1px solid #e3e5e7;
  border-radius: 10px;
  background: #f6f7f8;
}

.probe > div {
  display: flex;
  flex-wrap: wrap;
  justify-content: space-between;
  gap: 8px;
  font-size: 14px;
}

.probe p {
  margin: 6px 0 0;
  color: #61666d;
  font-size: 13px;
  line-height: 1.6;
  overflow-wrap: anywhere;
}

.probe[data-state='Passed'] span {
  color: #217346;
}

.probe[data-state='Failed'] span {
  color: #b42318;
}

.nav-row:focus-visible,
.back-button:focus-visible {
  outline: 2px solid #fb7299;
  outline-offset: -2px;
}

:deep(.n-base-selection),
:deep(.n-input) {
  min-height: 46px;
}

:deep(.n-button) {
  min-height: 42px;
}
</style>
