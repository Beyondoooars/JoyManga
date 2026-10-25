import test from 'node:test'
import assert from 'node:assert/strict'
import {
  buildCalendarWeeks,
  compactNumber,
  dayOfMonth,
  expProgress,
  isSignedToday,
  monthlyCheckInText,
} from '../src/utils/dailyCheckIn.ts'

function stateWith(days) {
  return { record: [days], signedDays: 0, elapsedDays: 0 }
}

test('今天那一格 signed 为 true 才算已签', () => {
  const now = new Date(2026, 9, 25, 9, 0, 0)
  assert.equal(isSignedToday(stateWith([{ date: '25', signed: true }]), now), true)
  assert.equal(isSignedToday(stateWith([{ date: '25', signed: false }]), now), false)
})

test('未来日期的 null 不能当成已签', () => {
  const now = new Date(2026, 9, 25, 9, 0, 0)
  assert.equal(isSignedToday(stateWith([{ date: '25', signed: null }]), now), false)
})

test('补零的 date 也能匹配到今天', () => {
  const now = new Date(2026, 9, 5, 9, 0, 0)
  assert.equal(isSignedToday(stateWith([{ date: '05', signed: true }]), now), true)
})

test('面板里找不到今天时按未签处理，交给服务端兜底', () => {
  const now = new Date(2026, 9, 25, 9, 0, 0)
  assert.equal(isSignedToday(stateWith([{ date: '24', signed: true }]), now), false)
  assert.equal(isSignedToday(undefined, now), false)
})

test('跨周分组时仍能找到今天', () => {
  const now = new Date(2026, 9, 3, 9, 0, 0)
  const state = {
    record: [
      [{ date: '1', signed: true }],
      [{ date: '2', signed: true }, { date: '3', signed: true }],
    ],
    signedDays: 3,
    elapsedDays: 3,
  }
  assert.equal(isSignedToday(state, now), true)
})

test('本月进度只算到今天为止的天数', () => {
  assert.equal(monthlyCheckInText({ record: [], signedDays: 5, elapsedDays: 8 }), '本月已签 5/8 天')
  assert.equal(monthlyCheckInText({ record: [], signedDays: 0, elapsedDays: 0 }), '')
  assert.equal(monthlyCheckInText(undefined), '')
})

test('经验进度优先用服务端百分比', () => {
  const progress = expProgress({ exp: '320', nextLevelExp: 1000, expPercent: 32 })
  assert.deepEqual(progress, { current: 320, target: 1000, percent: 32 })
})

test('服务端百分比缺失时按经验值计算', () => {
  assert.deepEqual(expProgress({ exp: '250', nextLevelExp: 500, expPercent: 0 }), {
    current: 250,
    target: 500,
    percent: 50,
  })
})

test('百分比越界或经验缺失时回落到安全值', () => {
  assert.equal(expProgress({ exp: '999', nextLevelExp: 100, expPercent: 0 }).percent, 100)
  assert.equal(expProgress({ exp: '999', nextLevelExp: 0, expPercent: 123 }).percent, 0)
  assert.deepEqual(expProgress(undefined), { current: 0, target: 0, percent: 0 })
})

test('经验字符串带千分位也能解析', () => {
  assert.equal(expProgress({ exp: '1,234', nextLevelExp: 5000, expPercent: 0 }).current, 1234)
})

test('金币过万折成万位显示', () => {
  assert.equal(compactNumber(9999), '9999')
  assert.equal(compactNumber(12000), '1.2万')
  assert.equal(compactNumber(20000), '2万')
  assert.equal(compactNumber(Number.NaN), '0')
})

test('dayOfMonth 与面板 date 形态一致', () => {
  assert.equal(dayOfMonth(new Date(2026, 9, 5)), '5')
  assert.equal(dayOfMonth(new Date(2026, 9, 25)), '25')
})

test('月历按真实星期补位，1 号落在它该在的那一列', () => {
  // 2026 年 10 月 1 日是周四 → 前面补 3 个空位（周一、二、三）
  const state = {
    record: [[1, 2, 3, 4, 5, 6, 7, 8, 9, 10].map((d) => ({ date: String(d), signed: null, bonus: false }))],
    signedDays: 0,
    elapsedDays: 0,
  }
  const weeks = buildCalendarWeeks(state, new Date(2026, 9, 15))
  assert.equal(weeks[0][0], null)
  assert.equal(weeks[0][1], null)
  assert.equal(weeks[0][2], null)
  assert.equal(weeks[0][3].date, '1')
  // 每一行都是完整的 7 列
  for (const week of weeks) assert.equal(week.length, 7)
})

test('月历跨周时行数与星期对齐正确', () => {
  const days = Array.from({ length: 31 }, (_, i) => ({ date: String(i + 1), signed: null, bonus: false }))
  const state = { record: [days.slice(0, 7), days.slice(7)], signedDays: 0, elapsedDays: 0 }
  // 2026 年 10 月：1 号周四，31 号周六 → 补齐月初 3 格后共 5 行
  const weeks = buildCalendarWeeks(state, new Date(2026, 9, 15))
  assert.equal(weeks.length, 5)
  assert.equal(weeks[4][4].date, '30')
  assert.equal(weeks[4][5].date, '31')
  assert.equal(weeks[4][6], null)
})

test('空面板不产生任何行', () => {
  assert.deepEqual(buildCalendarWeeks(undefined), [])
  assert.deepEqual(buildCalendarWeeks({ record: [], signedDays: 0, elapsedDays: 0 }), [])
})
