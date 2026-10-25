<script setup lang="tsx">
import { computed, ref, watchEffect } from 'vue'
import { NIcon, DropdownOption } from 'naive-ui'
import { PhChecks, PhExport, PhTrash } from '@phosphor-icons/vue'
import { SelectionArea, SelectionEvent } from '@viselect/vue'
import ExportProgress from './ExportProgress.vue'
import { useStore } from '../../../store.ts'
import { useDropdown } from '../../../composables/useDropdown.ts'

// 进度数据由 store 中常驻的导出事件监听器维护：
// 无论从书库还是传输页发起导出，都不会丢失进度。
// 已完成项只在本次会话内保留，超过上限时淘汰最早的记录，避免长期运行无限增长。
export interface ProgressData {
  uuid: string
  exportType: 'cbz'
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
    class="export-container"
    :options="{ selectables: '.selectable', features: { deselectOnBlur: true } }"
    @contextmenu="showDropdown"
    @move="updateSelectedIds"
    @start="unselectAll">
    <div v-if="progresses.size === 0" class="jm-empty export-empty">
      <div class="jm-empty-icon is-muted"><PhExport :size="28" weight="duotone" /></div>
      <strong>暂无导出任务</strong>
      <p>在书库勾选漫画后点「导出」，进度会显示在这里。</p>
    </div>

    <template v-else>
      <div class="export-head">
        <span>{{ progresses.size }} 个导出任务</span>
        <span class="export-hint">拖动框选 · 右键操作</span>
      </div>

      <div class="export-list">
        <ExportProgress
          v-for="[uuid, p] in progresses"
          :key="uuid"
          :data-key="uuid"
          class="selectable"
          :class="{ 'is-selected': selectedIds.has(uuid) }"
          :p="p"
          :handle-context-menu="handleProgressContextMenu" />
      </div>
    </template>

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
.export-container {
  height: 100%;
  overflow-y: auto;
  padding: 12px var(--jm-gutter) calc(24px + var(--safe-bottom));
  user-select: none;
}

.export-empty {
  min-height: 260px;
}

.export-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
  padding-bottom: 10px;
  color: var(--jm-text-3);
  font-size: var(--jm-fs-xs);
}

.export-hint {
  color: var(--jm-text-4);
  font-size: 9.5px;
}

.export-list {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

:global(.selection-area) {
  background-color: rgba(251, 114, 153, 0.14) !important;
  border: 1px solid var(--jm-brand) !important;
  border-radius: 2px;
}

.selectable.is-selected :deep(.export-card) {
  border-color: var(--jm-brand);
  background: var(--jm-brand-soft);
}
</style>
