import { ref } from 'vue'
import { getVersion } from '@tauri-apps/api/app'
import { openExternalUrl, REPO_NAME, REPO_OWNER } from '../utils/externalLink.ts'
import { compareVersions } from '../utils/semver.ts'

export interface GithubReleaseAsset {
  name: string
  browser_download_url: string
  size: number
  content_type?: string
}

export interface GithubReleaseData {
  tag_name: string
  name: string
  body: string
  html_url: string
  published_at?: string
  assets: GithubReleaseAsset[]
}

const IGNORED_VERSION_KEY = 'joymanga_ignored_update_version'
// 冷启动静默检查每隔一天做一次即可；失败后半小时再试，避免离线时每次启动都请求 GitHub。
const LAST_CHECK_KEY = 'joymanga_update_last_check'
const SILENT_INTERVAL_MS = 24 * 60 * 60 * 1000
const RETRY_INTERVAL_MS = 30 * 60 * 1000

// 全局单例响应式状态，避免不同组件状态脱节
const checking = ref(false)
const showUpdateDialog = ref(false)
const hasUpdate = ref(false)
const currentVersion = ref('')
const latestVersion = ref('')
const releaseNotes = ref('')
const releaseUrl = ref('')
const downloadUrl = ref('')
const downloadFileName = ref('')
const errorMessage = ref('')

function safeStorageGet(key: string): string | null {
  try {
    return globalThis.localStorage?.getItem(key) ?? null
  } catch (error) {
    console.warn(`读取本地存储 ${key} 失败`, error)
    return null
  }
}

function safeStorageSet(key: string, value: string): void {
  try {
    globalThis.localStorage?.setItem(key, value)
  } catch (error) {
    console.warn(`写入本地存储 ${key} 失败`, error)
  }
}

/** 距上次静默检查是否已超过允许的间隔；手动检查不受限制。 */
function silentCheckDue(): boolean {
  const last = Number(safeStorageGet(LAST_CHECK_KEY) ?? 0)
  if (!Number.isFinite(last) || last <= 0) return true
  return Date.now() - last >= SILENT_INTERVAL_MS
}

function markChecked(success: boolean) {
  const next = success ? Date.now() : Date.now() - SILENT_INTERVAL_MS + RETRY_INTERVAL_MS
  safeStorageSet(LAST_CHECK_KEY, String(next))
}

export function useAppUpdate() {
  async function checkForUpdates(silent = false) {
    if (checking.value) return
    if (silent && !silentCheckDue()) return
    checking.value = true
    errorMessage.value = ''

    try {
      // 1. 获取本地运行版本
      if (!currentVersion.value) {
        currentVersion.value = await getVersion()
      }

      // 2. 请求 GitHub 官方最新发布 API
      //    超时保护：网络半挂时 fetch 可能长期不返回，会把 checking 卡住并挡住后续所有检查
      const controller = new AbortController()
      const timeout = setTimeout(() => controller.abort(), 15_000)
      let res: Response
      try {
        res = await fetch(
          `https://api.github.com/repos/${REPO_OWNER}/${REPO_NAME}/releases/latest`,
          { headers: { Accept: 'application/vnd.github.v3+json' }, signal: controller.signal },
        )
      } finally {
        clearTimeout(timeout)
      }

      if (!res.ok) {
        if (res.status === 404) {
          throw new Error('未在 GitHub 找到已发布的 Release 版本')
        }
        throw new Error(`GitHub 接口响应异常 (HTTP ${res.status})`)
      }

      const data: GithubReleaseData = await res.json()
      markChecked(true)
      const tag = (data.tag_name || '').replace(/^v/, '').trim()
      latestVersion.value = tag
      releaseNotes.value = data.body || '无更新说明'
      releaseUrl.value = data.html_url || `https://github.com/${REPO_OWNER}/${REPO_NAME}/releases`

      // 3. 匹配 arm64-v8a 安装包（本应用仅面向 Android，ABI 固定为 arm64-v8a）
      const assets = data.assets || []
      const arm64Apk = assets.find((a) => a.name.toLowerCase().endsWith('.apk') && a.name.toLowerCase().includes('arm64'))
      const anyApk = assets.find((a) => a.name.toLowerCase().endsWith('.apk'))
      const target = arm64Apk || anyApk
      if (target) {
        downloadUrl.value = target.browser_download_url
        downloadFileName.value = target.name
      } else {
        downloadUrl.value = releaseUrl.value
        downloadFileName.value = '请前往 Release 页面下载 APK'
      }

      // 4. 比对版本号
      if (compareVersions(tag, currentVersion.value) > 0) {
        hasUpdate.value = true
        // 如果是启动时的静默检查，检查用户是否已设置“跳过此版本”
        const ignoredVersion = safeStorageGet(IGNORED_VERSION_KEY)
        if (silent && ignoredVersion === tag) {
          showUpdateDialog.value = false
        } else {
          showUpdateDialog.value = true
        }
      } else {
        hasUpdate.value = false
        if (!silent) {
          showUpdateDialog.value = false
        }
      }
    } catch (err: unknown) {
      const errStr = err instanceof Error ? err.message : String(err)
      errorMessage.value = errStr
      markChecked(false)
      // 静默检查失败不打扰用户；手动检查的错误由调用方读取 errorMessage 显示
    } finally {
      checking.value = false
    }
  }

  function ignoreCurrentUpdate() {
    if (latestVersion.value) {
      safeStorageSet(IGNORED_VERSION_KEY, latestVersion.value)
    }
    showUpdateDialog.value = false
  }

  function downloadUpdate() {
    const targetUrl = downloadUrl.value || releaseUrl.value
    if (!targetUrl) return
    void openExternalUrl(targetUrl)
  }

  return {
    checking,
    showUpdateDialog,
    hasUpdate,
    currentVersion,
    latestVersion,
    releaseNotes,
    releaseUrl,
    downloadUrl,
    downloadFileName,
    errorMessage,
    checkForUpdates,
    ignoreCurrentUpdate,
    downloadUpdate,
    compareVersions,
  }
}
