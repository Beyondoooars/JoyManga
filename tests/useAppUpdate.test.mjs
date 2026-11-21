import { test } from 'node:test'
import assert from 'node:assert/strict'

class MockWindow extends EventTarget {
  constructor() {
    super()
    this.openCalls = []
    this.__TAURI_INTERNALS__ = {
      invoke: async (command, args) => {
        if (command === 'plugin:app|version') return '0.3.18'
        if (command === 'plugin:opener|open_url') {
          this.openCalls.push(args?.url)
          return null
        }
        return null
      },
    }
  }
  open(url) {
    this.openCalls.push(url)
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
  memoryStorage.clear()
  storageThrow = false
  mockWindow.openCalls.length = 0
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

test('downloadUpdate opens direct apk URL or falls back to release URL via external browser', async () => {
  const state = resetUpdateState()
  state.downloadUrl.value = 'https://example.invalid/app-arm64.apk'
  state.downloadFileName.value = 'app-arm64.apk'
  state.releaseUrl.value = 'https://example.invalid/release'

  state.downloadUpdate()
  await Promise.resolve()
  assert.deepEqual(mockWindow.openCalls, ['https://example.invalid/app-arm64.apk'])

  // 仅存在 Release 页链接时，也通过外部浏览器打开
  mockWindow.openCalls.length = 0
  state.downloadUrl.value = ''
  state.downloadUpdate()
  await Promise.resolve()
  assert.deepEqual(mockWindow.openCalls, ['https://example.invalid/release'])

  // 无任何可用链接时不触发打开
  mockWindow.openCalls.length = 0
  state.releaseUrl.value = ''
  state.downloadUpdate()
  await Promise.resolve()
  assert.deepEqual(mockWindow.openCalls, [])
})
