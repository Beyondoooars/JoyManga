import { test } from 'node:test'
import assert from 'node:assert/strict'
import { createPinia } from 'pinia'
import { useStore } from '../src/store.ts'
import { READING_STORAGE_KEY } from '../src/utils/readingState.ts'

function memoryStorage() {
  const values = new Map()
  return {
    values,
    storage: {
      getItem: (key) => values.get(key) ?? null,
      setItem: (key, value) => values.set(key, String(value)),
    },
  }
}

const snapshot = {
  version: 1,
  progress: {
    12: { comicId: 12, chapterId: 25, comicTitle: '测试漫画', chapterTitle: '第一章', page: 8, totalPages: 20, updatedAt: 123 },
  },
  bookmarks: [],
}

test('importing a backup snapshot writes storage before replacing state', () => {
  const { values, storage } = memoryStorage()
  globalThis.localStorage = storage
  try {
    const store = useStore(createPinia())
    assert.equal(store.importReadingStateSnapshot(snapshot), 'applied')
    assert.equal(store.readingState.progress['12'].page, 8)
    assert.equal(JSON.parse(values.get(READING_STORAGE_KEY)).progress['12'].page, 8)
    assert.equal(store.importReadingStateSnapshot({ version: 2 }), 'invalid')
  } finally {
    delete globalThis.localStorage
  }
})

test('a failed storage write keeps the previous reading state', () => {
  const { storage } = memoryStorage()
  globalThis.localStorage = storage
  try {
    const store = useStore(createPinia())
    storage.setItem = () => { throw new Error('quota exceeded') }
    assert.equal(store.importReadingStateSnapshot(snapshot), 'write-failed')
    assert.deepEqual(store.readingState.progress, {})
    assert.deepEqual(store.readingState.bookmarks, [])
    assert.match(store.readingPersistenceError, /保存失败/)
  } finally {
    delete globalThis.localStorage
  }
})
