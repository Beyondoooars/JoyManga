import { test } from 'node:test'
import assert from 'node:assert/strict'
import {
  detectSearchLanguage,
  matchesLanguageFilter,
  normalizeForLanguage,
} from '../src/utils/searchLanguage.ts'

function detect(name, extra = {}) {
  return detectSearchLanguage({ name, ...extra })
}

function languages(name, extra = {}) {
  return detect(name, extra).languages
}

test('structured category sub is trusted before title heuristics', () => {
  assert.deepEqual(languages('タイトル [English]', { categorySub: { id: 'chinese', title: '汉化' } }), ['zh'])
  assert.deepEqual(languages('English Title', { categorySub: { id: 'japanese', title: '日语' } }), ['ja'])
  assert.deepEqual(languages('标题', { categorySub: { id: null, title: '汉化' } }), ['zh'])
  // 非语言副分类不能参与判定
  assert.deepEqual(languages('Only Title', { categorySub: { id: null, title: '日常' } }), ['en'])
})

test('explicit Chinese markers win over script detection', () => {
  for (const name of [
    '[サークル] タイトル [中国翻訳] [DL版]',
    '[漢化] 夏の思い出',
    '【中文】夏の思い出',
    '[繁體] 夏の思い出',
    '[中国翻訳] [カラー]',
  ]) {
    assert.deepEqual(languages(name), ['zh'], name)
  }
})

test('explicit Japanese, English and Korean markers', () => {
  assert.deepEqual(languages('夏の思い出 [DL版]'), ['ja'])
  assert.deepEqual(languages('夏の思い出 [日本語]'), ['ja'])
  assert.deepEqual(languages('[English] Only Title'), ['en'])
  assert.deepEqual(languages('[한글] 여름'), ['ko'])
  assert.deepEqual(languages('여름의 추억'), ['ko'])
})

test('short language words only count in brackets or with 版 suffix', () => {
  // 正文里的“中文”不算显式标记，最终由脚本统计判定
  assert.deepEqual(languages('中文标题'), ['zh'])
  assert.deepEqual(languages('中文版 タイトル'), ['zh'])
  assert.deepEqual(languages('[中文] タイトル'), ['zh'])
})

test('script statistics classify plain titles', () => {
  assert.deepEqual(languages('山间旅行'), ['zh'])
  assert.deepEqual(languages('だけの物語'), ['ja'])
  assert.deepEqual(languages('Summer Memories'), ['en'])
  // 纯汉字日文标题靠作者假名兜底
  assert.deepEqual(languages('東京', { author: 'あかね' }), ['ja'])
  // 已知取舍：中文标题 + 假名作者会判成日文（可能是日文作品的汉化）
  assert.deepEqual(languages('山间旅行', { author: 'あかね' }), ['ja'])
  assert.deepEqual(languages('山间旅行', { author: '小林' }), ['zh'])
})

test('metadata tokens do not push Japanese titles to Chinese or English', () => {
  for (const name of ['[DL版] 夏の思い出', '[無修正] 夏の思い出', '[同人] 夏の思い出', '[生肉] 夏の思い出']) {
    assert.deepEqual(languages(name), ['ja'], name)
  }
})

test('unknown titles stay unknown', () => {
  for (const name of ['[DL版]', '[無修正]', '[同人]', '[生肉]', 'CG']) {
    const info = detect(name)
    assert.deepEqual(info.languages, [], name)
    assert.equal(info.unknown, true, name)
  }
})

test('full color is an independent attribute', () => {
  const colorOnly = detect('[カラー]')
  assert.equal(colorOnly.fullColor, true)
  assert.deepEqual(colorOnly.languages, [])
  assert.equal(colorOnly.unknown, true)

  const chineseColor = detect('[中国翻訳] [カラー]')
  assert.equal(chineseColor.fullColor, true)
  assert.deepEqual(chineseColor.languages, ['zh'])

  assert.equal(detect('全彩 中文标题').fullColor, true)
  assert.equal(detect('全綵 中文標題').fullColor, true)
  assert.equal(detect('Ｆｕｌｌ　Ｃｏｌｏｒ').fullColor, true)
  assert.equal(detect('普通标题').fullColor, false)
})

test('normalization folds traditional characters and full-width brackets', () => {
  assert.equal(normalizeForLanguage('【繁體】'), '[繁体]')
  assert.equal(normalizeForLanguage('全綵'), '全彩')
  assert.deepEqual(languages('［中文］タイトル'), ['zh'])
  assert.deepEqual(languages('（日本語）タイトル'), ['ja'])
})

test('category fallback only applies when nothing else matches', () => {
  // 纯拉丁标题 + 韩漫分类：脚本先判英文，分类不覆盖
  assert.deepEqual(languages('Only Title', { category: { id: 'hanman', title: '韩漫' } }), ['en'])
  // 无脚本线索时才用主分类兜底
  assert.deepEqual(languages('123', { category: { id: 'hanman', title: '韩漫' } }), ['ko'])
  assert.deepEqual(languages('123', { category: { id: 'english_site', title: '英文站' } }), ['en'])
})

test('matchesLanguageFilter hides unknown languages but keeps them under 全部', () => {
  const unknown = detect('[DL版]')
  assert.equal(matchesLanguageFilter(unknown, 'all'), true)
  assert.equal(matchesLanguageFilter(unknown, 'zh'), false)

  const chinese = detect('山间旅行')
  assert.equal(matchesLanguageFilter(chinese, 'zh'), true)
  assert.equal(matchesLanguageFilter(chinese, 'ja'), false)

  const color = detect('[カラー]')
  assert.equal(matchesLanguageFilter(color, 'fullColor'), true)
  assert.equal(matchesLanguageFilter(chinese, 'fullColor'), false)
})
