<script setup lang="ts">
import { computed, onMounted } from 'vue'
import { useStore } from '../../store.ts'
import UncompletedProgresses from './components/UncompletedProgresses.vue'
import CompletedProgresses from './components/CompletedProgresses.vue'
import ExportProgresses from './components/ExportProgresses.vue'
import { useMessage } from 'naive-ui'
import { PhArrowClockwise, PhWarningCircle } from '@phosphor-icons/vue'

const store = useStore()
const message = useMessage()
const queueStats = computed(() => {
  const tasks = [...store.progresses.values()]
  return {
    downloading: tasks.filter((task) => task.state === 'Downloading' || task.state === 'Pending').length,
    paused: tasks.filter((task) => task.state === 'Paused').length,
    failed: tasks.filter((task) => task.state === 'Failed').length,
  }
})

const tabs = computed(() => [
  { value: 'uncompleted' as const, label: '进行中', count: queueStats.value.downloading + queueStats.value.paused + queueStats.value.failed },
  { value: 'completed' as const, label: '已完成', count: 0 },
  { value: 'export' as const, label: '导出', count: store.exportProgresses.size },
])

const taskLoadError = computed(() => store.taskLoadError)
const loadingTasks = computed(() => store.loadingTasks)
async function loadTaskSnapshots() {
  await store.reloadTasks()
}
onMounted(() => {
  void store.initDownloadListener().catch((error) => message.error(String(error)))
  // 单独打开传输页时也要确保导出监听已就绪（initStateListeners 已注册过则为空操作）
  void store.initExportListener().catch((error) => message.error(String(error)))
})
</script>

<template>
  <div v-if="store.config !== undefined" class="progress-pane">
    <!-- 队列概览：三个数字 + 实时速度 -->
    <div class="queue-overview">
      <div class="queue-stat">
        <strong>{{ queueStats.downloading }}</strong>
        <span>下载与排队</span>
      </div>
      <div class="queue-stat">
        <strong>{{ queueStats.paused }}</strong>
        <span>已暂停</span>
      </div>
      <div class="queue-stat" :class="{ 'is-warn': queueStats.failed > 0 }">
        <strong>{{ queueStats.failed }}</strong>
        <span>待重试</span>
      </div>
      <div class="queue-speed" :title="store.downloadSpeed || '暂无速度'">
        <span v-if="store.downloadSpeed" class="speed-live" aria-hidden="true"></span>
        {{ store.downloadSpeed || '—' }}
      </div>
    </div>

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
        <span v-if="tab.value === 'uncompleted' && tab.count > 0" class="tab-count">{{ tab.count }}</span>
        <span v-else-if="tab.value === 'export' && tab.count > 0" class="tab-count">{{ tab.count }}</span>
      </button>
    </div>

    <div class="transfer-body">
      <UncompletedProgresses v-if="store.progressesPaneTabName === 'uncompleted'" />
      <CompletedProgresses v-else-if="store.progressesPaneTabName === 'completed'" />
      <ExportProgresses v-else />
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

.queue-overview {
  position: relative;
  display: grid;
  flex: 0 0 auto;
  grid-template-columns: repeat(3, minmax(0, 1fr));
  gap: 8px;
  margin: 12px var(--jm-gutter) 0;
  padding: 13px 14px 15px;
  border: 1px solid var(--jm-line);
  border-radius: var(--jm-r-lg);
  background: linear-gradient(120deg, #fff6fa 0%, #ffffff 55%, #f2fbff 100%);
}

.queue-stat {
  display: flex;
  min-width: 0;
  flex-direction: column;
  gap: 1px;
}

.queue-stat strong {
  color: var(--jm-text);
  font-size: 22px;
  font-weight: 800;
  font-variant-numeric: tabular-nums;
  line-height: 1.15;
}

.queue-stat span {
  color: var(--jm-text-3);
  font-size: 10px;
}

.queue-stat.is-warn strong {
  color: var(--jm-danger);
}

.queue-speed {
  position: absolute;
  top: 12px;
  right: 14px;
  display: inline-flex;
  max-width: 40%;
  align-items: center;
  gap: 4px;
  overflow: hidden;
  color: var(--jm-accent);
  font-family: ui-monospace, 'SF Mono', Menlo, monospace;
  font-size: 10px;
  font-weight: 600;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.speed-live {
  width: 6px;
  height: 6px;
  flex: 0 0 auto;
  border-radius: 50%;
  background: var(--jm-accent);
  animation: speed-pulse 1.6s ease-in-out infinite;
}

@keyframes speed-pulse {
  0%,
  100% {
    opacity: 1;
  }
  50% {
    opacity: 0.35;
  }
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
