import test from 'node:test'
import assert from 'node:assert/strict'
import {
  isRtl,
  pageActionForHorizontalGesture,
  pageActionForTapZone,
  pageActionForKey,
} from '../src/utils/readerNav.ts'

test('rtl is only RightToLeft', () => {
  assert.equal(isRtl('RightToLeft'), true)
  assert.equal(isRtl('LeftToRight'), false)
})

test('tap zones follow reading direction', () => {
  // LTR：左侧上一页、右侧下一页
  assert.equal(pageActionForTapZone('LeftToRight', 'left'), 'prev')
  assert.equal(pageActionForTapZone('LeftToRight', 'right'), 'next')
  // RTL：反过来
  assert.equal(pageActionForTapZone('RightToLeft', 'left'), 'next')
  assert.equal(pageActionForTapZone('RightToLeft', 'right'), 'prev')
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
  assert.equal(pageActionForKey('LeftToRight', 'ArrowLeft'), 'prev')
  assert.equal(pageActionForKey('LeftToRight', 'ArrowRight'), 'next')
  assert.equal(pageActionForKey('RightToLeft', 'ArrowLeft'), 'next')
  assert.equal(pageActionForKey('RightToLeft', 'ArrowRight'), 'prev')
  // A/D 与方向键同义
  assert.equal(pageActionForKey('RightToLeft', 'a'), 'next')
  assert.equal(pageActionForKey('RightToLeft', 'D'), 'prev')
  // 空格/翻页键永远向后/向前
  for (const direction of ['LeftToRight', 'RightToLeft']) {
    assert.equal(pageActionForKey(direction, ' '), 'next')
    assert.equal(pageActionForKey(direction, 'PageDown'), 'next')
    assert.equal(pageActionForKey(direction, 'PageUp'), 'prev')
  }
  // 无关按键返回 null，避免误吞其他快捷键
  assert.equal(pageActionForKey('RightToLeft', 'Escape'), null)
  assert.equal(pageActionForKey('RightToLeft', 'f'), null)
})
