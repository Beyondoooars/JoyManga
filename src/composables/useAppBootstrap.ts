import { nextTick, onMounted, onUnmounted, ref, watch } from 'vue'
import { useMessage } from 'naive-ui'
import { commands, type LoginRespData } from '../bindings.ts'
import { useStore } from '../store.ts'
import type { CurrentTabName } from '../types.ts'
import { getErrorMessage } from '../utils/errors.ts'
import { runAutoLogin } from '../utils/autoLogin.ts'
import { isAndroid } from './useIsMobile.ts'
import { useAppUpdate } from './useAppUpdate.ts'
import {
  canWritePublicDownloads,
  downloadsPath,
  hasAllFilesAccess,
  requestAllFilesAccessAndWait,
  requiresAllFilesAccess,
  supportsPublicStorage,
  watchNetworkMetered,
  watchNetworkVpn,
} from '../utils/androidStorage.ts'
import { onOpenExternalComic } from '../libraryApi.ts'

export function useAppBootstrap(options: { onNavigateToTab: (tab: CurrentTabName) => void }) {
  const store = useStore()
  const message = useMessage()
  const { checkForUpdates } = useAppUpdate()

  const initializing = ref(false)
  const initializationError = ref('')
  // 首次启动（未完成过存储初始化且未获所有文件权限）时弹存储引导；不阻塞应用使用
  const storageOnboardingShowing = ref(false)
  // 配置加载与监听注册全部完成前不渲染主界面：否则“拿到配置但监听注册失败”时
  // 错误页永远不出现，用户只看到一闪而过的提示、下载事件却无人接收。
  const appReady = ref(false)

  let unmounted = false
  let updateCheckTimer: ReturnType<typeof setTimeout> | undefined
  let stopNetworkWatch: (() => void) | undefined
  let stopVpnWatch: (() => void) | undefined
  let networkStateQueue = Promise.resolve()
  let stopCancelListener: (() => void) | undefined
  let stopOpenFileListener: (() => void) | undefined
  let stopBackgroundFailureListener: (() => void) | undefined
  let stopConfigFlushListener: (() => void) | undefined

  watch(
    () => [store.activeDownloadCount, store.downloadSpeed, store.activeDownloadPages] as const,
    ([active, speed, pages], oldValue) => {
      const prevActive = oldValue?.[0] ?? 0
      const notifyCompleted = prevActive > 0 && active === 0 && store.lastDownloadBatchOutcome === 'completed'
      if (isAndroid) {
        if (window.JmAndroid?.downloadStatusWithCompletion) {
          window.JmAndroid.downloadStatusWithCompletion(active, speed, pages.current, pages.total, notifyCompleted)
        } else {
          window.JmAndroid?.downloadStatus(active, speed, pages.current, pages.total)
        }
      }
    },
    { immediate: true },
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
        const libraryLocationUninitialized = !store.config.androidStorageInitialized
        // 仅当未初始化，或当前下载目录确实需要「所有文件访问」权限却尚未授权时，才请求权限；
        // 用户已改用应用专用目录时不再反复弹出公共存储授权并把目录改回 Download/JoyManga。
        const needsPublicPermission = requiresAllFilesAccess(store.config.downloadDir) && !hasAllFilesAccess()
        if (libraryLocationUninitialized || needsPublicPermission) {
          if (!supportsPublicStorage()) {
            // Android 10 既没有「所有文件访问」也无法申请写外部存储：直接落到应用专用目录
            if (libraryLocationUninitialized) {
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
            if (libraryLocationUninitialized) {
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
            } else if (requiresAllFilesAccess(store.config.downloadDir) && !hasAllFilesAccess()) {
              storageOnboardingShowing.value = true
            }
          }
        }
      }
      await store.initDownloadListener()
      for (const notice of await commands.getStartupNotices()) message.warning(notice, { duration: 10_000 })
      await nextTick()
      if (unmounted) return
      store.enableConfigAutoSave((error) => {
        message.error(`保存配置失败：${getErrorMessage(error)}`)
      })
      appReady.value = true
      if (!stopOpenFileListener) {
        // 文件管理器/其他应用「用 JoyManga 打开」CBZ：等配置与存储目录初始化完再接单，
        // 避免冷启动 pending URI 抢在 downloadDir 就绪前导入到旧目录。
        stopOpenFileListener = onOpenExternalComic({
          onImported: (report) => {
            store.invalidateDownloadedLibrary()
            // 导入是逐文件进行的：损坏/非法 CBZ 会进入 errors，成功数可能为 0。不能一律提示成功。
            if (report.imported > 0) {
              message.success(`已导入 ${report.imported} 本`)
              store.downloadedSubTab = 'library'
              options.onNavigateToTab('downloaded')
            } else if (report.errors.length === 0) {
              message.info(report.skipped > 0 ? `漫画已在书库中（${report.skipped} 本）` : '没有可导入的漫画')
            }
            if (report.errors.length > 0) {
              message.error(`导入失败：${report.errors[0]}`)
            }
          },
          onError: (text) => message.error(`打开文件失败：${text}`),
        })
      }
    } catch (error) {
      if (unmounted) return
      console.error(error)
      initializationError.value = getErrorMessage(error)
      message.error(`应用初始化失败：${initializationError.value}`)
      return
    } finally {
      initializing.value = false
    }

    if (store.config?.autoLogin && store.config.username !== '' && store.config.password !== '') {
      const username = store.config.username
      const password = store.config.password
      const startSessionEpoch = store.sessionEpoch
      const isOwnSettledSession = (settledData?: LoginRespData) => settledData !== undefined
        && store.sessionEpoch === settledData.sessionEpoch
        && store.userProfile?.uid === settledData.profile.uid
        && !store.manualLoginInProgress
        && store.config?.username === username
        && store.config?.password === password
      const outcome = await runAutoLogin({
        username,
        password,
        login: (name, secret) => commands.login(name, secret),
        // 用户已手动登录、正在手动登录，或手动改了表单：停止自动重试，
        // 避免自动登录的新请求把正在进行的会话顶掉；若是本次自动登录的 SessionChangedEvent 先到达，则放行完成。
        shouldStop: (settledData) => unmounted
          || (!isOwnSettledSession(settledData) && (store.sessionEpoch !== startSessionEpoch || store.userProfile !== undefined))
          || store.manualLoginInProgress
          || store.config?.username !== username
          || store.config?.password !== password,
        onRetry: (attempt, error) => console.warn(`自动登录第 ${attempt} 次失败，准备重试`, error),
      })
      if (unmounted) return
      if (outcome.status === 'success') {
        if (store.sessionEpoch !== startSessionEpoch && !isOwnSettledSession(outcome.data)) return
        store.applyLoginResult(outcome.data)
        message.success(outcome.attempts > 1 ? `自动登录成功（第 ${outcome.attempts} 次尝试）` : '自动登录成功')
      } else if (outcome.status === 'failed') {
        if (store.sessionEpoch !== startSessionEpoch) return
        const errorMessage = typeof outcome.error === 'string' ? outcome.error : outcome.error.err_message
        console.error(outcome.error)
        message.warning(`自动登录失败：${errorMessage}`)
      }
    }
  }

  onMounted(() => {
    unmounted = false
    document.addEventListener('contextmenu', preventContextMenu)
    const flushOnBackground = () => store.flushPendingConfigSave()
    const onVisibilityChange = () => {
      if (document.visibilityState === 'hidden') flushOnBackground()
    }
    document.addEventListener('visibilitychange', onVisibilityChange)
    window.addEventListener('pagehide', flushOnBackground)
    stopConfigFlushListener = () => {
      document.removeEventListener('visibilitychange', onVisibilityChange)
      window.removeEventListener('pagehide', flushOnBackground)
    }
    const enqueueNetworkStateUpdate = (task: () => Promise<unknown>) => {
      // IPC 命令可能并发执行；按原生事件顺序串行提交，避免计费/VPN 回调竞态相互覆盖。
      networkStateQueue = networkStateQueue
        .then(task)
        .then(() => undefined, () => { /* 旧后端无此命令时忽略，且不能阻断后续上报 */ })
    }
    // 把 Android 的网络计费状态同步给后端，「仅 Wi-Fi 下载」依赖它做判断
    stopNetworkWatch = watchNetworkMetered((metered) => {
      enqueueNetworkStateUpdate(() => commands.setNetworkMetered(metered))
    })
    // VPN 开启时后端跳过手动代理，否则「开 VPN 反而连不上」
    stopVpnWatch = watchNetworkVpn((active, networkKey) => {
      enqueueNetworkStateUpdate(() => commands.setNetworkVpn(active, networkKey))
    })
    // 通知栏"取消当前"：由原生转发事件，这里执行取消并给出反馈
    const onCancelCurrent = async () => {
      try {
        const cancelled = await store.cancelCurrentDownload()
        if (cancelled) message.info(`已取消：${cancelled}`)
      } catch (error) {
        console.error('取消当前下载失败', error)
        message.error(`取消下载失败：${getErrorMessage(error)}`)
      }
    }
    window.addEventListener('jm-download-cancel', onCancelCurrent)
    stopCancelListener = () => window.removeEventListener('jm-download-cancel', onCancelCurrent)
    stopBackgroundFailureListener = store.initBackgroundFailureListener((title, description) => {
      message.error(`${title}：${description}`, { duration: 8000 })
    })
    void store.initSpeedListener().catch((error) => console.error('初始化下载速率监听失败', error))
    void initializeApp()
    // 延迟 2.5 秒静默检测更新，避开冷启动峰值
    updateCheckTimer = setTimeout(() => {
      updateCheckTimer = undefined
      if (!unmounted) void checkForUpdates(true)
    }, 2500)
  })

  onUnmounted(() => {
    unmounted = true
    store.disableConfigAutoSave({ flush: true })
    document.removeEventListener('contextmenu', preventContextMenu)
    stopConfigFlushListener?.()
    stopConfigFlushListener = undefined
    stopNetworkWatch?.()
    stopNetworkWatch = undefined
    stopVpnWatch?.()
    stopVpnWatch = undefined
    stopCancelListener?.()
    stopCancelListener = undefined
    stopOpenFileListener?.()
    stopOpenFileListener = undefined
    stopBackgroundFailureListener?.()
    stopBackgroundFailureListener = undefined
    if (updateCheckTimer !== undefined) {
      clearTimeout(updateCheckTimer)
      updateCheckTimer = undefined
    }
  })

  return {
    initializing,
    initializationError,
    storageOnboardingShowing,
    appReady,
    initializeApp,
  }
}
