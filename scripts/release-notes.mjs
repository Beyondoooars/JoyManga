import { execFileSync } from 'node:child_process'
import fs from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

/**
 * 生成 Release 正文：把上一个版本 tag 到本次发布之间的提交，按类型整理成
 * 「新功能 / 修复 / 优化 / 其他」四组要点。
 *
 * 发布说明直接面向用户：去掉 conventional commit 前缀（feat(update): 这类），
 * 只保留改动本身的中文描述。
 *
 * 用法：node scripts/release-notes.mjs <版本号> <输出文件>
 */

const __dirname = path.dirname(fileURLToPath(import.meta.url))
const rootDir = path.resolve(__dirname, '..')

const version = (process.argv[2] || '').trim().replace(/^v/, '')
const outPath = path.resolve(rootDir, process.argv[3] || 'release-notes.md')

if (!version) {
  console.error('[release-notes] 缺少版本号，用法：node scripts/release-notes.mjs <版本号> <输出文件>')
  process.exit(1)
}

function git(args) {
  return execFileSync('git', args, { cwd: rootDir, encoding: 'utf8' }).trim()
}

/** 上一个版本 tag；当前版本自己的 tag 要排除（tag 构建时 HEAD 正好被标记）。 */
function previousTag() {
  try {
    return git(['describe', '--tags', '--abbrev=0', '--match', 'v*', '--exclude', `v${version}`, 'HEAD'])
  } catch {
    return ''
  }
}

const from = previousTag()
// 没有上一个 tag（首个版本）时只取最近 20 条，避免把整个历史搬进发布说明
const range = from ? [`${from}..HEAD`] : ['HEAD', '--max-count=20']

let subjects = []
try {
  subjects = git(['log', ...range, '--no-merges', '--pretty=format:%s'])
    .split('\n')
    .map((line) => line.trim())
    .filter(Boolean)
} catch (error) {
  console.error('[release-notes] 读取提交记录失败：', error.message)
}

const SECTIONS = [
  { title: '新功能', types: ['feat'] },
  { title: '修复', types: ['fix', 'hotfix'] },
  { title: '优化', types: ['perf', 'refactor', 'style', 'build'] },
  { title: '其他', types: ['chore', 'docs', 'ci', 'test', 'revert'] },
]

/** `feat(update)!: 精简弹窗` → `精简弹窗`；无前缀的原样保留 */
function describe(subject) {
  return subject.replace(/^[a-z]+(?:\([^)]*\))?!?:\s*/i, '').trim()
}

function typeOf(subject) {
  const match = subject.match(/^([a-z]+)(?:\([^)]*\))?!?:/i)
  return match ? match[1].toLowerCase() : ''
}

const grouped = new Map(SECTIONS.map((section) => [section.title, []]))
const seen = new Set()

for (const subject of subjects) {
  // 版本号提交是流水线自己产生的，与用户无关
  if (/^chore:\s*发布\s*v/i.test(subject)) continue
  const text = describe(subject)
  if (!text || seen.has(text)) continue
  seen.add(text)
  const type = typeOf(subject)
  const section = SECTIONS.find((item) => item.types.includes(type))
  grouped.get(section ? section.title : '其他').push(text)
}

const lines = []
for (const section of SECTIONS) {
  const items = grouped.get(section.title)
  if (!items.length) continue
  lines.push(`### ${section.title}`, '')
  // 用 `*` 列表：GitHub 正常渲染，应用内更新弹窗也会转成「·」
  for (const item of items.slice(0, 30)) lines.push(`* ${item}`)
  lines.push('')
}

if (!lines.length) lines.push('本次发布以构建与发布流程调整为主。', '')

fs.mkdirSync(path.dirname(outPath), { recursive: true })
fs.writeFileSync(outPath, lines.join('\n').trimEnd() + '\n', 'utf8')
console.log(`[release-notes] 已生成 ${outPath}${from ? `（对比 ${from}）` : '（无历史 tag，取最近提交）'}`)
