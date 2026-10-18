export interface ReadingProgress {
  comicId: number; chapterId: number; comicTitle: string; chapterTitle: string
  page: number; totalPages: number; updatedAt: number
}
export interface ReadingBookmark extends ReadingProgress { id: string }
export interface ReadingState {
  version: 1
  progress: Record<string, ReadingProgress>
  bookmarks: ReadingBookmark[]
}
export const READING_STORAGE_KEY = 'joymanga_reading_state_v1'
export const emptyReadingState = (): ReadingState => ({ version: 1, progress: {}, bookmarks: [] })

function validProgress(value: unknown): value is ReadingProgress {
  const item = value as ReadingProgress | null
  return !!item && Number.isSafeInteger(item.comicId) && Number.isSafeInteger(item.chapterId)
    && Number.isSafeInteger(item.page) && Number.isSafeInteger(item.totalPages)
    && item.page >= 1 && item.totalPages >= item.page && typeof item.comicTitle === 'string'
    && typeof item.chapterTitle === 'string' && Number.isFinite(item.updatedAt)
}

export function parseReadingState(raw: string | null): ReadingState {
  if (!raw) return emptyReadingState()
  try {
    const parsed = JSON.parse(raw)
    if (parsed.version !== 1) return emptyReadingState()
    const progress: ReadingState['progress'] = {}
    for (const item of Object.values(parsed.progress ?? {}).filter(validProgress)
      .sort((a, b) => b.updatedAt - a.updatedAt).slice(0, 1_000)) {
      progress[String(item.comicId)] = item
    }
    const bookmarks = (Array.isArray(parsed.bookmarks) ? parsed.bookmarks : [])
      .filter((item: unknown): item is ReadingBookmark => validProgress(item)
        && typeof (item as ReadingBookmark).id === 'string').slice(0, 500)
    return { version: 1, progress, bookmarks }
  } catch {
    return emptyReadingState()
  }
}

export function bookmarkId(comicId: number, chapterId: number, page: number) {
  return `${comicId}:${chapterId}:${page}`
}

export function recordProgress(state: ReadingState, progress: ReadingProgress): ReadingState {
  if (!validProgress(progress)) return state
  const entries = Object.values({ ...state.progress, [String(progress.comicId)]: progress })
    .sort((a, b) => b.updatedAt - a.updatedAt).slice(0, 1_000)
  return { ...state, progress: Object.fromEntries(entries.map((item) => [String(item.comicId), item])) }
}

export function toggleBookmark(state: ReadingState, progress: ReadingProgress): ReadingState {
  if (!validProgress(progress)) return state
  const id = bookmarkId(progress.comicId, progress.chapterId, progress.page)
  const bookmarks = state.bookmarks.some((item) => item.id === id)
    ? state.bookmarks.filter((item) => item.id !== id)
    : [{ ...progress, id }, ...state.bookmarks].slice(0, 500)
  return { ...state, bookmarks }
}
