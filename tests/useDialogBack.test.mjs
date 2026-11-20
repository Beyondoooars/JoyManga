import { test } from 'node:test'
import assert from 'node:assert/strict'
import { effectScope } from 'vue'

class MockWindow extends EventTarget {
  constructor() {
    super()
    this.location = { href: 'http://localhost/' }
    this.history = { pushState() {} }
  }
}

const mockWindow = new MockWindow()
Object.defineProperty(globalThis, 'navigator', {
  value: { userAgent: 'Mozilla/5.0 (Linux; Android 14)' },
  configurable: true,
})
Object.defineProperty(globalThis, 'window', {
  value: mockWindow,
  configurable: true,
})

const { openDialogWithBack } = await import('../src/composables/useDialogBack.ts')
const { registerBackHandler } = await import('../src/composables/useAndroidBack.ts')

function deferred() {
  let resolve
  let reject
  const promise = new Promise((res, rej) => {
    resolve = res
    reject = rej
  })
  return { promise, resolve, reject }
}

function createFakeDialogApi() {
  const captured = []
  const api = {
    warning(opts) {
      const instance = {
        loading: false,
        destroyed: 0,
        destroy() {
          instance.destroyed += 1
        },
      }
      captured.push({ opts, instance })
      return instance
    },
  }
  return { api, latest: () => captured[captured.length - 1] }
}

test('openDialogWithBack settles once across negative, mask, close, afterLeave and back button', async () => {
  const { api, latest } = createFakeDialogApi()

  // 1. Negative click + return false retention
  let allowNegative = false
  const negPromise = openDialogWithBack(api, {
    title: 'Confirm',
    onNegativeClick: () => allowNegative,
  })
  const neg = latest()
  assert.equal(neg.opts.onNegativeClick(new Event('click')), false)
  assert.equal(neg.instance.destroyed, 0)
  allowNegative = true
  assert.equal(neg.opts.onNegativeClick(new Event('click')), true)
  assert.equal(await negPromise, false)
  assert.equal(neg.instance.destroyed, 1)

  // 2. Mask click respects maskClosable
  const blockedMaskPromise = openDialogWithBack(api, {
    title: 'Modal',
    maskClosable: false,
    onMaskClick: () => 'custom-kept',
  })
  const blockedMask = latest()
  assert.equal(blockedMask.opts.maskClosable, false)
  assert.equal(blockedMask.opts.onMaskClick(new Event('click')), 'custom-kept')
  assert.equal(blockedMask.instance.destroyed, 0)
  blockedMask.opts.onClose()
  assert.equal(await blockedMaskPromise, false)
  assert.equal(blockedMask.instance.destroyed, 1)

  const closableMaskPromise = openDialogWithBack(api, { title: 'Closable' })
  const closableMask = latest()
  assert.equal(closableMask.opts.onMaskClick(new Event('click')), true)
  assert.equal(await closableMaskPromise, false)
  assert.equal(closableMask.instance.destroyed, 1)

  // 3. Android back press overrides lower priority handler and unregisters after close
  let lowerCalls = 0
  const unregLower = registerBackHandler(() => {
    lowerCalls += 1
    return true
  }, 50)
  try {
    const backPromise = openDialogWithBack(api, { title: 'BackDialog' })
    const backDialog = latest()
    mockWindow.dispatchEvent(new Event('jm-android-back'))
    assert.equal(await backPromise, false)
    assert.equal(backDialog.instance.destroyed, 1)
    assert.equal(lowerCalls, 0)

    // Once settled, lower handler receives subsequent back presses
    mockWindow.dispatchEvent(new Event('jm-android-back'))
    assert.equal(lowerCalls, 1)

    // 4. onAfterLeave settles idempotently if triggered before anything else
    let afterLeaveCalls = 0
    const leavePromise = openDialogWithBack(api, {
      title: 'AfterLeave',
      onAfterLeave: () => {
        afterLeaveCalls += 1
      },
    })
    const leave = latest()
    leave.opts.onAfterLeave()
    leave.opts.onClose()
    assert.equal(await leavePromise, false)
    assert.equal(afterLeaveCalls, 1)
    assert.equal(leave.instance.destroyed, 1)
  } finally {
    unregLower()
  }
})

test('openDialogWithBack async confirmation lock blocks concurrent closes and handles errors', async () => {
  const { api, latest } = createFakeDialogApi()

  // 1. Positive confirm resolves true while rejecting back/cancel during in-flight work
  const gate = deferred()
  const confirmPromise = openDialogWithBack(api, {
    title: 'AsyncConfirm',
    onPositiveClick: () => gate.promise,
  })
  const current = latest()

  const clickPromise = current.opts.onPositiveClick(new Event('click'))
  assert.equal(current.instance.loading, true)
  assert.equal(await current.opts.onPositiveClick(new Event('click')), false)
  assert.equal(current.opts.onNegativeClick(new Event('click')), false)
  assert.equal(current.opts.onMaskClick(new Event('click')), false)
  assert.equal(current.opts.onClose(), false)

  // Back press is consumed but does not destroy/settle while confirming
  mockWindow.dispatchEvent(new Event('jm-android-back'))
  assert.equal(current.instance.destroyed, 0)

  gate.resolve(undefined)
  assert.equal(await clickPromise, true)
  assert.equal(await confirmPromise, true)
  assert.equal(current.instance.destroyed, 1)

  // 2. Rejected onPositiveClick releases confirming & loading so dialog can still close
  const failGate = deferred()
  const retryPromise = openDialogWithBack(api, {
    title: 'RetryConfirm',
    onPositiveClick: () => failGate.promise,
  })
  const retryDialog = latest()
  const failingClick = retryDialog.opts.onPositiveClick(new Event('click'))
  failGate.reject(new Error('network error'))
  await assert.rejects(failingClick, /network error/)
  assert.equal(retryDialog.instance.loading, false)
  assert.equal(retryDialog.instance.destroyed, 0)
  assert.equal(retryDialog.opts.onNegativeClick(new Event('click')), true)
  assert.equal(await retryPromise, false)
  assert.equal(retryDialog.instance.destroyed, 1)
})

test('openDialogWithBack settles false and cleans up when parent effectScope disposes', async () => {
  const { api, latest } = createFakeDialogApi()
  let lowerCalls = 0
  const unregLower = registerBackHandler(() => {
    lowerCalls += 1
    return true
  }, 50)
  const scope = effectScope()
  try {
    let dialogPromise
    scope.run(() => {
      dialogPromise = openDialogWithBack(api, { title: 'ScopedDialog' })
    })
    const active = latest()
    scope.stop()
    assert.equal(await dialogPromise, false)
    assert.equal(active.instance.destroyed, 1)
    mockWindow.dispatchEvent(new Event('jm-android-back'))
    assert.equal(lowerCalls, 1)
  } finally {
    unregLower()
  }
})
