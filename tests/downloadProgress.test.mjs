import { test } from 'node:test'
import assert from 'node:assert/strict'
import { applyDownloadEvent } from '../src/utils/downloadProgress.ts'

function snapshot(revision, state = 'Downloading', taskId = 'first') {
  return { event: 'Create', data: {
    taskId, revision, state, comic: { id: 1 }, chapterInfo: { chapterId: 12 },
    downloadedImgCount: 2, totalImgCount: 4, errorMessage: null,
  } }
}

test('late startup snapshot cannot roll back a newer completion event', () => {
  const tasks = new Map()
  applyDownloadEvent(tasks, snapshot(10))
  applyDownloadEvent(tasks, { event: 'Update', data: {
    chapterId: 12, revision: 12, state: 'Completed',
    downloadedImgCount: 4, totalImgCount: 4, errorMessage: null,
  } })
  applyDownloadEvent(tasks, snapshot(11))
  assert.equal(tasks.get(12).state, 'Completed')
  assert.equal(tasks.get(12).percentage, 100)
})

test('retry resets the previous failure and ignores stale events', () => {
  const tasks = new Map()
  const failed = snapshot(15, 'Failed')
  failed.data.errorMessage = '第 3 张图片读取超时'
  applyDownloadEvent(tasks, failed)
  assert.equal(tasks.get(12).errorMessage, failed.data.errorMessage)
  applyDownloadEvent(tasks, snapshot(16, 'Pending', 'retry'))
  applyDownloadEvent(tasks, failed)
  assert.equal(tasks.get(12).taskId, 'retry')
  assert.equal(tasks.get(12).errorMessage, null)
  assert.equal(tasks.get(12).state, 'Pending')
})

test('restored paused tasks have a correct indicator before the first live event', () => {
  const tasks = new Map()
  applyDownloadEvent(tasks, snapshot(1, 'Paused'))
  assert.equal(tasks.get(12).indicator, '已暂停 2/4')
  assert.equal(tasks.get(12).percentage, 50)
})
