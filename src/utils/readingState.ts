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
export const MAX_PROGRESS_ENTRIES = 1_000
export const MAX_BOOKMARKS = 500
export const MAX_TITLE_LENGTH = 256
export const emptyReadingState = (): ReadingState => ({ version: 1, progress: {}, bookmarks: [] })

function validProgress(value: unknown): value is ReadingProgress {
  const item = value as ReadingProgress | null
  return !!item && Number.isSafeInteger(item.comicId) && Number.isSafeInteger(item.chapterId)
    && Number.isSafeInteger(item.page) && Number.isSafeInteger(item.totalPages)
    && item.page >= 1 && item.totalPages >= item.page && typeof item.comicTitle === 'string'
    && typeof item.chapterTitle === 'string' && Number.isFinite(item.updatedAt)
}

function truncateTitle(title: string) {
  return title.length <= MAX_TITLE_LENGTH ? title : Array.from(title).slice(0, MAX_TITLE_LENGTH).join('')
}

function normalizeProgress(item: ReadingProgress): ReadingProgress {
  return { ...item, comicTitle: truncateTitle(item.comicTitle), chapterTitle: truncateTitle(item.chapterTitle) }
}

/**
 * 把本地存储或备份里的原始值整理成可信的阅读状态。
 * 同一本漫画只保留更新时间最新的一条；书签 id 必须与位置一致，重复项丢弃。
 */
export function sanitizeReadingStateValue(value: unknown): ReadingState | null {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null
  const parsed = value as Partial<ReadingState>
  if (parsed.version !== 1) return null
  const latest = new Map<number, ReadingProgress>()
  for (const item of Object.values(parsed.progress ?? {})) {
    if (!validProgress(item)) continue
    const normalized = normalizeProgress(item)
    const current = latest.get(normalized.comicId)
    if (!current || normalized.updatedAt > current.updatedAt) latest.set(normalized.comicId, normalized)
  }
  const progress: ReadingState['progress'] = {}
  for (const item of [...latest.values()]
    .sort((a, b) => b.updatedAt - a.updatedAt).slice(0, MAX_PROGRESS_ENTRIES)) {
    progress[String(item.comicId)] = item
  }

  const seen = new Set<string>()
  const bookmarks: ReadingBookmark[] = []
  for (const item of Array.isArray(parsed.bookmarks) ? parsed.bookmarks : []) {
    if (!validProgress(item) || typeof (item as ReadingBookmark).id !== 'string') continue
    const bookmark = item as ReadingBookmark
    if (bookmark.id !== bookmarkId(bookmark.comicId, bookmark.chapterId, bookmark.page)) continue
    if (seen.has(bookmark.id)) continue
    seen.add(bookmark.id)
    bookmarks.push({ ...normalizeProgress(bookmark), id: bookmark.id })
    if (bookmarks.length >= MAX_BOOKMARKS) break
  }
  return { version: 1, progress, bookmarks }
}

export function parseReadingState(raw: string | null): ReadingState {
  if (!raw) return emptyReadingState()
  try {
    return sanitizeReadingStateValue(JSON.parse(raw)) ?? emptyReadingState()
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
