import { test } from 'node:test'
import assert from 'node:assert/strict'
import { emptyReadingState, MAX_TITLE_LENGTH, parseReadingState, recordProgress,
  sanitizeReadingStateValue, toggleBookmark } from '../src/utils/readingState.ts'

const progress = { comicId: 12, chapterId: 25, comicTitle: '测试漫画', chapterTitle: '第一章', page: 8, totalPages: 20, updatedAt: 123 }
test('reading progress and bookmarks survive a storage round trip', () => {
  const state = toggleBookmark(recordProgress(emptyReadingState(), progress), progress)
  const restored = parseReadingState(JSON.stringify(state))
  assert.equal(restored.progress['12'].page, 8)
  assert.equal(restored.bookmarks[0].id, '12:25:8')
  assert.equal(toggleBookmark(restored, progress).bookmarks.length, 0)
  assert.equal(recordProgress(restored, { ...progress, chapterId: 26, page: 2 }).progress['12'].chapterId, 26)
})
test('malformed records and impossible page positions are ignored', () => {
  assert.deepEqual(parseReadingState('{broken'), emptyReadingState())
  assert.deepEqual(parseReadingState(JSON.stringify({ version: 2 })), emptyReadingState())
  const state = recordProgress(emptyReadingState(), { ...progress, page: 21 })
  assert.deepEqual(state.progress, {})
  const parsed = parseReadingState(JSON.stringify({ version: 1, progress: { a: progress, b: { ...progress, comicId: 13, page: 0 } }, bookmarks: [null] }))
  assert.deepEqual(Object.keys(parsed.progress), ['12'])
  assert.deepEqual(parsed.bookmarks, [])
})
test('old reading history is bounded without losing the latest book', () => {
  let state = emptyReadingState()
  for (let id = 1; id <= 1005; id++) state = recordProgress(state, { ...progress, comicId: id, updatedAt: id })
  assert.equal(Object.keys(state.progress).length, 1000)
  assert.equal(state.progress['1'], undefined)
  assert.equal(state.progress['1005'].page, 8)
})
test('duplicate progress keeps the most recently updated entry', () => {
  const state = sanitizeReadingStateValue({
    version: 1,
    progress: {
      older: { ...progress, page: 3, updatedAt: 100 },
      newer: { ...progress, page: 9, updatedAt: 200 },
    },
    bookmarks: [],
  })
  assert.equal(Object.keys(state.progress).length, 1)
  assert.equal(state.progress['12'].page, 9)
})
test('bookmarks require a matching id and long titles are truncated', () => {
  const longTitle = 'x'.repeat(MAX_TITLE_LENGTH + 20)
  const valid = toggleBookmark(emptyReadingState(), { ...progress, comicTitle: longTitle }).bookmarks[0]
  const state = sanitizeReadingStateValue({
    version: 1,
    progress: {},
    bookmarks: [{ ...valid, id: '12:25:999' }, valid, valid],
  })
  assert.equal(state.bookmarks.length, 1)
  assert.equal(state.bookmarks[0].id, '12:25:8')
  assert.equal(state.bookmarks[0].comicTitle.length, MAX_TITLE_LENGTH)
})
test('sanitizer rejects values that are not reading states', () => {
  assert.equal(sanitizeReadingStateValue(null), null)
  assert.equal(sanitizeReadingStateValue([]), null)
  assert.equal(sanitizeReadingStateValue({ version: 2 }), null)
})
