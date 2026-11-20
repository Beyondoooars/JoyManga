<script setup lang="ts">
import { computed, nextTick, onActivated, onBeforeUnmount, onDeactivated, onMounted, ref, toRaw, watch } from 'vue'
import { PhArrowClockwise, PhCaretRight } from '@phosphor-icons/vue'
import { useMessage } from 'naive-ui'
import {
  commands,
  type ImageDomainPreset,
  type ImageSpeedReport,
  type NetworkCheckReport,
  type NetworkProbe,
  type ProxyMode,
} from '../../bindings.ts'
import { useStore } from '../../store.ts'
import { watchNetworkVpn } from '../../utils/androidStorage.ts'
import { getErrorMessage } from '../../utils/errors.ts'

const domainStatus = defineModel<NetworkProbe | undefined>('domainStatus', { required: true })

const store = useStore()
const message = useMessage()

const manualNetwork = ref(false)
const advancedNetwork = ref(false)
const imagePresets = ref<ImageDomainPreset[]>([])
const checking = ref(false)
const speedTesting = ref(false)
const applyingFastest = ref(false)
const speedReport = ref<ImageSpeedReport>()
const speedError = ref('')
const checkResult = ref<NetworkCheckReport>()
const networkError = ref('')

const SPEED_TEST_TIMEOUT_MS = 90_000
let checkGeneration = 0
let speedGeneration = 0
let activeSpeedRunId = 0
let speedTimeoutTimer: ReturnType<typeof setTimeout> | undefined
let disposed = false
let stopVpnWatch: (() => void) | undefined
let lastVpnActive: boolean | undefined
let lastNetworkPathKey: string | null | undefined

function clearSpeedTimeout() {
  if (speedTimeoutTimer !== undefined) {
    clearTimeout(speedTimeoutTimer)
    speedTimeoutTimer = undefined
  }
}

function armSpeedTimeout(runId: number, current: number) {
  clearSpeedTimeout()
  speedTimeoutTimer = setTimeout(() => {
    speedTimeoutTimer = undefined
    if (activeSpeedRunId !== runId) return
    cancelRunningSpeedTest()
    activeSpeedRunId++
    speedTesting.value = false
    if (!disposed && current === speedGeneration) {
      speedError.value = '测速超时，请稍后重试'
    }
  }, SPEED_TEST_TIMEOUT_MS)
}

onBeforeUnmount(() => {
  disposed = true
  stopVpnWatch?.()
  stopVpnWatch = undefined
  cancelRunningSpeedTest()
  clearSpeedTimeout()
  activeSpeedRunId++
  checkGeneration++
  speedGeneration++
})

// 设置页被 KeepAlive 缓存：离开后 onBeforeUnmount 不会触发，在途请求的回调
// 仍会写进这份状态。失活时同样作废代数，重新进入时再刷新一次。
// 在途测速不在这里放开 speedTesting：后端请求仍在跑，必须等它结束（见 finally）。
onDeactivated(() => {
  stopVpnWatch?.()
  stopVpnWatch = undefined
  cancelRunningSpeedTest()
  clearSpeedTimeout()
  lastVpnActive = undefined
  lastNetworkPathKey = undefined
  checkGeneration++
  speedGeneration++
  checking.value = false
})

onActivated(() => {
  if (disposed) return
  subscribeVpnWatch()
  void refreshDomainStatus()
  if (speedTesting.value && speedTimeoutTimer === undefined) {
    armSpeedTimeout(activeSpeedRunId, speedGeneration)
  }
})

onMounted(() => {
  subscribeVpnWatch()
  void refreshDomainStatus()
})

void commands
  .getImageDomainPresets()
  .then((value) => {
    if (!disposed) imagePresets.value = value
  })
  .catch((error) => {
    // 线路列表加载失败不影响其他设置项，只记日志；写进 networkError 会让用户
    // 一进网络面板就看到一条与「检查连接」无关的报错。
    console.error('加载图片线路预设失败', error)
  })

async function refreshDomainStatus() {
  try {
    const result = await commands.getApiDomainStatus()
    if (disposed) return
    if (result.status === 'ok') domainStatus.value = result.data
  } catch (error) {
    console.error('刷新接口域名状态失败', error)
  }
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

// 代理输入框通过计算属性读写，避免模板里对可能为空的 store.config 直接取属性
const proxyHost = computed({
  get: () => store.config?.proxyHost ?? '',
  set: (value: string) => {
    if (store.config) store.config.proxyHost = value
  },
})

// 输入框被清空时 naive-ui 会给出 null；null 进入配置会让 Rust 的 u16 反序列化失败，
// 导致此后每一次保存/检查连接都报“设置格式无效”。这里兜底为默认端口。
const proxyPort = computed<number | null>({
  get: () => store.config?.proxyPort ?? 7890,
  set: (value) => {
    if (store.config) store.config.proxyPort = value === null || !Number.isFinite(value) ? 7890 : value
  },
})

const networkKey = computed(() => {
  const config = store.config
  return config
    ? JSON.stringify([
        config.apiDomainMode,
        config.customApiDomain,
        config.imageDomainMode,
        config.customImageDomain,
        config.proxyMode,
        config.proxyHost,
        config.proxyPort,
      ])
    : ''
})

watch(networkKey, () => {
  if (applyingFastest.value) return
  cancelRunningSpeedTest()
  checkGeneration++
  speedGeneration++
  checking.value = false
  speedReport.value = undefined
  speedError.value = ''
  checkResult.value = undefined
  networkError.value = ''
  void refreshDomainStatus()
})

function subscribeVpnWatch() {
  // onMounted 与 onActivated 都会调用；先解绑避免 KeepAlive 往返后重复监听。
  stopVpnWatch?.()
  stopVpnWatch = watchNetworkVpn((active, currentNetworkKey) => {
    // 只比较 VPN 布尔值会漏掉 Wi-Fi↔蜂窝：后端把默认网络标识变化同样视为新路径。
    const changed = lastVpnActive !== undefined
      && (lastVpnActive !== active || lastNetworkPathKey !== currentNetworkKey)
    lastVpnActive = active
    lastNetworkPathKey = currentNetworkKey
    if (!changed) return
    // 旧路径下的测速结果不再可信；同时让后端停止剩余线路。
    cancelRunningSpeedTest()
    checkGeneration++
    speedGeneration++
    checking.value = false
    speedReport.value = undefined
    speedError.value = ''
    checkResult.value = undefined
    networkError.value = ''
    void refreshDomainStatus()
  })
}

/** 让后端尽快停止剩余线路，避免离开设置页或改配置后继续占用带宽。 */
function cancelRunningSpeedTest() {
  if (!speedTesting.value) return
  void commands.cancelImageSpeedTest().catch(() => { /* 旧后端没有该命令 */ })
}

async function testImageSpeed() {
  if (!store.config || speedTesting.value) return
  const current = ++speedGeneration
  const runId = ++activeSpeedRunId
  speedTesting.value = true
  speedReport.value = undefined
  speedError.value = ''
  armSpeedTimeout(runId, current)
  try {
    const sampleId = store.pickedComic?.id ?? store.searchResult?.content[0]?.id ?? store.getWeeklyResult?.list[0]?.id
    if (!sampleId || sampleId <= 0) throw new Error('先打开一本在线漫画或加载每周必看，再测速')
    const result = await commands.testImageDownloadSpeed(structuredClone(toRaw(store.config)), sampleId)
    if (disposed || current !== speedGeneration || runId !== activeSpeedRunId) return
    if (result.status === 'error') speedError.value = result.error.err_message
    else speedReport.value = result.data
  } catch (error) {
    if (current === speedGeneration && !disposed && runId === activeSpeedRunId) speedError.value = getErrorMessage(error)
  } finally {
    if (runId === activeSpeedRunId) {
      clearSpeedTimeout()
      // 前端同一时刻只允许一次测速，失效的旧请求结束后才能放开按钮。
      speedTesting.value = false
    }
  }
}

async function applyFastestImageDomain() {
  const best = speedReport.value?.best
  const config = store.config
  if (!best || !config || applyingFastest.value) return
  applyingFastest.value = true
  const previous = config.imageDomainMode
  const previousReport = speedReport.value
  config.imageDomainMode = best
  try {
    await store.saveConfigNow()
    message.success('已使用本次测速最快的图片线路；网络路径变化后需重新测速')
  } catch (error) {
    config.imageDomainMode = previous
    // 回退配置会再次触发 networkKey watcher 清空报告；等 watcher 跑完再恢复，
    // 否则一次保存失败会让用户丢掉整轮测速结果、只能重跑。
    await nextTick()
    speedReport.value = previousReport
    message.error(`图片线路未能保存：${getErrorMessage(error)}`)
  } finally {
    applyingFastest.value = false
  }
}

function formatSpeed(bytesPerSecond: number) {
  return `${(bytesPerSecond / 1024 / 1024).toFixed(2)} MB/s`
}

async function checkNetwork() {
  if (!store.config || checking.value) return
  const current = ++checkGeneration
  checking.value = true
  checkResult.value = undefined
  networkError.value = ''
  try {
    const sampleId = store.pickedComic?.id ?? store.searchResult?.content[0]?.id ?? store.getWeeklyResult?.list[0]?.id
    const result = await commands.checkNetwork(
      structuredClone(toRaw(store.config)),
      sampleId && sampleId > 0 ? sampleId : null,
    )
    if (disposed || current !== checkGeneration) return
    if (result.status === 'error') networkError.value = result.error.err_message
    else checkResult.value = result.data
  } catch (error) {
    if (current === checkGeneration && !disposed) networkError.value = getErrorMessage(error)
  } finally {
    if (current === checkGeneration) checking.value = false
  }
}
</script>

<template>
  <div v-if="store.config">
    <section class="group">
      <div class="row">
        <span>域名自动更新</span>
        <span class="row-value">{{ domainStatus ? domainStatus.message : '正在读取…' }}</span>
      </div>
      <button
        type="button"
        class="nav-row inner"
        :aria-expanded="manualNetwork"
        @click="manualNetwork = !manualNetwork">
        <div class="nav-copy">
          <strong>手动设置线路</strong>
          <span>加载异常时更换</span>
        </div>
        <PhCaretRight :size="16" class="chevron" :class="{ expanded: manualNetwork }" aria-hidden="true" />
      </button>
      <div v-if="manualNetwork" class="field-stack">
        <div class="speed-test">
          <div class="row-copy">
            <strong>图片线路测速</strong>
            <span>
              每条线路先预热连接，再并发下载同一封面 {{ speedReport?.sampleRequests ?? 4 }} 次并统计实际吞吐。
              连接或断开 VPN 后重新测速，不保证 VPN 一定提速。
            </span>
          </div>
          <n-button secondary :loading="speedTesting" @click="testImageSpeed">测速全部图片线路</n-button>
        </div>
        <n-alert v-if="speedError" type="error" :show-icon="false">{{ speedError }}</n-alert>
        <div v-if="speedReport" class="speed-results">
          <p class="note">
            {{ speedReport.vpnActive ? '测速时已识别到系统 VPN' : '测速时未识别到系统 VPN' }}；以下为本次实际结果。
          </p>
          <div v-for="result in speedReport.results" :key="result.mode" class="speed-result">
            <span>{{ result.label }} · {{ result.domain }}</span>
            <strong v-if="result.bytesPerSecond != null">
              {{ formatSpeed(result.bytesPerSecond) }} · {{ result.elapsedMs }} ms
              <template v-if="result.successfulSamples != null">
                · {{ result.successfulSamples }}/{{ speedReport.sampleRequests }} 次成功
              </template>
            </strong>
            <span v-else class="speed-error">{{ result.error }}</span>
          </div>
          <n-button
            v-if="speedReport.best"
            type="primary"
            block
            :loading="applyingFastest"
            @click="applyFastestImageDomain">
            使用最快线路
          </n-button>
        </div>
        <div class="field">
          <label class="field-label">漫画信息</label>
          <n-select v-model:value="store.config.apiDomainMode" :options="apiOptions" />
          <n-input
            v-if="store.config.apiDomainMode === 'Custom'"
            v-model:value="store.config.customApiDomain"
            placeholder="服务器域名，不含 https://" />
        </div>
        <div class="field">
          <label class="field-label">图片下载</label>
          <n-select v-model:value="store.config.imageDomainMode" :options="imageOptions" />
          <n-input
            v-if="store.config.imageDomainMode === 'Custom'"
            v-model:value="store.config.customImageDomain"
            placeholder="图片服务器域名，不含 https://" />
        </div>
        <n-button secondary :loading="checking" @click="checkNetwork">
          <template #icon><PhArrowClockwise :size="16" aria-hidden="true" /></template>
          检查连接
        </n-button>
        <n-alert v-if="networkError" type="error" :show-icon="false">{{ networkError }}</n-alert>
        <div v-if="checkResult" class="probe-list" aria-live="polite">
          <div
            v-for="probe in [checkResult.api, checkResult.image, checkResult.domain]"
            :key="probe.label"
            class="probe"
            :data-state="probe.state">
            <div>
              <strong>{{ probe.label }}</strong>
              <span>
                {{ probe.state === 'Passed' ? '正常' : probe.state === 'Skipped' ? '未检查' : '连接异常' }}
                <template v-if="probe.elapsedMs !== null">· {{ probe.elapsedMs }} ms</template>
              </span>
            </div>
            <p>{{ probe.message }}</p>
          </div>
        </div>
      </div>
    </section>

    <section class="group">
      <button
        type="button"
        class="nav-row inner"
        :aria-expanded="advancedNetwork"
        @click="advancedNetwork = !advancedNetwork">
        <div class="nav-copy">
          <strong>代理</strong>
          <span>
            {{ proxyMode === 'Custom' ? '手动 HTTP 代理' : proxyMode === 'NoProxy' ? '直接连接' : '跟随系统' }}
          </span>
        </div>
        <PhCaretRight :size="16" class="chevron" :class="{ expanded: advancedNetwork }" aria-hidden="true" />
      </button>
      <div v-if="advancedNetwork" class="field-stack">
        <n-select v-model:value="proxyMode" :options="proxyOptions" />
        <div v-if="proxyMode === 'Custom'" class="proxy-grid">
          <div class="field">
            <label class="field-label">主机</label>
            <n-input v-model:value="proxyHost" placeholder="127.0.0.1" />
          </div>
          <div class="field">
            <label class="field-label">端口</label>
            <n-input-number v-model:value="proxyPort" :min="1" :max="65535" :show-button="false" />
          </div>
        </div>
        <p class="note">
          VPN 由系统接管流量。检测到系统 VPN
          时，应用会暂时忽略这里的手动代理直连，避免继续连接本机代理端口导致无法联网。开 VPN
          后仍连不上时，先用「跟随系统」或「直接连接」判断是不是线路本身的问题；设置 → 数据 → 导出诊断包
          会记录当时是否识别到 VPN、用的是哪条线路。
        </p>
      </div>
    </section>
  </div>
</template>

<style scoped src="./settingsShared.css"></style>

<style scoped>
.proxy-grid {
  display: grid;
  grid-template-columns: minmax(0, 1fr) 120px;
  gap: 8px;
}

.speed-test,
.speed-result {
  display: flex;
  flex-direction: column;
  gap: 10px;
  padding: 8px 0;
}

.speed-result {
  flex-direction: row;
  align-items: flex-start;
  justify-content: space-between;
  padding: 8px 0;
  border-bottom: 1px solid var(--jm-line);
  font-size: var(--jm-fs-xs);
}

.speed-result > span:first-child,
.speed-error {
  overflow-wrap: anywhere;
}

.speed-result strong {
  flex-shrink: 0;
  color: var(--jm-brand);
}

.speed-error {
  color: var(--jm-danger);
}

.proxy-grid .field {
  padding: 0;
}

.probe-list {
  display: grid;
  gap: 8px;
}

.probe {
  padding: 12px 14px;
  border: 1px solid var(--jm-line);
  border-radius: var(--jm-r-md);
  background: var(--jm-sunken);
}

.probe > div {
  display: flex;
  flex-wrap: wrap;
  justify-content: space-between;
  gap: 8px;
  font-size: var(--jm-fs-sm);
}

.probe p {
  margin: 6px 0 0;
  color: var(--jm-text-2);
  font-size: var(--jm-fs-xs);
  line-height: 1.6;
  overflow-wrap: anywhere;
}

.probe[data-state='Passed'] span {
  color: var(--jm-success);
}

.probe[data-state='Failed'] span {
  color: var(--jm-danger);
}
</style>
