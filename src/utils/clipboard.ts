export interface ToastNotifier {
  success: (content: string) => unknown
  error: (content: string) => unknown
}

export async function copyText(text: string): Promise<boolean> {
  try {
    if (typeof navigator === 'undefined' || !navigator.clipboard?.writeText) {
      return false
    }
    await navigator.clipboard.writeText(text)
    return true
  } catch {
    return false
  }
}

export async function copyPathWithToast(
  path: string,
  message: ToastNotifier,
  successText = '已复制到剪贴板',
): Promise<boolean> {
  if (await copyText(path)) {
    message.success(successText)
    return true
  }
  message.error('复制失败，请重试')
  return false
}
