<script setup lang="tsx">
import { computed, ref, watchEffect } from 'vue'
import { NIcon, DropdownOption } from 'naive-ui'
import { PhChecks, PhTrash } from '@phosphor-icons/vue'
import { SelectionArea, SelectionEvent } from '@viselect/vue'
import ExportProgress from './ExportProgress.vue'
import { useStore } from '../../../store.ts'
import { useDropdown } from '../../../composables/useDropdown.ts'

// 进度数据由 store 中常驻的导出事件监听器维护：
// 无论从书库还是传输页发起导出，都不会丢失进度。
// 已完成项只在本次会话内保留，超过上限时淘汰最早的记录，避免长期运行无限增长。
export interface ProgressData {
  uuid: string
  exportType: 'cbz' | 'pdf'
  state: 'Processing' | 'Error' | 'End'
  comicTitle: string
  current: number
  total: number
  percentage: number
  indicator: string
  chapterExportDir?: string
}

const store = useStore()
// 使用 computed 而非拷贝：删除操作直接作用于 store 中的 Map，导出监听器继续更新剩余项
const progresses = computed(() => store.exportProgresses as Map<string, ProgressData>)

const selectedIds = ref<Set<string>>(new Set())
const { dropdownX, dropdownY, dropdownShowing, showDropdown, closeDropdown } = useDropdown()

const dropdownOptions: DropdownOption[] = [
  {
    label: '全选',
    key: 'check all',
    icon: () => (
      <NIcon size="20">
        <PhChecks />
      </NIcon>
    ),
    props: {
      onClick: () => {
        progresses.value.forEach((_, uuid) => selectedIds.value.add(uuid))
        dropdownShowing.value = false
      },
    },
  },
  {
    label: '删除',
    key: 'delete',
    icon: () => (
      <NIcon size="20">
        <PhTrash />
      </NIcon>
    ),
    props: {
      onClick: () => {
        selectedIds.value.forEach((uuid) => progresses.value.delete(uuid))
        dropdownShowing.value = false
      },
    },
  },
]

const MAX_FINISHED_EXPORTS = 50
watchEffect(() => {
  // 保证selectedIds中的uuid在progresses中存在
  const uuids = new Set(progresses.value.keys())
  for (const uuid of selectedIds.value) {
    if (!uuids.has(uuid)) {
      selectedIds.value.delete(uuid)
    }
  }
  // 只淘汰已结束的记录，进行中的导出永远保留（Map 迭代顺序即插入顺序）
  const finished = [...progresses.value.keys()].filter((uuid) => progresses.value.get(uuid)?.state !== 'Processing')
  for (const uuid of finished.slice(0, Math.max(0, finished.length - MAX_FINISHED_EXPORTS))) {
    progresses.value.delete(uuid)
  }
})

function extractIds(elements: Element[]): string[] {
  return elements
    .map((element) => element.getAttribute('data-key'))
    .filter(Boolean)
    .filter((uuid) => uuid !== null)
}

function updateSelectedIds({
  store: {
    changed: { added, removed },
  },
}: SelectionEvent) {
  extractIds(added).forEach((uuid) => selectedIds.value.add(uuid))
  extractIds(removed).forEach((uuid) => selectedIds.value.delete(uuid))
}

function unselectAll({ event, selection }: SelectionEvent) {
  if (!event?.ctrlKey && !event?.metaKey) {
    selection.clearSelection()
    selectedIds.value.clear()
  }
}

function handleProgressContextMenu(p: ProgressData) {
  if (selectedIds.value.has(p.uuid)) {
    return
  }
  selectedIds.value.clear()
  selectedIds.value.add(p.uuid)
}
</script>

<template>
  <SelectionArea
    class="h-full flex flex-col selection-container px-2"
    :options="{ selectables: '.selectable', features: { deselectOnBlur: true } }"
    @contextmenu="showDropdown"
    @move="updateSelectedIds"
    @start="unselectAll">
    <div v-if="progresses.size === 0" class="export-empty">
      <div><PhExport :size="27" weight="duotone" /></div>
      <strong>暂无导出任务</strong>
      <span>从本地库存导出 CBZ 或 PDF</span>
    </div>
    <div v-else class="flex">
      <span class="export-hint">拖动框选 · 右键操作</span>
    </div>

    <ExportProgress
      v-for="[uuid, p] in progresses"
      :key="uuid"
      :data-key="uuid"
      :class="['selectable ', selectedIds.has(uuid) ? 'selected shadow-md' : 'hover:bg-gray-1']"
      :p="p"
      :handle-context-menu="handleProgressContextMenu" />

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
.selection-container {
  @apply select-none overflow-auto;
}

.selection-container .selected {
  background-color: rgba(251, 114, 153, 0.12) !important;
  border-color: #fb7299 !important;
}

.export-empty {
  display: flex;
  min-height: 230px;
  flex: 1;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  color: #aaa59d;
  text-align: center;
}

.export-empty div {
  display: grid;
  width: 58px;
  height: 58px;
  margin-bottom: 14px;
  color: #fb7299;
  place-items: center;
  border-radius: 17px;
  background: rgba(251, 114, 153, 0.1);
}

.export-empty strong {
  color: #4d4a45;
  font-size: 13px;
}

.export-empty span,
.export-hint {
  margin-top: 5px;
  color: #aaa59d;
  font-size: 9px;
}

.export-hint {
  margin: 2px 3px 8px auto;
}

:global(.selection-area) {
  background-color: rgba(251, 114, 153, 0.18) !important;
  border: 1px solid #fb7299 !important;
}
</style>
