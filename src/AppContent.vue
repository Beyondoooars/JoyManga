<script setup lang="tsx">
import { computed, defineAsyncComponent, nextTick, onMounted, onUnmounted, ref, toRaw, watch } from 'vue'
import type { Component } from 'vue'
import { commands } from './bindings.ts'
import { useMessage, useNotification } from 'naive-ui'
import {
  PhBooks,
  PhGearSix,
  PhMagnifyingGlass,
  PhUser,
} from '@phosphor-icons/vue'
import { useStore } from './store.ts'
import icon from './assets/icon.png'
import type { CurrentTabName } from './types.ts'
import { getErrorMessage } from './utils/errors.ts'
import { handleRootBackExit, useBackHandler } from './composables/useAndroidBack.ts'
import { useIsMobile } from './composables/useIsMobile.ts'
import { useAppUpdate } from './composables/useAppUpdate.ts'
import { canWritePublicDownloads, downloadsPath, hasAllFilesAccess, requestAllFilesAccessAndWait, supportsPublicStorage, watchNetworkMetered } from './utils/androidStorage.ts'
import { onOpenExternalComic } from './libraryApi.ts'

const UserCenterDialog = defineAsyncComponent(() => import('./dialogs/UserCenterDialog.vue'))
const SettingsDialog = defineAsyncComponent(() => import('./dialogs/SettingsDialog.vue'))
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
}

const store = useStore()
const message = useMessage()
const notification = useNotification()
const { isAndroid } = useIsMobile()
const { checkForUpdates } = useAppUpdate()

const userCenterShowing = computed({ get: () => store.userCenterShowing, set: (value: boolean) => { store.userCenterShowing = value } })
const settingsDialogShowing = ref<boolean>(false)

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

function navigateToTab(tab: CurrentTabName) {
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
  if (userCenterShowing.value) {
    userCenterShowing.value = false
    return true
  }
  if (settingsDialogShowing.value) {
    settingsDialogShowing.value = false
    return true
  }
  if (store.chapterDetailShowing) {
    store.closeComicDetail()
    return true
  }
  if (store.currentTabName === 'downloaded' && store.downloadedSubTab === 'transfers') {
    store.downloadedSubTab = 'library'
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
    if (isAndroid && store.config && !store.config.androidStorageInitialized && window.JmAndroid) {
      const bridge = window.JmAndroid
      if (!supportsPublicStorage()) {
        // Android 10 既没有「所有文件访问」也无法申请写外部存储：直接落到应用专用目录
        const result = await commands.initializeAndroidStorage(bridge.libraryPath() ?? '')
        if (result.status === 'error') throw new Error(result.error.err_message)
        store.applyConfig(result.data)
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
        } else {
          // 用户拒绝授权或公共目录暂不可写：兜底引导，可改用应用专用目录
          storageOnboardingShowing.value = true
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
let stopCancelListener: (() => void) | undefined
let stopOpenFileListener: (() => void) | undefined

onMounted(() => {
  document.addEventListener('contextmenu', preventContextMenu)
  // 把 Android 的网络计费状态同步给后端，「仅 Wi-Fi 下载」依赖它做判断
  stopNetworkWatch = watchNetworkMetered((metered) => {
    void commands.setNetworkMetered(metered).catch(() => { /* 旧后端无此命令时忽略 */ })
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
  <div v-if="appReady && store.config !== undefined" class="bili-app is-mobile-app">
    <!-- 顶部栏：移动端只保留品牌与设置，导航在底部 -->
    <header class="bili-header" aria-label="应用主导航">
      <div class="header-left">
        <!-- Brand Logo -->
        <button class="brand-button" type="button" title="回到发现" @click="navigateToTab('search')">
          <svg class="brand-mark" viewBox="0 0 48 48" aria-hidden="true" focusable="false">
            <rect x="24.2" y="5.4" width="11.6" height="11.6" rx="2.4" transform="rotate(45 30 11.2)" fill="#00AEEC" />
            <path d="M30 21.5 V30 A9 9 0 0 1 13 30" fill="none" stroke="#FB7299" stroke-width="10" stroke-linecap="round" />
          </svg>
          <span class="brand-title">JoyManga</span>
        </button>
      </div>

      <!-- Header Right Controls -->
      <div class="header-right">
        <!-- Settings Action -->
        <div class="header-actions">
          <button type="button" class="header-icon-btn" title="应用设置" @click="settingsDialogShowing = true">
            <PhGearSix :size="18" />
            <span class="sr-only">应用设置</span>
          </button>
        </div>
      </div>
    </header>

    <!-- Main Workspace Surface (Full Width) -->
    <n-alert v-if="store.config?.apiDomainMigrationPending" type="warning" closable @close="store.config.apiDomainMigrationPending = false">
      已保留升级前的线路设置，可在设置 → 网络中核对。
    </n-alert>
    <main class="main-viewport">
      <section class="content-surface">
        <KeepAlive>
          <ChapterPane
            v-if="store.chapterDetailShowing"
            key="chapter-detail"
            @back="store.closeComicDetail" />
          <component
            v-else
            :is="currentPaneComponent"
            :key="store.currentTabName"
            @open-login="userCenterShowing = true" />
        </KeepAlive>
      </section>
    </main>

    <!-- Mobile Bottom Navigation Bar (3 Tabs: 发现, 书库(含传输), 我的) -->
    <nav class="mobile-nav" aria-label="移动端底部导航">
      <!-- 发现 Tab -->
      <button
        type="button"
        :class="['mobile-nav-btn', { active: store.currentTabName === 'search' && !store.chapterDetailShowing }]"
        @click="navigateToTab('search')">
        <PhMagnifyingGlass :size="20" weight="duotone" />
        <span>发现</span>
      </button>

      <!-- 书库 Tab (包含已入库与传输) -->
      <button
        type="button"
        :class="['mobile-nav-btn', { active: store.currentTabName === 'downloaded' && !store.chapterDetailShowing }]"
        @click="navigateToTab('downloaded')">
        <div class="relative flex items-center justify-center">
          <PhBooks :size="20" weight="duotone" />
          <span v-if="store.activeDownloadCount > 0" class="mobile-badge">{{ store.activeDownloadCount }}</span>
        </div>
        <span>书库</span>
      </button>

      <!-- 我的 Tab -->
      <button
        type="button"
        :class="['mobile-nav-btn', { active: userCenterShowing }]"
        @click="userCenterShowing = true">
        <PhUser :size="20" weight="duotone" />
        <span>我的</span>
      </button>
    </nav>

    <!-- Modals and Dialogs -->
    <UserCenterDialog v-if="userCenterShowing" v-model:showing="userCenterShowing" />
    <SettingsDialog v-if="settingsDialogShowing" v-model:showing="settingsDialogShowing" />
    <StorageOnboardingDialog v-if="storageOnboardingShowing" v-model:show="storageOnboardingShowing" />
    <ReaderDialog v-if="store.activeReaderTarget" />
    <UpdateDialog />
  </div>

  <div v-else-if="initializationError" class="app-loading" role="alert">
    <img :src="icon" alt="JoyManga" class="w-12 h-12 rounded-xl object-contain mb-2 shadow-lg" />
    <div class="loading-copy">
      <strong class="text-[#18191c] text-base">应用初始化失败</strong>
      <span class="text-[#61666d] text-xs">{{ initializationError }}</span>
    </div>
    <n-button type="primary" :loading="initializing" @click="initializeApp">重试</n-button>
  </div>

  <div v-else class="app-loading" aria-live="polite">
    <img :src="icon" alt="JoyManga" class="w-12 h-12 rounded-xl object-contain mb-2 animate-pulse shadow-lg" />
    <div class="loading-copy">
      <strong class="text-[#18191c] text-base font-bold tracking-tight">Joy<span class="text-[#fb7299]">Manga</span></strong>
      <span class="text-[#61666d] text-xs">正在加载精彩漫库…</span>
    </div>
    <div class="loading-bar"><i></i></div>
  </div>
</template>

<style scoped>
.bili-app {
  display: flex;
  width: 100%;
  height: 100vh;
  flex-direction: column;
  overflow: hidden;
  background: #f6f7f8;
  color: #18191c;
}

/* Bilibili Header */
.bili-header {
  position: relative;
  z-index: 20;
  display: flex;
  height: 60px;
  flex: 0 0 auto;
  align-items: center;
  justify-content: space-between;
  padding: 0 22px;
  background: rgba(255, 255, 255, 0.94);
  border-bottom: 1px solid #e3e5e7;
  box-shadow: 0 2px 8px rgba(0, 0, 0, 0.04);
}

.header-left {
  display: flex;
  align-items: center;
  gap: 20px;
  min-width: 0;
}

.brand-button {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 4px 6px;
  cursor: pointer;
  border: 0;
  border-radius: 10px;
  background: transparent;
  transition: all 160ms ease;
}

.brand-button:hover {
  background: #f1f2f3;
}

.brand-button:active {
  transform: scale(0.95);
}

.brand-mark {
  width: 28px;
  height: 28px;
  flex: 0 0 auto;
}

.brand-title {
  font-size: 15px;
  font-weight: 800;
  color: #18191c;
  letter-spacing: -0.02em;
}

.brand-button:hover .brand-title {
  color: #fb7299;
}

.header-right {
  display: flex;
  align-items: center;
  gap: 12px;
  flex-shrink: 0;
}

.header-actions {
  display: flex;
  align-items: center;
  gap: 6px;
}


.header-icon-btn {
  display: flex;
  width: 32px;
  height: 32px;
  align-items: center;
  justify-content: center;
  color: #61666d;
  cursor: pointer;
  border: 1px solid transparent;
  border-radius: 8px;
  background: transparent;
  transition: all 160ms ease;
}

.header-icon-btn:hover {
  color: #fb7299;
  background: #f1f2f3;
  border-color: #e3e5e7;
}

/* 触屏：顶栏图标按钮补足 44dp 可点区域（视觉尺寸仍 32px） */
@media (max-width: 768px) {
  .header-icon-btn {
    width: 44px;
    height: 44px;
  }

  .brand-button {
    min-height: 44px;
  }

  .brand-mark {
    width: 30px;
    height: 30px;
  }
}

.header-icon-btn:active {
  transform: scale(0.94);
}

/* Main Viewport & Content Surface */
.main-viewport {
  display: flex;
  min-height: 0;
  flex: 1;
  flex-direction: column;
  overflow: hidden;
  padding: 10px 14px 12px;
  background: #f6f7f8;
}

.content-surface {
  display: flex;
  min-height: 0;
  flex: 1;
  flex-direction: column;
  overflow: hidden;
  border: 1px solid #e3e5e7;
  border-radius: var(--radius-panel);
  background: #ffffff;
  box-shadow: var(--shadow-surface);
}

.content-surface > * {
  height: 100%;
}

/* Mobile Bottom Navigation */
.mobile-nav {
  display: none;
  position: fixed;
  bottom: 0;
  left: 0;
  right: 0;
  z-index: 30;
  height: calc(56px + env(safe-area-inset-bottom, 0px));
  align-items: center;
  justify-content: space-around;
  padding: 0 8px env(safe-area-inset-bottom, 0px);
  background: #ffffff;
  border-top: 1px solid #e3e5e7;
  box-shadow: 0 -2px 10px rgba(0, 0, 0, 0.04);
}

.mobile-nav-btn {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 2px;
  padding: 6px 12px;
  color: #61666d;
  font-size: 10px;
  font-weight: 600;
  cursor: pointer;
  border: 0;
  background: transparent;
  transition: color 150ms ease;
}

.mobile-nav-btn.active {
  color: #fb7299;
  font-weight: 700;
}

.mobile-badge {
  position: absolute;
  top: -4px;
  right: -8px;
  display: flex;
  align-items: center;
  justify-content: center;
  min-width: 14px;
  height: 14px;
  padding: 0 3px;
  color: #fff;
  border-radius: 999px;
  background: #fb7299;
  font-size: 9px;
  font-weight: 800;
}

/* Loading States */
.app-loading {
  display: flex;
  width: 100%;
  height: 100%;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 16px;
  color: #18191c;
  background: #f6f7f8;
}

.loading-copy {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 4px;
}

.loading-bar {
  width: 150px;
  height: 3px;
  overflow: hidden;
  border-radius: 999px;
  background: #e3e5e7;
}

.loading-bar i {
  display: block;
  width: 45%;
  height: 100%;
  border-radius: inherit;
  background: #fb7299;
  animation: loading-slide 1.1s ease-in-out infinite alternate;
}

.sr-only {
  position: absolute;
  width: 1px;
  height: 1px;
  padding: 0;
  margin: -1px;
  overflow: hidden;
  clip: rect(0, 0, 0, 0);
  white-space: nowrap;
  border: 0;
}

@keyframes loading-slide {
  from {
    transform: translateX(0);
  }
  to {
    transform: translateX(122%);
  }
}

/* Android Mobile Specific Layout */
.bili-app.is-mobile-app .mobile-nav {
  display: flex;
}

.bili-app.is-mobile-app .main-viewport {
  padding: 8px 8px calc(64px + env(safe-area-inset-bottom, 0px)) 8px;
}

.bili-app.is-mobile-app .bili-header {
  padding: env(safe-area-inset-top, 0px) 12px 0 12px;
  height: calc(52px + env(safe-area-inset-top, 0px));
}

:global(.bili-drawer .n-drawer-header) {
  border-bottom: 1px solid #e3e5e7;
  background: #ffffff;
}

:global(.bili-drawer .n-drawer-body-content-wrapper) {
  background: #ffffff;
}
</style>
