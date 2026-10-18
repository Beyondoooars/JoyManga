import test from 'node:test'
import assert from 'node:assert/strict'
import { formatCommandError, getErrorHint } from '../src/utils/errors.ts'

test('unknown kind keeps the message clean without a hint', () => {
  const text = formatCommandError({ err_title: '下载失败', err_message: '未知原因', kind: 'Unknown' })
  assert.equal(text, '下载失败：未知原因')
})

test('kind without value behaves like unknown', () => {
  const text = formatCommandError({ err_title: '下载失败', err_message: '原因' })
  assert.equal(text, '下载失败：原因')
})

test('each actionable kind adds its own suggestion', () => {
  assert.match(formatCommandError({ err_title: 't', err_message: 'm', kind: 'Network' }), /更换线路/)
  assert.match(formatCommandError({ err_title: 't', err_message: 'm', kind: 'Timeout' }), /稍后重试/)
  assert.match(formatCommandError({ err_title: 't', err_message: 'm', kind: 'Auth' }), /重新登录/)
  assert.match(formatCommandError({ err_title: 't', err_message: 'm', kind: 'NotFound' }), /已下架/)
  assert.match(formatCommandError({ err_title: 't', err_message: 'm', kind: 'Storage' }), /磁盘空间/)
})

test('hints are only exposed for actionable kinds', () => {
  assert.equal(getErrorHint(undefined), null)
  assert.equal(getErrorHint('Unknown'), null)
  assert.ok(getErrorHint('Auth'))
})

test('missing fields fall back instead of printing undefined', () => {
  const text = formatCommandError({})
  assert.equal(text, '操作失败：未知错误')
})
