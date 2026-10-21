import { test } from 'node:test'
import assert from 'node:assert/strict'
import { imageUrlFor } from '../src/utils/imageCache.ts'

test('路径整体按 UTF-8 百分号编码，只保留 URL 非保留字符', () => {
  // 分隔符也编码，避免路径里的 `..`、空格、中文被 URL 解析层改写
  assert.equal(
    imageUrlFor('/storage/emulated/0/JoyManga/a-b_c.jpg'),
    'http://jmimg.localhost/%2Fstorage%2Femulated%2F0%2FJoyManga%2Fa-b_c.jpg',
  )
  assert.equal(
    imageUrlFor('/storage/emulated/0/漫画 第1话/001.jpg'),
    'http://jmimg.localhost/%2Fstorage%2Femulated%2F0%2F%E6%BC%AB%E7%94%BB%20%E7%AC%AC1%E8%AF%9D%2F001.jpg',
  )
})

test('CBZ 的「归档路径|条目名」中的竖线被编码，避免被 URL 解析改写', () => {
  const url = imageUrlFor('/storage/a.cbz|第1话/001.jpg')
  assert.equal(
    url,
    'http://jmimg.localhost/%2Fstorage%2Fa.cbz%7C%E7%AC%AC1%E8%AF%9D%2F001.jpg',
  )
  assert.ok(!url.includes('|'))
})

test('重试令牌只以查询参数形式出现，便于换 URL 绕过缓存', () => {
  assert.equal(imageUrlFor('page-1'), 'http://jmimg.localhost/page-1')
  assert.equal(imageUrlFor('page-1', 2), 'http://jmimg.localhost/page-1?retry=2')
})
