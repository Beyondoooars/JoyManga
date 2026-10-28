import { test } from 'node:test'
import assert from 'node:assert/strict'
import { applyDownloadEvent, describeFailure, failureKindLabels } from '../src/utils/downloadProgress.ts'

function snapshot(revision, state = 'Downloading', taskId = 'first', overrides = {}) {
  return { event: 'Create', data: {
    taskId, revision, state, comic: { id: 1 }, chapterInfo: { chapterId: 12 },
    downloadedImgCount: 2, totalImgCount: 4, errorMessage: null,
    stage: 'downloading', failure: null, attempts: 1, imageFailures: [],
    ...overrides,
  } }
}

test('late startup snapshot cannot roll back a newer completion event', () => {
  const tasks = new Map()
  applyDownloadEvent(tasks, snapshot(10))
  applyDownloadEvent(tasks, { event: 'Update', data: {
    chapterId: 12, revision: 12, state: 'Completed', stage: 'done',
    downloadedImgCount: 4, totalImgCount: 4, errorMessage: null,
    failure: null, attempts: 1, imageFailures: [],
  } })
  applyDownloadEvent(tasks, snapshot(11))
  assert.equal(tasks.get(12).state, 'Completed')
  assert.equal(tasks.get(12).percentage, 100)
})

test('retry resets the previous failure and ignores stale events', () => {
  const tasks = new Map()
  const failed = snapshot(15, 'Failed', 'first', {
    stage: 'downloading',
    failure: {
      kind: 'network', message: '第 3 张图片读取超时', retryable: true,
      stage: 'downloading', attempts: 1, line: 'img.example', at: 1,
    },
    attempts: 1,
  })
  failed.data.errorMessage = '第 3 张图片读取超时'
  applyDownloadEvent(tasks, failed)
  assert.equal(tasks.get(12).errorMessage, failed.data.errorMessage)
  assert.equal(tasks.get(12).indicator, '下载失败 · 网络问题')
  applyDownloadEvent(tasks, snapshot(16, 'Pending', 'retry', { stage: 'queued', attempts: 1 }))
  applyDownloadEvent(tasks, failed)
  assert.equal(tasks.get(12).taskId, 'retry')
  assert.equal(tasks.get(12).errorMessage, null)
  assert.equal(tasks.get(12).failure, null)
  assert.equal(tasks.get(12).state, 'Pending')
})

test('restored paused tasks have a correct indicator before the first live event', () => {
  const tasks = new Map()
  applyDownloadEvent(tasks, snapshot(1, 'Paused'))
  assert.equal(tasks.get(12).indicator, '已暂停 2/4')
  assert.equal(tasks.get(12).percentage, 50)
})

test('stage overrides the indicator only while downloading', () => {
  const tasks = new Map()
  applyDownloadEvent(tasks, snapshot(1, 'Downloading', 'meta', { stage: 'fetchMeta', downloadedImgCount: 0, totalImgCount: 0 }))
  assert.equal(tasks.get(12).indicator, '获取章节信息')

  applyDownloadEvent(tasks, snapshot(2, 'Downloading', 'verify', { stage: 'verifying' }))
  assert.equal(tasks.get(12).indicator, '校验中 2/4')

  applyDownloadEvent(tasks, snapshot(3, 'Paused', 'paused', { stage: 'verifying' }))
  assert.equal(tasks.get(12).indicator, '已暂停 2/4')
})

test('failure summary carries kind, stage, attempts and line', () => {
  const failure = {
    kind: 'storage', message: 'no space left on device', retryable: false,
    stage: 'archiving', attempts: 3, line: 'cdn.example', at: 1,
  }
  assert.equal(describeFailure(failure), '存储失败 · 归档阶段 · 第 3 次尝试 · cdn.example')
  assert.equal(describeFailure(null), '')
  assert.equal(failureKindLabels.upstream, '站点异常')
})
