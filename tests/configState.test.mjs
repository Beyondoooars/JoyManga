import { test } from 'node:test'
import assert from 'node:assert/strict'
import { mergeServerConfig, hasConfigChanges } from '../src/utils/configState.ts'

test('new server route reaches the UI while an unrelated local edit survives', () => {
  const base = { revision: 1, schemaVersion: 1, apiDomainMode: 'Domain2', imgConcurrency: 4 }
  const local = { ...base, imgConcurrency: 7 }
  const server = { ...base, revision: 2, apiDomainMode: 'Domain3' }
  const merged = mergeServerConfig(local, base, server)
  assert.equal(merged.apiDomainMode, 'Domain3')
  assert.equal(merged.imgConcurrency, 7)
  assert.equal(merged.revision, 2)
  assert.equal(hasConfigChanges(merged, server), true)
})

test('acknowledging a persisted config does not create an automatic save loop', () => {
  const base = { revision: 1, schemaVersion: 1, imgConcurrency: 4 }
  const local = { ...base, imgConcurrency: 7 }
  const server = { ...local, revision: 2 }
  assert.equal(hasConfigChanges(mergeServerConfig(local, base, server), server), false)
})

test('library and export remain unified while an unsaved library edit survives a server event', () => {
  const base = { revision: 1, downloadDir: 'old-library', exportDir: 'old-library' }
  const local = { ...base, downloadDir: 'new-library' }
  const server = { ...base, revision: 2, apiDomainMode: 'Domain3' }
  const merged = mergeServerConfig(local, base, server)
  assert.equal(merged.downloadDir, 'new-library')
  assert.equal(merged.exportDir, 'new-library')
})

test('server-owned fields never make the local config look dirty', () => {
  // 后端会强制 exportDir = downloadDir，并在 Android 初始化时置位 androidStorageInitialized。
  // 这些派生字段若参与脏检查，会在每次配置事件后触发一次无意义的保存。
  const base = { revision: 1, schemaVersion: 1, downloadDir: 'lib', exportDir: 'lib', androidStorageInitialized: false }
  const server = { ...base, revision: 2, androidStorageInitialized: true }
  assert.equal(hasConfigChanges(server, base), false)

  // exportDir 与 downloadDir 不一致时也不应被视为用户改动
  const drifted = { ...base, exportDir: 'stale-export' }
  assert.equal(hasConfigChanges(drifted, base), false)
})
