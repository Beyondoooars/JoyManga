import test from 'node:test'
import assert from 'node:assert/strict'
import {
  currentPageFromViewport,
  currentPageFromVisiblePages,
  isRtl,
  isTapGesture,
  pageActionForHorizontalGesture,
  pageActionForTapZone,
  pageActionForKey,
} from '../src/utils/readerNav.ts'

test('rtl is only RightToLeft', () => {
  assert.equal(isRtl('RightToLeft'), true)
  assert.equal(isRtl('LeftToRight'), false)
})

test('tap zones are always left-back / right-forward', () => {
  // 点击区不随阅读方向翻转：左退右进是移动端最自然的手感
  assert.equal(pageActionForTapZone('left'), 'prev')
  assert.equal(pageActionForTapZone('right'), 'next')
})

test('horizontal swipe follows reading direction', () => {
  // LTR：左滑下一页（手指向左 = 内容左移 = 前进）
  assert.equal(pageActionForHorizontalGesture('LeftToRight', 'left'), 'next')
  assert.equal(pageActionForHorizontalGesture('LeftToRight', 'right'), 'prev')
  // RTL：右滑下一页
  assert.equal(pageActionForHorizontalGesture('RightToLeft', 'right'), 'next')
  assert.equal(pageActionForHorizontalGesture('RightToLeft', 'left'), 'prev')
})

test('keyboard mirrors tap zones, space/paging keys stay direction-independent', () => {
  // 方向键与点击区一致：左退右进，不随阅读方向翻转
  assert.equal(pageActionForKey('ArrowLeft'), 'prev')
  assert.equal(pageActionForKey('ArrowRight'), 'next')
  // A/D 与方向键同义
  assert.equal(pageActionForKey('a'), 'prev')
  assert.equal(pageActionForKey('D'), 'next')
  // 空格/翻页键永远向后/向前
  assert.equal(pageActionForKey(' '), 'next')
  assert.equal(pageActionForKey('PageDown'), 'next')
  assert.equal(pageActionForKey('PageUp'), 'prev')
  // 无关按键返回 null，避免误吞其他快捷键
  assert.equal(pageActionForKey('Escape'), null)
  assert.equal(pageActionForKey('f'), null)
})

test('scroll mode treats the last page as current once the end is reached', () => {
  // 末尾几页不足一屏：按最靠上会停在倒数第二页，读完了也显示没读完
  const visible = [
    { page: 4, top: -300 },
    { page: 5, top: 200 },
  ]
  assert.equal(currentPageFromViewport(visible, false), 4)
  assert.equal(currentPageFromViewport(visible, true), 5)
  // 没有可见页时仍返回 null，不要凭空造一个页码
  assert.equal(currentPageFromViewport([], true), null)
})

test('tap detection ignores drags and long presses', () => {
  const start = { x: 100, y: 200, time: 0 }
  // 原地轻点：算一次 tap
  assert.equal(isTapGesture(start, { x: 103, y: 204, time: 120 }), true)
  // 位移超过容差（想滚动却只挪了一点）：不是 tap
  assert.equal(isTapGesture(start, { x: 140, y: 200, time: 200 }), false)
  assert.equal(isTapGesture(start, { x: 100, y: 240, time: 200 }), false)
  // 按住不放：不是 tap
  assert.equal(isTapGesture(start, { x: 100, y: 200, time: 1500 }), false)
})

test('scroll mode picks the topmost visible page as current', () => {
  assert.equal(currentPageFromVisiblePages([]), null)
  // 多页同时可见时取最靠上的那页，而不是回调顺序里的第一个
  assert.equal(
    currentPageFromVisiblePages([
      { page: 7, top: 320 },
      { page: 6, top: -40 },
      { page: 8, top: 900 },
    ]),
    6,
  )
  assert.equal(currentPageFromVisiblePages([{ page: 3, top: 12 }]), 3)
})
