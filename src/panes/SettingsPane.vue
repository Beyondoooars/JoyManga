<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import {
  PhBookOpenText,
  PhCaretRight,
  PhDatabase,
  PhFolderSimple,
  PhInfo,
  PhWifiHigh,
} from '@phosphor-icons/vue'
import PageHeader from '../components/PageHeader.vue'
import { getVersion } from '@tauri-apps/api/app'
import { useDialog, useMessage } from 'naive-ui'
import {
  type NetworkProbe,
  type ReadingDirection,
  type ReadingMode,
} from '../bindings.ts'
import { useStore } from '../store.ts'
import { useBackHandler } from '../composables/useAndroidBack.ts'
import { openDialogWithBack } from '../composables/useDialogBack.ts'
import { isAndroid as isAndroidDevice } from '../composables/useIsMobile.ts'
import { useAppUpdate } from '../composables/useAppUpdate.ts'
import { getErrorMessage } from '../utils/errors.ts'
import PathField from '../components/PathField.vue'
import StorageLocationDialog from '../dialogs/StorageLocationDialog.vue'
import ThirdPartyNoticesDialog from '../dialogs/ThirdPartyNoticesDialog.vue'
import SettingsNetworkPanel from './settings/SettingsNetworkPanel.vue'
import SettingsDataPanel from './settings/SettingsDataPanel.vue'

const store = useStore()
const message = useMessage()
const dialog = useDialog()
// vue-tsc 3.3.14 在多分支模板中会对导入布尔值触发误解包（TS2551），保留 computed 规避
const isAndroid = computed(() => isAndroidDevice)
const emit = defineEmits<{ (e: 'back'): void }>()

const showStoragePicker = ref(false)
const showThirdPartyNotices = ref(false)
// 二级面板：设置主页只列分类，点进去才看到具体项，避免一屏塞满所有输入框。
const openPanel = ref<'storage' | 'network' | 'reading' | 'data' | 'about' | null>(null)
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
  if (showThirdPartyNotices.value) {
    showThirdPartyNotices.value = false
    return true
  }
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
})

onMounted(() => {
  void getVersion()
    .then((value) => {
      if (!disposed) appVersion.value = value
    })
    .catch((error) => {
      console.error('读取应用版本失败', error)
    })
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
const readingModeOptions = [
  { label: '单页翻页', value: 'Paginated' },
  { label: '上下滚动', value: 'Scroll' },
]
const readingMode = computed({
  get: () => store.config?.readingMode ?? 'Paginated',
  set: (value: ReadingMode) => {
    if (store.config) store.config.readingMode = value
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
// 并发上限与后端 validate() 保持一致：越界会让整份配置保存失败。
// 输入框清空时 naive-ui 给 null，同样兜底到默认值而不是写入 0。
const chapterConcurrency = computed({
  get: () => store.config?.chapterConcurrency ?? 2,
  set: (value: number | null) => {
    if (!store.config) return
    const next = value === null || !Number.isFinite(value) ? 2 : Math.round(value)
    store.config.chapterConcurrency = Math.min(10, Math.max(1, next))
  },
})
const imgConcurrency = computed({
  get: () => store.config?.imgConcurrency ?? 8,
  set: (value: number | null) => {
    if (!store.config) return
    const next = value === null || !Number.isFinite(value) ? 8 : Math.round(value)
    store.config.imgConcurrency = Math.min(50, Math.max(1, next))
  },
})

/** 目录选择器确认后的书库位置变更 */
function chooseDownloadDir(path: string) {
  openDialogWithBack(dialog, {
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
        store.invalidateDownloadedLibrary({ clear: true })
        message.success('书库位置已更新')
      } catch (error) {
        config.downloadDir = previous
        message.error(`书库位置未能保存：${getErrorMessage(error)}`)
      }
    },
  })
}

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

const readingSummary = computed(() => {
  if (readingMode.value === 'Scroll') return '上下滚动'
  return readingDirection.value === 'RightToLeft' ? '单页 · 从右往左' : '单页 · 从左往右'
})
</script>

<template>
  <div v-if="store.config" class="settings-page">
    <PageHeader
      :title="panelTitle"
      :subtitle="openPanel === null ? '调整下载、网络与阅读习惯' : undefined"
      @back="goBack" />

    <div class="settings-scroll">
      <!-- 一级：分类列表 -->
      <template v-if="openPanel === null">
        <div class="group">
          <button type="button" class="nav-row" @click="openPanel = 'storage'">
            <span class="row-icon is-brand"><PhFolderSimple :size="19" weight="duotone" /></span>
            <div class="nav-copy">
              <strong>下载与保存</strong>
              <span>{{ store.config.downloadDir }}</span>
            </div>
            <PhCaretRight :size="16" aria-hidden="true" />
          </button>
          <button type="button" class="nav-row" @click="openPanel = 'network'">
            <span class="row-icon is-accent"><PhWifiHigh :size="19" weight="duotone" /></span>
            <div class="nav-copy">
              <strong>网络</strong>
              <span>{{ domainStatus ? domainStatus.message : '正在读取线路状态…' }}</span>
            </div>
            <PhCaretRight :size="16" aria-hidden="true" />
          </button>
          <button type="button" class="nav-row" @click="openPanel = 'reading'">
            <span class="row-icon is-brand"><PhBookOpenText :size="19" weight="duotone" /></span>
            <div class="nav-copy">
              <strong>阅读</strong>
              <span>{{ readingSummary }}</span>
            </div>
            <PhCaretRight :size="16" aria-hidden="true" />
          </button>
        </div>

        <div class="group">
          <button type="button" class="nav-row" @click="openPanel = 'data'">
            <span class="row-icon is-neutral"><PhDatabase :size="19" weight="duotone" /></span>
            <div class="nav-copy">
              <strong>数据</strong>
              <span>备份、缓存与诊断包</span>
            </div>
            <PhCaretRight :size="16" aria-hidden="true" />
          </button>
          <button type="button" class="nav-row" @click="openPanel = 'about'">
            <span class="row-icon is-neutral"><PhInfo :size="19" weight="duotone" /></span>
            <div class="nav-copy">
              <strong>关于</strong>
              <span>{{ appVersion ? 'v' + appVersion : 'JoyManga' }}</span>
            </div>
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
          <p v-if="isAndroid" class="note">公共下载书库（Download/JoyManga）卸载后保留；应用专用书库会随卸载清理。</p>
          <div v-if="isAndroid" class="row">
            <div class="row-copy">
              <strong>仅 Wi-Fi 下载</strong>
              <span>移动数据下自动暂停下载</span>
            </div>
            <n-switch v-model:value="downloadOnlyWifi" aria-label="仅 Wi-Fi 下载" />
          </div>
          <div class="field">
            <label class="field-label">章节并发数</label>
            <n-slider
              v-model:value="chapterConcurrency"
              :min="1"
              :max="10"
              :step="1"
              :marks="{ 1: '1', 2: '2', 10: '10' }" />
            <span class="field-hint">同时下载几话。新配置默认 2；网络不稳定时调低更稳。</span>
          </div>
          <div class="field">
            <label class="field-label">单话图片并发数</label>
            <n-slider
              v-model:value="imgConcurrency"
              :min="1"
              :max="50"
              :step="1"
              :marks="{ 1: '1', 8: '8', 50: '50' }" />
            <span class="field-hint">
              同一话内同时下载几张图。所有章节共用此总上限；新配置默认 8，网络易超时时调低。
            </span>
          </div>
          <StorageLocationDialog v-model:show="showStoragePicker" @select="chooseDownloadDir" />
        </section>
      </template>

      <!-- 二级：阅读 -->
      <template v-else-if="openPanel === 'reading'">
        <section class="group">
          <div class="field">
            <label class="field-label">阅读方式</label>
            <n-select v-model:value="readingMode" :options="readingModeOptions" />
          </div>
          <div v-if="readingMode === 'Paginated'" class="field">
            <label class="field-label">翻页方向</label>
            <n-select v-model:value="readingDirection" :options="readingDirectionOptions" />
            <span class="field-hint">点击左侧上一页、右侧下一页；滑动翻页跟随此方向。</span>
          </div>
          <div class="row">
            <div class="row-copy"><strong>阅读时屏幕常亮</strong></div>
            <n-switch v-model:value="keepScreenOn" aria-label="阅读时屏幕常亮" />
          </div>
          <div v-if="isAndroid" class="row">
            <div class="row-copy">
              <strong>音量键翻页</strong>
              <span>音量减为下一页</span>
            </div>
            <n-switch v-model:value="volumeKeyPaging" aria-label="音量键翻页" />
          </div>
        </section>
      </template>

      <!-- 二级：关于 -->
      <template v-else-if="openPanel === 'about'">
        <section class="group">
          <div class="brand-row">
            <svg class="brand-mark" viewBox="0 0 48 48" aria-hidden="true" focusable="false">
              <rect width="48" height="48" rx="12" fill="#241C2B" />
              <g
                transform="translate(24 24) scale(0.78) translate(-24 -24)"
                stroke="#FFFFFF"
                stroke-width="1.6"
                stroke-linejoin="round">
                <path
                  d="M3.5 5.5H15C20.5228 5.5 25 9.97715 25 15.5V40.5C25 36.0817 21.4183 32.5 17 32.5H3.5V5.5Z"
                  fill="#FB7299" />
                <path
                  d="M44.5 5.5H33C27.4772 5.5 23 9.97715 23 15.5V40.5C23 36.0817 26.5817 32.5 31 32.5H44.5V5.5Z"
                  fill="#00AEEC" />
              </g>
            </svg>
            <div class="brand-copy">
              <strong>JoyManga</strong>
              <span>下载 · 归档 · 阅读</span>
            </div>
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
              <n-button v-if="updateHasUpdate" type="primary" size="small" @click="showUpdateDialog = true">
                查看新版
              </n-button>
              <n-button size="small" secondary :loading="updateChecking" @click="checkForUpdates(false)">
                检查更新
              </n-button>
            </div>
          </div>
          <button type="button" class="nav-row inner" @click="showThirdPartyNotices = true">
            <div class="nav-copy">
              <strong>开源许可</strong>
              <span>第三方组件与许可证全文</span>
            </div>
            <PhCaretRight :size="16" aria-hidden="true" />
          </button>
        </section>
        <ThirdPartyNoticesDialog v-model:show="showThirdPartyNotices" />
      </template>

      <!-- 保留 KeepAlive 期间的激活/失活钩子，避免离开设置页后测速定时器与 VPN 监听漂移 -->
      <SettingsNetworkPanel v-show="openPanel === 'network'" v-model:domain-status="domainStatus" />
      <SettingsDataPanel v-show="openPanel === 'data'" />
    </div>
  </div>
</template>

<style scoped src="./settings/settingsShared.css"></style>

<style scoped>
.settings-page {
  display: flex;
  height: 100%;
  min-height: 0;
  flex-direction: column;
  background: var(--jm-bg);
}

.settings-scroll {
  flex: 1 1 auto;
  min-height: 0;
  overflow-y: auto;
  overscroll-behavior: contain;
  touch-action: pan-y;
  padding: 12px var(--jm-gutter) 28px;
}

.row-icon {
  display: grid;
  width: 34px;
  height: 34px;
  flex: 0 0 auto;
  place-items: center;
  border-radius: var(--jm-r-sm);
}

.row-icon.is-brand {
  color: var(--jm-brand);
  background: var(--jm-brand-soft);
}

.row-icon.is-accent {
  color: var(--jm-accent);
  background: var(--jm-accent-soft);
}

.row-icon.is-neutral {
  color: var(--jm-text-2);
  background: var(--jm-sunken);
}

.field-hint {
  color: var(--jm-text-3);
  font-size: var(--jm-fs-xs);
  line-height: 1.5;
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
  border-radius: var(--jm-r-md);
}

.brand-copy {
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.brand-copy strong {
  font-size: 17px;
  font-weight: 750;
  letter-spacing: -0.015em;
}

.brand-copy span {
  color: var(--jm-text-3);
  font-size: var(--jm-fs-xs);
}

.update-cell {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
  justify-content: flex-end;
}

.update-new {
  color: var(--jm-brand);
  font-size: var(--jm-fs-xs);
  font-weight: 650;
}

.update-ok {
  color: var(--jm-success);
  font-size: var(--jm-fs-xs);
}

.update-error {
  color: var(--jm-danger);
  font-size: var(--jm-fs-xs);
}

/* 触控尺寸：控件统一 46dp。
   关键是同时改 --n-height——naive-ui 把它作为**内联样式**写在 select/input
   根元素上，内层 label、文字行、后缀箭头全按它排版；只把外层 min-height
   拉到 46px 会让文字贴顶、箭头被 top:50% 顶到偏上。作者样式的 !important
   能压过内联声明，所以这里要带 !important。 */
.settings-page :deep(.n-base-selection),
.settings-page :deep(.n-input) {
  --n-height: 46px !important;
  min-height: 46px;
}

.settings-page :deep(.n-button) {
  min-height: 42px;
}
</style>
