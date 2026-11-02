import { test } from 'node:test'
import assert from 'node:assert/strict'
import { createPinia } from 'pinia'
import { commands } from '../src/bindings.ts'
import { useStore } from '../src/store.ts'

function deferred() {
  let resolve
  const promise = new Promise((res) => {
    resolve = res
  })
  return { promise, resolve }
}

const comic = { id: 1000, name: '测试漫画', is_favorite: false, chapterInfos: [] }

test('opening transfers invalidates an in-flight comic detail request', async () => {
  const store = useStore(createPinia())
  const pending = deferred()
  const originalGetComic = commands.getComic
  commands.getComic = () => pending.promise
  try {
    const opening = store.openComic(comic.id)
    store.openTransfers()
    pending.resolve({ status: 'ok', data: comic })
    assert.deepEqual(await opening, { status: 'stale' })
    assert.equal(store.chapterDetailShowing, false)
    assert.equal(store.currentTabName, 'downloaded')
  } finally {
    commands.getComic = originalGetComic
  }
})

test('keyword search invalidates an in-flight comic detail request', async () => {
  const store = useStore(createPinia())
  const pending = deferred()
  const originalGetComic = commands.getComic
  commands.getComic = () => pending.promise
  try {
    const opening = store.openComic(comic.id)
    store.searchKeyword('  作者  ')
    pending.resolve({ status: 'ok', data: comic })
    assert.deepEqual(await opening, { status: 'stale' })
    assert.equal(store.chapterDetailShowing, false)
    assert.equal(store.currentTabName, 'search')
    assert.equal(store.pendingSearchKeyword, '作者')
  } finally {
    commands.getComic = originalGetComic
  }
})

test('blank keyword leaves the current detail request untouched', async () => {
  const store = useStore(createPinia())
  const pending = deferred()
  const originalGetComic = commands.getComic
  commands.getComic = () => pending.promise
  try {
    const opening = store.openComic(comic.id)
    store.searchKeyword('   ')
    pending.resolve({ status: 'ok', data: comic })
    const result = await opening
    assert.equal(result.status, 'ok')
    assert.equal(store.chapterDetailShowing, true)
    assert.equal(store.pickedComic.id, comic.id)
  } finally {
    commands.getComic = originalGetComic
  }
})
