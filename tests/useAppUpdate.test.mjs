import { test } from 'node:test'
import assert from 'node:assert/strict'

class MockWindow extends EventTarget {
  constructor() {
    super()
    this.installCalls = []
    this.syncCalls = 0
    this.openCalls = []
    this.__TAURI_INTERNALS__ = {
      invoke: async (command) => {
        if (command === 'plugin:app|version') return '0.3.18'
        return null
      },
    }
    this.JmAndroid = {
      downloadAndInstallApk: (url, fileName) => {
        this.installCalls.push({ url, fileName })
      },
      updateDownloadState: () => {
        this.syncCalls += 1
      },
    }
  }
}

class MockDocument extends EventTarget {
  constructor() {
    super()
    this.visibilityState = 'visible'
  }
  createElement() {
    return {}
  }
}

const mockWindow = new MockWindow()
const mockDocument = new MockDocument()
const memoryStorage = new Map()
let storageThrow = false

Object.defineProperty(globalThis, 'navigator', {
  value: { userAgent: 'Mozilla/5.0 (Linux; Android 14)' },
  configurable: true,
})
Object.defineProperty(globalThis, 'window', {
  value: mockWindow,
  configurable: true,
})
Object.defineProperty(globalThis, 'document', {
  value: mockDocument,
  configurable: true,
})
Object.defineProperty(globalThis, 'localStorage', {
  value: {
    getItem(key) {
      if (storageThrow) throw new Error('storage blocked')
      return memoryStorage.get(key) ?? null
    },
    setItem(key, value) {
      if (storageThrow) throw new Error('storage blocked')
      memoryStorage.set(key, String(value))
    },
  },
  configurable: true,
})

const { useAppUpdate } = await import('../src/composables/useAppUpdate.ts')

function resetUpdateState() {
  const state = useAppUpdate()
  state.checking.value = false
  state.showUpdateDialog.value = false
  state.hasUpdate.value = false
  state.latestVersion.value = ''
  state.releaseNotes.value = ''
  state.releaseUrl.value = ''
  state.downloadUrl.value = ''
  state.downloadFileName.value = ''
  state.errorMessage.value = ''
  state.installing.value = false
  state.installProgress.value = 0
  state.installMessage.value = ''
  memoryStorage.clear()
  storageThrow = false
  mockWindow.installCalls.length = 0
  mockWindow.syncCalls = 0
  return state
}

test('checkForUpdates selects arm64 apk and ignores pre-release older than current', async () => {
  const originalFetch = globalThis.fetch
  const state = resetUpdateState()
  try {
    globalThis.fetch = async () => ({
      ok: true,
      json: async () => ({
        tag_name: 'v0.4.0',
        name: 'v0.4.0',
        body: '修复问题',
        html_url: 'https://github.com/Beyondoooars/JoyManga/releases/tag/v0.4.0',
        assets: [
          { name: 'joymanga-universal.apk', browser_download_url: 'https://example.invalid/univ.apk', size: 10 },
          { name: 'joymanga-arm64-v8a.apk', browser_download_url: 'https://example.invalid/arm64.apk', size: 20 },
        ],
      }),
    })
    await state.checkForUpdates(false)
    assert.equal(state.hasUpdate.value, true)
    assert.equal(state.showUpdateDialog.value, true)
    assert.equal(state.latestVersion.value, '0.4.0')
    assert.equal(state.downloadUrl.value, 'https://example.invalid/arm64.apk')
    assert.equal(state.downloadFileName.value, 'joymanga-arm64-v8a.apk')

    // Prerelease of the same version sorts behind stable 0.3.18
    globalThis.fetch = async () => ({
      ok: true,
      json: async () => ({
        tag_name: 'v0.3.18-rc.1',
        name: 'rc',
        body: '',
        html_url: 'https://github.com/Beyondoooars/JoyManga/releases/tag/v0.3.18-rc.1',
        assets: [],
      }),
    })
    await state.checkForUpdates(false)
    assert.equal(state.hasUpdate.value, false)
    assert.equal(state.showUpdateDialog.value, false)
  } finally {
    globalThis.fetch = originalFetch
  }
})

test('ignoreCurrentUpdate suppresses silent check dialog while throwing localStorage stays safe', async () => {
  const originalFetch = globalThis.fetch
  const originalWarn = console.warn
  console.warn = () => {}
  const state = resetUpdateState()
  try {
    globalThis.fetch = async () => ({
      ok: true,
      json: async () => ({
        tag_name: 'v0.5.0',
        name: 'v0.5.0',
        body: 'notes',
        html_url: 'https://github.com/Beyondoooars/JoyManga/releases/tag/v0.5.0',
        assets: [{ name: 'joymanga-arm64-v8a.apk', browser_download_url: 'https://example.invalid/0.5.apk', size: 1 }],
      }),
    })

    await state.checkForUpdates(true)
    assert.equal(state.showUpdateDialog.value, true)
    state.ignoreCurrentUpdate()
    assert.equal(state.showUpdateDialog.value, false)

    // Clear last-check key so next silent check hits fetch again while ignored version stays stored
    memoryStorage.delete('joymanga_update_last_check')
    await state.checkForUpdates(true)
    assert.equal(state.hasUpdate.value, true)
    assert.equal(state.showUpdateDialog.value, false)

    // Manual check ignores the skip preference and shows dialog
    await state.checkForUpdates(false)
    assert.equal(state.showUpdateDialog.value, true)

    // Storage access errors must not fail update checks
    storageThrow = true
    state.showUpdateDialog.value = false
    await state.checkForUpdates(true)
    assert.equal(state.hasUpdate.value, true)
    assert.equal(state.showUpdateDialog.value, true)
    state.ignoreCurrentUpdate()
    assert.equal(state.showUpdateDialog.value, false)
  } finally {
    console.warn = originalWarn
    globalThis.fetch = originalFetch
  }
})

test('downloadUpdate tracks progress, resets on installer, keeps late error listener, and syncs on foreground', () => {
  const state = resetUpdateState()
  state.downloadUrl.value = 'https://example.invalid/app-arm64.apk'
  state.downloadFileName.value = 'app-arm64.apk'

  state.downloadUpdate()
  assert.equal(mockWindow.installCalls.length, 1)
  assert.equal(state.installing.value, true)

  mockWindow.dispatchEvent(new CustomEvent('jm-update-progress', { detail: { state: 'progress', percent: 60 } }))
  assert.equal(state.installProgress.value, 60)

  mockWindow.dispatchEvent(new CustomEvent('jm-update-progress', { detail: { state: 'done' } }))
  assert.equal(state.installProgress.value, 100)

  mockWindow.dispatchEvent(new CustomEvent('jm-update-progress', { detail: { state: 'installer', message: '确认安装' } }))
  assert.equal(state.installing.value, false)
  assert.equal(state.installMessage.value, '确认安装')

  // Returning to foreground syncs state; idle when not installing must preserve installer message
  mockDocument.visibilityState = 'visible'
  mockDocument.dispatchEvent(new Event('visibilitychange'))
  assert.equal(mockWindow.syncCalls, 1)

  mockWindow.dispatchEvent(new CustomEvent('jm-update-progress', { detail: { state: 'idle' } }))
  assert.equal(state.installMessage.value, '确认安装')

  // Late PackageInstaller error after installer prompt is still captured
  mockWindow.dispatchEvent(new CustomEvent('jm-update-progress', { detail: { state: 'error', message: '安装被取消' } }))
  assert.equal(state.errorMessage.value, '安装被取消')

  // Idle during active download resets installing back to false
  state.errorMessage.value = ''
  state.downloadUpdate()
  assert.equal(state.installing.value, true)
  mockWindow.dispatchEvent(new CustomEvent('jm-update-progress', { detail: { state: 'idle' } }))
  assert.equal(state.installing.value, false)
  assert.equal(state.installMessage.value, '')
})
