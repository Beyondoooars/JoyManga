<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, toRaw, watch } from 'vue'
import { PhFolder, PhGlobe, PhWrench, PhInfo, PhX, PhArrowClockwise, PhCaretDown, PhBookOpen } from '@phosphor-icons/vue'
import { getVersion } from '@tauri-apps/api/app'
import { useDialog, useMessage } from 'naive-ui'
import { commands, type CbzMode, type ImageDomainPreset, type LogLevel, type NetworkCheckReport, type NetworkProbe, type ProxyMode, type ReadingDirection } from '../bindings.ts'
import { useStore } from '../store.ts'
import { useBackHandler } from '../composables/useAndroidBack.ts'
import { useIsMobile } from '../composables/useIsMobile.ts'
import { useAppUpdate } from '../composables/useAppUpdate.ts'
import { withAndroidSelection, exportToAndroidDownloads, isUnderPublicDownloads } from '../utils/androidStorage.ts'
import { openExternalUrl, REPOSITORY_URL } from '../utils/externalLink.ts'
import { formatCommandError, getErrorMessage } from '../utils/errors.ts'
import PathField from '../components/PathField.vue'
import StorageLocationDialog from './StorageLocationDialog.vue'

const showing = defineModel<boolean>('showing', { required: true })
const store = useStore()
const message = useMessage()
const dialog = useDialog()
const { isAndroid } = useIsMobile()
const advancedNetwork = ref(false)

// 输入框被清空时 naive-ui 会给出 null；null 进入配置会让 Rust 的 u16 反序列化失败，
// 导致此后每一次保存/检查连接都报“设置格式无效”。这里兜底为默认端口。
function updateProxyPort(value: number | null) {
  if (!store.config) return
  store.config.proxyPort = value === null || !Number.isFinite(value) ? 7890 : value
}
const advancedData = ref(false)
const manualNetwork = ref(false)
const showStoragePicker = ref(false)
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
const projectUrl = REPOSITORY_URL
const licenseUrl = `${projectUrl}/blob/main/LICENSE`
const {
  checking: updateChecking,
  hasUpdate: updateHasUpdate,
  latestVersion: updateLatestVersion,
  errorMessage: updateError,
  showUpdateDialog,
  checkForUpdates,
} = useAppUpdate()

useBackHandler(() => {
  if (!showing.value) return false
  // 目录选择器与确认框开着时，返回键先关它们，不能把设置页整个关掉
  if (showStoragePicker.value) {
    showStoragePicker.value = false
    return true
  }
  showing.value = false
  return true
}, 50)
onBeforeUnmount(() => { disposed = true; generation++ })
void commands.getImageDomainPresets().then((value) => { if (!disposed) imagePresets.value = value }).catch((error) => { networkError.value = getErrorMessage(error) })

onMounted(() => {
  void refreshDomainStatus()
  void getVersion().then((value) => { if (!disposed) appVersion.value = value })
})

async function refreshDomainStatus() {
  const result = await commands.getApiDomainStatus()
  if (disposed) return
  if (result.status === 'ok') domainStatus.value = result.data
}

async function handleCheckUpdate() {
  await checkForUpdates(false)
}

const apiOptions = computed(() => [...store.apiDomainPresets.map((preset) => ({ label: `${preset.label} · ${preset.domain}`, value: preset.mode })), { label: '自定义地址', value: 'Custom' }])
const imageOptions = computed(() => [...imagePresets.value.map((preset) => ({ label: `${preset.label} · ${preset.domain}`, value: preset.mode })), { label: '自定义地址', value: 'Custom' }])
const proxyOptions = [
  { label: '使用系统网络', value: 'System' },
  { label: '直接连接', value: 'NoProxy' },
  { label: '手动 HTTP 代理', value: 'Custom' },
]
const proxyMode = computed({ get: () => store.config?.proxyMode ?? 'System', set: (value: ProxyMode) => { if (store.config) store.config.proxyMode = value } })
const cbzModeOptions = [
  { label: '合并为整本 CBZ', value: 'merge' },
  { label: '逐话保留 CBZ', value: 'perChapter' },
]
const cbzMode = computed({ get: () => store.config?.defaultCbzMode ?? 'merge', set: (value: CbzMode) => { if (store.config) store.config.defaultCbzMode = value } })
const logLevel = computed({ get: () => store.config?.logLevel ?? 'INFO', set: (value: LogLevel) => { if (store.config) store.config.logLevel = value } })
const readingDirectionOptions = [
  { label: '从右往左（日漫）', value: 'RightToLeft' },
  { label: '从左往右', value: 'LeftToRight' },
]
const readingDirection = computed({
  get: () => store.config?.readingDirection ?? 'RightToLeft',
  set: (value: ReadingDirection) => { if (store.config) store.config.readingDirection = value },
})
const keepScreenOn = computed({
  get: () => store.config?.keepScreenOn ?? true,
  set: (value: boolean) => { if (store.config) store.config.keepScreenOn = value },
})
const volumeKeyPaging = computed({
  get: () => store.config?.volumeKeyPaging ?? false,
  set: (value: boolean) => { if (store.config) store.config.volumeKeyPaging = value },
})
const downloadOnlyWifi = computed({
  get: () => store.config?.downloadOnlyWifi ?? false,
  set: (value: boolean) => { if (store.config) store.config.downloadOnlyWifi = value },
})
const networkKey = computed(() => {
  const config = store.config
  return config ? JSON.stringify([config.apiDomainMode, config.customApiDomain, config.imageDomainMode, config.customImageDomain, config.proxyMode, config.proxyHost, config.proxyPort]) : ''
})
watch(networkKey, () => { generation++; checking.value = false; checkResult.value = undefined; networkError.value = ''; void refreshDomainStatus() })

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
  } catch (error) { if (current === generation) networkError.value = getErrorMessage(error) }
  finally { if (current === generation) checking.value = false }
}

async function dataAction(name: string, action: () => Promise<void>) {
  if (dataBusy.value) return
  dataBusy.value = name
  dataError.value = ''
  try { await action() }
  catch (error) { dataError.value = getErrorMessage(error); message.error(dataError.value) }
  finally { dataBusy.value = '' }
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
  } catch (error) { dataError.value = getErrorMessage(error) }
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
    title: '重置应用设置',
    content: '恢复默认设置并清除账号与任务队列；已下载的漫画保留。',
    positiveText: '重置设置', negativeText: '取消',
    onPositiveClick: () => dataAction('reset', async () => {
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
    else message.error(formatCommandError(result.error))
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
  return `日志占用 ${(logsDirSize.value / 1048576).toFixed(1)} MB`
})

async function clearLogsAction() {
  await dataAction('logs-clear', async () => {
    // runConfigMaintenance 会取消待执行的防抖保存；先显式落盘，避免用户的设置改动被静默丢弃
    await store.saveConfigNow()
    const result = await store.runConfigMaintenance(() => commands.clearLogs())
    if (result.status === 'error') throw new Error(formatCommandError(result.error))
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
  <n-modal v-if="store.config" v-model:show="showing" :auto-focus="true" :trap-focus="true">
    <section class="settings-shell" role="dialog" aria-modal="true" aria-labelledby="settings-title">
      <header class="settings-header">
        <h2 id="settings-title">应用设置</h2>
        <button class="settings-close" type="button" aria-label="关闭设置" @click="showing = false"><PhX :size="20" aria-hidden="true" /></button>
      </header>
      <div class="settings-scroll" role="region" aria-label="设置内容" tabindex="0">
        <section class="settings-section">
          <h3><PhFolder :size="20" aria-hidden="true" />保存位置</h3>
          <div class="setting-row">
            <PathField v-model:value="store.config.downloadDir" label="漫画书库" />
          </div>
          <div class="setting-row">
            <n-button secondary size="small" @click="showStoragePicker = true">更改位置</n-button>
          </div>
          <div class="setting-row">
            <label class="setting-label" for="cbz-mode">下载整理</label>
            <n-select aria-label="下载完成后 CBZ 整理方式" v-model:value="cbzMode" :options="cbzModeOptions" />
          </div>
          <p class="settings-note">下载后自动整理；单次下载时可在本子详情页临时选择。更改后仅影响新下载，已下载内容保留在原处。</p>
          <p v-if="isAndroid" class="settings-note">公共下载书库（Download/JoyManga）卸载后保留；应用专用书库会随卸载清理，需要长期保留时请改到公共目录或导出到系统下载。</p>
          <StorageLocationDialog v-model:show="showStoragePicker" @select="chooseDownloadDir" />
        </section>

        <section class="settings-section">
          <h3><PhGlobe :size="20" aria-hidden="true" />网络连接</h3>
          <div class="setting-row">
            <label class="setting-label">域名自动更新</label>
            <p class="status-line">{{ domainStatus ? domainStatus.message : '正在读取状态…' }}</p>
          </div>
          <button type="button" class="disclosure" :aria-expanded="manualNetwork" aria-controls="manual-network-settings" @click="manualNetwork = !manualNetwork"><span>手动设置</span><PhCaretDown :size="16" :class="{ expanded: manualNetwork }" aria-hidden="true" /></button>
          <div v-if="manualNetwork" id="manual-network-settings" class="advanced-content">
            <div class="setting-row">
              <label class="setting-label" for="api-route">漫画信息</label>
              <n-select aria-label="漫画信息线路" v-model:value="store.config.apiDomainMode" :options="apiOptions" />
              <n-input v-if="store.config.apiDomainMode === 'Custom'" v-model:value="store.config.customApiDomain" aria-label="漫画信息服务器地址" placeholder="服务器域名，不含 https://" />
            </div>
            <div class="setting-row">
              <label class="setting-label" for="image-route">图片下载</label>
              <n-select aria-label="图片下载线路" v-model:value="store.config.imageDomainMode" :options="imageOptions" />
              <n-input v-if="store.config.imageDomainMode === 'Custom'" v-model:value="store.config.customImageDomain" aria-label="图片服务器地址" placeholder="图片服务器域名，不含 https://" />
            </div>
            <p class="settings-note">加载异常时更换对应线路。</p>
            <div class="diagnostics-actions">
              <n-button secondary :loading="checking" @click="checkNetwork"><template #icon><PhArrowClockwise :size="16" aria-hidden="true" /></template>检查连接</n-button>
            </div>
            <n-alert v-if="networkError" type="error" :show-icon="false">{{ networkError }}</n-alert>
            <div v-if="checkResult" class="connection-results" aria-live="polite">
              <div v-for="probe in [checkResult.api, checkResult.image, checkResult.domain]" :key="probe.label" class="connection-result" :data-state="probe.state">
                <div><strong>{{ probe.label }}</strong><span>{{ probe.state === 'Passed' ? '正常' : probe.state === 'Skipped' ? '未检查' : '连接异常' }}<template v-if="probe.elapsedMs !== null"> · {{ probe.elapsedMs }} ms</template></span></div>
                <p>{{ probe.message }}</p>
              </div>
            </div>
          </div>
          <button type="button" class="disclosure" :aria-expanded="advancedNetwork" aria-controls="proxy-settings" @click="advancedNetwork = !advancedNetwork"><span>代理设置</span><PhCaretDown :size="16" :class="{ expanded: advancedNetwork }" aria-hidden="true" /></button>
          <div v-if="advancedNetwork" id="proxy-settings" class="advanced-content">
            <n-select aria-label="代理方式" v-model:value="proxyMode" :options="proxyOptions" />
            <div v-if="proxyMode === 'Custom'" class="proxy-fields">
              <div><label class="setting-label">主机</label><n-input v-model:value="store.config.proxyHost" aria-label="代理主机" placeholder="127.0.0.1" /></div>
              <div><label class="setting-label">端口</label><n-input-number :value="store.config.proxyPort" aria-label="代理端口" :min="1" :max="65535" :show-button="false" @update:value="updateProxyPort" /></div>
            </div>
            <p class="settings-note">VPN 随系统生效；仅 HTTP 端口的代理工具在此设置（127.0.0.1 指本机）。开启 VPN 时应用会自动跳过此代理。</p>
          </div>
        </section>

        <section class="settings-section">
          <h3><PhBookOpen :size="20" aria-hidden="true" />阅读</h3>
          <div class="setting-row">
            <label class="setting-label" for="reading-direction">翻页方向</label>
            <n-select aria-label="翻页方向" v-model:value="readingDirection" :options="readingDirectionOptions" />
          </div>
          <p class="settings-note">决定点击左右区域和左右方向键的翻页方向；空格与滚轮始终向后翻。</p>
          <div class="setting-row switch-row">
            <label class="setting-label">阅读时屏幕常亮</label>
            <n-switch v-model:value="keepScreenOn" aria-label="阅读时屏幕常亮" />
          </div>
          <div v-if="isAndroid" class="setting-row switch-row">
            <label class="setting-label">音量键翻页</label>
            <n-switch v-model:value="volumeKeyPaging" aria-label="音量键翻页" />
          </div>
          <p v-if="isAndroid" class="settings-note">开启后音量减为下一页、音量加为上一页。</p>
          <div v-if="isAndroid" class="setting-row switch-row">
            <label class="setting-label">仅 Wi-Fi 下载</label>
            <n-switch v-model:value="downloadOnlyWifi" aria-label="仅 Wi-Fi 下载" />
          </div>
          <p v-if="isAndroid" class="settings-note">开启后在移动数据下无法新建下载任务；已排队的任务不受影响。</p>
        </section>

        <section class="settings-section">
          <h3><PhWrench :size="20" aria-hidden="true" />数据管理</h3>
          <div class="data-actions"><n-button :disabled="!!dataBusy" :loading="dataBusy === 'cache'" @click="clearCache">清理缓存</n-button><n-button :disabled="!!dataBusy" :loading="dataBusy === 'backup'" @click="backup">导出备份</n-button><n-button :disabled="!!dataBusy" :loading="dataBusy === 'restore'" @click="restore">恢复备份</n-button></div>
          <p class="settings-note">备份含设置和任务；不含漫画文件与账号密码。</p>
          <n-alert v-if="dataError" type="error" :show-icon="false">{{ dataError }}</n-alert>
          <button class="disclosure" type="button" :aria-expanded="advancedData" aria-controls="maintenance-settings" @click="advancedData = !advancedData"><span>更多维护</span><PhCaretDown :size="16" :class="{ expanded: advancedData }" aria-hidden="true" /></button>
          <div v-if="advancedData" id="maintenance-settings" class="advanced-content">
            <div class="data-actions"><n-button :disabled="!!dataBusy" :loading="dataBusy === 'logs'" @click="exportLogs">导出诊断日志</n-button><n-button :disabled="!!dataBusy" type="error" secondary @click="resetSettings">重置设置</n-button></div>
            <div class="log-settings">
              <div class="setting-row">
                <label class="setting-label" for="log-level">日志收集等级</label>
                <n-select input-id="log-level" aria-label="日志收集等级" v-model:value="logLevel" :options="logLevelOptions" />
              </div>
              <div class="setting-row log-usage-row">
                <span class="log-usage" aria-live="polite">{{ logsSizeText }}</span>
                <n-button :disabled="!!dataBusy" :loading="dataBusy === 'logs-clear'" @click="clearLogsAction">清理日志</n-button>
              </div>
              <p class="settings-note">日志按天滚动保存在应用数据目录，保留 7 天，总量上限 20MB；出问题可用于导出诊断。</p>
            </div>
          </div>
        </section>

        <section class="settings-section">
          <h3><PhInfo :size="20" aria-hidden="true" />关于</h3>
          <div class="about-brand">
            <svg class="about-mark" viewBox="0 0 48 48" aria-hidden="true" focusable="false">
              <rect width="48" height="48" rx="12" fill="#241C2B" />
              <rect x="24.2" y="5.4" width="11.6" height="11.6" rx="2.4" transform="rotate(45 30 11.2)" fill="#00AEEC" />
              <path d="M30 21.5 V30 A9 9 0 0 1 13 30" fill="none" stroke="#FB7299" stroke-width="10" stroke-linecap="round" />
            </svg>
            <div class="about-brand-copy">
              <strong>JoyManga</strong>
              <span>下载 · 归档 · 阅读</span>
            </div>
          </div>
          <div class="about-rows">
            <div class="about-row"><span>当前版本</span><strong>{{ appVersion ? 'v' + appVersion : '—' }}</strong></div>
            <div class="about-row">
              <span>版本更新</span>
              <div class="about-update">
                <span v-if="updateHasUpdate" class="about-update-new">发现新版 v{{ updateLatestVersion }}</span>
                <span v-else-if="updateError" class="about-update-error">{{ updateError }}</span>
                <span v-else-if="!updateChecking && updateLatestVersion" class="about-update-ok">已是最新 (v{{ updateLatestVersion }})</span>
                <n-button v-if="updateHasUpdate" type="primary" size="small" @click="showUpdateDialog = true">查看新版</n-button>
                <n-button size="small" secondary :loading="updateChecking" @click="handleCheckUpdate">检查更新</n-button>
              </div>
            </div>
            <div class="about-row">
              <span>项目主页</span>
              <!-- 用系统浏览器打开：WebView 内直接导航会把整个应用界面替换成网页且无法返回 -->
              <n-a :href="projectUrl" target="_blank" @click.prevent="openExternalUrl(projectUrl)">GitHub 仓库</n-a>
            </div>
          </div>
          <p class="settings-note">Copyright © 2024-2026 Beyondoooars · <n-a :href="licenseUrl" target="_blank" @click.prevent="openExternalUrl(licenseUrl)">LICENSE</n-a></p>
        </section>
      </div>
      <footer class="settings-footer"><span>更改后自动保存</span><n-button type="primary" color="#b73963" text-color="#ffffff" @click="showing = false">完成</n-button></footer>
    </section>
  </n-modal>
</template>

<style scoped>
.settings-shell { --settings-text: var(--text-primary, #18191c); --settings-muted: #5e6672; --settings-border: var(--border-color, #e3e5e7); width: min(720px, calc(100vw - 40px)); height: min(800px, calc(100vh - 48px)); height: min(800px, calc(100dvh - 48px)); min-height: 0; display: flex; flex-direction: column; overflow: hidden; color: var(--settings-text); background: var(--bg-card, #fff); border: 1px solid var(--settings-border); border-radius: var(--radius-panel); box-shadow: 0 16px 56px rgb(0 0 0 / 16%); }
.settings-header { flex: 0 0 auto; display: flex; align-items: center; justify-content: space-between; padding: 16px 24px; border-bottom: 1px solid var(--settings-border); }
.settings-header h2 { margin: 0; font-size: 20px; font-weight: 650; }
.settings-close { display: grid; place-items: center; width: 44px; height: 44px; padding: 0; border: 0; border-radius: 10px; background: transparent; color: var(--settings-muted); cursor: pointer; }
.settings-close:hover { background: var(--bg-subtle); color: var(--settings-text); }
.settings-scroll { flex: 1 1 auto; min-height: 0; overflow-y: auto; overscroll-behavior: contain; touch-action: pan-y; scrollbar-gutter: stable; padding: 0 24px; }
.settings-section { display: block; padding: 24px 0; border-bottom: 1px solid var(--settings-border); }
.settings-section:last-child { border-bottom: 0; }
.settings-section h3 { display: flex; align-items: center; gap: 10px; margin: 0 0 20px; font-size: 16px; font-weight: 650; }
.settings-section h3 svg { color: var(--bili-pink-active); }
.status-line { margin: 0; color: var(--settings-muted); font-size: 14px; line-height: 1.65; overflow-wrap: anywhere; }
.diagnostics-actions { display: flex; flex-wrap: wrap; gap: 12px; }
.about-brand { display: flex; align-items: center; gap: 12px; padding: 4px 0 12px; }
.about-mark { width: 44px; height: 44px; flex: 0 0 auto; }
.about-brand-copy { display: flex; flex-direction: column; gap: 2px; }
.about-brand-copy strong { font-size: 16px; font-weight: 750; letter-spacing: -0.01em; }
.about-brand-copy span { color: var(--settings-muted); font-size: 12px; }
.about-rows { display: grid; gap: 2px; }
.about-row { display: flex; align-items: center; justify-content: space-between; gap: 12px; min-height: 44px; padding: 4px 0; font-size: 14px; }
.about-row > span { color: var(--settings-muted); }
.about-row strong { font-weight: 550; }
.about-update { display: flex; align-items: center; gap: 10px; flex-wrap: wrap; }
.about-update-new { color: #b73963; font-weight: 550; }
.about-update-ok { color: #217346; }
.about-update-error { color: #b42318; }
.setting-row { display: flex; flex-direction: column; gap: 8px; margin-bottom: 16px; min-width: 0; }
/* 开关类设置：标签与开关同一行，避免开关独占整行过宽 */
.setting-row.switch-row { flex-direction: row; align-items: center; justify-content: space-between; gap: 16px; }
.setting-row.switch-row .setting-label { margin: 0; }
.setting-label { display: block; font-size: 14px; line-height: 1.5; font-weight: 550; }
.settings-note { margin: 12px 0 0; color: var(--settings-muted); font-size: 14px; line-height: 1.65; overflow-wrap: anywhere; }
.disclosure { display: flex; align-items: center; justify-content: space-between; gap: 12px; width: 100%; min-height: 48px; padding: 12px 0; margin-top: 12px; border: 0; background: transparent; color: var(--settings-text); cursor: pointer; text-align: left; font-size: 14px; font-weight: 550; }
.disclosure svg { transition: transform 160ms ease; }.disclosure .expanded { transform: rotate(180deg); }
.advanced-content { display: grid; gap: 12px; padding: 8px 0; }
.proxy-fields { display: grid; grid-template-columns: minmax(0, 1fr) 120px; gap: 12px; }.proxy-fields .setting-label { margin-bottom: 8px; }
.data-actions { display: flex; flex-wrap: wrap; gap: 12px; }
.log-settings { display: grid; gap: 4px; padding-top: 8px; border-top: 1px solid var(--settings-border); }
.log-usage-row { flex-direction: row; align-items: center; justify-content: space-between; gap: 16px; }
.log-usage { color: var(--settings-muted); font-size: 14px; line-height: 1.65; }
.connection-results { display: grid; gap: 8px; margin-top: 16px; }
.connection-result { padding: 12px 14px; border: 1px solid var(--settings-border); border-radius: 10px; background: var(--bg-subtle); }
.connection-result > div { display: flex; flex-wrap: wrap; justify-content: space-between; gap: 8px; font-size: 14px; }.connection-result p { margin: 6px 0 0; color: var(--settings-muted); font-size: 13px; line-height: 1.6; overflow-wrap: anywhere; }
.connection-result[data-state="Passed"] span { color: #217346; }.connection-result[data-state="Failed"] span { color: #b42318; }
.settings-footer { flex: 0 0 auto; display: flex; align-items: center; justify-content: space-between; gap: 12px; padding: 16px 24px; border-top: 1px solid var(--settings-border); background: var(--bg-card); }.settings-footer span { font-size: 13px; color: var(--settings-muted); }
.settings-close:focus-visible,.disclosure:focus-visible,.settings-scroll:focus-visible { outline: 2px solid var(--bili-pink-active); outline-offset: -2px; }
:deep(.path-field) { height: 44px; grid-template-columns: 78px minmax(0, 1fr) 44px 44px; } :deep(.path-field-label) { font-size: 13px; } :deep(.path-field-value) { font-size: 14px; }
:deep(.n-base-selection),:deep(.n-input) { min-height: 42px; } :deep(.n-button) { min-height: 40px; }
@media (max-width: 600px) { .settings-shell { width: 100vw; height: 100vh; height: 100dvh; max-height: 100dvh; border: 0; border-radius: 0; }.settings-header { padding: calc(12px + env(safe-area-inset-top, 0px)) 16px 12px; }.settings-scroll { padding: 0 16px; }.settings-footer { padding: 12px 16px calc(12px + env(safe-area-inset-bottom, 0px)); }.settings-section { padding: 24px 0; }.data-actions { display: grid; grid-template-columns: 1fr; }.data-actions :deep(.n-button),.diagnostics-actions :deep(.n-button) { min-height: 48px; } :deep(.n-base-selection),:deep(.n-input) { min-height: 48px; } }
@media (max-width: 600px) { :deep(.path-field) { height: 48px; grid-template-columns: 78px minmax(0, 1fr) 48px 48px; } :deep(.path-field-value),:deep(.n-input__input-el),:deep(.n-base-selection-label) { font-size: 16px; } .settings-note { font-size: 15px; } }
@media (prefers-reduced-motion: reduce) { .disclosure svg { transition: none; } }
</style>
