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
