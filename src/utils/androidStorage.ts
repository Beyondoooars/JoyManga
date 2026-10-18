import { isAndroid } from '../composables/useIsMobile.ts'
import { commands } from '../bindings.ts'

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
      hasAllFilesAccess(): boolean
      requestAllFilesAccess(): void
      setKeepScreenOn(enabled: boolean): void
      setVolumeKeyPaging(enabled: boolean): void
    }
  }
}

function request(action: (id: string) => void): Promise<MobileResult> {
  if (!isAndroid || !window.JmAndroid) return Promise.reject(new Error('Android 文件服务尚未就绪，请重新打开应用'))
  const requestId = crypto.randomUUID()
  return new Promise((resolve, reject) => {
    const listener = (event: Event) => {
      const result = (event as CustomEvent<MobileResult>).detail
      if (result.requestId !== requestId) return
      window.removeEventListener('jm-mobile-result', listener)
      if (result.error) reject(new Error(result.error))
      else resolve(result)
    }
    window.addEventListener('jm-mobile-result', listener)
    try { action(requestId) } catch (error) { window.removeEventListener('jm-mobile-result', listener); reject(error) }
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

/**
 * 阅读时的常亮与音量键。
 * Android 的常亮必须走原生 window flag（前端 CSS/JS 无法阻止系统息屏），
 * 桌面端则走 Rust 命令；这里统一成一个入口，调用方不必分支。
 */
export function setKeepScreenOn(enabled: boolean) {
  if (isAndroid) {
    window.JmAndroid?.setKeepScreenOn(enabled)
    return
  }
  void commands.setKeepScreenOn(enabled).catch(() => { /* 常亮失败不影响阅读 */ })
}

export function setVolumeKeyPaging(enabled: boolean) {
  if (isAndroid) window.JmAndroid?.setVolumeKeyPaging(enabled)
}

/**
 * Android 原生侧会把当前网络是否计费通过事件推上来（启动与切换网络时各一次）。
 * 桌面端没有计费概念，直接视为不计费。返回值用于取消订阅。
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

/** 音量键翻页事件（Android 原生转发），返回取消订阅函数 */
export function onVolumeKeyPage(handler: (action: 'next' | 'prev') => void): () => void {
  const listener = (event: Event) => {
    const detail = (event as CustomEvent<'next' | 'prev'>).detail
    if (detail === 'next' || detail === 'prev') handler(detail)
  }
  window.addEventListener('jm-volume-page', listener)
  return () => window.removeEventListener('jm-volume-page', listener)
}

