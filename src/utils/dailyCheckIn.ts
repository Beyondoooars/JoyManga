import type { DailyCheckInState, GetUserProfileRespData } from '../bindings.ts'

/** 面板里 `record[].date` 是「当月第几天」，补零与否都出现过；统一成不补零的写法再比较。 */
function normalizeDay(date: string): string {
  const value = Number.parseInt(String(date).trim(), 10)
  return Number.isSafeInteger(value) ? String(value) : String(date).trim()
}

/** 设备本地日期对应的「当月第几天」，形态与 `record[].date` 一致（不补零）。 */
export function dayOfMonth(now: Date): string {
  return String(now.getDate())
}

/** 面板里的一格是否就是 `now` 当天。 */
export function isTodayDate(date: string, now: Date = new Date()): boolean {
  return normalizeDay(date) === dayOfMonth(now)
}

/**
 * 今天是否已签到。
 *
 * 面板里 `signed === true` 才是已签；`null` 表示还没结算（今天未签或未来日期），
 * `false` 表示那天漏签。面板里找不到今天（跨月、时区差一天）时按未签处理，
 * 由服务端在打卡时兜底：重复签到同样回 200，后端再用面板差值判定结果。
 */
export function isSignedToday(state: DailyCheckInState | undefined, now: Date = new Date()): boolean {
  if (!state) return false
  const today = dayOfMonth(now)
  for (const week of state.record) {
    for (const day of week) {
      if (normalizeDay(day.date) === today) return day.signed === true
    }
  }
  return false
}

/**
 * 本月签到进度文案，例如 `本月已签 5/8 天`。
 * 分母只算到「今天」为止（`elapsedDays`），避免显示成「5/31」让人觉得全程没签。
 */
export function monthlyCheckInText(state: DailyCheckInState | undefined): string {
  if (!state || state.elapsedDays <= 0) return ''
  return `本月已签 ${state.signedDays}/${state.elapsedDays} 天`
}

export interface ExpProgress {
  current: number
  target: number
  /** 0–100 的整数百分比，用于进度条宽度与文案 */
  percent: number
}

function toNumber(value: number | string | null | undefined): number {
  if (typeof value === 'number') return Number.isFinite(value) ? value : 0
  if (typeof value !== 'string') return 0
  // 经验偶尔以 "1,234" 之类的千分位字符串返回
  const parsed = Number(value.replace(/[^\d.-]/g, ''))
  return Number.isFinite(parsed) ? parsed : 0
}

/**
 * 等级经验进度：优先用服务端给的 `expPercent`，缺失或超出 0–100 时按
 * `exp / nextLevelExp` 自行计算。经验值本身以 `exp` 字段为准（可能是字符串）。
 */
export function expProgress(profile: GetUserProfileRespData | undefined | null): ExpProgress {
  const current = Math.max(0, toNumber(profile?.exp))
  const target = Math.max(0, toNumber(profile?.nextLevelExp))
  const serverPercent = toNumber(profile?.expPercent)
  const computed = target > 0 ? (current / target) * 100 : 0
  const raw = serverPercent > 0 && serverPercent <= 100 ? serverPercent : computed
  return {
    current,
    target,
    percent: Math.min(100, Math.max(0, Math.round(raw))),
  }
}

/** 金币等计数器的展示文案：万位以上折成 `1.2万`，避免窄屏把数字挤成两行。 */
export function compactNumber(value: number): string {
  if (!Number.isFinite(value)) return '0'
  if (Math.abs(value) < 10000) return String(Math.trunc(value))
  return `${(value / 10000).toFixed(1).replace(/\.0$/, '')}万`
}
