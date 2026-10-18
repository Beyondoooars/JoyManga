import { test } from 'node:test'
import assert from 'node:assert/strict'
import { ImageByteCache, imageDataToBlob } from '../src/utils/imageCache.ts'

test('memory budget evicts least recently used pages while preserving visible pages', () => {
  const revoked = []
  const cache = new ImageByteCache(10, (url) => revoked.push(url))
  cache.put('first', 'url-1', 4, new Set())
  cache.put('second', 'url-2', 4, new Set())
  cache.touch('first')
  assert.deepEqual(cache.put('third', 'url-3', 4, new Set(['third'])), ['second'])
  assert.equal(cache.byteSize, 8)
  assert.deepEqual(revoked, ['url-2'])
  cache.clear()
  assert.equal(cache.byteSize, 0)
  assert.equal(revoked.length, 3)
})
test('replacing the same page releases its old object URL', () => {
  const revoked = []
  const cache = new ImageByteCache(10, (url) => revoked.push(url))
  cache.put('page', 'old-url', 4, new Set())
  cache.put('page', 'new-url', 5, new Set(['page']))
  assert.deepEqual(revoked, ['old-url'])
  assert.equal(cache.byteSize, 5)
  assert.equal(cache.touch('page'), 'new-url')
})
test('base64 image payloads become typed blobs and malformed data is rejected', async () => {
  const blob = imageDataToBlob('data:image/png;base64,aGVsbG8=')
  assert.equal(blob.type, 'image/png')
  assert.equal(blob.size, 5)
  assert.equal(await blob.text(), 'hello')
  assert.throws(() => imageDataToBlob('https://example.invalid/image.png'), /图片数据格式/)
})

test('decoded image cost evicts older pages without revoking the visible URL', () => {
  const revoked = []
  const cache = new ImageByteCache(10, (url) => revoked.push(url))
  cache.put('old', 'old-url', 2, new Set())
  cache.put('visible', 'visible-url', 2, new Set())
  assert.deepEqual(cache.resize('visible', 9, new Set(['visible'])), ['old'])
  assert.deepEqual(revoked, ['old-url'])
  assert.equal(cache.touch('visible'), 'visible-url')
  assert.equal(cache.byteSize, 9)
})
