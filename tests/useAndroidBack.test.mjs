import { test } from 'node:test'
import assert from 'node:assert/strict'
import { createRenderer, defineComponent, h, KeepAlive, nextTick, ref } from 'vue'

class MockWindow extends EventTarget {
  constructor() {
    super()
    this.historyCalls = []
    this.exitCalls = 0
    this.location = { href: 'http://localhost/' }
    this.history = {
      pushState: (...args) => {
        this.historyCalls.push(args)
      },
    }
    this.JmAndroid = {
      exitApp: () => {
        this.exitCalls += 1
      },
    }
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

const { handleRootBackExit, registerBackHandler, useBackHandler } = await import('../src/composables/useAndroidBack.ts')

function createHeadlessRenderer() {
  const createNode = (type) => ({ type, children: [], parentNode: null })
  const renderer = createRenderer({
    patchProp() {},
    insert(child, parent, anchor) {
      child.parentNode = parent
      if (anchor) {
        const index = parent.children.indexOf(anchor)
        if (index !== -1) {
          parent.children.splice(index, 0, child)
          return
        }
      }
      parent.children.push(child)
    },
    remove(child) {
      const parent = child.parentNode
      if (!parent) return
      const index = parent.children.indexOf(child)
      if (index !== -1) parent.children.splice(index, 1)
      child.parentNode = null
    },
    createElement: (tag) => createNode(tag),
    createText: (text) => createNode(text),
    createComment: (text) => createNode(text),
    setText() {},
    setElementText() {},
    parentNode: (node) => node.parentNode,
    nextSibling(node) {
      const parent = node.parentNode
      if (!parent) return null
      const index = parent.children.indexOf(node)
      return index === -1 ? null : (parent.children[index + 1] ?? null)
    },
  })
  return { renderer, root: createNode('root') }
}

test('registerBackHandler dispatches by priority then LIFO and stops after handled', () => {
  const calls = []
  const unregLow = registerBackHandler(() => {
    calls.push('low')
    return true
  }, 10)
  const unregMidFirst = registerBackHandler(() => {
    calls.push('mid-1')
    return false
  }, 50)
  const unregMidSecond = registerBackHandler(() => {
    calls.push('mid-2')
    return true
  }, 50)
  const unregHighThrow = registerBackHandler(() => {
    calls.push('high-throw')
    throw new Error('boom')
  }, 100)

  const originalError = console.error
  console.error = () => {}
  try {
    let prevented = false
    let stopped = false
    const keyEvent = new Event('keydown')
    keyEvent.key = 'Escape'
    keyEvent.preventDefault = () => {
      prevented = true
    }
    keyEvent.stopPropagation = () => {
      stopped = true
    }
    mockWindow.dispatchEvent(keyEvent)
    assert.deepEqual(calls, ['high-throw', 'mid-2'])
    assert.equal(prevented, true)
    assert.equal(stopped, true)

    calls.length = 0
    unregHighThrow()
    unregMidSecond()
    mockWindow.dispatchEvent(new Event('jm-android-back'))
    assert.deepEqual(calls, ['mid-1', 'low'])

    unregLow()
    unregMidFirst()
    let unhandledPrevented = false
    const unhandledKey = new Event('keydown')
    unhandledKey.key = 'Escape'
    unhandledKey.preventDefault = () => {
      unhandledPrevented = true
    }
    unhandledKey.stopPropagation = () => {}
    mockWindow.dispatchEvent(unhandledKey)
    assert.equal(unhandledPrevented, false)
  } finally {
    console.error = originalError
    unregLow()
    unregMidFirst()
    unregMidSecond()
    unregHighThrow()
  }
})

test('useBackHandler detaches on KeepAlive deactivation and reattaches on activation', async () => {
  const activeTab = ref('a')
  const aCalls = []
  const bCalls = []

  const PageA = defineComponent({
    name: 'PageA',
    setup() {
      useBackHandler(() => {
        aCalls.push('a')
        return true
      }, 20)
      return () => h('div', 'A')
    },
  })

  const PageB = defineComponent({
    name: 'PageB',
    setup() {
      useBackHandler(() => {
        bCalls.push('b')
        return true
      }, 10)
      return () => h('div', 'B')
    },
  })

  const Host = defineComponent({
    setup() {
      return () =>
        h(KeepAlive, null, {
          default: () => (activeTab.value === 'a' ? h(PageA) : h(PageB)),
        })
    },
  })

  const { renderer, root } = createHeadlessRenderer()
  const app = renderer.createApp(Host)
  app.mount(root)

  mockWindow.dispatchEvent(new Event('jm-android-back'))
  assert.deepEqual(aCalls, ['a'])
  assert.deepEqual(bCalls, [])

  activeTab.value = 'b'
  await nextTick()
  mockWindow.dispatchEvent(new Event('jm-android-back'))
  assert.deepEqual(aCalls, ['a'])
  assert.deepEqual(bCalls, ['b'])

  activeTab.value = 'a'
  await nextTick()
  mockWindow.dispatchEvent(new Event('jm-android-back'))
  assert.deepEqual(aCalls, ['a', 'a'])
  assert.deepEqual(bCalls, ['b'])

  app.unmount()
  await nextTick()
  mockWindow.dispatchEvent(new Event('jm-android-back'))
  assert.deepEqual(aCalls, ['a', 'a'])
  assert.deepEqual(bCalls, ['b'])
})

test('handleRootBackExit prompts once then exits on second press within window', (t) => {
  t.mock.timers.enable({ apis: ['Date'] })
  mockWindow.exitCalls = 0
  const messages = []
  try {
    t.mock.timers.setTime(10_000)
    assert.equal(handleRootBackExit((msg) => messages.push(msg)), true)
    assert.deepEqual(messages, ['再按一次返回桌面'])
    assert.equal(mockWindow.exitCalls, 0)

    t.mock.timers.setTime(10_500)
    assert.equal(handleRootBackExit((msg) => messages.push(msg)), true)
    assert.equal(mockWindow.exitCalls, 1)

    // After exiting reset, next press prompts again cleanly
    t.mock.timers.setTime(11_000)
    assert.equal(handleRootBackExit((msg) => messages.push(msg)), true)
    assert.deepEqual(messages, ['再按一次返回桌面', '再按一次返回桌面'])
    assert.equal(mockWindow.exitCalls, 1)
  } finally {
    t.mock.timers.reset()
  }
})
