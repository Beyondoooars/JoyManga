import { ref } from 'vue'
import { getVersion } from '@tauri-apps/api/app'
import { openUrl } from '@tauri-apps/plugin-opener'
import { isAndroid } from './useIsMobile.ts'
import { downloadAndInstallApk } from '../utils/androidStorage.ts'

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

const REPO_OWNER = 'Beyondoooars'
const REPO_NAME = 'JoyManga'
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
const releaseTitle = ref('')
const releaseNotes = ref('')
const publishedAt = ref('')
const releaseUrl = ref('')
const downloadUrl = ref('')
const downloadFileName = ref('')
const downloadFileSize = ref('')
const errorMessage = ref('')

// 应用内更新（Android）：原生下载 APK 的进度与提示
const installing = ref(false)
const installProgress = ref(0)
const installMessage = ref('')
let stopInstallWatch: (() => void) | undefined

function compareVersions(v1: string, v2: string): number {
  const p1 = v1.replace(/^v/, '').split('.').map((n) => parseInt(n, 10) || 0)
  const p2 = v2.replace(/^v/, '').split('.').map((n) => parseInt(n, 10) || 0)
  for (let i = 0; i < Math.max(p1.length, p2.length); i++) {
    const num1 = p1[i] || 0
    const num2 = p2[i] || 0
    if (num1 > num2) return 1
    if (num1 < num2) return -1
  }
  return 0
}

function formatFileSize(bytes: number): string {
  if (!bytes || bytes <= 0) return ''
  const mb = bytes / (1024 * 1024)
  return `${mb.toFixed(2)} MB`
}

function formatDate(isoStr?: string): string {
  if (!isoStr) return ''
  try {
    const d = new Date(isoStr)
    return d.toLocaleDateString()
  } catch {
    return ''
  }
}

/** 距上次静默检查是否已超过允许的间隔；手动检查不受限制。 */
function silentCheckDue(): boolean {
  const last = Number(localStorage.getItem(LAST_CHECK_KEY) ?? 0)
  if (!Number.isFinite(last) || last <= 0) return true
  return Date.now() - last >= SILENT_INTERVAL_MS
}

function markChecked(success: boolean) {
  const next = success ? Date.now() : Date.now() - SILENT_INTERVAL_MS + RETRY_INTERVAL_MS
  localStorage.setItem(LAST_CHECK_KEY, String(next))
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
      releaseTitle.value = data.name || `v${tag}`
      releaseNotes.value = data.body || '无更新说明'
      publishedAt.value = formatDate(data.published_at)
      releaseUrl.value = data.html_url || `https://github.com/${REPO_OWNER}/${REPO_NAME}/releases`

      // 3. 匹配 arm64-v8a 安装包（本应用仅面向 Android，ABI 固定为 arm64-v8a）
      const assets = data.assets || []
      const arm64Apk = assets.find((a) => a.name.toLowerCase().endsWith('.apk') && a.name.toLowerCase().includes('arm64'))
      const anyApk = assets.find((a) => a.name.toLowerCase().endsWith('.apk'))
      const target = arm64Apk || anyApk
      if (target) {
        downloadUrl.value = target.browser_download_url
        downloadFileName.value = target.name
        downloadFileSize.value = formatFileSize(target.size)
      } else {
        downloadUrl.value = releaseUrl.value
        downloadFileName.value = '请前往 Release 页面下载 APK'
        downloadFileSize.value = ''
      }

      // 4. 比对版本号
      if (compareVersions(tag, currentVersion.value) > 0) {
        hasUpdate.value = true
        // 如果是启动时的静默检查，检查用户是否已设置“跳过此版本”
        const ignoredVersion = localStorage.getItem(IGNORED_VERSION_KEY)
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
      if (!silent) {
        // 手动检查时若出错，由调用方或界面显示提示
      }
    } finally {
      checking.value = false
    }
  }

  function ignoreCurrentUpdate() {
    if (latestVersion.value) {
      localStorage.setItem(IGNORED_VERSION_KEY, latestVersion.value)
    }
    showUpdateDialog.value = false
  }

  async function openExternal(url: string) {
    try {
      await openUrl(url)
    } catch (e) {
      console.warn('opener.openUrl failed, fallback to window.open', e)
      window.open(url, '_blank')
    }
  }

  function downloadUpdate() {
    const targetUrl = downloadUrl.value || releaseUrl.value
    if (!targetUrl) return
    // 只有拿到 .apk 直链时才走应用内安装；否则退回浏览器打开 Release 页
    const isApk = downloadUrl.value !== '' && downloadFileName.value.toLowerCase().endsWith('.apk')
    if (isAndroid && isApk && window.JmAndroid) {
      if (installing.value) return
      installing.value = true
      installProgress.value = 0
      installMessage.value = '正在下载安装包…'
      stopInstallWatch?.()
      stopInstallWatch = downloadAndInstallApk(targetUrl, downloadFileName.value, (progress) => {
        if (progress.state === 'progress') {
          installProgress.value = progress.percent ?? installProgress.value
        } else if (progress.state === 'done') {
          installProgress.value = 100
          installMessage.value = '下载完成，正在打开安装器…'
        } else if (progress.state === 'installer') {
          installMessage.value = progress.message ?? '请在系统安装器中确认安装'
        } else {
          const message = progress.message ?? (progress.state === 'permission' ? '请允许安装未知应用后重试' : '更新失败')
          installMessage.value = message
          installProgress.value = 0
          if (progress.state === 'error') errorMessage.value = message
        }
        // done 之后原生才拉起安装器，失败信息会晚一步到，所以这时先继续监听，
        // 下一次点击更新时再移除旧监听器。
        if (progress.state === 'permission' || progress.state === 'error') {
          installing.value = false
          stopInstallWatch?.()
          stopInstallWatch = undefined
        }
      })
      return
    }
    void openExternal(targetUrl)
  }

  function openReleasePage() {
    if (releaseUrl.value) {
      void openExternal(releaseUrl.value)
    }
  }

  return {
    checking,
    showUpdateDialog,
    hasUpdate,
    currentVersion,
    latestVersion,
    releaseTitle,
    releaseNotes,
    publishedAt,
    releaseUrl,
    downloadUrl,
    downloadFileName,
    downloadFileSize,
    errorMessage,
    installing,
    installProgress,
    installMessage,
    checkForUpdates,
    ignoreCurrentUpdate,
    downloadUpdate,
    openReleasePage,
    compareVersions,
  }
}
