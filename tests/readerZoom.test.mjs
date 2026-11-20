import test from 'node:test'
import assert from 'node:assert/strict'
import {
  FIT_MODES,
  MAX_SCALE,
  clampTransform,
  computeScrollWindow,
  nextFitMode,
  panTransform,
  pinchTransform,
  resetForPageChange,
  toggleZoom,
} from '../src/utils/readerZoom.ts'

const container = { width: 1000, height: 800 }

test('fit mode cycles through all four modes', () => {
  let mode = FIT_MODES[0]
  const seen = new Set()
  for (let i = 0; i < FIT_MODES.length; i++) {
    seen.add(mode)
    mode = nextFitMode(mode)
  }
  assert.equal(seen.size, 4)
  // 循环一圈回到起点，不会卡住
  assert.equal(mode, FIT_MODES[0])
})

test('double tap zooms to 2x around the tapped point then resets', () => {
  const point = { x: container.width / 2 + 100, y: container.height / 2 }
  const zoomed = toggleZoom(resetForPageChange(), point, container)
  assert.equal(zoomed.scale, 2)
  // 点在中心右侧，内容应向左移，使该点回到视野中央
  assert.ok(zoomed.offsetX < 0)

  const reset = toggleZoom(zoomed, point, container)
  assert.deepEqual(reset, { scale: 1, offsetX: 0, offsetY: 0 })
})

test('pinch is clamped to the allowed scale range', () => {
  const start = { scale: 1, offsetX: 0, offsetY: 0 }
  const anchor = { x: 500, y: 400 }
  assert.equal(pinchTransform(start, anchor, 100, container).scale, MAX_SCALE)
  assert.equal(pinchTransform(start, anchor, 0.1, container).scale, 1)
})

test('pan is clamped so the image cannot leave the viewport', () => {
  const clamped = clampTransform({ scale: 2, offsetX: 99999, offsetY: -99999 }, container)
  assert.equal(clamped.offsetX, container.width / 2)
  assert.equal(clamped.offsetY, -container.height / 2)
})

test('scale back to 1 clears any offset', () => {
  const result = clampTransform({ scale: 1, offsetX: 40, offsetY: 40 }, container)
  assert.deepEqual(result, { scale: 1, offsetX: 0, offsetY: 0 })
})

test('pinchTransform preserves existing offset and avoids drift when clamped at MAX_SCALE', () => {
  const start = { scale: 4, offsetX: 200, offsetY: -100 }
  const anchor = { x: 700, y: 500 }
  const zoomedToMax = pinchTransform(start, anchor, 10, container)
  const exactToMax = pinchTransform(start, anchor, MAX_SCALE / 4, container)
  assert.equal(zoomedToMax.scale, MAX_SCALE)
  assert.deepEqual(zoomedToMax, exactToMax)
})

test('panTransform pans within bounds when zoomed and returns identity when unzoomed', () => {
  const start = { scale: 2, offsetX: 100, offsetY: -50 }
  assert.deepEqual(panTransform({ scale: 1, offsetX: 0, offsetY: 0 }, 50, -50, container), {
    scale: 1,
    offsetX: 0,
    offsetY: 0,
  })
  assert.deepEqual(panTransform({ scale: 1.005, offsetX: 25, offsetY: -15 }, 50, -50, container), {
    scale: 1,
    offsetX: 0,
    offsetY: 0,
  })
  const panned = panTransform(start, 60, 80, container)
  assert.deepEqual(panned, { scale: 2, offsetX: 160, offsetY: 30 })
  assert.deepEqual(start, { scale: 2, offsetX: 100, offsetY: -50 })

  const cumulative = panTransform(panned, 100, -100, container)
  assert.deepEqual(cumulative, { scale: 2, offsetX: 260, offsetY: -70 })

  const clamped = panTransform({ scale: MAX_SCALE, offsetX: -1000, offsetY: 1000 }, -5000, 5000, container)
  assert.equal(clamped.scale, MAX_SCALE)
  assert.equal(clamped.offsetX, -(container.width * (MAX_SCALE - 1)) / 2)
  assert.equal(clamped.offsetY, (container.height * (MAX_SCALE - 1)) / 2)
})

test('computeScrollWindow bounds page window near start, middle, end and empty chapters', () => {
  assert.deepEqual(computeScrollWindow(1, 0, 8), { start: 1, end: 0 })
  assert.deepEqual(computeScrollWindow(1, 50, 8), { start: 1, end: 9 })
  assert.deepEqual(computeScrollWindow(20, 50, 8), { start: 12, end: 28 })
  assert.deepEqual(computeScrollWindow(50, 50, 8), { start: 42, end: 50 })
})
