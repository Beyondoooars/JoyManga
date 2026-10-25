<script setup lang="tsx">
import { computed, defineAsyncComponent, nextTick, onMounted, onUnmounted, ref, toRaw, watch } from 'vue'
import type { Component } from 'vue'
import { commands } from './bindings.ts'
import { useMessage, useNotification } from 'naive-ui'
import { PhBooks, PhCompass, PhDownloadSimple, PhHeart, PhUser } from '@phosphor-icons/vue'
import { useStore } from './store.ts'
import icon from './assets/icon.png'
import type { CurrentTabName } from './types.ts'
import { getErrorMessage } from './utils/errors.ts'
import { handleRootBackExit, useBackHandler } from './composables/useAndroidBack.ts'
import { useIsMobile } from './composables/useIsMobile.ts'
import { useAppUpdate } from './composables/useAppUpdate.ts'
import { canWritePublicDownloads, downloadsPath, hasAllFilesAccess, requestAllFilesAccessAndWait, supportsPublicStorage, watchNetworkMetered, watchNetworkVpn } from './utils/androidStorage.ts'
import { onOpenExternalComic } from './libraryApi.ts'

const StorageOnboardingDialog = defineAsyncComponent(() => import('./dialogs/StorageOnboardingDialog.vue'))
const ReaderDialog = defineAsyncComponent(() => import('./dialogs/ReaderDialog.vue'))
const UpdateDialog = defineAsyncComponent(() => import('./dialogs/UpdateDialog.vue'))
const ChapterPane = defineAsyncComponent(() => import('./panes/ChapterPane.vue'))

const pageComponents: Record<CurrentTabName, Component> = {
  search: defineAsyncComponent(() => import('./panes/SearchPane.vue')),
  favorite: defineAsyncComponent(() => import('./panes/FavoritePane.vue')),
  weekly: defineAsyncComponent(() => import('./panes/WeeklyPane.vue')),
  downloaded: defineAsyncComponent(() => import('./panes/DownloadedPane/DownloadedPane.vue')),
  chapter: defineAsyncComponent(() => import('./panes/ChapterPane.vue')),
  account: defineAsyncComponent(() => import('./panes/AccountPane.vue')),
  settings: defineAsyncComponent(() => import('./panes/SettingsPane.vue')),
}

const store = useStore()
const message = useMessage()
const notification = useNotification()
const { isAndroid } = useIsMobile()
const { checkForUpdates } = useAppUpdate()

/** 底部导航的四个一级页面；其余（详情、设置）是压栈的二级页面 */
const MAIN_TABS: CurrentTabName[] = ['search', 'favorite', 'downloaded', 'account']

const initializing = ref(false)
const initializationError = ref('')
// 首次启动（未完成过存储初始化且未获所有文件权限）时弹存储引导；不阻塞应用使用
const storageOnboardingShowing = ref(false)
// 配置加载与监听注册全部完成前不渲染主界面：否则“拿到配置但监听注册失败”时
// 错误页永远不出现，用户只看到一闪而过的提示、下载事件却无人接收。
const appReady = ref(false)
let configLoaded = false
let configSaveRevision = 0
let configSaveTimer: ReturnType<typeof setTimeout> | undefined
let configSaveQueue = Promise.resolve()

store.registerConfigSaveWaiter(async () => {
  configSaveRevision++
  if (configSaveTimer !== undefined) {
    clearTimeout(configSaveTimer)
    configSaveTimer = undefined
  }
  await configSaveQueue
})

const chromeVisible = computed(() => MAIN_TABS.includes(store.currentTabName) && !store.chapterDetailShowing)

function navigateToTab(tab: CurrentTabName) {
  if (tab === 'account') {
    // 走 openAccount：记录来处以便返回，并按最新配置重置登录表单
    store.openAccount()
    return
  }
  if (tab === 'settings') {
    store.openSettings()
    return
  }
  store.closeComicDetail()
  if (store.currentTabName === tab && tab === 'downloaded') {
    store.downloadedSubTab = 'library'
  }
  store.currentTabName = tab
}

const currentPaneComponent = computed(() => pageComponents[store.currentTabName])

useBackHandler(() => {
  if (store.activeReaderTarget !== null) {
    store.closeReader()
    return true
  }
  // 设置页自己注册了更高优先级的返回处理（先退二级面板再离开页面），这里只兜底
  if (store.currentTabName === 'settings') {
    return store.closeSettings()
  }
  if (store.chapterDetailShowing) {
    store.closeComicDetail()
    return true
  }
  if (store.currentTabName === 'downloaded' && store.downloadedSubTab === 'transfers') {
    store.downloadedSubTab = 'library'
    return true
  }
  // 「我的」是从其他标签进去的：返回键先回到来处，而不是直接提示退出应用
  if (store.closeAccount()) {
    return true
  }
  // 其余二级标签回到「发现」，避免一次返回就直接退出应用
  if (store.currentTabName === 'favorite' || store.currentTabName === 'downloaded') {
    store.currentTabName = 'search'
    return true
  }
  return handleRootBackExit((msg) => message.info(msg))
}, 1)

watch(
  () => [store.activeDownloadCount, store.downloadSpeed, store.activeDownloadPages] as const,
  ([active, speed, pages]) => {
    if (isAndroid) window.JmAndroid?.downloadStatus(active, speed, pages.current, pages.total)
  },
  { immediate: true },
)

watch(
  () => store.config,
  (config) => {
    if (!configLoaded || config === undefined || store.configMaintenanceInProgress || !store.configIsDirty()) {
      return
    }

    const revision = ++configSaveRevision
    const snapshot = structuredClone(toRaw(config))
    const base = structuredClone(toRaw(store.authoritativeConfig!))
    if (configSaveTimer !== undefined) {
      clearTimeout(configSaveTimer)
    }
    configSaveTimer = setTimeout(() => {
      configSaveQueue = configSaveQueue
        .catch((error) => {
          console.error('上一次配置保存任务异常退出', error)
        })
        .then(async () => {
          if (revision !== configSaveRevision || store.configMaintenanceInProgress) return
          try {
            await store.persistConfig(snapshot, base)
          } catch (error) {
            console.error(error)
            if (revision === configSaveRevision) {
              message.error(`保存配置失败：${getErrorMessage(error)}`)
            }
          }
        })
    }, 400)
  },
  { deep: true },
)

/** 屏蔽 WebView 长按菜单，但输入框要保留系统粘贴菜单。 */
function preventContextMenu(event: MouseEvent) {
  const target = event.target as HTMLElement | null
  if (target?.closest('input, textarea, [contenteditable="true"]')) return
  event.preventDefault()
}

async function initializeApp() {
  if (initializing.value) return

  initializing.value = true
  initializationError.value = ''
  appReady.value = false
  try {
    await store.initStateListeners()
    store.applyConfig(await commands.getConfig())
    if (isAndroid && store.config && window.JmAndroid) {
      const bridge = window.JmAndroid
      // 已初始化也可能停在旧的私有目录（旧版配置迁移过来的），或用户撤销了
      // 「所有文件访问」权限。这两种情况都要重新选书库，否则下载写进一个目录、
      // 书库扫描另一个目录，表现就是「下载完成却看不到本子」。
      const libraryLocationUninitialized = !store.config.androidStorageInitialized
      if (libraryLocationUninitialized || !hasAllFilesAccess()) {
        if (!supportsPublicStorage()) {
          // Android 10 既没有「所有文件访问」也无法申请写外部存储：直接落到应用专用目录
          if (!store.config.androidStorageInitialized) {
            const result = await commands.initializeAndroidStorage(bridge.libraryPath() ?? '')
            if (result.status === 'error') throw new Error(result.error.err_message)
            store.applyConfig(result.data)
          }
        } else {
          // 默认就落在公共下载 Download/JoyManga：未授权时先申请「所有文件访问」，
          // 用户返回后再完成初始化，不再让用户手动挑目录。
          if (!hasAllFilesAccess()) {
            await requestAllFilesAccessAndWait()
          }
          if (hasAllFilesAccess() && canWritePublicDownloads()) {
            const result = await commands.initializeAndroidStorage(downloadsPath())
            if (result.status === 'error') {
              // 初始化失败不再让启动进入不可恢复的错误页，交给引导让用户选应用专用目录
              console.error(result.error)
              storageOnboardingShowing.value = true
            } else {
              store.applyConfig(result.data)
            }
          } else if (libraryLocationUninitialized) {
            // 用户拒绝授权或公共目录暂不可写：兜底引导，可改用应用专用目录
            storageOnboardingShowing.value = true
          }
        }
      }
    }
    await store.initDownloadListener()
    await nextTick()
    configLoaded = true
    appReady.value = true
  } catch (error) {
    console.error(error)
    initializationError.value = getErrorMessage(error)
    message.error(`应用初始化失败：${initializationError.value}`)
    return
  } finally {
    initializing.value = false
  }

  if (store.config?.autoLogin && store.config.username !== '' && store.config.password !== '') {
    try {
      const result = await commands.login(store.config.username, store.config.password)
      if (result.status === 'error') {
        console.error(result.error)
        message.warning(`自动登录失败：${result.error.err_message}`)
        return
      }
      store.applyLoginResult(result.data)
      message.success('自动登录成功')
    } catch (error) {
      console.error(error)
      message.warning(`自动登录失败：${getErrorMessage(error)}`)
    }
  }
}

let stopNetworkWatch: (() => void) | undefined
let stopVpnWatch: (() => void) | undefined
let stopCancelListener: (() => void) | undefined
let stopOpenFileListener: (() => void) | undefined

onMounted(() => {
  document.addEventListener('contextmenu', preventContextMenu)
  // 把 Android 的网络计费状态同步给后端，「仅 Wi-Fi 下载」依赖它做判断
  stopNetworkWatch = watchNetworkMetered((metered) => {
    void commands.setNetworkMetered(metered).catch(() => { /* 旧后端无此命令时忽略 */ })
  })
  // VPN 开启时后端跳过手动代理，否则「开 VPN 反而连不上」
  stopVpnWatch = watchNetworkVpn((active) => {
    void commands.setNetworkVpn(active).catch(() => { /* 旧后端无此命令时忽略 */ })
  })
  // 通知栏"取消当前"：由原生转发事件，这里执行取消并给出反馈
  const onCancelCurrent = async () => {
    const cancelled = await store.cancelCurrentDownload()
    if (cancelled) message.info(`已取消：${cancelled}`)
  }
  window.addEventListener('jm-download-cancel', onCancelCurrent)
  stopCancelListener = () => window.removeEventListener('jm-download-cancel', onCancelCurrent)
  // 文件管理器/其他应用「用 JoyManga 打开」CBZ：导入后切换到本地书库
  stopOpenFileListener = onOpenExternalComic({
    onImported: (report) => {
      void store.refreshDownloadedComics().catch((error) => console.error('导入后刷新书库失败', error))
      // 导入是逐文件进行的：损坏/非法 CBZ 会进入 errors，成功数可能为 0。不能一律提示成功。
      if (report.imported > 0) {
        message.success(`已导入 ${report.imported} 本`)
        navigateToTab('downloaded')
      } else if (report.errors.length === 0) {
        message.info(report.skipped > 0 ? `漫画已在书库中（${report.skipped} 本）` : '没有可导入的漫画')
      }
      if (report.errors.length > 0) {
        message.error(`导入失败：${report.errors[0]}`)
      }
    },
    onError: (text) => message.error(`打开文件失败：${text}`),
  })
  void store.initLogListener(notification)
  void store.initSpeedListener()
  void initializeApp()
  // 延迟 2.5 秒静默检测更新，避开冷启动峰值
  setTimeout(() => {
    void checkForUpdates(true)
  }, 2500)
})

onUnmounted(() => {
  document.removeEventListener('contextmenu', preventContextMenu)
  stopNetworkWatch?.()
  stopNetworkWatch = undefined
  stopVpnWatch?.()
  stopVpnWatch = undefined
  stopCancelListener?.()
  stopCancelListener = undefined
  stopOpenFileListener?.()
  stopOpenFileListener = undefined
  if (configSaveTimer !== undefined) {
    clearTimeout(configSaveTimer)
  }
})
</script>

<template>
  <div v-if="appReady && store.config !== undefined" class="app-shell">
    <!-- 顶栏：品牌 + 下载状态 + 设置；二级页面自带导航，不再重复一条 -->
    <header v-if="chromeVisible" class="app-header">
      <button type="button" class="brand-button" title="回到发现" @click="navigateToTab('search')">
        <span class="brand-mark" aria-hidden="true">
          <svg viewBox="0 0 48 48" focusable="false">
            <g stroke="#241C2B" stroke-width="3" stroke-linejoin="round">
              <path d="M3.5 5.5H15C20.5228 5.5 25 9.97715 25 15.5V40.5C25 36.0817 21.4183 32.5 17 32.5H3.5V5.5Z" fill="#FB7299" />
              <path d="M44.5 5.5H33C27.4772 5.5 23 9.97715 23 15.5V40.5C23 36.0817 26.5817 32.5 31 32.5H44.5V5.5Z" fill="#00AEEC" />
            </g>
          </svg>
        </span>
        <span class="brand-title">Joy<span>Manga</span></span>
      </button>

      <div class="header-tools">
        <button
          v-if="store.activeDownloadCount > 0"
          type="button"
          class="download-chip"
          :aria-label="`${store.activeDownloadCount} 个章节正在下载，查看传输`"
          @click="store.openTransfers()">
          <PhDownloadSimple :size="15" weight="bold" />
          <span class="download-chip-count">{{ store.activeDownloadCount }}</span>
          <span v-if="store.downloadSpeed" class="download-chip-speed">{{ store.downloadSpeed }}</span>
        </button>
      </div>
    </header>

    <n-alert
      v-if="store.config?.apiDomainMigrationPending"
      class="shell-alert"
      type="warning"
      closable
      @close="store.config.apiDomainMigrationPending = false">
      已保留升级前的线路设置，可在设置 → 网络中核对。
    </n-alert>

    <main class="app-body">
      <KeepAlive>
        <ChapterPane
          v-if="store.chapterDetailShowing"
          key="chapter-detail"
          @back="store.closeComicDetail" />
        <component
          v-else
          :is="currentPaneComponent"
          :key="store.currentTabName"
          @open-login="navigateToTab('account')"
          @back="store.closeSettings"
          @open-transfers="store.openTransfers()" />
      </KeepAlive>
    </main>

    <!-- 底部导航：发现 / 收藏 / 书库 / 我的 -->
    <nav v-if="chromeVisible" class="app-tabbar" aria-label="主导航">
      <button
        type="button"
        class="tab-item"
        :class="{ 'is-active': store.currentTabName === 'search' }"
        :aria-current="store.currentTabName === 'search' ? 'page' : undefined"
        @click="navigateToTab('search')">
        <span class="tab-icon"><PhCompass :size="23" :weight="store.currentTabName === 'search' ? 'fill' : 'regular'" /></span>
        <span class="tab-label">发现</span>
      </button>

      <button
        type="button"
        class="tab-item"
        :class="{ 'is-active': store.currentTabName === 'favorite' }"
        :aria-current="store.currentTabName === 'favorite' ? 'page' : undefined"
        @click="navigateToTab('favorite')">
        <span class="tab-icon"><PhHeart :size="23" :weight="store.currentTabName === 'favorite' ? 'fill' : 'regular'" /></span>
        <span class="tab-label">收藏</span>
      </button>

      <button
        type="button"
        class="tab-item"
        :class="{ 'is-active': store.currentTabName === 'downloaded' }"
        :aria-current="store.currentTabName === 'downloaded' ? 'page' : undefined"
        @click="navigateToTab('downloaded')">
        <span class="tab-icon">
          <PhBooks :size="23" :weight="store.currentTabName === 'downloaded' ? 'fill' : 'regular'" />
          <span v-if="store.activeDownloadCount > 0" class="tab-badge">{{ store.activeDownloadCount > 99 ? '99+' : store.activeDownloadCount }}</span>
        </span>
        <span class="tab-label">书库</span>
      </button>

      <button
        type="button"
        class="tab-item"
        :class="{ 'is-active': store.currentTabName === 'account' }"
        :aria-current="store.currentTabName === 'account' ? 'page' : undefined"
        @click="navigateToTab('account')">
        <span class="tab-icon"><PhUser :size="23" :weight="store.currentTabName === 'account' ? 'fill' : 'regular'" /></span>
        <span class="tab-label">我的</span>
      </button>
    </nav>

    <!-- Modals and Dialogs -->
    <StorageOnboardingDialog v-if="storageOnboardingShowing" v-model:show="storageOnboardingShowing" />
    <ReaderDialog v-if="store.activeReaderTarget" />
    <UpdateDialog />
  </div>

  <div v-else-if="initializationError" class="app-loading" role="alert">
    <img :src="icon" alt="JoyManga" class="loading-logo" />
    <div class="loading-copy">
      <strong>应用初始化失败</strong>
      <span>{{ initializationError }}</span>
    </div>
    <n-button type="primary" :loading="initializing" @click="initializeApp">重试</n-button>
  </div>

  <div v-else class="app-loading" aria-live="polite">
    <img :src="icon" alt="JoyManga" class="loading-logo is-pulsing" />
    <div class="loading-copy">
      <strong>Joy<span>Manga</span></strong>
      <span>正在加载精彩漫库…</span>
    </div>
    <div class="loading-bar"><i></i></div>
  </div>
</template>

<style scoped>
.app-shell {
  display: flex;
  width: 100%;
  height: 100vh;
  height: 100dvh;
  flex-direction: column;
  overflow: hidden;
  background: var(--jm-bg);
  color: var(--jm-text);
}

/* 顶栏：52px，随状态栏安全区内缩 */
.app-header {
  display: flex;
  height: calc(var(--jm-header-h) + var(--safe-top));
  flex: 0 0 auto;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: var(--safe-top) calc(var(--jm-gutter) - 4px) 0 calc(var(--jm-gutter) - 4px);
  border-bottom: 1px solid var(--jm-line);
  background: rgba(255, 255, 255, 0.96);
  backdrop-filter: saturate(180%) blur(12px);
}

.brand-button {
  display: flex;
  min-height: 44px;
  align-items: center;
  gap: 9px;
  padding: 4px 6px;
  border: 0;
  border-radius: var(--jm-r-md);
  background: transparent;
  cursor: pointer;
  transition: background-color 160ms ease;
}

.brand-button:active {
  background: var(--jm-sunken);
}

.brand-mark {
  display: grid;
  width: 30px;
  height: 30px;
  flex: 0 0 auto;
  place-items: center;
  border-radius: 9px;
  background: var(--jm-brand-soft);
}

.brand-mark svg {
  width: 22px;
  height: 22px;
}

.brand-title {
  color: var(--jm-text);
  font-size: 17px;
  font-weight: 800;
  letter-spacing: -0.02em;
}

.brand-title span {
  color: var(--jm-brand);
}

.header-tools {
  display: flex;
  align-items: center;
  gap: 4px;
}

/* 下载中的常驻胶囊：点击直达传输页 */
.download-chip {
  display: inline-flex;
  height: 34px;
  align-items: center;
  gap: 6px;
  padding: 0 10px;
  color: var(--jm-accent);
  border: 0;
  border-radius: var(--jm-r-pill);
  background: var(--jm-accent-soft);
  font-size: var(--jm-fs-sm);
  font-weight: 700;
  cursor: pointer;
  transition: background-color 160ms ease;
}

.download-chip:active {
  background: rgba(0, 174, 236, 0.18);
}

.download-chip-count {
  font-variant-numeric: tabular-nums;
}

.download-chip-speed {
  max-width: 92px;
  overflow: hidden;
  color: var(--jm-accent-deep);
  font-family: ui-monospace, 'SF Mono', Menlo, monospace;
  font-size: 10.5px;
  font-weight: 600;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.shell-alert {
  flex: 0 0 auto;
  border-radius: 0;
}

.app-body {
  position: relative;
  display: flex;
  min-height: 0;
  flex: 1 1 auto;
  flex-direction: column;
  overflow: hidden;
  background: var(--jm-surface);
}

.app-body > * {
  height: 100%;
}

/* 底部导航：58px + 安全区，四等分 */
.app-tabbar {
  display: flex;
  height: calc(var(--jm-nav-h) + var(--safe-bottom));
  flex: 0 0 auto;
  align-items: stretch;
  padding: 0 4px var(--safe-bottom);
  border-top: 1px solid var(--jm-line);
  background: rgba(255, 255, 255, 0.98);
  backdrop-filter: saturate(180%) blur(12px);
}

.tab-item {
  display: flex;
  flex: 1 1 0;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 2px;
  padding: 6px 0 7px;
  color: var(--jm-text-3);
  border: 0;
  background: transparent;
  cursor: pointer;
  transition: color 160ms ease;
  -webkit-tap-highlight-color: transparent;
}

.tab-icon {
  position: relative;
  display: grid;
  width: 52px;
  height: 30px;
  place-items: center;
  border-radius: var(--jm-r-pill);
  transition:
    background-color 180ms ease,
    transform 180ms ease;
}

.tab-item.is-active {
  color: var(--jm-brand);
}

.tab-item.is-active .tab-icon {
  background: var(--jm-brand-soft);
}

.tab-item:active .tab-icon {
  transform: scale(0.92);
}

.tab-label {
  font-size: 10.5px;
  font-weight: 600;
  letter-spacing: 0.01em;
}

.tab-item.is-active .tab-label {
  font-weight: 700;
}

.tab-badge {
  position: absolute;
  top: -3px;
  right: 6px;
  display: flex;
  min-width: 16px;
  height: 16px;
  align-items: center;
  justify-content: center;
  padding: 0 4px;
  color: #fff;
  border: 1.5px solid var(--jm-surface);
  border-radius: var(--jm-r-pill);
  background: var(--jm-accent);
  font-size: 9px;
  font-weight: 800;
  line-height: 1;
  font-variant-numeric: tabular-nums;
}

/* 加载 / 错误 */
.app-loading {
  display: flex;
  width: 100%;
  height: 100%;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 18px;
  padding: 32px;
  color: var(--jm-text);
  background: var(--jm-bg);
}

.loading-logo {
  width: 72px;
  height: 72px;
  border-radius: 20px;
  box-shadow: var(--jm-shadow-2);
  object-fit: contain;
}

.loading-logo.is-pulsing {
  animation: loading-breathe 1.8s ease-in-out infinite;
}

.loading-copy {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 6px;
}

.loading-copy strong {
  font-size: 18px;
  font-weight: 800;
  letter-spacing: -0.02em;
}

.loading-copy strong span {
  color: var(--jm-brand);
}

.loading-copy span {
  max-width: 280px;
  color: var(--jm-text-3);
  font-size: var(--jm-fs-sm);
  text-align: center;
  line-height: 1.6;
  overflow-wrap: anywhere;
}

.loading-bar {
  width: 132px;
  height: 3px;
  overflow: hidden;
  border-radius: var(--jm-r-pill);
  background: var(--jm-line);
}

.loading-bar i {
  display: block;
  width: 42%;
  height: 100%;
  border-radius: inherit;
  background: var(--jm-brand);
  animation: loading-slide 1.1s ease-in-out infinite alternate;
}

@keyframes loading-slide {
  from {
    transform: translateX(0);
  }
  to {
    transform: translateX(138%);
  }
}

@keyframes loading-breathe {
  0%,
  100% {
    transform: scale(1);
    opacity: 1;
  }
  50% {
    transform: scale(0.94);
    opacity: 0.82;
  }
}
</style>
