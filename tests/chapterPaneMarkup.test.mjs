import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'

const source = await readFile(new URL('../src/panes/ChapterPane.vue', import.meta.url), 'utf8')

test('chapter list does not mount the empty-selector viselect layer', () => {
  assert.doesNotMatch(source, /@viselect\/vue|<SelectionArea|selectables/)
})

test('chapter list removes desktop context menu and supports single-tap reading on downloaded chapters', () => {
  assert.match(source, /<div class="selection-container">/)
  assert.match(source, /<n-checkbox-group v-model:value="checkedIds"/)
  assert.match(source, /@click="downloadChapters"/)
  assert.match(source, /@click="openChapterReader\(item\.chapterId\)"/)
  assert.doesNotMatch(source, /custom-scrollbar|@contextmenu|showCbzModeDialog|@dblclick/)
  assert.doesNotMatch(source, /@click="handleChapterClick/)
})
