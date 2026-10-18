import { test } from 'node:test'
import assert from 'node:assert/strict'
import { isMergeableComic, localChapters } from '../src/utils/mergeableComics.ts'

const chapter = (id, path, downloaded = true) => ({ chapterId: id, order: id, chapterTitle: `第${id}话`, isDownloaded: downloaded, chapterDownloadDir: path })
const comic = (chapters) => ({ id: 1, isDownloaded: true, comicDownloadDir: 'library/book', chapterInfos: chapters })

test('only independently downloaded chapters of a local comic expose merge', () => {
  assert.equal(isMergeableComic(comic([chapter(1, 'a.cbz')])), false)
  assert.equal(isMergeableComic(comic([chapter(1, 'a.cbz'), chapter(2, null, false)])), false)
  assert.equal(isMergeableComic(comic([chapter(1, 'a.cbz'), chapter(2, 'b.cbz'), chapter(3, null, false)])), true)
  assert.equal(isMergeableComic({ ...comic([chapter(1, 'a.cbz'), chapter(2, 'b.cbz')]), isDownloaded: false }), false)
})

test('one archive with multiple chapters and repeated chapter records cannot be merged again', () => {
  assert.equal(isMergeableComic(comic([chapter(1, 'all.cbz'), chapter(2, 'all.cbz')])), false)
  assert.equal(isMergeableComic(comic([chapter(1, 'library\\all.cbz'), chapter(2, 'library/all.cbz')])), false)
  const repeated = comic([chapter(1, 'a.cbz'), chapter(1, 'b.cbz')])
  assert.equal(localChapters(repeated).length, 1)
  assert.equal(isMergeableComic(repeated), false)
})
