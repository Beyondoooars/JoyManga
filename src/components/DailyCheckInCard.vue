<script setup lang="ts">
import { computed, onActivated, ref, watch } from 'vue'
import { useMessage } from 'naive-ui'
import { commands } from '../bindings.ts'
import type { DailyCheckInRecordDay, DailyCheckInState } from '../bindings.ts'
import { useStore } from '../store.ts'
import { formatCommandError, getErrorMessage } from '../utils/errors.ts'
import {
  WEEKDAY_LABELS,
  buildCalendarWeeks,
  isSignedToday,
  isTodayDate,
  monthlyCheckInText,
} from '../utils/dailyCheckIn.ts'
import { PhArrowClockwise, PhCheck, PhCoin, PhGift, PhTrendUp } from '@phosphor-icons/vue'

const store = useStore()
const message = useMessage()

const state = ref<DailyCheckInState>()
const loading = ref(false)
const signing = ref(false)
const errorMessage = ref('')

const uid = computed(() => store.userProfile?.uid)
/**
 * 只在「账号 + 会话轮次」变化时重建面板。签到成功后后端会刷新用户资料
 * （新对象、同 uid），若监听整个 profile 就会白白重拉一次、把刚拿到的日历清空。
 */
const sessionKey = computed(() => (uid.value === undefined ? '' : `${uid.value}\u0000${store.sessionEpoch}`))
const signedToday = computed(() => isSignedToday(state.value))
const progressText = computed(() => monthlyCheckInText(state.value))
/** 接口的「周」分组不一定整周，铺进网格前按真实星期重新对齐 */
const calendarWeeks = computed(() => buildCalendarWeeks(state.value))
/** 接口给的是 "33.33%" 之类的字符串，原样喂给进度条宽度前先夹到 0–100 */
const bonusProgress = computed(() => {
  const raw = Number.parseFloat((state.value?.currentProgress ?? '').replace('%', ''))
  return Number.isFinite(raw) ? Math.min(100, Math.max(0, raw)) : 0
})
const bonusRewardVisible = computed(() => {
  const s = state.value
  return !!s && (s.threeDaysCoin !== '' || s.threeDaysExp !== '' || s.sevenDaysCoin !== '' || s.sevenDaysExp !== '')
})

// 与收藏区一致：账号切换后丢弃旧请求的结果，避免把上一个账号的签到面板显示出来
let requestGeneration = 0
watch(
  sessionKey,
  async (key) => {
    requestGeneration += 1
    state.value = undefined
    errorMessage.value = ''
    if (key === '') return
    await load()
  },
  { immediate: true },
)

// KeepAlive 缓存：隔天再回到本页时日历还停在旧的一天，重新拉一次面板
let activatedOnce = false
onActivated(() => {
  if (!activatedOnce) {
    activatedOnce = true
    return
  }
  if (uid.value !== undefined && !loading.value && !signing.value) void load()
})

async function load() {
  if (uid.value === undefined) return
  const generation = ++requestGeneration
  loading.value = true
  try {
    const result = await commands.getDailyCheckIn()
    if (generation !== requestGeneration) return
    if (result.status === 'error') {
      console.error(result.error)
      errorMessage.value = formatCommandError(result.error)
      return
    }
    state.value = result.data
    errorMessage.value = ''
  } catch (error) {
    if (generation !== requestGeneration) return
    console.error(error)
    errorMessage.value = `获取签到状态失败：${getErrorMessage(error)}`
  } finally {
    if (generation === requestGeneration) loading.value = false
  }
}

async function signIn() {
  if (signing.value || signedToday.value) return
  const epoch = store.sessionEpoch
  const key = sessionKey.value
  signing.value = true
  try {
    const result = await commands.dailyCheckIn(epoch)
    // 签到期间换了账号：结果属于上一个账号，连同面板一起丢弃
    if (key !== sessionKey.value) return
    if (result.status === 'error') {
      console.error(result.error)
      message.error(formatCommandError(result.error))
      // 签到可能已提交但回查失败，刷新一次让界面与服务端对齐
      void load()
      return
    }
    // 后端已按「已签天数是否增加」判定结果，服务端文案只是提示
    state.value = result.data.state
    if (result.data.justSigned) message.success(result.data.message || '签到成功')
    else message.info(result.data.message || '今日已签到')
  } catch (error) {
    console.error(error)
    message.error(`签到失败：${getErrorMessage(error)}`)
  } finally {
    signing.value = false
  }
}

function dayClass(day: DailyCheckInRecordDay) {
  return {
    'is-today': isTodayDate(day.date),
    'is-signed': day.signed === true,
    'is-missed': day.signed === false,
    'is-bonus': day.bonus,
    'is-future': day.signed === null,
  }
}
</script>

<template>
  <section class="check-in-card">
    <header class="check-in-head">
      <div class="check-in-title">
        <PhGift :size="17" weight="duotone" class="check-in-icon" />
        <h4>{{ state?.eventName || '每日签到' }}</h4>
        <span v-if="progressText" class="check-in-progress">{{ progressText }}</span>
      </div>
      <button
        type="button"
        class="check-in-refresh"
        title="刷新签到状态"
        aria-label="刷新签到状态"
        :disabled="loading || signing"
        @click="load">
        <PhArrowClockwise :size="15" :class="{ spinning: loading }" />
      </button>
    </header>

    <div v-if="loading && state === undefined" class="check-in-loading">
      <n-spin size="small" />
    </div>

    <p v-else-if="errorMessage" class="check-in-error">
      {{ errorMessage }}
      <button type="button" class="check-in-retry" :disabled="loading" @click="load">重试</button>
    </p>

    <template v-else-if="state">
      <div v-if="calendarWeeks.length > 0" class="check-in-calendar">
        <span v-for="label in WEEKDAY_LABELS" :key="label" class="check-in-weekday">{{ label }}</span>
        <template v-for="(week, weekIndex) in calendarWeeks" :key="weekIndex">
          <template v-for="(day, dayIndex) in week" :key="day ? day.date : `blank-${weekIndex}-${dayIndex}`">
            <span v-if="day === null" class="check-in-day is-blank"></span>
            <span
              v-else
              class="check-in-day"
              :class="dayClass(day)"
              :title="`${day.date} 日${day.signed === true ? ' · 已签到' : day.bonus ? ' · 奖励日' : ''}`">
              <PhCheck v-if="day.signed === true" :size="13" weight="bold" />
              <PhGift v-else-if="day.bonus" :size="12" weight="duotone" />
              <template v-else>{{ day.date }}</template>
            </span>
          </template>
        </template>
      </div>

      <div v-if="bonusRewardVisible" class="check-in-bonus">
        <div class="check-in-bonus-bar" role="progressbar" :aria-valuenow="Math.round(bonusProgress)" aria-valuemin="0" aria-valuemax="100">
          <div class="check-in-bonus-fill" :style="{ width: `${bonusProgress}%` }"></div>
        </div>
        <div class="check-in-bonus-legend">
          <span>
            <PhCoin :size="13" weight="duotone" />
            连签 3 天
            <template v-if="state.threeDaysCoin">+{{ state.threeDaysCoin }}</template>
            <template v-if="state.threeDaysExp">/{{ state.threeDaysExp }} exp</template>
          </span>
          <span>
            <PhTrendUp :size="13" weight="duotone" />
            连签 7 天
            <template v-if="state.sevenDaysCoin">+{{ state.sevenDaysCoin }}</template>
            <template v-if="state.sevenDaysExp">/{{ state.sevenDaysExp }} exp</template>
          </span>
        </div>
      </div>

      <n-button
        type="primary"
        size="small"
        class="check-in-btn"
        :loading="signing"
        :disabled="signing || signedToday"
        @click="signIn">
        {{ signedToday ? '今日已签到' : '立即签到' }}
      </n-button>
    </template>
  </section>
</template>

<style scoped>
.check-in-card {
  display: flex;
  flex: 0 0 auto;
  flex-direction: column;
  gap: 10px;
  margin: 0 var(--jm-gutter);
  padding: 14px 16px;
  border: 1px solid var(--jm-line);
  border-radius: var(--jm-r-lg);
  background: var(--jm-surface);
}

.check-in-head {
  display: flex;
  align-items: center;
  gap: 8px;
}

.check-in-title {
  display: flex;
  min-width: 0;
  flex: 1;
  align-items: center;
  gap: 6px;
}

.check-in-icon {
  flex-shrink: 0;
  color: var(--jm-brand);
}

.check-in-title h4 {
  margin: 0;
  overflow: hidden;
  color: var(--jm-text);
  font-size: 14px;
  font-weight: 700;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.check-in-progress {
  flex-shrink: 0;
  color: var(--jm-text-3);
  font-size: 11px;
}

.check-in-refresh {
  display: flex;
  width: 30px;
  height: 30px;
  flex-shrink: 0;
  align-items: center;
  justify-content: center;
  color: var(--jm-text-3);
  border: none;
  border-radius: var(--jm-r-sm);
  background: none;
  cursor: pointer;
  transition:
    color 150ms ease,
    background-color 150ms ease;
}

.check-in-refresh:hover:not(:disabled) {
  color: var(--jm-brand);
  background: var(--jm-brand-soft);
}

.check-in-refresh:disabled {
  cursor: default;
  opacity: 0.6;
}

.spinning {
  animation: check-in-spin 0.9s linear infinite;
}

@keyframes check-in-spin {
  to {
    transform: rotate(360deg);
  }
}

.check-in-loading {
  display: flex;
  justify-content: center;
  padding: 14px 0;
}

.check-in-error {
  margin: 0;
  color: var(--jm-danger);
  font-size: var(--jm-fs-xs);
  line-height: 1.6;
}

.check-in-retry {
  margin-left: 6px;
  color: var(--jm-brand);
  border: none;
  background: none;
  font-size: var(--jm-fs-xs);
  text-decoration: underline;
  cursor: pointer;
}

/* 固定 7 列，与表头「一二三四五六日」逐列对齐 */
.check-in-calendar {
  display: grid;
  grid-template-columns: repeat(7, minmax(0, 1fr));
  gap: 6px 4px;
}

.check-in-weekday {
  color: var(--jm-text-4);
  font-size: 10px;
  font-weight: 600;
  text-align: center;
  user-select: none;
}

/* 固定高度而不是 aspect-ratio：后者会让格子随列宽变高，
   360dp 的小屏上 6 行日历能吃掉半屏，把收藏区挤到屏幕外 */
.check-in-day {
  display: flex;
  height: 30px;
  align-items: center;
  justify-content: center;
  color: var(--jm-text-2);
  border: 1px solid var(--jm-line);
  border-radius: var(--jm-r-sm);
  background: var(--jm-surface);
  font-size: 12px;
  font-weight: 600;
  font-variant-numeric: tabular-nums;
  user-select: none;
}

/* 月初/月末补位：不画框，避免看着像可签的日期 */
.check-in-day.is-blank {
  border-color: transparent;
  background: transparent;
}

.check-in-day.is-today {
  border-color: var(--jm-brand);
  box-shadow: 0 0 0 2px rgba(251, 114, 153, 0.15);
}

.check-in-day.is-signed {
  color: #fff;
  border-color: var(--jm-brand);
  background: var(--jm-brand);
}

.check-in-day.is-missed {
  color: var(--jm-text-4);
  border-style: dashed;
  background: var(--jm-sunken);
}

/* 奖励日：未签到时用礼盒图标和浅粉底提示，签完直接显示对勾 */
.check-in-day.is-bonus:not(.is-signed):not(.is-missed) {
  color: var(--jm-brand);
  border-color: rgba(251, 114, 153, 0.45);
  background: var(--jm-brand-soft);
}

.check-in-day.is-future {
  color: var(--jm-text-4);
  background: var(--jm-sunken);
}

.check-in-bonus {
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.check-in-bonus-bar {
  height: 6px;
  overflow: hidden;
  border-radius: var(--jm-r-pill);
  background: var(--jm-sunken);
}

.check-in-bonus-fill {
  height: 100%;
  border-radius: var(--jm-r-pill);
  background: linear-gradient(90deg, var(--jm-brand) 0%, var(--jm-accent) 100%);
  transition: width 0.3s;
}

.check-in-bonus-legend {
  display: flex;
  flex-wrap: wrap;
  justify-content: space-between;
  gap: 6px 12px;
  color: var(--jm-text-3);
  font-size: 11px;
}

.check-in-bonus-legend > span {
  display: flex;
  align-items: center;
  gap: 4px;
}

.check-in-btn {
  align-self: flex-start;
  border-radius: var(--jm-r-pill);
  padding: 0 20px;
}

/* 已签到时按钮处于禁用态：naive 默认给 50% 透明度，白字在浅粉底上几乎看不清。
   改成浅粉底 + 品牌色文字，既表明不可点又保持可读。 */
.check-in-btn.n-button--disabled {
  color: var(--jm-brand) !important;
  background: var(--jm-brand-soft) !important;
  opacity: 1;
}

@media (max-width: 760px) {
  .check-in-card {
    padding: 12px;
  }

  .check-in-calendar {
    gap: 4px 3px;
  }

  .check-in-day {
    height: 26px;
    border-radius: var(--jm-r-xs);
    font-size: 11px;
  }
}

/* 320dp 级别的窄屏：7 列只剩 ~34px，再缩一号，避免日期贴边 */
@media (max-width: 360px) {
  .check-in-card {
    padding: 10px;
    gap: 8px;
  }

  .check-in-calendar {
    gap: 4px 2px;
  }

  .check-in-day {
    height: 24px;
    border-radius: var(--jm-r-xs);
    font-size: 10px;
  }

  .check-in-bonus-legend {
    font-size: 10px;
  }
}
</style>
