import { openUrl } from '@tauri-apps/plugin-opener'

export const REPO_OWNER = 'Beyondoooars'
export const REPO_NAME = 'JoyManga'

/** 项目主页；设置页「关于」与更新检查共用同一份地址。 */
export const REPOSITORY_URL = `https://github.com/${REPO_OWNER}/${REPO_NAME}`

/**
 * 用系统浏览器/默认应用打开外部链接。
 * WebView 内直接导航会把整个应用页面替换成网页且回不来，所以外部链接一律走原生打开。
 */
export async function openExternalUrl(url: string): Promise<void> {
  try {
    await openUrl(url)
  } catch (error) {
    console.warn('opener.openUrl failed, fallback to window.open', error)
    window.open(url, '_blank')
  }
}
