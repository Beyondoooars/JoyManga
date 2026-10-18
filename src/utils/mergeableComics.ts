import type { Comic } from '../bindings.ts'

export function localChapters(comic: Comic) {
  const ids = new Set<number>()
  return comic.chapterInfos.filter((chapter) => {
    if (!chapter.isDownloaded || !chapter.chapterDownloadDir || ids.has(chapter.chapterId)) return false
    ids.add(chapter.chapterId)
    return true
  })
}

export function isMergeableComic(comic: Comic): boolean {
  if (!comic.isDownloaded || !comic.comicDownloadDir) return false
  const chapters = localChapters(comic)
  if (chapters.length < 2 || chapters.length !== comic.chapterInfos.filter((chapter) => chapter.isDownloaded && chapter.chapterDownloadDir).length) return false
  const paths = chapters.map((chapter) => chapter.chapterDownloadDir!.replace(/\\/g, '/').replace(/\/+$/, ''))
  return new Set(paths).size === chapters.length
}
