/**
 * 简繁双变体搜索的合并流。
 *
 * 两个变体的结果集合只有包含关系时才可能精确换算总数，所以分页不能由 total 驱动。
 * 这里维护：
 * - 每个变体自己的服务端页游标与缓冲；
 * - 整个会话共享的 `seen`，保证跨页、跨变体都不重复；
 * - `ordered`：按展示顺序追加的合并结果，只增不改；
 * - 语言筛选是合并流上的谓词，填页时会继续向后取页，避免「当前页全被筛掉」的空页。
 *
 * 模块不依赖 Vue/Tauri，便于用 node 测试。
 */

import type { Comic, ComicInSearch, SearchSort } from '../bindings'
import type { SearchFilter, SearchLanguageInfo } from './searchLanguage.ts'
import { detectSearchLanguage, matchesLanguageFilter } from './searchLanguage.ts'

export const SEARCH_PAGE_SIZE = 80
/** 每个变体在每个逻辑页最多扫多少个服务端页，防止筛选稀疏时无限请求 */
export const MAX_SCAN_PAGES_PER_VARIANT = 4

export type SearchVariantPage =
  | { kind: 'result'; content: ComicInSearch[]; total: number; hasMore: boolean }
  | { kind: 'comic'; comic: Comic }
  | { kind: 'error'; message: string }

export interface DualSearchResponse {
  original: SearchVariantPage | null
  converted: SearchVariantPage | null
  convertedKeyword: string | null
  warnings: string[]
}

export interface DualSearchRequest {
  originalPage: number | null
  convertedPage: number | null
}

export type DualSearchFetcher = (request: DualSearchRequest) => Promise<DualSearchResponse>

type StreamKey = 'original' | 'converted'
type OrderMode = 'updateAt' | 'alternate'

interface Stream {
  key: StreamKey
  keyword: string
  nextPage: number
  buffer: ComicInSearch[]
  exhausted: boolean
  failed: boolean
  total: number | null
}

function errorMessage(error: unknown): string {
  if (error instanceof Error && error.message !== '') return error.message
  return String(error)
}

export class SearchSession {
  private readonly fetcher: DualSearchFetcher
  private readonly streams: Stream[]
  private readonly seen = new Set<number>()
  private readonly warnings: string[] = []
  private readonly languageCache = new Map<number, SearchLanguageInfo>()
  private ordered: ComicInSearch[] = []
  private convertedKeyword: string | null = null
  private filter: SearchFilter = 'all'
  private orderMode: OrderMode
  private alternateTurn = 0
  private firstFetch = true
  private redirectComic: Comic | null = null
  private visibleCache: { filter: SearchFilter; length: number; items: ComicInSearch[] } | null = null

  constructor(options: { keyword: string; sort: SearchSort; fetcher: DualSearchFetcher }) {
    this.fetcher = options.fetcher
    this.orderMode = options.sort === 'Latest' ? 'updateAt' : 'alternate'
    this.streams = [this.createStream('original', options.keyword, 1)]
  }

  private createStream(key: StreamKey, keyword: string, nextPage: number): Stream {
    return { key, keyword, nextPage, buffer: [], exhausted: false, failed: false, total: null }
  }

  private stream(key: StreamKey): Stream | undefined {
    return this.streams.find((item) => item.key === key)
  }

  private languageInfo(comic: ComicInSearch): SearchLanguageInfo {
    const cached = this.languageCache.get(comic.id)
    if (cached !== undefined) return cached
    const info = detectSearchLanguage({
      name: comic.name,
      author: comic.author,
      category: comic.category,
      categorySub: comic.categorySub,
    })
    this.languageCache.set(comic.id, info)
    return info
  }

  private visibleItems(): ComicInSearch[] {
    const cache = this.visibleCache
    if (cache !== null && cache.filter === this.filter && cache.length === this.ordered.length) {
      return cache.items
    }
    const items =
      this.filter === 'all'
        ? this.ordered
        : this.ordered.filter((comic) => matchesLanguageFilter(this.languageInfo(comic), this.filter))
    this.visibleCache = { filter: this.filter, length: this.ordered.length, items }
    return items
  }

  setFilter(filter: SearchFilter): void {
    this.filter = filter
    this.visibleCache = null
  }

  get convertedKeywordValue(): string | null {
    return this.convertedKeyword
  }

  get warningsList(): string[] {
    return [...this.warnings]
  }

  get redirect(): Comic | null {
    return this.redirectComic
  }

  get visibleCount(): number {
    return this.visibleItems().length
  }

  /** 已加载的合并结果数（未筛选） */
  get loadedCount(): number {
    return this.ordered.length
  }

  get streamCount(): number {
    return this.streams.length
  }

  get exhausted(): boolean {
    if (this.redirectComic !== null) return true
    return this.streams.every(
      (stream) => (stream.exhausted || stream.failed) && stream.buffer.length === 0,
    )
  }

  get fatalError(): string | null {
    if (this.streams.length === 0) return null
    if (this.streams.every((stream) => stream.failed)) {
      return '原文与转换写法都加载失败，请稍后重试'
    }
    return null
  }

  /**
   * 只有一个变体、且未筛选时，服务端 total 是精确的；
   * 双变体的并集大小无法预先知道，返回 null。
   */
  get exactTotal(): number | null {
    if (this.filter !== 'all') return null
    if (this.streams.length !== 1) return null
    const total = this.streams[0].total
    return total !== null && total >= 0 ? total : null
  }

  /** 当前筛选下，语言未知而被隐藏的已加载条目数 */
  get hiddenUnknownCount(): number {
    if (this.filter === 'all' || this.filter === 'fullColor') return 0
    let count = 0
    for (const comic of this.ordered) {
      if (this.languageInfo(comic).unknown) count++
    }
    return count
  }

  pageItems(page: number): ComicInSearch[] {
    const start = (page - 1) * SEARCH_PAGE_SIZE
    return this.visibleItems().slice(start, start + SEARCH_PAGE_SIZE)
  }

  hasMoreAfter(page: number): boolean {
    if (this.redirectComic !== null) return false
    if (this.visibleItems().length > page * SEARCH_PAGE_SIZE) return true
    return !this.exhausted
  }

  /**
   * 继续取页、合并、筛选，直到当前筛选下至少有 `target` 条可见结果，
   * 或两个变体都结束/把本轮的扫描预算用完。
   */
  async ensureVisible(target: number): Promise<void> {
    const budget = new Map<StreamKey, number>()
    while (this.visibleItems().length < target) {
      if (this.redirectComic !== null) return

      const need = this.streams.filter(
        (stream) =>
          !stream.exhausted &&
          !stream.failed &&
          stream.buffer.length === 0 &&
          (budget.get(stream.key) ?? 0) < MAX_SCAN_PAGES_PER_VARIANT,
      )
      if (need.length > 0) {
        await this.fetchRound(need, budget)
        continue
      }

      const stream = this.pickStream()
      if (stream === null) break
      const comic = stream.buffer.shift()
      if (comic !== undefined) this.ordered.push(comic)
    }
  }

  private async fetchRound(need: Stream[], budget: Map<StreamKey, number>): Promise<void> {
    const original = this.stream('original')
    const converted = this.stream('converted')
    const request: DualSearchRequest = this.firstFetch
      ? { originalPage: 1, convertedPage: 1 }
      : {
          originalPage:
            need.some((stream) => stream.key === 'original') && original !== undefined
              ? original.nextPage
              : null,
          convertedPage:
            need.some((stream) => stream.key === 'converted') && converted !== undefined
              ? converted.nextPage
              : null,
        }

    if (request.originalPage !== null) {
      budget.set('original', (budget.get('original') ?? 0) + 1)
    }
    if (request.convertedPage !== null) {
      budget.set('converted', (budget.get('converted') ?? 0) + 1)
    }

    let response: DualSearchResponse
    try {
      response = await this.fetcher(request)
    } catch (error) {
      const message = errorMessage(error)
      if (request.originalPage !== null) this.failStream('original', message, budget)
      if (request.convertedPage !== null) this.failStream('converted', message, budget)
      if (this.firstFetch) {
        this.firstFetch = false
        if (request.convertedPage !== null) this.ensureConvertedStream(null)
      }
      return
    }

    if (this.firstFetch) {
      this.firstFetch = false
      this.ensureConvertedStream(response.convertedKeyword)
    }
    for (const warning of response.warnings) this.pushWarning(warning)

    if (request.originalPage !== null) this.applyPage('original', response.original)
    if (request.convertedPage !== null && this.stream('converted') !== undefined) {
      this.applyPage('converted', response.converted)
    }
  }

  private ensureConvertedStream(convertedKeyword: string | null): void {
    this.convertedKeyword = convertedKeyword
    if (convertedKeyword === null || this.stream('converted') !== undefined) return
    // 第一页已经和原文变体一起取过，游标从第 2 页开始
    this.streams.push(this.createStream('converted', convertedKeyword, 2))
  }

  private applyPage(key: StreamKey, page: SearchVariantPage | null): void {
    const stream = this.stream(key)
    if (stream === undefined) return
    if (page === null) {
      this.failStream(key, '服务端未返回结果')
      return
    }
    if (page.kind === 'error') {
      this.failStream(key, page.message)
      return
    }
    if (page.kind === 'comic') {
      if (key === 'original') {
        // 原文是 JM 号：立即跳转，不再合并
        this.redirectComic = page.comic
        for (const item of this.streams) item.exhausted = true
      } else {
        this.pushWarning('转换写法的搜索结果被识别为漫画编号，已忽略')
        stream.exhausted = true
      }
      return
    }

    if (this.orderMode === 'updateAt' && !this.isMonotonic(page.content)) {
      // 服务端的“最新”页出现置顶/非单调条目时，跨变体无法再按时间归并，
      // 退化为按名次交替，保证两个变体都能出现在前面。
      this.orderMode = 'alternate'
    }

    for (const comic of page.content) {
      if (this.seen.has(comic.id)) continue
      this.seen.add(comic.id)
      stream.buffer.push(comic)
    }
    stream.total = page.total
    stream.nextPage += 1
    if (page.content.length === 0 || !page.hasMore) stream.exhausted = true
  }

  private isMonotonic(content: ComicInSearch[]): boolean {
    for (let index = 1; index < content.length; index++) {
      if (content[index].updateAt > content[index - 1].updateAt) return false
    }
    return true
  }

  private pickStream(): Stream | null {
    const available = this.streams.filter((stream) => stream.buffer.length > 0)
    if (available.length === 0) return null
    if (available.length === 1) return available[0]

    if (this.orderMode === 'updateAt') {
      return available.reduce((best, stream) => {
        const current = stream.buffer[0]
        const champion = best.buffer[0]
        if (current.updateAt > champion.updateAt) return stream
        if (current.updateAt === champion.updateAt && stream.key === 'original') return stream
        return best
      })
    }

    for (let offset = 0; offset < this.streams.length; offset++) {
      const index = (this.alternateTurn + offset) % this.streams.length
      const stream = this.streams[index]
      if (stream.buffer.length > 0) {
        this.alternateTurn = (index + 1) % this.streams.length
        return stream
      }
    }
    return available[0]
  }

  private failStream(
    key: StreamKey,
    message: string,
    budget?: Map<StreamKey, number>,
  ): void {
    const stream = this.stream(key)
    if (stream === undefined) return
    if (budget !== undefined) {
      // 失败也要消耗预算，避免同一轮里对同一个变体反复重试
      budget.set(key, MAX_SCAN_PAGES_PER_VARIANT)
    }
    if (stream.failed) return
    stream.failed = true
    stream.buffer = []
    this.pushWarning(`${key === 'original' ? '原文' : '转换写法'}加载失败：${message}`)
  }

  private pushWarning(message: string): void {
    if (message === '' || this.warnings.includes(message)) return
    this.warnings.push(message)
  }
}
