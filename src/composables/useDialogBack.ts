import { getCurrentInstance, getCurrentScope, onActivated, onDeactivated, onScopeDispose, onUnmounted } from 'vue'
import type { DialogApi, DialogOptions, DialogReactive } from 'naive-ui'
import { registerBackHandler } from './useAndroidBack.ts'

type DialogCallback = (event: MouseEvent, dialog: DialogReactive) => boolean | void | Promise<boolean | void>
type CancelCallback = (event: MouseEvent) => boolean | void

export type BackHandledDialogOptions = Omit<
  DialogOptions,
  'onPositiveClick' | 'onNegativeClick' | 'onClose' | 'onMaskClick' | 'onEsc' | 'onAfterLeave'
> & {
  onPositiveClick?: DialogCallback
  onNegativeClick?: CancelCallback
  onClose?: () => boolean | void
  onMaskClick?: CancelCallback
  onEsc?: () => boolean | void
  onAfterLeave?: () => void
}

/**
 * 让命令式弹窗的每条关闭路径都恰好结算一次。
 * 异步确认中返回只消费事件；返回 false 仍沿用 Naive UI 的保留弹窗语义。
 */
export function openDialogWithBack(
  api: Pick<DialogApi, 'warning'>,
  options: BackHandledDialogOptions,
): Promise<boolean> {
  let settled = false
  let confirming = false
  let unregister: (() => void) | undefined
  let resolveResult!: (confirmed: boolean) => void
  const result = new Promise<boolean>((resolve) => {
    resolveResult = resolve
  })

  const settle = (confirmed: boolean) => {
    if (settled) return
    settled = true
    unregister?.()
    unregister = undefined
    resolveResult(confirmed)
    instance.destroy()
  }

  const cancel = (event: MouseEvent, callback?: CancelCallback) => {
    if (settled) return true
    if (confirming) return false
    if (callback?.(event) === false) return false
    settle(false)
    return true
  }

  const cancelWithoutEvent = (callback?: () => boolean | void) => {
    if (settled || confirming) return false
    if (callback?.() === false) return false
    settle(false)
    return true
  }

  const detach = () => {
    unregister?.()
    unregister = undefined
    settle(false)
  }

  const attach = () => {
    if (settled || unregister) return
    unregister = registerBackHandler(() => {
      if (settled) return false
      if (confirming) return true
      settle(false)
      return true
    }, 100)
  }

  const maskClosable = options.maskClosable !== false
  const instance = api.warning({
    ...options,
    // Naive UI 会忽略 onMaskClick 的返回值；关闭统一交给 settle。
    maskClosable: false,
    onPositiveClick: async (event: MouseEvent) => {
      if (settled || confirming) return false
      confirming = true
      instance.loading = true
      try {
        const accepted = await options.onPositiveClick?.(event, instance)
        if (accepted === false) return false
        settle(true)
        return true
      } finally {
        confirming = false
        if (!settled) instance.loading = false
      }
    },
    onNegativeClick: (event: MouseEvent) => cancel(event, options.onNegativeClick),
    onMaskClick: maskClosable
      ? (event: MouseEvent) => cancel(event, options.onMaskClick)
      : options.onMaskClick,
    onClose: () => cancelWithoutEvent(options.onClose),
    // Escape 统一经 useAndroidBack 的捕获阶段处理，避免弹窗与全局同时关闭。
    onAfterLeave: () => {
      settle(false)
      options.onAfterLeave?.()
    },
  })

  attach()
  if (getCurrentInstance()) {
    onActivated(attach)
    onDeactivated(detach)
    onUnmounted(detach)
  }
  if (getCurrentScope()) onScopeDispose(detach)
  return result
}
