import { test } from 'node:test'
import assert from 'node:assert/strict'
import { createPinia } from 'pinia'
import { nextTick } from 'vue'
import { commands } from '../src/bindings.ts'
import { useStore } from '../src/store.ts'

function deferred() {
  let resolve
  const promise = new Promise((res) => {
    resolve = res
  })
  return { promise, resolve }
}

async function flushMicrotasks() {
  for (let i = 0; i < 4; i++) {
    await Promise.resolve()
  }
}

function makeConfig(overrides = {}) {
  return {
    revision: 1,
    username: '',
    password: '',
    downloadDir: 'initial-directory',
    proxyMode: 'System',
    proxyHost: '127.0.0.1',
    proxyPort: 7890,
    chapterConcurrency: 3,
    imgConcurrency: 8,
    apiDomainMode: 'Domain1',
    customApiDomain: '',
    imageDomainMode: 'Domain1',
    customImageDomain: '',
    ...overrides,
  }
}

test('maintenance waits for an in-flight save, cancels stale pending save, and skips restored config', async () => {
  const store = useStore(createPinia())
  const originalSaveConfig = commands.saveConfig
  let pendingSave = deferred()
  let saveCalls = 0
  const savedSnapshots = []
  commands.saveConfig = async (snapshot) => {
    saveCalls++
    savedSnapshots.push(snapshot.downloadDir)
    return pendingSave.promise
  }

  try {
    const initial = makeConfig({ revision: 1, downloadDir: 'dir-1' })
    store.applyConfig(initial, true)
    store.enableConfigAutoSave()

    // Trigger an edit and flush it so save #1 is in-flight
    store.config.downloadDir = 'dir-edited'
    await nextTick()
    const firstFlush = store.flushPendingConfigSave()
    await flushMicrotasks()
    assert.equal(saveCalls, 1)

    let restoreStarted = false
    const restored = makeConfig({ revision: 2, downloadDir: 'restored-directory' })
    const restoring = store.runConfigMaintenance(async () => {
      restoreStarted = true
      store.applyConfig(restored, true)
    })

    await nextTick()
    assert.equal(restoreStarted, false)
    await assert.rejects(store.runConfigMaintenance(async () => {}), /配置维护正在进行/)

    // Resolve in-flight save #1 with server response
    pendingSave.resolve({ status: 'ok', data: makeConfig({ revision: 2, downloadDir: 'dir-edited' }) })
    await Promise.all([firstFlush, restoring])

    assert.equal(restoreStarted, true)
    assert.equal(store.config.downloadDir, 'restored-directory')
    assert.equal(saveCalls, 1, 'restored config must not trigger an auto-save')
    assert.equal(store.configMaintenanceInProgress, false)

    // Subsequent user edit resumes auto-save cleanly
    pendingSave = deferred()
    store.config.downloadDir = 'user-changed-directory'
    await nextTick()
    const secondFlush = store.flushPendingConfigSave()
    await flushMicrotasks()
    assert.equal(saveCalls, 2)
    assert.equal(savedSnapshots[1], 'user-changed-directory')
    pendingSave.resolve({ status: 'ok', data: makeConfig({ revision: 3, downloadDir: 'user-changed-directory' }) })
    await secondFlush
  } finally {
    store.disableConfigAutoSave()
    commands.saveConfig = originalSaveConfig
  }
})

test('maintenance cancels an un-flushed pending debounce edit before applying restored config', async () => {
  const store = useStore(createPinia())
  const originalSaveConfig = commands.saveConfig
  let saveCalls = 0
  commands.saveConfig = async (snapshot) => {
    saveCalls++
    return { status: 'ok', data: snapshot }
  }

  try {
    store.applyConfig(makeConfig({ revision: 1, downloadDir: 'dir-1' }), true)
    store.enableConfigAutoSave()

    // Edit schedules the 400ms timer but is not flushed yet
    store.config.downloadDir = 'stale-edit'
    await nextTick()

    const restored = makeConfig({ revision: 2, downloadDir: 'restored-dir' })
    await store.runConfigMaintenance(async () => {
      store.applyConfig(restored, true)
    })
    await store.flushPendingConfigSave()
    await flushMicrotasks()
    assert.equal(saveCalls, 0)
    assert.equal(store.config.downloadDir, 'restored-dir')
  } finally {
    store.disableConfigAutoSave()
    commands.saveConfig = originalSaveConfig
  }
})

test('failed maintenance releases the guard for the next attempt', async () => {
  const store = useStore(createPinia())
  await assert.rejects(store.runConfigMaintenance(async () => {
    throw new Error('write failed')
  }), /write failed/)
  assert.equal(store.configMaintenanceInProgress, false)
  assert.equal(await store.runConfigMaintenance(async () => 'retried'), 'retried')
})
