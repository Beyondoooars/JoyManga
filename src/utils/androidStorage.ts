import { isAndroid } from '../composables/useIsMobile.ts'

export type PickMode = 'cbz' | 'backup' | 'directory'
export interface MobileResult { requestId: string; paths?: string[]; cancelled?: boolean; error?: string; exported?: number }

declare global {
  interface Window {
    JmAndroid?: {
      libraryPath(): string
      pickDocuments(requestId: string, mode: string): void
      discardImports(requestId: string): void
      exportDocuments(requestId: string, path: string): void
      downloadStatus(active: number, speed: string, current: number, total: number): void
      exitApp(): void
      setKeepScreenOn(enabled: boolean): void
      setImmersiveMode(enabled: boolean): void
      setVolumeKeyPaging(enabled: boolean): void
      downloadsPath(): string
      hasAllFilesAccess(): boolean
      supportsPublicStorage(): boolean
      canWritePublicDownloads(): boolean
      requestAllFilesAccess(): void
      downloadAndInstallApk(url: string, fileName: string): void
      updateDownloadState(): void
      notifyOpenFileReady(): void
    }
  }
}

function request(action: (id: string) => void): Promise<MobileResult> {
  if (!isAndroid || !window.JmAndroid) return Promise.reject(new Error('Android 文件服务尚未就绪，请重新打开应用'))
  const requestId = crypto.randomUUID()
  return new Promise((resolve, reject) => {
    // 原生侧若因 Activity 被系统回收而丢失了 pendingPicker，就不会再回传结果。
    // 没有超时的话调用方（导入/备份/导出）会永久挂起，界面停在 loading。
    const timeout = setTimeout(() => {
      window.removeEventListener('jm-mobile-result', listener)
      reject(new Error('等待系统文件选择器超时，请重试'))
    }, 5 * 60 * 1000)
    const listener = (event: Event) => {
      const result = (event as CustomEvent<MobileResult>).detail
      if (result.requestId !== requestId) return
      clearTimeout(timeout)
      window.removeEventListener('jm-mobile-result', listener)
      if (result.error) reject(new Error(result.error))
      else resolve(result)
    }
    window.addEventListener('jm-mobile-result', listener)
    try { action(requestId) } catch (error) { clearTimeout(timeout); window.removeEventListener('jm-mobile-result', listener); reject(error) }
  })
}

export async function withAndroidSelection<T>(mode: PickMode, action: (paths: string[]) => Promise<T>): Promise<T | undefined> {
  const result = await request((id) => window.JmAndroid!.pickDocuments(id, mode))
  try {
    if (result.cancelled || !result.paths?.length) return
    return await action(result.paths)
  } finally { window.JmAndroid?.discardImports(result.requestId) }
}

export function exportToAndroidDownloads(path: string) {
  return request((id) => window.JmAndroid!.exportDocuments(id, path))
}

/** 公共下载目录下的默认书库（Download/JoyManga）绝对路径；桥不可用时返回空串。 */
export function downloadsPath() {
  return window.JmAndroid?.downloadsPath() ?? ''
}

/**
 * 文件是否已经位于公共下载目录内。
 *
 * 书库就在公共下载时，备份/诊断日志本来就已经是用户可见文件；再走一次
 * MediaStore 发布只会生成 `xxx (1).json` 这样的副本。
 */
export function isUnderPublicDownloads(path: string): boolean {
  const root = downloadsPath()
  return root !== '' && (path === root || path.startsWith(`${root}/`))
}

/** 是否已持有「所有文件访问」权限；桥不可用或调用异常时视为未授权。 */
export function hasAllFilesAccess() {
  try {
    return window.JmAndroid?.hasAllFilesAccess() ?? false
  } catch (error) {
    console.error('读取公共存储权限失败', error)
    return false
  }
}

/** 当前系统是否支持公共下载书库（Android 10 上不可用，应直接使用应用专用目录）。 */
export function supportsPublicStorage() {
  try {
    return window.JmAndroid?.supportsPublicStorage() ?? false
  } catch (error) {
    console.error('读取公共存储支持状态失败', error)
    return false
  }
}

/** 公共下载目录当前是否可写；写探测失败时前端退回应用专用目录。 */
export function canWritePublicDownloads() {
  try {
    return window.JmAndroid?.canWritePublicDownloads() ?? false
  } catch (error) {
    console.error('检测公共下载目录可写性失败', error)
    return false
  }
}

/** 跳转到系统设置申请「所有文件访问」权限（结果由用户在系统页决定）。 */
export function requestAllFilesAccess() {
  window.JmAndroid?.requestAllFilesAccess()
}

/**
 * 申请「所有文件访问」并等待用户从系统页返回。
 *
 * Android 11+ 跳系统设置页，返回应用时 WebView 重新可见；API 29 以下弹运行时
 * 权限对话框，结果经 `jm-permission-result` 回传。两条路径以先到者为准，
 * 用户拒绝也立即结算——否则 API 29 以下没有可见性变化事件，启动会一直卡在加载页。
 */
export function requestAllFilesAccessAndWait(): Promise<boolean> {
  if (hasAllFilesAccess()) return Promise.resolve(true)
  return new Promise((resolve) => {
    let settled = false
    let retryTimer: ReturnType<typeof setTimeout> | undefined
    const finish = (granted: boolean) => {
      if (settled) return
      settled = true
      clearTimeout(retryTimer)
      retryTimer = undefined
      document.removeEventListener('visibilitychange', onVisibility)
      removePermissionListener()
      resolve(granted)
    }
    const onVisibility = () => {
      if (document.visibilityState !== 'visible') return
      // 从设置页返回时系统的权限状态可能稍后才刷新，延后一拍再读。
      // 只保留最后一次待执行的检查：句柄被覆盖会让兜底定时器再也清不掉。
      clearTimeout(retryTimer)
      retryTimer = setTimeout(() => finish(hasAllFilesAccess()), 300)
    }
    const removePermissionListener = onPermissionResult((granted) => {
      // 拒绝也要结算：API 29 以下没有可见性变化事件，只能靠这次回调
      finish(granted)
    })
    document.addEventListener('visibilitychange', onVisibility)
    requestAllFilesAccess()
    // 兜底：系统页打不开时应用始终可见，按当前状态结算，不能让启动停在加载页。
    // 用户还停留在系统页（应用不可见）则继续等，返回后由可见性回调给出结果。
    const fallback = () => {
      if (settled) return
      if (document.visibilityState !== 'visible') {
        retryTimer = setTimeout(fallback, 90_000)
        return
      }
      finish(hasAllFilesAccess())
    }
    retryTimer = setTimeout(fallback, 90_000)
  })
}

/** 监听运行时权限结果（API 29 以下的存储授权没有可见性变化事件）。返回取消监听的函数。 */
export function onPermissionResult(listener: (granted: boolean) => void): () => void {
  const handler = (event: Event) => {
    const detail = (event as CustomEvent<{ request?: string; granted?: boolean }>).detail
    if (detail?.request === 'storage') listener(detail.granted === true)
  }
  window.addEventListener('jm-permission-result', handler)
  return () => window.removeEventListener('jm-permission-result', handler)
}

/**
 * 阅读时的常亮与音量键，全部由 Android 原生层处理。
 */
export function setKeepScreenOn(enabled: boolean) {
  window.JmAndroid?.setKeepScreenOn(enabled)
}

/**
 * 沉浸式全屏：隐藏状态栏与导航栏。
 *
 * 仅在原生桥可用时生效；浏览器/测试环境下由阅读器自身的全屏样式兜底。
 */
export function setImmersiveMode(enabled: boolean) {
  try {
    window.JmAndroid?.setImmersiveMode(enabled)
  } catch (error) {
    console.error('切换沉浸式全屏失败', error)
  }
}

export function setVolumeKeyPaging(enabled: boolean) {
  if (isAndroid) window.JmAndroid?.setVolumeKeyPaging(enabled)
}

/**
 * Android 原生侧会把当前网络是否计费通过事件推上来（启动与切换网络时各一次）。
 * 非 Android 环境（开发/测试）直接视为不计费。返回值用于取消订阅。
 */
export function watchNetworkMetered(handler: (metered: boolean) => void): () => void {
  if (!isAndroid) {
    handler(false)
    return () => {}
  }
  const listener = (event: Event) => handler(Boolean((event as CustomEvent<boolean>).detail))
  window.addEventListener('jm-network-metered', listener)
  return () => window.removeEventListener('jm-network-metered', listener)
}

/**
 * 监听系统 VPN 状态（启动与切换网络时各一次）。
 *
 * 后端在 VPN 开启时跳过手动 HTTP 代理：VPN 已在系统层接管流量，再连本机代理端口
 * 往往连不上，表现就是「开了 VPN 反而上不了网」。非 Android 环境恒为 false。
 * 返回值用于取消订阅。
 */
export function watchNetworkVpn(handler: (active: boolean) => void): () => void {
  if (!isAndroid) {
    handler(false)
    return () => {}
  }
  const listener = (event: Event) => handler(Boolean((event as CustomEvent<boolean>).detail))
  window.addEventListener('jm-network-vpn', listener)
  return () => window.removeEventListener('jm-network-vpn', listener)
}

/**
 * 「用 JoyManga 打开 CBZ」：原生把外部文件复制到暂存区后推送此事件。
 * 返回取消订阅函数；回调负责导入并在结束后调用 `discardImports(id)`。
 */
export function onOpenFile(handler: (detail: { id?: string; path: string }) => void): () => void {
  const listener = (event: Event) => {
    const detail = (event as CustomEvent<{ id?: string; path?: string }>).detail
    if (detail?.path) handler({ id: detail.id, path: detail.path })
  }
  window.addEventListener('jm-open-file', listener)
  // WebView 创建早于页面加载，原生在监听器注册前派发的事件会丢失。
  // 注册完成后主动通知原生，把冷启动/早期到达的「用 JoyManga 打开」补发过来。
  try { window.JmAndroid?.notifyOpenFileReady() } catch (error) { console.error('通知原生打开文件监听器就绪失败', error) }
  return () => window.removeEventListener('jm-open-file', listener)
}

export interface UpdateProgress { state: 'progress' | 'done' | 'installer' | 'permission' | 'error' | 'idle'; percent?: number; message?: string }

/**
 * 应用内更新：原生在前台服务里下载 APK，完成后直接拉起系统安装器。
 * 进度与结果经 `jm-update-progress` 回传，返回取消订阅函数。
 */
export function downloadAndInstallApk(url: string, fileName: string, handler: (progress: UpdateProgress) => void): () => void {
  if (!isAndroid || !window.JmAndroid) {
    handler({ state: 'error', message: '当前环境不支持应用内安装' })
    return () => {}
  }
  const listener = (event: Event) => {
    const detail = (event as CustomEvent<UpdateProgress>).detail
    if (detail?.state) handler(detail)
  }
  window.addEventListener('jm-update-progress', listener)
  try {
    window.JmAndroid.downloadAndInstallApk(url, fileName)
  } catch (error) {
    window.removeEventListener('jm-update-progress', listener)
    handler({ state: 'error', message: error instanceof Error ? error.message : String(error) })
  }
  return () => window.removeEventListener('jm-update-progress', listener)
}

/**
 * 主动拉取一次更新下载的实时状态。
 *
 * 下载在原生前台服务里进行，应用被切到后台再回来时可能错过进度事件；
 * 回到前台后调用一次，界面就能接着显示「下载中 x%」而不是回到初始状态。
 */
export function syncUpdateDownloadState() {
  if (!isAndroid || !window.JmAndroid) return
  try {
    window.JmAndroid.updateDownloadState()
  } catch (error) {
    console.error('读取更新下载状态失败', error)
  }
}

/** 音量键翻页事件（Android 原生转发），返回取消订阅函数 */
export function onVolumeKeyPage(handler: (action: 'next' | 'prev') => void): () => void {
  const listener = (event: Event) => {
    const detail = (event as CustomEvent<'next' | 'prev'>).detail
    if (detail === 'next' || detail === 'prev') handler(detail)
  }
  window.addEventListener('jm-volume-page', listener)
  return () => window.removeEventListener('jm-volume-page', listener)
}

