<script setup lang="ts">
import { ProgressData } from '../../../types.ts'
import { ref, watch, computed } from 'vue'
import { commands, DownloadTaskState } from '../../../bindings.ts'
import { ProgressProps, useDialog, useMessage } from 'naive-ui'
import { getErrorMessage, formatCommandError } from '../../../utils/errors.ts'
import { describeFailure } from '../../../utils/downloadProgress.ts'
import { useStore } from '../../../store.ts'
import { openDialogWithBack } from '../../../composables/useDialogBack.ts'
import { useBackHandler } from '../../../composables/useAndroidBack.ts'
import { useLongPress } from '../../../composables/useLongPress.ts'
import {
  PhPause,
  PhChecks,
  PhTrash,
  PhCaretRight,
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
const selectionMode = ref(false)

const stateOrder: Record<DownloadTaskState, number> = {
  Downloading: 0,
  Pending: 1,
  Paused: 2,
  Failed: 3,
  Completed: 4,
  Cancelled: 5,
}

const uncompletedProgresses = computed<[number, ProgressData][]>(() =>
  Array.from(store.progresses.entries())
    .filter(([, { state }]) => state !== 'Completed' && state !== 'Cancelled')
    .sort(
      (a, b) =>
        (stateOrder[a[1].state] ?? 9) - (stateOrder[b[1].state] ?? 9) ||
        (a[1].createdSeq ?? 0) - (b[1].createdSeq ?? 0) ||
        a[0] - b[0],
    ),
)

watch(uncompletedProgresses, (list) => {
  if (list.length === 0) {
    if (selectedIds.value.size > 0) selectedIds.value.clear()
    selectionMode.value = false
    return
  }
  if (selectedIds.value.size === 0) return
  const uncompletedIds = new Set(list.map(([chapterId]) => chapterId))
  let changed = false
  const next = new Set<number>()
  for (const chapterId of selectedIds.value) {
    if (uncompletedIds.has(chapterId)) next.add(chapterId)
    else changed = true
  }
  if (changed) selectedIds.value = next
})

useBackHandler(() => {
  if (!selectionMode.value || store.currentTabName !== 'downloaded' || store.downloadedSubTab !== 'transfers') {
    return false
  }
  clearSelection()
  return true
}, 30)

function toggleSelect(chapterId: number) {
  selectionMode.value = true
  if (selectedIds.value.has(chapterId)) selectedIds.value.delete(chapterId)
  else selectedIds.value.add(chapterId)
}

const allSelected = computed(
  () => uncompletedProgresses.value.length > 0 && selectedIds.value.size >= uncompletedProgresses.value.length,
)

function toggleSelectAll() {
  selectionMode.value = true
  if (allSelected.value) {
    selectedIds.value.clear()
  } else {
    selectedIds.value = new Set(uncompletedProgresses.value.map(([chapterId]) => chapterId))
  }
}

function clearSelection() {
  selectedIds.value.clear()
  selectionMode.value = false
}

// 长按进入选择：手机上没有右键，长按是批量操作的入口；失败卡片里的按钮不参与长按选择。
const { startPress, movePress, endPress, consumeLongPressClick } = useLongPress<number>({
  shouldStart: (event) => !(event.target as HTMLElement | null)?.closest('button'),
  onLongPress: (chapterId) => {
    selectionMode.value = true
    selectedIds.value.add(chapterId)
  },
})

function handleCardClick(event: MouseEvent, chapterId: number) {
  // 长按已经进入选择时，抬起后紧跟的 click 不应再取消刚选中的这一项
  if (consumeLongPressClick(event)) return
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
    if (result.status === 'error') message.error(formatCommandError(result.error))
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
    if (result.status === 'error') message.error(formatCommandError(result.error))
  } catch (error) {
    message.error(getErrorMessage(error))
  } finally {
    pendingActions.value.delete(chapterId)
  }
}

const batchOperating = ref(false)

type BatchActionResult = { status: 'ok' } | { status: 'error'; error: unknown }

// 批量操作统一执行器：单个失败不影响其余项，action 返回 undefined 表示跳过，结束后统一提示失败数量
async function runBatch<T>(
  items: Iterable<T>,
  action: (item: T) => Promise<BatchActionResult | undefined>,
  failureMessage = (failed: number) => `${failed} 个任务操作失败，请重试`,
) {
  if (batchOperating.value) return
  batchOperating.value = true
  let failed = 0
  try {
    for (const item of items) {
      try {
        const result = await action(item)
        if (result === undefined) continue
        if (result.status === 'error') {
          console.error(result.error)
          failed += 1
        }
      } catch (error) {
        console.error(error)
        failed += 1
      }
    }
    if (failed > 0) message.warning(failureMessage(failed))
  } finally {
    batchOperating.value = false
  }
}

async function resumeSelected() {
  await runBatch(
    [...selectedIds.value],
    async (chapterId) => {
      const progressData = store.progresses.get(chapterId)
      if (progressData === undefined) return undefined
      const { state, comic } = progressData
      if (state === 'Completed' || state === 'Downloading' || state === 'Pending') return undefined
      return state === 'Cancelled' || state === 'Failed'
        ? commands.createDownloadTask(comic, chapterId, null)
        : commands.resumeDownloadTask(chapterId)
    },
    (failed) => `${failed} 个任务继续失败`,
  )
}

async function pauseSelected() {
  await runBatch(
    [...selectedIds.value],
    async (chapterId) => {
      const progressData = store.progresses.get(chapterId)
      if (
        progressData === undefined ||
        progressData.state === 'Cancelled' ||
        progressData.state === 'Completed' ||
        progressData.state === 'Failed'
      ) {
        return undefined
      }
      return commands.pauseDownloadTask(chapterId)
    },
    (failed) => `${failed} 个任务暂停失败`,
  )
}

async function cancelSelected() {
  await runBatch(
    [...selectedIds.value],
    async (chapterId) => {
      const progressData = store.progresses.get(chapterId)
      if (
        progressData === undefined ||
        progressData.state === 'Cancelled' ||
        progressData.state === 'Completed'
      ) {
        return undefined
      }
      return commands.cancelDownloadTask(chapterId)
    },
    (failed) => `${failed} 个任务取消失败`,
  )
}

const hasActiveTasks = computed(() =>
  uncompletedProgresses.value.some(([, { state }]) => state === 'Downloading' || state === 'Pending'),
)

const hasPausedTasks = computed(() =>
  uncompletedProgresses.value.some(([, { state }]) => state === 'Paused'),
)

const hasFailedTasks = computed(() =>
  uncompletedProgresses.value.some(([, { state }]) => state === 'Failed'),
)

async function pauseAll() {
  const active = uncompletedProgresses.value
    .filter(([, { state }]) => state === 'Downloading' || state === 'Pending')
    .map(([chapterId]) => chapterId)
  await runBatch(active, (chapterId) => commands.pauseDownloadTask(chapterId))
}

async function resumeAll() {
  const paused = uncompletedProgresses.value
    .filter(([, { state }]) => state === 'Paused')
    .map(([chapterId]) => chapterId)
  await runBatch(paused, (chapterId) => commands.resumeDownloadTask(chapterId))
}

async function retryAllFailed() {
  const failed = uncompletedProgresses.value.filter(([, { state }]) => state === 'Failed')
  await runBatch(
    failed,
    ([chapterId, { comic }]) => commands.createDownloadTask(comic, chapterId, null),
    (count) => `${count} 个任务重试失败`,
  )
}

async function cancelAll() {
  if (batchOperating.value) return
  const ids = uncompletedProgresses.value.map(([chapterId]) => chapterId)
  if (ids.length === 0) return
  // “全部取消”不可恢复，先二次确认；返回/Escape 与其他关闭路径都结束等待。
  const confirmed = await openDialogWithBack(dialog, {
    title: '取消全部任务？',
    content: `将取消 ${ids.length} 个未完成任务，已下载的临时图片会保留用于续传。`,
    positiveText: '全部取消',
    negativeText: '返回',
  })
  if (!confirmed) return
  await runBatch(ids, (chapterId) => commands.cancelDownloadTask(chapterId))
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
          <button type="button" class="batch-btn is-neutral" :disabled="batchOperating" @click="clearSelection">
            <PhX :size="13" weight="bold" />
            退出
          </button>
          <span class="sel-count">已选 {{ selectedIds.size }} 个</span>
          <button type="button" class="batch-btn is-neutral" :disabled="batchOperating" @click="toggleSelectAll">
            <PhChecks :size="13" weight="bold" />
            {{ allSelected ? '取消全选' : '全选' }}
          </button>
          <button type="button" class="batch-btn is-accent" :disabled="batchOperating" @click="resumeSelected">
            <PhCaretRight :size="13" weight="bold" />
            继续
          </button>
          <button type="button" class="batch-btn" :disabled="batchOperating" @click="pauseSelected">
            <PhPause :size="13" weight="bold" />
            暂停
          </button>
          <button type="button" class="batch-btn is-danger" :disabled="batchOperating" @click="cancelSelected">
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
            <span v-if="selectionMode" class="task-check" :class="{ 'is-on': selectedIds.has(chapterId) }" aria-hidden="true"></span>
            <span class="task-indicator">{{ indicator }}</span>
            <button
              v-if="!selectionMode"
              type="button"
              class="task-action-btn"
              :class="{ 'is-failed': state === 'Failed', 'is-paused': state === 'Paused' }"
              :disabled="pendingActions.has(chapterId)"
              :title="state === 'Downloading' || state === 'Pending' ? '暂停' : state === 'Paused' ? '继续' : '重试'"
              :aria-label="state === 'Downloading' || state === 'Pending' ? '暂停' : state === 'Paused' ? '继续' : '重试'"
              @click.stop="handleProgressDoubleClick(state, chapterId)">
              <PhPause v-if="state === 'Downloading' || state === 'Pending'" :size="13" weight="bold" />
              <PhCaretRight v-else-if="state === 'Paused'" :size="13" weight="fill" />
              <PhArrowClockwise v-else-if="state === 'Failed'" :size="13" weight="bold" />
            </button>
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
  place-items: center;
  border: 1.5px solid var(--jm-line);
  border-radius: var(--jm-r-pill);
  background: var(--jm-surface);
}

.task-check.is-on {
  border-color: var(--jm-brand);
  background: var(--jm-brand);
}

.task-check.is-on::after {
  content: '';
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: #fff;
}

.task-action-btn {
  display: inline-flex;
  width: 28px;
  height: 28px;
  flex: 0 0 auto;
  align-items: center;
  justify-content: center;
  padding: 0;
  color: var(--jm-text-2);
  border: 1px solid var(--jm-line);
  border-radius: 50%;
  background: var(--jm-surface);
  cursor: pointer;
  transition:
    color 160ms ease,
    border-color 160ms ease,
    background-color 160ms ease,
    transform 120ms ease;
}

.task-action-btn:hover {
  border-color: var(--jm-brand);
  color: var(--jm-brand);
}

.task-action-btn:active {
  transform: scale(0.92);
}

.task-action-btn.is-paused {
  color: var(--jm-accent);
  border-color: rgba(0, 194, 255, 0.35);
  background: var(--jm-accent-soft);
}

.task-action-btn.is-failed {
  color: var(--jm-danger);
  border-color: rgba(222, 59, 59, 0.35);
  background: var(--jm-danger-soft);
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
