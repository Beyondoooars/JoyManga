import { readonly, ref } from 'vue'

/** 运行环境是否为 Android（决定是否启用 JmAndroid 原生桥）。 */
export const isAndroid = typeof navigator !== 'undefined' && /Android/i.test(navigator.userAgent)

/**
 * 应用只面向 Android，布局恒为移动端。
 *
 * 保留 `useIsMobile()` 与 `isMobile` 的原有形状，避免大范围改动调用点；
 * 不再监听窗口尺寸——桌面端已被移除。
 */
const isMobile = readonly(ref(true))

export function useIsMobile() {
  return {
    isMobile,
    isAndroid,
  }
}
