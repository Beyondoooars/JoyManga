import { test } from 'node:test'
import assert from 'node:assert/strict'
import { createPinia } from 'pinia'
import { commands, events } from '../src/bindings.ts'
import { useStore } from '../src/store.ts'

function deferred() {
  let resolve
  const promise = new Promise((res) => {
    resolve = res
  })
  return { promise, resolve }
}

function makeSnapshot(comicId, chapterId, state = 'Completed') {
  return {
    taskId: `task-${comicId}-${chapterId}`,
    revision: 1,
    state,
    comic: {
      id: comicId,
      name: `Comic ${comicId}`,
      comicDownloadDir: `/lib/${comicId}`,
      isDownloaded: true,
      chapterInfos: [],
    },
    chapterInfo: {
      chapterId,
      chapterTitle: `Ch ${chapterId}`,
      order: 1,
      isDownloaded: true,
      chapterDownloadDir: `/lib/${comicId}/${chapterId}.cbz`,
    },
    downloadedImgCount: 10,
    totalImgCount: 10,
    errorMessage: null,
    stage: state === 'Completed' ? 'Completed' : 'Downloading',
    failure: null,
    attempts: 1,
    retryable: false,
    bytesDownloaded: 1024,
  }
}

test('cold-start discovery badges survive unrelated live downloads before library loads', () => {
  const store = useStore(createPinia())
  const epoch0 = store.downloadedLibraryResetEpoch

  assert.equal(store.isComicDownloaded(100, true, epoch0), true)
  assert.equal(store.isComicDownloaded(200, false, epoch0), false)

  store.markComicDownloaded(200)
  store.invalidateDownloadedLibrary()

  assert.equal(store.isComicDownloaded(100, true, epoch0), true)
  assert.equal(store.isComicDownloaded(200, false, epoch0), true)
})

test('hydrated completed tasks do not revive deleted badges while live completions mark comic downloaded', async () => {
  const store = useStore(createPinia())
  const originalListen = events.downloadTaskEvent.listen
  const originalGetTasks = commands.getDownloadTasks
  let emitTask

  events.downloadTaskEvent.listen = async (handler) => {
    emitTask = handler
    return () => {}
  }
  commands.getDownloadTasks = async () => [makeSnapshot(300, 3001, 'Completed')]

  try {
    await store.initDownloadListener()
    // Historical Completed task hydrated from disk must NOT mark comic as downloaded
    assert.equal(store.isComicDownloaded(300, false), false)

    // Live completion arrives after hydration
    store.downloadedComicsStale = false
    emitTask({ payload: { event: 'Create', data: makeSnapshot(400, 4001, 'Completed') } })
    assert.equal(store.isComicDownloaded(400, false), true)
    assert.equal(store.downloadedComicsStale, true)
  } finally {
    events.downloadTaskEvent.listen = originalListen
    commands.getDownloadTasks = originalGetTasks
  }
})

test('markComicRemoved immediately updates badges, downloadedComics, and pickedComic markers, and allows re-download', async () => {
  const store = useStore(createPinia())
  const originalGetDownloaded = commands.getDownloadedComics
  const comic100 = {
    id: 100,
    name: 'Book 100',
    is_favorite: false,
    isDownloaded: true,
    comicDownloadDir: '/lib/100.cbz',
    chapterInfos: [{ chapterId: 1001, chapterTitle: 'Ch 1', order: 1, isDownloaded: true, chapterDownloadDir: '/lib/100.cbz' }],
  }
  const comic200 = {
    id: 200,
    name: 'Book 200',
    is_favorite: false,
    isDownloaded: true,
    comicDownloadDir: '/lib/200.cbz',
    chapterInfos: [],
  }
  commands.getDownloadedComics = async () => [comic100, comic200]

  try {
    await store.refreshDownloadedComics()
    store.showComic(structuredClone(comic100))
    assert.equal(store.isComicDownloaded(100, true), true)
    assert.equal(store.downloadedComicIds.has(100), true)

    store.markComicRemoved(100)
    assert.equal(store.isComicDownloaded(100, true), false)
    assert.equal(store.downloadedComicIds.has(100), false)
    assert.deepEqual(store.downloadedComics.map((c) => c.id), [200])
    assert.equal(store.pickedComic.isDownloaded, false)
    assert.equal(store.pickedComic.comicDownloadDir, null)
    assert.equal(store.pickedComic.chapterInfos[0].isDownloaded, false)

    // Re-downloading in the same session restores badge immediately
    store.markComicDownloaded(100)
    assert.equal(store.isComicDownloaded(100, false), true)
    assert.equal(store.downloadedComicIds.has(100), true)
  } finally {
    commands.getDownloadedComics = originalGetDownloaded
  }
})

test('invalidateDownloadedLibrary({ clear: true }) invalidates old epoch discovery badges and pickedComic', () => {
  const store = useStore(createPinia())
  const oldEpoch = store.downloadedLibraryResetEpoch
  store.showComic({
    id: 500,
    name: 'Book 500',
    is_favorite: false,
    isDownloaded: true,
    comicDownloadDir: '/old/500.cbz',
    chapterInfos: [{ chapterId: 5001, chapterTitle: 'Ch 1', order: 1, isDownloaded: true, chapterDownloadDir: '/old/500.cbz' }],
  })
  store.markComicDownloaded(501)
  assert.equal(store.isComicDownloaded(500, true, oldEpoch), true)
  assert.equal(store.isComicDownloaded(501, false, oldEpoch), true)

  store.invalidateDownloadedLibrary({ clear: true })
  assert.notEqual(store.downloadedLibraryResetEpoch, oldEpoch)
  assert.equal(store.isComicDownloaded(500, true, oldEpoch), false)
  assert.equal(store.isComicDownloaded(501, false, oldEpoch), false)
  assert.equal(store.pickedComic.isDownloaded, false)
  assert.equal(store.pickedComic.comicDownloadDir, null)
})

test('stale refreshDownloadedComics scan cannot overwrite newer removal state', async () => {
  const store = useStore(createPinia())
  const originalGetDownloaded = commands.getDownloadedComics
  let pendingScan = deferred()
  commands.getDownloadedComics = () => pendingScan.promise

  try {
    const staleRefresh = store.refreshDownloadedComics()
    store.markComicRemoved(100)
    store.invalidateDownloadedLibrary()

    pendingScan.resolve([{ id: 100, name: 'Stale', chapterInfos: [] }])
    assert.equal(await staleRefresh, false)
    assert.equal(store.isComicDownloaded(100, true), false)

    pendingScan = deferred()
    const freshRefresh = store.refreshDownloadedComics()
    pendingScan.resolve([{ id: 200, name: 'Fresh', chapterInfos: [] }])
    assert.equal(await freshRefresh, true)
    assert.equal(store.isComicDownloaded(100), false)
    assert.equal(store.isComicDownloaded(200), true)
  } finally {
    commands.getDownloadedComics = originalGetDownloaded
  }
})
