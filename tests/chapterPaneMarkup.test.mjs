import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'

const source = await readFile(new URL('../src/panes/ChapterPane.vue', import.meta.url), 'utf8')

test('chapter list does not mount the empty-selector viselect layer', () => {
  assert.doesNotMatch(source, /@viselect\/vue|<SelectionArea|selectables/)
})

test('chapter list keeps scrolling, context menu, checkboxes, and downloads', () => {
  assert.match(source, /<div class="selection-container custom-scrollbar" @contextmenu="onContextMenu">/)
  assert.match(source, /<n-checkbox-group v-model:value="checkedIds"/)
  assert.match(source, /@click="downloadChapters"/)
  assert.match(source, /@dblclick="handleChapterDoubleClick/)
})
