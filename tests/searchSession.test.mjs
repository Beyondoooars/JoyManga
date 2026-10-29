import { test } from 'node:test'
import assert from 'node:assert/strict'
import { SearchSession, SEARCH_PAGE_SIZE } from '../src/utils/searchSession.ts'

function comic(id, options = {}) {
  const {
    updateAt = id,
    name = '山间旅行',
    isFavorite = false,
    categorySub = { id: null, title: null },
  } = options
  return {
    id,
    author: '小林',
    name,
    image: '',
    category: { id: null, title: null },
    categorySub,
    liked: false,
    isFavorite,
    updateAt,
    isDownloaded: false,
    comicDownloadDir: '',
  }
}

function range(from, to) {
  const items = []
  for (let id = from; id <= to; id++) items.push(comic(id))
  return items
}

function resultPage(content, hasMore, total = content.length) {
  return { kind: 'result', content, total, hasMore }
}

function ids(items) {
  return items.map((item) => item.id)
}

function singleVariantFetcher(pages, calls) {
  return async (request) => {
    calls.push(request)
    return {
      original: pages[request.originalPage - 1] ?? resultPage([], false),
      converted: null,
      convertedKeyword: null,
      warnings: [],
    }
  }
}

test('single variant fills logical pages and knows the exact total', async () => {
  const calls = []
  const pages = [resultPage(range(1, 80), true, 100), resultPage(range(81, 100), false, 100)]
  const session = new SearchSession({
    keyword: '漫画',
    sort: 'Latest',
    fetcher: singleVariantFetcher(pages, calls),
  })

  await session.ensureVisible(SEARCH_PAGE_SIZE)
  assert.deepEqual(ids(session.pageItems(1)), ids(range(1, 80)))
  assert.equal(session.hasMoreAfter(1), true)
  assert.equal(session.exactTotal, 100)
  assert.deepEqual(calls[0], { originalPage: 1, convertedPage: 1 })

  await session.ensureVisible(SEARCH_PAGE_SIZE * 2)
  assert.deepEqual(ids(session.pageItems(2)), ids(range(81, 100)))
  assert.equal(session.hasMoreAfter(2), false)
  assert.equal(session.exhausted, true)
  assert.deepEqual(calls[1], { originalPage: 2, convertedPage: null })
})

test('dual variants alternate and never repeat across pages', async () => {
  const calls = []
  const originalPages = [range(1, 80), range(81, 160)]
  const convertedPages = [range(1001, 1080), range(1081, 1160)]
  const fetcher = async (request) => {
    calls.push(request)
    return {
      original:
        request.originalPage === null ? null : resultPage(originalPages[request.originalPage - 1], request.originalPage < 2, 160),
      converted:
        request.convertedPage === null ? null : resultPage(convertedPages[request.convertedPage - 1], request.convertedPage < 2, 160),
      convertedKeyword: '漫畫',
      warnings: [],
    }
  }
  const session = new SearchSession({ keyword: '漫画', sort: 'View', fetcher })

  await session.ensureVisible(80)
  // 非 Latest 排序没有跨变体指标，按名次交替
  assert.equal(ids(session.pageItems(1))[0], 1)
  assert.equal(ids(session.pageItems(1))[1], 1001)
  assert.equal(session.exactTotal, null)

  await session.ensureVisible(160)
  const all = [...session.pageItems(1), ...session.pageItems(2)]
  assert.equal(all.length, 160)
  assert.equal(new Set(ids(all)).size, 160)
})

test('overlapping variants dedupe across pages and keep the original favorite flag', async () => {
  const fetcher = async (request) => {
    const original =
      request.originalPage === null
        ? null
        : request.originalPage === 1
          ? resultPage(
              range(1, 80).map((item) => (item.id === 70 ? { ...item, isFavorite: true } : item)),
              true,
              160,
            )
          : resultPage(range(81, 160), false, 160)
    const converted =
      request.convertedPage === null
        ? null
        : request.convertedPage === 1
          ? resultPage(range(61, 140), true, 220)
          : resultPage(range(141, 220), false, 220)
    return { original, converted, convertedKeyword: '漫畫', warnings: [] }
  }
  const session = new SearchSession({ keyword: '漫画', sort: 'View', fetcher })

  await session.ensureVisible(160)
  const page1 = session.pageItems(1)
  const page2 = session.pageItems(2)
  assert.equal(page1.length, 80)
  assert.equal(page2.length, 80)
  assert.equal(session.visibleCount, 160)
  assert.equal(new Set(ids([...page1, ...page2])).size, 160)
  assert.equal([...page1, ...page2].find((item) => item.id === 70)?.isFavorite, true)
  // 两个变体的第 2 页还有未消费的条目
  assert.equal(session.hasMoreAfter(2), true)

  await session.ensureVisible(240)
  assert.equal(session.visibleCount, 220)
  assert.equal(session.pageItems(3).length, 60)
  assert.equal(session.exhausted, true)
})

test('Latest merges both variants by updateAt', async () => {
  const fetcher = async (request) => ({
    original:
      request.originalPage === null
        ? null
        : resultPage([comic(1, { updateAt: 100 }), comic(3, { updateAt: 80 })], false),
    converted:
      request.convertedPage === null
        ? null
        : resultPage([comic(2, { updateAt: 90 }), comic(4, { updateAt: 70 })], false),
    convertedKeyword: '漫畫',
    warnings: [],
  })
  const session = new SearchSession({ keyword: '漫画', sort: 'Latest', fetcher })

  await session.ensureVisible(4)
  assert.deepEqual(ids(session.pageItems(1)), [1, 2, 3, 4])
})

test('non-monotonic updateAt falls back to alternating order', async () => {
  const fetcher = async (request) => ({
    original:
      request.originalPage === null
        ? null
        : resultPage([comic(1, { updateAt: 10 }), comic(3, { updateAt: 50 }), comic(5, { updateAt: 40 })], false),
    converted:
      request.convertedPage === null
        ? null
        : resultPage([comic(2, { updateAt: 45 }), comic(4, { updateAt: 30 })], false),
    convertedKeyword: '漫畫',
    warnings: [],
  })
  const session = new SearchSession({ keyword: '漫画', sort: 'Latest', fetcher })

  await session.ensureVisible(5)
  // 原文页内 updateAt 非降序（10, 50, 40）-> 退化为交替
  assert.deepEqual(ids(session.pageItems(1)), [1, 2, 3, 4, 5])
})

test('language filter keeps scanning until the page is filled', async () => {
  const calls = []
  const pages = [
    resultPage(range(1, 80), true, 160),
    resultPage(range(81, 160).map((item) => ({ ...item, name: '[한글] 여름' })), false, 160),
  ]
  const session = new SearchSession({
    keyword: '漫画',
    sort: 'Latest',
    fetcher: singleVariantFetcher(pages, calls),
  })
  session.setFilter('ko')

  await session.ensureVisible(80)
  assert.equal(session.visibleCount, 80)
  assert.deepEqual(calls.map((call) => call.originalPage), [1, 2])
  assert.equal(session.pageItems(1)[0].id, 81)
})

test('filter change reuses already loaded results without refetching', async () => {
  const calls = []
  const pages = [
    resultPage(range(1, 80), true, 160),
    resultPage(range(81, 160).map((item) => ({ ...item, name: '[한글] 여름' })), false, 160),
  ]
  const session = new SearchSession({
    keyword: '漫画',
    sort: 'Latest',
    fetcher: singleVariantFetcher(pages, calls),
  })

  await session.ensureVisible(160)
  const callsBefore = calls.length
  session.setFilter('ko')
  await session.ensureVisible(80)

  assert.equal(calls.length, callsBefore)
  assert.equal(session.visibleCount, 80)
})

test('filter stops at the scan budget without pretending the search ended', async () => {
  const calls = []
  const pages = Array.from({ length: 10 }, (_, index) => resultPage(range(index * 80 + 1, index * 80 + 80), true, 800))
  const session = new SearchSession({
    keyword: '漫画',
    sort: 'Latest',
    fetcher: singleVariantFetcher(pages, calls),
  })
  session.setFilter('ko')

  await session.ensureVisible(80)
  assert.equal(calls.length, 4)
  assert.equal(session.visibleCount, 0)
  assert.equal(session.exhausted, false)
  assert.equal(session.hasMoreAfter(1), true)
})

test('one variant failure degrades with a warning', async () => {
  const fetcher = async (request) => ({
    original: request.originalPage === null ? null : resultPage(range(1, 80), false),
    converted:
      request.convertedPage === null
        ? null
        : { kind: 'error', message: '模拟失败' },
    convertedKeyword: '漫畫',
    warnings: [],
  })
  const session = new SearchSession({ keyword: '漫画', sort: 'Latest', fetcher })

  await session.ensureVisible(80)
  assert.equal(session.visibleCount, 80)
  assert.equal(session.fatalError, null)
  assert.match(session.warningsList.join(' '), /转换写法加载失败/)
})

test('both variants failing is fatal', async () => {
  const fetcher = async (request) => ({
    original: request.originalPage === null ? null : { kind: 'error', message: '原文失败' },
    converted: request.convertedPage === null ? null : { kind: 'error', message: '转换失败' },
    convertedKeyword: '漫畫',
    warnings: [],
  })
  const session = new SearchSession({ keyword: '漫画', sort: 'Latest', fetcher })

  await session.ensureVisible(80)
  assert.equal(session.visibleCount, 0)
  assert.notEqual(session.fatalError, null)
  assert.equal(session.exhausted, true)
})

test('JM number redirect stops merging and exposes the comic', async () => {
  const redirectComic = { id: 123456 }
  const fetcher = async () => ({
    original: { kind: 'comic', comic: redirectComic },
    converted: null,
    convertedKeyword: null,
    warnings: [],
  })
  const session = new SearchSession({ keyword: '123456', sort: 'Latest', fetcher })

  await session.ensureVisible(80)
  assert.equal(session.redirect, redirectComic)
  assert.equal(session.visibleCount, 0)
  assert.equal(session.hasMoreAfter(1), false)
})

test('hidden unknown count reflects language filtering', async () => {
  const content = [
    comic(1, { name: '山间旅行' }),
    comic(2, { name: '[DL版]' }),
    comic(3, { name: '여름의 추억' }),
  ]
  const fetcher = singleVariantFetcher([resultPage(content, false)], [])
  const session = new SearchSession({ keyword: '测试', sort: 'Latest', fetcher })

  await session.ensureVisible(80)
  assert.equal(session.hiddenUnknownCount, 0)
  session.setFilter('ko')
  assert.equal(session.hiddenUnknownCount, 1)
  assert.equal(session.visibleCount, 1)
})
