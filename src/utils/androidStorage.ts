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
      setVolumeKeyPaging(enabled: boolean): void
      downloadsPath(): string
      hasAllFilesAccess(): boolean
      supportsPublicStorage(): boolean
      canWritePublicDownloads(): boolean
      requestAllFilesAccess(): void
      downloadAndInstallApk(url: string, fileName: string): void
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

export interface UpdateProgress { state: 'progress' | 'done' | 'installer' | 'permission' | 'error'; percent?: number; message?: string }

/**
 * 应用内更新：原生下载 APK 后直接拉起系统安装器。
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

/** 音量键翻页事件（Android 原生转发），返回取消订阅函数 */
export function onVolumeKeyPage(handler: (action: 'next' | 'prev') => void): () => void {
  const listener = (event: Event) => {
    const detail = (event as CustomEvent<'next' | 'prev'>).detail
    if (detail === 'next' || detail === 'prev') handler(detail)
  }
  window.addEventListener('jm-volume-page', listener)
  return () => window.removeEventListener('jm-volume-page', listener)
}

