import { test } from 'node:test'
import assert from 'node:assert/strict'
import { filterLibrary } from '../src/utils/librarySearch.ts'

const comics = [
  { id: 12345, name: '山间旅行', author: ['小林'], tags: ['风景', 'Adventure'] },
  { id: 67890, name: '城市日记', author: ['小张'], tags: ['生活'] },
]

test('library searches Chinese title, author, tags and numeric IDs', () => {
  for (const query of ['旅行', '小林', '12345', 'adventure']) {
    assert.deepEqual(filterLibrary(comics, query), [comics[0]])
  }
})

test('multiple keywords match across fields and an empty search preserves ordering', () => {
  assert.deepEqual(filterLibrary(comics, '  小林   ADVENTURE '), [comics[0]])
  assert.deepEqual(filterLibrary(comics, '小林 生活'), [])
  assert.equal(filterLibrary(comics, '   '), comics)
})
