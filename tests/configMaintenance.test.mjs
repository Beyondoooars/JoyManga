import { test } from 'node:test'
import assert from 'node:assert/strict'
import { createPinia } from 'pinia'
import { nextTick, watch } from 'vue'
import { useStore } from '../src/store.ts'

test('restore waits for an in-flight save and its new config is not auto-saved', async () => {
  const store = useStore(createPinia())
  let finishSave
  const saving = new Promise((resolve) => { finishSave = resolve })
  store.registerConfigSaveWaiter(() => saving)
  let restoreStarted = false
  const autoSaved = []
  const stop = watch(() => store.config, (config) => {
    if (!store.configMaintenanceInProgress) autoSaved.push(config)
  })
  const restored = { username: '', downloadDir: 'restored-directory' }
  const restoring = store.runConfigMaintenance(async () => {
    restoreStarted = true
    store.config = restored
  })
  await nextTick()
  assert.equal(restoreStarted, false)
  await assert.rejects(store.runConfigMaintenance(async () => {}), /配置维护正在进行/)
  finishSave()
  await restoring
  assert.equal(store.config.downloadDir, restored.downloadDir)
  assert.deepEqual(autoSaved, [])
  assert.equal(store.configMaintenanceInProgress, false)
  store.config = { ...restored, downloadDir: 'user-changed-directory' }
  await nextTick()
  assert.equal(autoSaved.length, 1)
  stop()
})

test('failed maintenance releases the guard for the next attempt', async () => {
  const store = useStore(createPinia())
  await assert.rejects(store.runConfigMaintenance(async () => {
    throw new Error('write failed')
  }), /write failed/)
  assert.equal(store.configMaintenanceInProgress, false)
  assert.equal(await store.runConfigMaintenance(async () => 'retried'), 'retried')
})
