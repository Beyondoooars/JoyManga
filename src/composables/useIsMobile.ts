/** 运行环境是否为 Android（决定是否启用 JmAndroid 原生桥）。 */
export const isAndroid = typeof navigator !== 'undefined' && /Android/i.test(navigator.userAgent)
