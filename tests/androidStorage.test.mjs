import { test } from 'node:test'
import assert from 'node:assert/strict'

const state = {
  allFilesAccess: false,
  pickMode: 'success',
  throwOnInstall: false,
  pickRequests: [],
  discarded: [],
  openFileReadyCalls: 0,
  snapshot: null,
  vpnActive: false,
  metered: false,
  networkKey: null,
}

class MockWindow extends EventTarget {
  constructor() {
    super()
    this.JmAndroid = {
      libraryPath: () => '/storage/emulated/0/Download/JoyManga',
      downloadsPath: () => '/storage/emulated/0/Download/JoyManga',
      hasAllFilesAccess: () => state.allFilesAccess,
      supportsPublicStorage: () => true,
      canWritePublicDownloads: () => true,
      requestAllFilesAccess: () => {},
      pickDocuments: (requestId, mode) => {
        state.pickRequests.push({ requestId, mode })
        if (state.pickMode === 'success') {
          this.dispatchEvent(new CustomEvent('jm-mobile-result', { detail: { requestId, paths: ['/tmp/a.cbz'] } }))
        } else if (state.pickMode === 'cancel') {
          this.dispatchEvent(new CustomEvent('jm-mobile-result', { detail: { requestId, cancelled: true } }))
        } else if (state.pickMode === 'error') {
          this.dispatchEvent(new CustomEvent('jm-mobile-result', { detail: { requestId, error: '读取取消或失败' } }))
        }
      },
      discardImports: (requestId) => {
        state.discarded.push(requestId)
      },
      exportDocuments: () => {},
      downloadStatus: () => {},
      exitApp: () => {},
      setKeepScreenOn: () => {},
      setImmersiveMode: () => {},
      setVolumeKeyPaging: () => {},
      isVpnActive: () => state.vpnActive,
      isMetered: () => state.metered,
      networkKey: () => state.networkKey,
      networkSnapshot: () => state.snapshot,
      downloadAndInstallApk: () => {
        if (state.throwOnInstall) throw new Error('service start rejected')
      },
      updateDownloadState: () => {},
      notifyOpenFileReady: () => {
        state.openFileReadyCalls += 1
      },
    }
  }
}

class MockDocument extends EventTarget {
  constructor() {
    super()
    this.visibilityState = 'visible'
  }
}

const mockWindow = new MockWindow()
const mockDocument = new MockDocument()

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

const {
  downloadAndInstallApk,
  onOpenFile,
  requestAllFilesAccessAndWait,
  requiresAllFilesAccess,
  watchNetworkMetered,
  watchNetworkVpn,
  withAndroidSelection,
} = await import('../src/utils/androidStorage.ts')

function resetState() {
  state.allFilesAccess = false
  state.pickMode = 'success'
  state.throwOnInstall = false
  state.pickRequests.length = 0
  state.discarded.length = 0
  state.openFileReadyCalls = 0
  state.snapshot = null
  state.vpnActive = false
  state.metered = false
  state.networkKey = null
  mockDocument.visibilityState = 'visible'
}

test('withAndroidSelection always discards imports on success, action throw, cancel and picker error', async () => {
  resetState()
  const okResult = await withAndroidSelection('cbz', async (paths) => paths[0])
  assert.equal(okResult, '/tmp/a.cbz')
  assert.deepEqual(state.discarded, [state.pickRequests[0].requestId])

  await assert.rejects(
    withAndroidSelection('cbz', async () => {
      throw new Error('import crashed')
    }),
    /import crashed/,
  )
  assert.deepEqual(state.discarded, [state.pickRequests[0].requestId, state.pickRequests[1].requestId])

  state.pickMode = 'cancel'
  let ranOnCancel = false
  const cancelled = await withAndroidSelection('backup', async () => {
    ranOnCancel = true
  })
  assert.equal(cancelled, undefined)
  assert.equal(ranOnCancel, false)
  assert.equal(state.discarded.length, 3)

  state.pickMode = 'error'
  await assert.rejects(withAndroidSelection('cbz', async () => {}), /读取取消或失败/)
  // Error before action does not emit extra discardImports since requestId cleanup happens on error listener
  assert.equal(state.discarded.length, 3)
})

test('withAndroidSelection times out after 5 minutes, discards imports, and ignores late results', async (t) => {
  resetState()
  state.pickMode = 'hang'
  t.mock.timers.enable({ apis: ['setTimeout'] })
  try {
    const selecting = withAndroidSelection('cbz', async () => 'unreachable')
    const requestId = state.pickRequests[0].requestId
    t.mock.timers.tick(5 * 60 * 1000)
    await assert.rejects(selecting, /等待系统文件选择器超时/)
    assert.deepEqual(state.discarded, [requestId])

    // Late result should not trigger unhandled error or extra discard
    mockWindow.dispatchEvent(new CustomEvent('jm-mobile-result', { detail: { requestId, paths: ['/tmp/late.cbz'] } }))
    assert.deepEqual(state.discarded, [requestId])
  } finally {
    t.mock.timers.reset()
  }
})

test('requestAllFilesAccessAndWait handles permission event, visibility resume, and hidden/visible 90s fallback', async (t) => {
  resetState()
  t.mock.timers.enable({ apis: ['setTimeout'] })
  try {
    // 1. Permission event settles immediately even on denial
    const deniedPromise = requestAllFilesAccessAndWait()
    mockWindow.dispatchEvent(new CustomEvent('jm-permission-result', { detail: { request: 'storage', granted: false } }))
    assert.equal(await deniedPromise, false)

    // 2. Visibility return waits 300ms then reads current access
    const grantedPromise = requestAllFilesAccessAndWait()
    state.allFilesAccess = true
    mockDocument.visibilityState = 'visible'
    mockDocument.dispatchEvent(new Event('visibilitychange'))
    t.mock.timers.tick(300)
    assert.equal(await grantedPromise, true)

    // 3. Fallback reschedules while app stays backgrounded, then settles once visible
    state.allFilesAccess = false
    mockDocument.visibilityState = 'hidden'
    let settled = false
    const fallbackPromise = requestAllFilesAccessAndWait().then((v) => {
      settled = true
      return v
    })
    t.mock.timers.tick(90_000)
    await Promise.resolve()
    assert.equal(settled, false)

    mockDocument.visibilityState = 'visible'
    t.mock.timers.tick(90_000)
    assert.equal(await fallbackPromise, false)
    assert.equal(settled, true)
  } finally {
    t.mock.timers.reset()
  }
})

test('network watchers, open-file listener and install wrapper read initial state and clean up subscriptions', () => {
  resetState()
  state.metered = true
  state.snapshot = JSON.stringify({ active: true, metered: true, networkKey: 'cell:1' })

  const vpnUpdates = []
  const stopVpn = watchNetworkVpn((active, key) => vpnUpdates.push([active, key]))
  mockWindow.dispatchEvent(new CustomEvent('jm-network-vpn', { detail: { active: false, networkKey: 'wifi:2' } }))
  mockWindow.dispatchEvent(new CustomEvent('jm-network-vpn', { detail: true }))
  stopVpn()
  mockWindow.dispatchEvent(new CustomEvent('jm-network-vpn', { detail: false }))
  assert.deepEqual(vpnUpdates, [
    [true, 'cell:1'],
    [false, 'wifi:2'],
    [true, null],
  ])

  const meteredUpdates = []
  const stopMetered = watchNetworkMetered((m) => meteredUpdates.push(m))
  mockWindow.dispatchEvent(new CustomEvent('jm-network-metered', { detail: false }))
  stopMetered()
  mockWindow.dispatchEvent(new CustomEvent('jm-network-metered', { detail: true }))
  assert.deepEqual(meteredUpdates, [true, false])

  // Fallback to networkSnapshot().metered when legacy JmAndroid lacks isMetered()
  const savedIsMetered = mockWindow.JmAndroid.isMetered
  delete mockWindow.JmAndroid.isMetered
  try {
    state.snapshot = JSON.stringify({ active: false, metered: true, networkKey: 'cell:2' })
    const fallbackMetered = []
    const stopFallback = watchNetworkMetered((m) => fallbackMetered.push(m))
    stopFallback()
    assert.deepEqual(fallbackMetered, [true])
  } finally {
    mockWindow.JmAndroid.isMetered = savedIsMetered
  }

  const opened = []
  const stopOpen = onOpenFile((detail) => opened.push(detail))
  assert.equal(state.openFileReadyCalls, 1)
  mockWindow.dispatchEvent(new CustomEvent('jm-open-file', { detail: { id: 'req-1' } }))
  mockWindow.dispatchEvent(new CustomEvent('jm-open-file', { detail: { id: 'req-2', path: '/tmp/book.cbz' } }))
  stopOpen()
  mockWindow.dispatchEvent(new CustomEvent('jm-open-file', { detail: { path: '/tmp/after.cbz' } }))
  assert.deepEqual(opened, [{ id: 'req-2', path: '/tmp/book.cbz' }])

  state.throwOnInstall = true
  const installEvents = []
  const stopInstall = downloadAndInstallApk('https://example.invalid/a.apk', 'a.apk', (p) => installEvents.push(p))
  stopInstall()
  assert.deepEqual(installEvents, [{ state: 'error', message: 'service start rejected' }])
})

test('requiresAllFilesAccess distinguishes app-private storage from public shared storage', () => {
  const savedLibraryPath = mockWindow.JmAndroid.libraryPath
  mockWindow.JmAndroid.libraryPath = () => '/data/user/0/com.joymanga.app/files/library'
  try {
    assert.equal(requiresAllFilesAccess(''), false)
    assert.equal(requiresAllFilesAccess('/data/user/0/com.joymanga.app/files/library'), false)
    assert.equal(requiresAllFilesAccess('/storage/emulated/0/Android/data/com.joymanga.app/files/JoyManga'), false)
    assert.equal(requiresAllFilesAccess('/storage/emulated/0/Download/JoyManga'), true)
    assert.equal(requiresAllFilesAccess('/storage/emulated/0/Pictures/Manga'), true)
  } finally {
    mockWindow.JmAndroid.libraryPath = savedLibraryPath
  }
})
