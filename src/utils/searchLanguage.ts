/**
 * 搜索结果的语言本地判定。
 *
 * JM 的搜索接口没有语言字段，搜索结果项也只有标题、作者与两级分类，所以只能：
 * 1. 优先用结构化的副分类（`category_sub.id` 为 `chinese`/`japanese`）；
 * 2. 其次看标题里的显式语言标记（`[汉化]`、`[中国翻訳]`、`[English]`）；
 * 3. 再用字符脚本统计兜底（假名→日文、谚文→韩文、汉字→中文、拉丁字母→英文）；
 * 4. 最后才用主分类兜底（韩漫/美漫/英文站）。
 *
 * 判定是本地启发式，允许误判；界面必须标注「本地判断」，并保留「全部」回退。
 */

export type LanguageTag = 'zh' | 'ja' | 'en' | 'ko'
export type SearchFilter = 'all' | LanguageTag | 'fullColor'

export interface SearchLanguageInfo {
  /** 固定按 zh, ja, en, ko 排序，便于测试与展示 */
  languages: LanguageTag[]
  fullColor: boolean
  /** 仅表示语言未知；全彩条目也可能 unknown */
  unknown: boolean
}

export interface SearchLanguageSource {
  name: string
  author?: string | null
  category?: { id?: string | null; title?: string | null } | null
  categorySub?: { id?: string | null; title?: string | null } | null
}

const FOLD_CHARS: Record<string, string> = {
  漢: '汉',
  譯: '译',
  訳: '译',
  國: '国',
  體: '体',
  語: '语',
  頁: '页',
  綵: '彩',
  韓: '韩',
  灣: '湾',
  臺: '台',
  無: '无',
  碼: '码',
  單: '单',
  話: '话',
  寫: '写',
  書: '书',
}

/** 归一化：全角转半角、小写、折常见繁体字、统一括号、压缩空白 */
export function normalizeForLanguage(text: string | null | undefined): string {
  if (!text) return ''
  return text
    .normalize('NFKC')
    .toLowerCase()
    .replace(/[漢譯訳國體語頁綵韓灣臺無碼單話寫書]/g, (char) => FOLD_CHARS[char] ?? char)
    .replace(/[【〔「『〈《]/g, '[')
    .replace(/[】〕」』〉》]/g, ']')
    .replace(/\s+/g, ' ')
    .trim()
}

/** 取出所有括号段（[] 与 ()），用于「只在括号里认」的短标记 */
function bracketSegments(text: string): string[] {
  const segments: string[] = []
  const regex = /[[(]([^\])]*)[\])]/g
  for (let match = regex.exec(text); match !== null; match = regex.exec(text)) {
    segments.push(match[1])
  }
  return segments
}

function stripBrackets(text: string): string {
  return text.replace(/[[(][^\])]*[\])]/g, ' ')
}

/** 统计字符脚本数量 */
function countScript(text: string, regex: RegExp): number {
  return (text.match(regex) ?? []).length
}

const HANGUL_RE = /[ᄀ-ᇿ㄰-㆏가-힯]/g
const KANA_RE = /[ぁ-ゖァ-ヺヽ-ヿ]/g
const HAN_RE = /[㐀-䶿一-鿿豈-﫿]/g
const ASCII_LETTER_RE = /[a-z]/g

const METADATA_RE =
  /dl版|dlsite|无修正|无码|有码|同人|单话|单本|短篇|生肉|cosplay|写真|全彩|彩页|彩色|カラー|フルカラー|\bcg\b/g
const FULL_COLOR_RE = /全彩|彩页|彩色版|カラー|フルカラー|full[\s-]?colou?r/

const SUB_ID_LANGUAGES: Record<string, LanguageTag> = {
  chinese: 'zh',
  japanese: 'ja',
  english: 'en',
  korean: 'ko',
}

const SUB_TITLE_LANGUAGES: Record<string, LanguageTag> = {
  汉化: 'zh',
  中文: 'zh',
  日语: 'ja',
  日文: 'ja',
  英文: 'en',
  英语: 'en',
  韩文: 'ko',
  韩语: 'ko',
}

const CATEGORY_ID_LANGUAGES: Record<string, LanguageTag> = {
  hanman: 'ko',
  meiman: 'en',
  english_site: 'en',
}

const CATEGORY_TITLE_LANGUAGES: Record<string, LanguageTag> = {
  韩漫: 'ko',
  美漫: 'en',
  英文站: 'en',
}

interface MarkerRule {
  marker: string
  /** 只在括号里认，避免正文里的同形词误判 */
  bracketOnly?: boolean
  /** 词很短，只在括号里或带「版」后缀时认 */
  short?: boolean
}

const LANGUAGE_MARKERS: Array<{ language: LanguageTag; rules: MarkerRule[] }> = [
  {
    language: 'zh',
    rules: [
      { marker: '中国翻译' },
      { marker: '中翻', short: true },
      { marker: '汉化' },
      { marker: '中文', short: true },
      { marker: '中译' },
      { marker: '简体' },
      { marker: '繁体' },
      { marker: '台湾' },
      { marker: '中国语' },
    ],
  },
  {
    language: 'ja',
    rules: [
      { marker: '日语' },
      { marker: '日文', short: true },
      { marker: '日本语' },
      { marker: '和译' },
      { marker: '日版' },
    ],
  },
  {
    language: 'en',
    rules: [
      { marker: '英文', short: true },
      { marker: '英语' },
      { marker: '英译' },
      { marker: 'english', bracketOnly: true },
      { marker: 'eng', bracketOnly: true },
    ],
  },
  {
    language: 'ko',
    rules: [
      { marker: '韩文', short: true },
      { marker: '韩语' },
      { marker: '韩国语' },
      { marker: '한글' },
      { marker: '한국어' },
      { marker: 'korean', bracketOnly: true },
      { marker: 'kor', bracketOnly: true },
    ],
  },
]

function matchesRule(text: string, segments: string[], rule: MarkerRule): boolean {
  if (rule.bracketOnly) {
    return segments.some((segment) => segment.includes(rule.marker))
  }
  if (rule.short) {
    return (
      segments.some((segment) => segment.includes(rule.marker)) ||
      text.includes(`${rule.marker}版`)
    )
  }
  return text.includes(rule.marker)
}

function explicitLanguages(normalizedName: string): LanguageTag[] {
  const segments = bracketSegments(normalizedName)
  return LANGUAGE_MARKERS.filter(({ rules }) =>
    rules.some((rule) => matchesRule(normalizedName, segments, rule)),
  ).map(({ language }) => language)
}

function structuredLanguage(source: SearchLanguageSource): LanguageTag | null {
  const subId = normalizeForLanguage(source.categorySub?.id)
  if (subId && SUB_ID_LANGUAGES[subId]) return SUB_ID_LANGUAGES[subId]

  const subTitle = normalizeForLanguage(source.categorySub?.title)
  if (subTitle && SUB_TITLE_LANGUAGES[subTitle]) return SUB_TITLE_LANGUAGES[subTitle]

  return null
}

function categoryLanguage(source: SearchLanguageSource): LanguageTag | null {
  const categoryId = normalizeForLanguage(source.category?.id)
  if (categoryId && CATEGORY_ID_LANGUAGES[categoryId]) {
    return CATEGORY_ID_LANGUAGES[categoryId]
  }
  const categoryTitle = normalizeForLanguage(source.category?.title)
  if (categoryTitle && CATEGORY_TITLE_LANGUAGES[categoryTitle]) {
    return CATEGORY_TITLE_LANGUAGES[categoryTitle]
  }
  return null
}

function scriptLanguage(name: string, author: string): LanguageTag | null {
  // 括号里通常是社团/作者/语言标记，先看括号外的标题正文；
  // 整个标题都在括号里时退回全文。
  const outside = stripBrackets(name)
  const candidate = (outside.trim() !== '' ? outside : name).replace(METADATA_RE, ' ')
  const counts = {
    hangul: countScript(candidate, HANGUL_RE),
    kana: countScript(candidate, KANA_RE),
    han: countScript(candidate, HAN_RE),
    ascii: countScript(candidate, ASCII_LETTER_RE),
  }
  const authorCounts = {
    hangul: countScript(author, HANGUL_RE),
    kana: countScript(author, KANA_RE),
  }

  if (counts.hangul >= 2) return 'ko'
  if (counts.kana >= 2) return 'ja'
  if (counts.han >= 3) {
    // 纯汉字标题可能是日文，作者名里的假名是唯一线索
    if (authorCounts.kana >= 2) return 'ja'
    if (authorCounts.hangul >= 2) return 'ko'
    return 'zh'
  }
  if (counts.ascii >= 4) return 'en'
  if (authorCounts.kana >= 2) return 'ja'
  if (authorCounts.hangul >= 2) return 'ko'
  return null
}

export function detectSearchLanguage(source: SearchLanguageSource): SearchLanguageInfo {
  const name = normalizeForLanguage(source.name)
  const fullColor = [name, normalizeForLanguage(source.category?.title), normalizeForLanguage(source.categorySub?.title)]
    .some((text) => text !== '' && FULL_COLOR_RE.test(text))

  const structured = structuredLanguage(source)
  if (structured !== null) {
    return { languages: [structured], fullColor, unknown: false }
  }

  const explicit = explicitLanguages(name)
  if (explicit.length > 0) {
    return { languages: explicit, fullColor, unknown: false }
  }

  const byScript = scriptLanguage(name, normalizeForLanguage(source.author)) ?? categoryLanguage(source)
  const languages = byScript === null ? [] : [byScript]
  return { languages, fullColor, unknown: languages.length === 0 }
}

export function matchesLanguageFilter(info: SearchLanguageInfo, filter: SearchFilter): boolean {
  if (filter === 'all') return true
  if (filter === 'fullColor') return info.fullColor
  return info.languages.includes(filter)
}

export const LANGUAGE_FILTER_LABELS: Record<SearchFilter, string> = {
  all: '全部',
  zh: '中文',
  ja: '日文',
  en: '英文',
  ko: '韩文',
  fullColor: '全彩',
}
