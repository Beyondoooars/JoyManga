<script setup lang="tsx">
import { ProgressData } from '../../../types.ts'
import { ref, watchEffect, computed, onBeforeUnmount } from 'vue'
import { commands, DownloadTaskState } from '../../../bindings.ts'
import { ProgressProps, useDialog, useMessage } from 'naive-ui'
import { getErrorMessage } from '../../../utils/errors.ts'
import { describeFailure } from '../../../utils/downloadProgress.ts'
import { useStore } from '../../../store.ts'
import {
  PhPause,
  PhChecks,
  PhTrash,
  PhCaretRight,
  PhCheck,
  PhCloudArrowDown,
  PhClock,
  PhWarningCircle,
  PhTray,
  PhArrowClockwise,
  PhX,
} from '@phosphor-icons/vue'

const store = useStore()
const message = useMessage()
const dialog = useDialog()
const pendingActions = ref(new Set<number>())

const selectedIds = ref<Set<number>>(new Set())
const selectionMode = computed(() => selectedIds.value.size > 0)

const uncompletedProgresses = computed<[number, ProgressData][]>(() =>
  Array.from(store.progresses.entries())
    .filter(([, { state }]) => state !== 'Completed' && state !== 'Cancelled')
    .sort((a, b) => b[1].totalImgCount - a[1].totalImgCount),
)

watchEffect(() => {
  // 只保留未完成的章节id
  const uncompletedIds = new Set(uncompletedProgresses.value.map(([chapterId]) => chapterId))
  selectedIds.value = new Set([...selectedIds.value].filter((chapterId) => uncompletedIds.has(chapterId)))
})

function toggleSelect(chapterId: number) {
  if (selectedIds.value.has(chapterId)) selectedIds.value.delete(chapterId)
  else selectedIds.value.add(chapterId)
}

function selectAll() {
  selectedIds.value = new Set(uncompletedProgresses.value.map(([chapterId]) => chapterId))
}

function clearSelection() {
  selectedIds.value.clear()
}

// 长按进入选择：手机上没有右键，长按是批量操作的入口。
// 按住期间一旦移动就视为滚动，取消计时，避免滑动列表时误选。
let pressTimer: ReturnType<typeof setTimeout> | undefined
let pressStart: { x: number; y: number } | undefined
let longPressed = false

function clearPress() {
  if (pressTimer !== undefined) {
    clearTimeout(pressTimer)
    pressTimer = undefined
  }
  pressStart = undefined
}

function startPress(event: PointerEvent, chapterId: number) {
  if (event.button !== 0) return
  // 失败卡片里的按钮和可复制文字不参与长按选择
  if ((event.target as HTMLElement).closest('button')) return
  clearPress()
  longPressed = false
  pressStart = { x: event.clientX, y: event.clientY }
  pressTimer = setTimeout(() => {
    pressTimer = undefined
    longPressed = true
    if (navigator.vibrate) navigator.vibrate(12)
    selectedIds.value.add(chapterId)
  }, 480)
}

function movePress(event: PointerEvent) {
  if (pressStart === undefined) return
  if (Math.abs(event.clientX - pressStart.x) > 10 || Math.abs(event.clientY - pressStart.y) > 10) clearPress()
}

function endPress() {
  clearPress()
}

onBeforeUnmount(clearPress)

function handleCardClick(event: MouseEvent, chapterId: number) {
  // 长按已经进入选择时，抬起后紧跟的 click 不应再取消刚选中的这一项
  if (longPressed) {
    longPressed = false
    event.preventDefault()
    event.stopPropagation()
    return
  }
  if (!selectionMode.value) return
  toggleSelect(chapterId)
}

async function handleProgressDoubleClick(state: DownloadTaskState, chapterId: number) {
  if (selectionMode.value || pendingActions.value.has(chapterId)) return
  pendingActions.value.add(chapterId)
  try {
    const progress = store.progresses.get(chapterId)
    if (progress === undefined) return
    const result = state === 'Downloading' || state === 'Pending'
      ? await commands.pauseDownloadTask(chapterId)
      : state === 'Paused'
        ? await commands.resumeDownloadTask(chapterId)
        : await commands.createDownloadTask(progress.comic, chapterId, null)
    if (result.status === 'error') message.error(getErrorMessage(result.error))
  } catch (error) {
    message.error(getErrorMessage(error))
  } finally {
    pendingActions.value.delete(chapterId)
  }
}

async function removeFailedTask(chapterId: number) {
  if (pendingActions.value.has(chapterId)) return
  pendingActions.value.add(chapterId)
  try {
    const result = await commands.cancelDownloadTask(chapterId)
    if (result.status === 'error') message.error(getErrorMessage(result.error))
  } catch (error) {
    message.error(getErrorMessage(error))
  } finally {
    pendingActions.value.delete(chapterId)
  }
}

async function resumeSelected() {
  for (const chapterId of [...selectedIds.value]) {
    const progressData = store.progresses.get(chapterId)
    if (progressData === undefined) continue

    const { state, comic } = progressData
    const result =
      state === 'Cancelled' || state === 'Completed' || state === 'Failed'
        ? await commands.createDownloadTask(comic, chapterId, null)
        : await commands.resumeDownloadTask(chapterId)
    if (result.status === 'error') {
      console.error(result.error)
    }
  }
}

async function pauseSelected() {
  for (const chapterId of [...selectedIds.value]) {
    const progressData = store.progresses.get(chapterId)
    if (
      progressData === undefined ||
      progressData.state === 'Cancelled' ||
      progressData.state === 'Completed' ||
      progressData.state === 'Failed'
    ) {
      continue
    }
    const result = await commands.pauseDownloadTask(chapterId)
    if (result.status === 'error') {
      console.error(result.error)
    }
  }
}

async function cancelSelected() {
  for (const chapterId of [...selectedIds.value]) {
    const progressData = store.progresses.get(chapterId)
    if (
      progressData === undefined ||
      progressData.state === 'Cancelled' ||
      progressData.state === 'Completed'
    ) {
      continue
    }
    const result = await commands.cancelDownloadTask(chapterId)
    if (result.status === 'error') {
      console.error(result.error)
    }
  }
}

const batchOperating = ref(false)

const hasActiveTasks = computed(() =>
  uncompletedProgresses.value.some(([, { state }]) => state === 'Downloading' || state === 'Pending'),
)

const hasPausedTasks = computed(() =>
  uncompletedProgresses.value.some(([, { state }]) => state === 'Paused'),
)

const hasFailedTasks = computed(() =>
  uncompletedProgresses.value.some(([, { state }]) => state === 'Failed'),
)

// 批量操作：单个失败不影响其余项，结束后统一提示失败数量
async function runBatch(
  ids: number[],
  action: (chapterId: number) => Promise<{ status: 'ok' } | { status: 'error'; error: unknown }>,
) {
  let failed = 0
  for (const chapterId of ids) {
    try {
      const result = await action(chapterId)
      if (result.status === 'error') failed += 1
    } catch {
      failed += 1
    }
  }
  if (failed > 0) message.warning(`${failed} 个任务操作失败，请重试`)
}

async function pauseAll() {
  if (batchOperating.value) return
  batchOperating.value = true
  try {
    const active = uncompletedProgresses.value
      .filter(([, { state }]) => state === 'Downloading' || state === 'Pending')
      .map(([chapterId]) => chapterId)
    await runBatch(active, (chapterId) => commands.pauseDownloadTask(chapterId))
  } finally {
    batchOperating.value = false
  }
}

async function resumeAll() {
  if (batchOperating.value) return
  batchOperating.value = true
  try {
    const paused = uncompletedProgresses.value
      .filter(([, { state }]) => state === 'Paused')
      .map(([chapterId]) => chapterId)
    await runBatch(paused, (chapterId) => commands.resumeDownloadTask(chapterId))
  } finally {
    batchOperating.value = false
  }
}

async function retryAllFailed() {
  if (batchOperating.value) return
  batchOperating.value = true
  try {
    const failed = uncompletedProgresses.value.filter(([, { state }]) => state === 'Failed')
    let failures = 0
    for (const [chapterId, { comic }] of failed) {
      try {
        const result = await commands.createDownloadTask(comic, chapterId, null)
        if (result.status === 'error') failures += 1
      } catch {
        failures += 1
      }
    }
    if (failures > 0) message.warning(`${failures} 个任务重试失败`)
  } finally {
    batchOperating.value = false
  }
}

async function cancelAll() {
  if (batchOperating.value) return
  const ids = uncompletedProgresses.value.map(([chapterId]) => chapterId)
  if (ids.length === 0) return
  // “全部取消”不可恢复，先二次确认
  const confirmed = await new Promise<boolean>((resolve) => {
    dialog.warning({
      title: '取消全部任务？',
      content: `将取消 ${ids.length} 个未完成任务，已下载的临时图片会保留用于续传。`,
      positiveText: '全部取消',
      negativeText: '返回',
      onPositiveClick: () => resolve(true),
      onNegativeClick: () => resolve(false),
      onClose: () => resolve(false),
      onMaskClick: () => resolve(false),
    })
  })
  if (!confirmed) return
  batchOperating.value = true
  try {
    await runBatch(ids, (chapterId) => commands.cancelDownloadTask(chapterId))
  } finally {
    batchOperating.value = false
  }
}

function stateToStatus(state: DownloadTaskState): ProgressProps['status'] {
  if (state === 'Completed') {
    return 'success'
  } else if (state === 'Paused') {
    return 'warning'
  } else if (state === 'Failed') {
    return 'error'
  } else {
    return 'default'
  }
}

</script>

<template>
  <div class="transfer-scroll">
    <div v-if="uncompletedProgresses.length === 0" class="jm-empty transfer-empty">
      <div class="jm-empty-icon is-muted"><PhTray :size="28" weight="duotone" /></div>
      <strong>队列空空如也</strong>
      <p>在漫画详情页勾选章节，就会出现在这里。</p>
    </div>

    <template v-else>
      <div class="transfer-toolbar">
        <template v-if="selectionMode">
          <button type="button" class="batch-btn is-neutral" @click="clearSelection">
            <PhX :size="13" weight="bold" />
            退出
          </button>
          <span class="sel-count">已选 {{ selectedIds.size }} 个</span>
          <button type="button" class="batch-btn is-neutral" @click="selectAll">
            <PhChecks :size="13" weight="bold" />
            全选
          </button>
          <button type="button" class="batch-btn is-accent" @click="resumeSelected">
            <PhCaretRight :size="13" weight="bold" />
            继续
          </button>
          <button type="button" class="batch-btn" @click="pauseSelected">
            <PhPause :size="13" weight="bold" />
            暂停
          </button>
          <button type="button" class="batch-btn is-danger" @click="cancelSelected">
            <PhTrash :size="13" weight="bold" />
            取消
          </button>
        </template>
        <template v-else>
          <button v-if="hasActiveTasks" type="button" class="batch-btn" :disabled="batchOperating" @click="pauseAll">
            <PhPause :size="13" weight="bold" />
            全部暂停
          </button>
          <button v-if="hasPausedTasks" type="button" class="batch-btn is-accent" :disabled="batchOperating" @click="resumeAll">
            <PhCaretRight :size="13" weight="bold" />
            全部继续
          </button>
          <button v-if="hasFailedTasks" type="button" class="batch-btn is-danger" :disabled="batchOperating" @click="retryAllFailed">
            <PhArrowClockwise :size="13" weight="bold" />
            重试失败
          </button>
          <button type="button" class="batch-btn is-neutral" :disabled="batchOperating" @click="cancelAll">全部取消</button>
        </template>
      </div>

      <div class="task-list">
        <div
          v-for="[chapterId, { state, comic, chapterInfo, percentage, indicator, failure, errorMessage }] in uncompletedProgresses"
          :key="chapterId"
          class="task-card"
          :class="{ 'is-selected': selectedIds.has(chapterId), 'is-failed': state === 'Failed' }"
          :aria-selected="selectionMode ? selectedIds.has(chapterId) : undefined"
          @click="(event) => handleCardClick(event, chapterId)"
          @dblclick="() => handleProgressDoubleClick(state, chapterId)"
          @pointerdown="(event) => startPress(event, chapterId)"
          @pointermove="movePress"
          @pointerup="endPress"
          @pointerleave="endPress"
          @pointercancel="endPress">
          <div class="task-head">
            <span class="task-state-icon" :data-state="state">
              <PhCloudArrowDown v-if="state === 'Downloading'" :size="14" weight="bold" />
              <PhClock v-else-if="state === 'Pending'" :size="14" weight="bold" />
              <PhPause v-else-if="state === 'Paused'" :size="14" weight="bold" />
              <PhWarningCircle v-else-if="state === 'Failed'" :size="14" weight="bold" />
            </span>
            <div class="task-titles">
              <strong :title="comic.name">{{ comic.name }}</strong>
              <span :title="chapterInfo.chapterTitle">{{ chapterInfo.chapterTitle }}</span>
            </div>
            <span v-if="selectionMode" class="task-check" :class="{ 'is-on': selectedIds.has(chapterId) }" aria-hidden="true">
              <PhCheck v-if="selectedIds.has(chapterId)" :size="12" weight="bold" />
            </span>
            <span class="task-indicator">{{ indicator }}</span>
          </div>

          <n-progress
            v-if="!isNaN(percentage)"
            class="task-progress"
            :status="stateToStatus(state)"
            :percentage="percentage"
            :processing="state === 'Downloading'"
            :show-indicator="false"
            :height="4" />

          <div v-if="state === 'Failed'" class="task-error" @dblclick.stop @mousedown.stop @pointerdown.stop>
            <div v-if="failure" class="task-failure-meta">{{ describeFailure(failure) }}</div>
            <div role="status">{{ failure?.message || errorMessage || '下载失败，请重试或查看日志' }}</div>
            <div class="task-error-actions">
              <button type="button" class="tiny-btn" :disabled="pendingActions.has(chapterId)" @click.stop="handleProgressDoubleClick(state, chapterId)">
                {{ pendingActions.has(chapterId) ? '正在重试…' : '重试 / 补全' }}
              </button>
              <button type="button" class="tiny-btn is-ghost" :disabled="pendingActions.has(chapterId)" @click.stop="removeFailedTask(chapterId)">
                移除任务
              </button>
            </div>
          </div>
        </div>
      </div>
    </template>
  </div>
</template>

<style scoped>
.transfer-scroll {
  height: 100%;
  overflow-y: auto;
  padding: 12px var(--jm-gutter) calc(24px + var(--safe-bottom));
  user-select: none;
}

.transfer-empty {
  min-height: 260px;
}

.transfer-toolbar {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 6px;
  margin-bottom: 10px;
}

.batch-btn {
  display: inline-flex;
  height: 32px;
  align-items: center;
  gap: 4px;
  padding: 0 12px;
  color: var(--jm-brand);
  border: 1px solid rgba(251, 114, 153, 0.26);
  border-radius: var(--jm-r-pill);
  background: var(--jm-brand-soft);
  font-size: 11.5px;
  font-weight: 650;
  white-space: nowrap;
  cursor: pointer;
}

.batch-btn:disabled {
  opacity: 0.55;
  cursor: wait;
}

.batch-btn.is-accent {
  color: var(--jm-accent);
  border-color: rgba(0, 174, 236, 0.26);
  background: var(--jm-accent-soft);
}

.batch-btn.is-danger {
  color: var(--jm-danger);
  border-color: rgba(222, 59, 59, 0.22);
  background: var(--jm-danger-soft);
}

.batch-btn.is-neutral {
  color: var(--jm-text-2);
  border-color: var(--jm-line);
  background: var(--jm-surface);
}

.sel-count {
  display: inline-flex;
  height: 32px;
  align-items: center;
  padding: 0 4px;
  color: var(--jm-brand);
  font-size: 11.5px;
  font-weight: 700;
  font-variant-numeric: tabular-nums;
  white-space: nowrap;
}

.task-list {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.task-card {
  padding: 11px 12px;
  border: 1px solid var(--jm-line);
  border-radius: var(--jm-r-md);
  background: var(--jm-surface);
  cursor: pointer;
  touch-action: pan-y;
  transition:
    border-color 160ms ease,
    background-color 160ms ease;
}

.task-card.is-selected {
  border-color: var(--jm-brand);
  background: var(--jm-brand-soft);
}

.task-card.is-failed {
  border-color: rgba(222, 59, 59, 0.24);
}

.task-head {
  display: flex;
  align-items: center;
  gap: 8px;
}

.task-state-icon {
  display: grid;
  width: 26px;
  height: 26px;
  flex: 0 0 auto;
  place-items: center;
  border-radius: var(--jm-r-sm);
}

.task-state-icon[data-state='Downloading'] {
  color: var(--jm-accent);
  background: var(--jm-accent-soft);
}

.task-state-icon[data-state='Pending'] {
  color: var(--jm-text-3);
  background: var(--jm-sunken);
}

.task-state-icon[data-state='Paused'] {
  color: var(--jm-warning);
  background: var(--jm-warning-soft);
}

.task-state-icon[data-state='Failed'] {
  color: var(--jm-danger);
  background: var(--jm-danger-soft);
}

.task-titles {
  display: flex;
  min-width: 0;
  flex: 1;
  flex-direction: column;
  gap: 1px;
}

.task-titles strong {
  overflow: hidden;
  color: var(--jm-text);
  font-size: 12.5px;
  font-weight: 650;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.task-titles span {
  overflow: hidden;
  color: var(--jm-text-3);
  font-size: 10.5px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.task-check {
  display: grid;
  width: 20px;
  height: 20px;
  flex: 0 0 auto;
  color: transparent;
  place-items: center;
  border: 1.5px solid var(--jm-line);
  border-radius: var(--jm-r-pill);
  background: var(--jm-surface);
}

.task-check.is-on {
  color: #fff;
  border-color: var(--jm-brand);
  background: var(--jm-brand);
}

.task-indicator {
  flex: 0 0 auto;
  color: var(--jm-text-3);
  font-family: ui-monospace, 'SF Mono', Menlo, monospace;
  font-size: 10px;
  font-variant-numeric: tabular-nums;
}

.task-progress {
  margin-top: 9px;
}

.task-progress :deep(.n-progress-graph-line) {
  border-radius: var(--jm-r-pill);
}

.task-error {
  margin-top: 9px;
  padding-top: 9px;
  border-top: 1px dashed rgba(222, 59, 59, 0.2);
  color: var(--jm-danger);
  font-size: var(--jm-fs-xs);
  user-select: text;
}

.task-failure-meta {
  color: var(--jm-text-2);
  font-size: 10.5px;
  font-weight: 650;
}

.task-failure-meta + div {
  margin-top: 3px;
}

.task-error [role='status'] {
  max-height: 96px;
  overflow: auto;
  overflow-wrap: anywhere;
  white-space: pre-wrap;
}

.task-error-actions {
  display: flex;
  gap: 7px;
  margin-top: 8px;
}

.tiny-btn {
  height: 30px;
  padding: 0 12px;
  color: var(--jm-danger);
  border: 1px solid rgba(222, 59, 59, 0.24);
  border-radius: var(--jm-r-pill);
  background: var(--jm-surface);
  font-size: 11.5px;
  font-weight: 650;
  cursor: pointer;
}

.tiny-btn.is-ghost {
  color: var(--jm-text-2);
  border-color: var(--jm-line);
}

.tiny-btn:disabled {
  opacity: 0.55;
  cursor: wait;
}
</style>
