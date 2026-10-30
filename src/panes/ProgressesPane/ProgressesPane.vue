<script setup lang="ts">
import { computed, onMounted } from 'vue'
import { useStore } from '../../store.ts'
import UncompletedProgresses from './components/UncompletedProgresses.vue'
import CompletedProgresses from './components/CompletedProgresses.vue'
import { useMessage } from 'naive-ui'
import { PhArrowClockwise, PhWarningCircle } from '@phosphor-icons/vue'

const store = useStore()
const message = useMessage()
const uncompletedCount = computed(
  () => [...store.progresses.values()].filter((task) => task.state !== 'Completed' && task.state !== 'Cancelled').length,
)

const tabs = computed(() => [
  { value: 'uncompleted' as const, label: '进行中', count: uncompletedCount.value },
  { value: 'completed' as const, label: '已完成', count: 0 },
])

const taskLoadError = computed(() => store.taskLoadError)
const loadingTasks = computed(() => store.loadingTasks)
async function loadTaskSnapshots() {
  await store.reloadTasks()
}
onMounted(() => {
  void store.initDownloadListener().catch((error) => message.error(String(error)))
})
</script>

<template>
  <div v-if="store.config !== undefined" class="progress-pane">
    <div v-if="taskLoadError" class="queue-error" role="alert">
      <PhWarningCircle :size="16" weight="fill" aria-hidden="true" />
      <span>{{ taskLoadError }}</span>
      <button type="button" class="queue-retry" :disabled="loadingTasks" @click="loadTaskSnapshots">
        <PhArrowClockwise :size="14" :class="{ 'jm-spin': loadingTasks }" />
        重新读取
      </button>
    </div>

    <!-- 视图切换 -->
    <div class="transfer-tabs" role="tablist" aria-label="传输视图">
      <button
        v-for="tab in tabs"
        :key="tab.value"
        type="button"
        role="tab"
        class="transfer-tab"
        :class="{ 'is-active': store.progressesPaneTabName === tab.value }"
        :aria-selected="store.progressesPaneTabName === tab.value"
        @click="store.progressesPaneTabName = tab.value">
        {{ tab.label }}
        <span v-if="tab.count > 0" class="tab-count">{{ tab.count }}</span>
      </button>
    </div>

    <div class="transfer-body">
      <UncompletedProgresses v-if="store.progressesPaneTabName === 'uncompleted'" />
      <CompletedProgresses v-else />
    </div>
  </div>
</template>

<style scoped>
.progress-pane {
  display: flex;
  height: 100%;
  min-height: 0;
  flex: 1;
  flex-direction: column;
  overflow: hidden;
  background: var(--jm-bg);
}

.queue-error {
  display: flex;
  flex: 0 0 auto;
  align-items: center;
  gap: 7px;
  margin: 8px var(--jm-gutter) 0;
  padding: 9px 12px;
  color: var(--jm-danger);
  border-radius: var(--jm-r-md);
  background: var(--jm-danger-soft);
  font-size: var(--jm-fs-xs);
}

.queue-error span {
  min-width: 0;
  flex: 1;
  overflow-wrap: anywhere;
}

.queue-retry {
  display: inline-flex;
  flex: 0 0 auto;
  align-items: center;
  gap: 4px;
  padding: 0;
  color: var(--jm-danger);
  border: 0;
  background: none;
  font-size: var(--jm-fs-xs);
  font-weight: 700;
  cursor: pointer;
}

.transfer-tabs {
  display: flex;
  flex: 0 0 auto;
  gap: 18px;
  margin-top: 8px;
  padding: 0 var(--jm-gutter);
  border-bottom: 1px solid var(--jm-line);
}

.transfer-tab {
  position: relative;
  display: inline-flex;
  height: 40px;
  align-items: center;
  gap: 5px;
  padding: 0;
  color: var(--jm-text-3);
  border: 0;
  background: none;
  font-size: var(--jm-fs-sm);
  font-weight: 600;
  cursor: pointer;
  transition: color 160ms ease;
}

.transfer-tab::after {
  position: absolute;
  right: 0;
  bottom: -1px;
  left: 0;
  height: 2.5px;
  border-radius: 2px;
  background: var(--jm-brand);
  opacity: 0;
  transform: scaleX(0.4);
  transition:
    opacity 180ms ease,
    transform 180ms ease;
  content: '';
}

.transfer-tab.is-active {
  color: var(--jm-text);
  font-weight: 700;
}

.transfer-tab.is-active::after {
  opacity: 1;
  transform: scaleX(1);
}

.tab-count {
  padding: 0 6px;
  color: var(--jm-text-3);
  border-radius: var(--jm-r-pill);
  background: var(--jm-sunken);
  font-size: 9.5px;
  font-weight: 700;
  font-variant-numeric: tabular-nums;
  line-height: 1.6;
}

.transfer-tab.is-active .tab-count {
  color: var(--jm-brand);
  background: var(--jm-brand-soft);
}

.transfer-body {
  min-height: 0;
  flex: 1;
  overflow: hidden;
}

.transfer-body > * {
  height: 100%;
}
</style>
