<script setup lang="tsx">
import { ProgressData } from '../../../types.ts'
import { ref, watchEffect, computed } from 'vue'
import { SelectionArea, SelectionEvent } from '@viselect/vue'
import { commands, DownloadTaskState } from '../../../bindings.ts'
import { DropdownOption, NIcon, ProgressProps, useDialog, useMessage } from 'naive-ui'
import { getErrorMessage } from '../../../utils/errors.ts'
import { useStore } from '../../../store.ts'
import {
  PhPause,
  PhChecks,
  PhTrash,
  PhCaretRight,
  PhCloudArrowDown,
  PhClock,
  PhWarningCircle,
  PhTray,
} from '@phosphor-icons/vue'

import { useDropdown } from '../../../composables/useDropdown.ts'

const store = useStore()
const message = useMessage()
const dialog = useDialog()
const pendingActions = ref(new Set<number>())

const selectedIds = ref<Set<number>>(new Set())
const selectionAreaRef = ref<InstanceType<typeof SelectionArea>>()
const selectableRefs = ref<HTMLDivElement[]>([])
const { dropdownX, dropdownY, dropdownShowing, showDropdown, closeDropdown } = useDropdown()

const dropdownOptions: DropdownOption[] = [
  {
    label: '全选',
    key: 'select-all',
    icon: () => (
      <NIcon size="20">
        <PhChecks />
      </NIcon>
    ),
    props: {
      onClick: () => {
        if (selectionAreaRef.value === undefined) {
          return
        }
        const selection = selectionAreaRef.value.selection
        if (selection === undefined) {
          return
        }
        selection.select(selectableRefs.value)
        dropdownShowing.value = false
      },
    },
  },
  {
    label: '继续',
    key: 'resume',
    icon: () => (
      <NIcon size="20">
        <PhCaretRight />
      </NIcon>
    ),
    props: {
      onClick: () => {
        void resumeSelected()
        dropdownShowing.value = false
      },
    },
  },
  {
    label: '暂停',
    key: 'pause',
    icon: () => (
      <NIcon size="20">
        <PhPause />
      </NIcon>
    ),
    props: {
      onClick: () => {
        void pauseSelected()
        dropdownShowing.value = false
      },
    },
  },
  {
    label: '取消',
    key: 'cancel',
    icon: () => (
      <NIcon size="20">
        <PhTrash />
      </NIcon>
    ),
    props: {
      onClick: () => {
        void cancelSelected()
        dropdownShowing.value = false
      },
    },
  },
]

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

function extractIds(elements: Element[]): number[] {
  return elements
    .map((element) => element.getAttribute('data-key'))
    .filter(Boolean)
    .map(Number)
}

function updateSelectedIds({
  store: {
    changed: { added, removed },
  },
}: SelectionEvent) {
  extractIds(added).forEach((chapterId) => selectedIds.value.add(chapterId))
  extractIds(removed).forEach((chapterId) => selectedIds.value.delete(chapterId))
}

function unselectAll({ event, selection }: SelectionEvent) {
  if (!event?.ctrlKey && !event?.metaKey) {
    selection.clearSelection()
    selectedIds.value.clear()
  }
}

async function handleProgressDoubleClick(state: DownloadTaskState, chapterId: number) {
  if (pendingActions.value.has(chapterId)) return
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

function handleProgressContextMenu(chapterId: number) {
  if (selectedIds.value.has(chapterId)) {
    return
  }
  selectedIds.value.clear()
  selectedIds.value.add(chapterId)
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

function stateToColorClass(state: DownloadTaskState) {
  if (state === 'Downloading') {
    return 'text-blue-500'
  } else if (state === 'Pending') {
    return 'text-gray-500'
  } else if (state === 'Paused') {
    return 'text-yellow-500'
  } else if (state === 'Failed') {
    return 'text-red-500'
  } else if (state === 'Completed') {
    return 'text-green-500'
  } else if (state === 'Cancelled') {
    return 'text-stone-500'
  }

  return ''
}
</script>

<template>
  <SelectionArea
    ref="selectionAreaRef"
    class="h-full flex flex-col selection-container px-2"
    :options="{ selectables: '.selectable', features: { deselectOnBlur: true } }"
    @contextmenu="showDropdown"
    @move="updateSelectedIds"
    @start="unselectAll">
    <div v-if="uncompletedProgresses.length === 0" class="progress-empty">
      <div><PhTray :size="27" weight="duotone" /></div>
      <strong>队列空空如也</strong>
      <span>从左侧选择漫画章节开始下载</span>
    </div>
    <div v-else class="flex flex-col gap-1 mb-2">
      <div class="flex items-center gap-1.5 flex-wrap">
        <n-button
          v-if="hasActiveTasks"
          size="tiny"
          secondary
          type="warning"
          :disabled="batchOperating"
          @click="pauseAll">
          全部暂停
        </n-button>
        <n-button
          v-if="hasPausedTasks"
          size="tiny"
          secondary
          type="primary"
          :disabled="batchOperating"
          @click="resumeAll">
          全部继续
        </n-button>
        <n-button
          v-if="hasFailedTasks"
          size="tiny"
          secondary
          type="error"
          :disabled="batchOperating"
          @click="retryAllFailed">
          重试失败
        </n-button>
        <n-button
          size="tiny"
          secondary
          :disabled="batchOperating"
          class="ml-auto"
          @click="cancelAll">
          全部取消
        </n-button>
      </div>
      <span class="progress-hint">拖动框选 · 右键操作 · 双击暂停/继续</span>
    </div>
    <div class="h-full select-none">
      <div
        v-for="[chapterId, { state, comic, chapterInfo, percentage, indicator, errorMessage }] in uncompletedProgresses"
        :key="chapterId"
        ref="selectableRefs"
        :data-key="chapterId"
        :class="[
          'selectable p-3 mb-2 rounded-lg',
          selectedIds.has(chapterId) ? 'selected shadow-md' : 'hover:bg-gray-1',
        ]"
        @dblclick="() => handleProgressDoubleClick(state, chapterId)"
        @contextmenu="() => handleProgressContextMenu(chapterId)">
        <div class="grid grid-cols-[minmax(0,1fr)_minmax(0,1fr)] gap-2">
          <div class="text-ellipsis whitespace-nowrap overflow-hidden" :title="comic.name">
            {{ comic.name }}
          </div>
          <div class="text-ellipsis whitespace-nowrap overflow-hidden" :title="chapterInfo.chapterTitle">
            {{ chapterInfo.chapterTitle }}
          </div>
        </div>
        <div class="flex">
          <n-icon :class="[stateToColorClass(state), 'mr-2']" :size="20">
            <PhCloudArrowDown v-if="state === 'Downloading'" />
            <PhClock v-else-if="state === 'Pending'" />
            <PhPause v-else-if="state === 'Paused'" />
            <PhWarningCircle v-else-if="state === 'Failed'" />
          </n-icon>
          <div v-if="isNaN(percentage)" class="ml-auto">{{ indicator }}</div>
          <n-progress
            v-else
            :class="stateToColorClass(state)"
            :status="stateToStatus(state)"
            :percentage="percentage"
            :processing="state === 'Downloading'">
            {{ indicator }}
          </n-progress>
        </div>
        <div v-if="state === 'Failed'" class="task-error" @dblclick.stop @mousedown.stop>
          <div role="status">{{ errorMessage || '下载失败，请重试或查看日志' }}</div>
          <div class="flex gap-2 mt-2">
            <n-button size="tiny" :loading="pendingActions.has(chapterId)" @click.stop="handleProgressDoubleClick(state, chapterId)">
              重试 / 补全
            </n-button>
            <n-button size="tiny" :disabled="pendingActions.has(chapterId)" @click.stop="removeFailedTask(chapterId)">
              移除任务
            </n-button>
          </div>
        </div>
      </div>
    </div>
    <n-dropdown
      placement="bottom-start"
      trigger="manual"
      :x="dropdownX"
      :y="dropdownY"
      :options="dropdownOptions"
      :show="dropdownShowing"
      :on-clickoutside="closeDropdown" />
  </SelectionArea>
</template>

<style scoped>
.task-error {
  margin-top: 8px;
  color: #b42318;
  font-size: 12px;
  user-select: text;
}

.task-error > div:first-child {
  max-height: 96px;
  overflow: auto;
  overflow-wrap: anywhere;
  white-space: pre-wrap;
}

.selection-container {
  @apply select-none overflow-auto;
}

.selection-container .selected {
  background-color: rgba(251, 114, 153, 0.12) !important;
  border-color: #fb7299 !important;
}

.progress-empty {
  display: flex;
  min-height: 230px;
  flex: 1;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  color: #8b877f;
  text-align: center;
}

.progress-empty div {
  display: grid;
  width: 58px;
  height: 58px;
  margin-bottom: 14px;
  color: #fb7299;
  place-items: center;
  border-radius: 17px;
  background: rgba(251, 114, 153, 0.1);
}

.progress-empty strong {
  color: #4d4a45;
  font-size: 13px;
}

.progress-empty span,
.progress-hint {
  margin-top: 5px;
  color: #aaa59d;
  font-size: 9px;
}

.progress-hint {
  display: block;
  margin: 2px 0 8px auto;
  padding: 0 3px;
}

:global(.selection-area) {
  background-color: rgba(251, 114, 153, 0.18) !important;
  border: 1px solid #fb7299 !important;
}
</style>
