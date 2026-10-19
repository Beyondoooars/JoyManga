import { ref } from 'vue'
import { getVersion } from '@tauri-apps/api/app'
import { openUrl } from '@tauri-apps/plugin-opener'

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

export function useAppUpdate() {
  async function checkForUpdates(silent = false) {
    if (checking.value) return
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
    if (targetUrl) {
      void openExternal(targetUrl)
    }
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
    checkForUpdates,
    ignoreCurrentUpdate,
    downloadUpdate,
    openReleasePage,
    compareVersions,
  }
}
