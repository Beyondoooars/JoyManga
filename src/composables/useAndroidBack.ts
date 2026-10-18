import { onActivated, onDeactivated, onMounted, onUnmounted } from 'vue'

export type BackHandler = () => boolean

interface RegisteredHandler {
  id: number
  priority: number
  handler: BackHandler
}

const handlers: RegisteredHandler[] = []
let nextId = 1
let isListenerAttached = false
let lastBackPressTime = 0

function handleBackAction(): boolean {
  // Sort descending by priority, then descending by id (latest registered first)
  const sorted = [...handlers].sort((a, b) => b.priority - a.priority || b.id - a.id)
  for (const item of sorted) {
    try {
      const handled = item.handler()
      if (handled) {
        return true
      }
    } catch (e) {
      console.error('Error in back handler:', e)
    }
  }
  return false
}

function onKeydown(event: KeyboardEvent) {
  if (event.key === 'Escape' || event.key === 'GoBack') {
    const handled = handleBackAction()
    if (handled) {
      event.preventDefault()
      event.stopPropagation()
    }
  }
}

function onPopState(event: PopEvent) {
  const handled = handleBackAction()
  if (handled) {
    event.preventDefault?.()
    window.history.pushState(null, '', window.location.href)
  }
}

type PopEvent = Event & { preventDefault?: () => void }

function initGlobalBackListeners() {
  if (isListenerAttached) return
  isListenerAttached = true

  window.addEventListener('keydown', onKeydown, { capture: true })
  window.addEventListener('popstate', onPopState as EventListener)
  window.addEventListener('jm-android-back', () => { handleBackAction() })

  // Seed history state so Android popstate can fire
  if (/Android/i.test(navigator.userAgent) && !window.JmAndroid) window.history.pushState(null, '', window.location.href)
}

export function registerBackHandler(handler: BackHandler, priority = 10): () => void {
  initGlobalBackListeners()
  const id = nextId++
  handlers.push({ id, priority, handler })

  return () => {
    const index = handlers.findIndex((h) => h.id === id)
    if (index !== -1) {
      handlers.splice(index, 1)
    }
  }
}

/**
 * 注册处理返回键 / Esc 的回调。
 *
 * 同时覆盖 KeepAlive 场景：组件被缓存后 `onUnmounted` 不会触发，如果只在 mounted 时注册，
 * 失活的页面会永久"吞掉"返回键（例如 Android 返回键、桌面 Esc）。因此在
 * activated/deactivated 里成对注册与注销，失活页面不再参与分发。
 */
export function useBackHandler(handler: BackHandler, priority = 10) {
  let unregister: (() => void) | undefined

  const attach = () => {
    unregister?.()
    unregister = registerBackHandler(handler, priority)
  }
  const detach = () => {
    unregister?.()
    unregister = undefined
  }

  onMounted(attach)
  onActivated(attach)
  onDeactivated(detach)
  onUnmounted(detach)
}

export const useAndroidBack = useBackHandler

export function handleRootBackExit(showExitToast: (msg: string) => void): boolean {
  if (!/Android/i.test(navigator.userAgent)) return false
  const now = Date.now()
  if (now - lastBackPressTime < 2000) {
    lastBackPressTime = 0
    if (window.JmAndroid) { window.JmAndroid.exitApp(); return true }
    return false
  }
  lastBackPressTime = now
  showExitToast('再按一次返回桌面')
  return true
}
