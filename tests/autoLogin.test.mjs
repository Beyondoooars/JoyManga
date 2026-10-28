import { test } from 'node:test'
import assert from 'node:assert/strict'
import { isRetryableAutoLoginError, runAutoLogin } from '../src/utils/autoLogin.ts'

const success = {
  status: 'ok',
  data: { profile: { uid: '1', s: '' }, sessionEpoch: 2, switchedApiDomainMode: null },
}

function commandError(kind, message = '失败') {
  return { err_title: '登录失败', err_message: message, kind }
}

test('auto login succeeds on the first attempt without waiting', async () => {
  let waits = 0
  const outcome = await runAutoLogin({
    username: 'alice',
    password: 'secret',
    login: async () => success,
    wait: async () => { waits++ },
  })

  assert.equal(outcome.status, 'success')
  assert.equal(outcome.attempts, 1)
  assert.equal(waits, 0)
})

test('auto login retries a network failure and reports the successful attempt', async () => {
  const waits = []
  let calls = 0
  const outcome = await runAutoLogin({
    username: 'alice',
    password: 'secret',
    login: async () => {
      calls++
      return calls === 1 ? { status: 'error', error: commandError('Network') } : success
    },
    wait: async (milliseconds) => { waits.push(milliseconds) },
  })

  assert.equal(outcome.status, 'success')
  assert.equal(outcome.attempts, 2)
  assert.deepEqual(waits, [1500])
})

test('auto login does not retry wrong credentials', async () => {
  let calls = 0
  const outcome = await runAutoLogin({
    username: 'alice',
    password: 'wrong',
    login: async () => {
      calls++
      return { status: 'error', error: commandError('Auth', '账号或密码错误') }
    },
  })

  assert.equal(outcome.status, 'failed')
  assert.equal(outcome.attempts, 1)
  assert.equal(calls, 1)
  assert.equal(outcome.error.err_message, '账号或密码错误')
})

test('auto login stops when the user has already logged in manually', async () => {
  let calls = 0
  let loggedIn = false
  const outcome = await runAutoLogin({
    username: 'alice',
    password: 'secret',
    login: async () => {
      calls++
      loggedIn = true
      return { status: 'error', error: commandError('Network') }
    },
    shouldStop: () => loggedIn,
    wait: async () => {},
  })

  assert.equal(outcome.status, 'skipped')
  assert.equal(calls, 1)
})

test('auto login stays silent when a cancellation came from manual login', async () => {
  let loggedIn = false
  const outcome = await runAutoLogin({
    username: 'alice',
    password: 'secret',
    login: async () => {
      loggedIn = true
      return { status: 'error', error: commandError('Auth', '登录已取消') }
    },
    shouldStop: () => loggedIn,
  })

  assert.equal(outcome.status, 'skipped')
})

test('auto login retries thrown transport errors and returns the last message', async () => {
  let calls = 0
  const outcome = await runAutoLogin({
    username: 'alice',
    password: 'secret',
    login: async () => {
      calls++
      throw new Error('连接被重置')
    },
    attempts: 2,
    wait: async () => {},
  })

  assert.equal(outcome.status, 'failed')
  assert.equal(outcome.attempts, 2)
  assert.equal(outcome.error, '连接被重置')
})

test('only network, timeout and unknown errors are retryable', () => {
  assert.equal(isRetryableAutoLoginError(commandError('Network')), true)
  assert.equal(isRetryableAutoLoginError(commandError('Timeout')), true)
  assert.equal(isRetryableAutoLoginError(commandError('Unknown')), true)
  assert.equal(isRetryableAutoLoginError(commandError('Auth')), false)
  assert.equal(isRetryableAutoLoginError(commandError('NotFound')), false)
  assert.equal(isRetryableAutoLoginError(commandError('Unknown', '登录已取消')), false)
  assert.equal(isRetryableAutoLoginError(commandError('Unknown', '登录期间网络设置已改变，请重试')), false)
  assert.equal(isRetryableAutoLoginError('抛出异常'), true)
})
