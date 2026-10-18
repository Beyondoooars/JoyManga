import test from 'node:test'
import assert from 'node:assert/strict'
import {
  FIT_MODES,
  MAX_SCALE,
  clampTransform,
  displaySize,
  nextFitMode,
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

test('fit width fills width, fit height fills height', () => {
  const image = { width: 500, height: 1000 } // 竖图
  const width = displaySize(image, container, 'width')
  assert.equal(width.width, 1000)
  assert.ok(Math.abs(width.height - 2000) < 0.01)

  const height = displaySize(image, container, 'height')
  assert.equal(height.height, 800)
  assert.ok(Math.abs(height.width - 400) < 0.01)
})

test('fit screen keeps aspect ratio inside container', () => {
  const image = { width: 2000, height: 1000 }
  const sized = displaySize(image, container, 'screen')
  assert.ok(sized.width <= container.width + 0.01)
  assert.ok(sized.height <= container.height + 0.01)
  // 宽图应由宽度受限
  assert.equal(sized.width, container.width)
})

test('original ignores container', () => {
  const image = { width: 3000, height: 2000 }
  assert.deepEqual(displaySize(image, container, 'original'), { width: 3000, height: 2000 })
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
