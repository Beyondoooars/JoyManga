<script setup lang="ts">
import { PhCircleNotch, PhFileZip, PhFolderOpen, PhWarningCircle } from '@phosphor-icons/vue'
import { ProgressData } from './ExportProgresses.vue'
import { commands } from '../../../bindings.ts'
import { isAndroid } from '../../../composables/useIsMobile.ts'
import { exportToAndroidDownloads, isUnderPublicDownloads } from '../../../utils/androidStorage.ts'
import { useMessage } from 'naive-ui'
import { ref } from 'vue'
const message = useMessage()
const publishing = ref(false)
async function publish() {
  if (!props.p.chapterExportDir || publishing.value) return
  publishing.value = true
  try {
    // 导出目录已在公共下载里（书库就是公共目录）时无需再发布一份副本
    if (isUnderPublicDownloads(props.p.chapterExportDir)) {
      message.success('文件已在公共下载目录中')
      return
    }
    const result = await exportToAndroidDownloads(props.p.chapterExportDir)
    if (!result.cancelled) message.success(`已保存 ${result.exported ?? 0} 个文件到公共目录`)
  } catch (error) {
    message.error(String(error))
  } finally {
    publishing.value = false
  }
}

const props = defineProps<{
  p: ProgressData
  handleContextMenu: (p: ProgressData) => void
}>()

async function showChapterExportDirInFileManager() {
  if (props.p.chapterExportDir === undefined) {
    return
  }

  const result = await commands.showPathInFileManager(props.p.chapterExportDir)
  if (result.status === 'error') {
    console.error(result.error)
  }
}
</script>

<template>
  <div class="export-card" @contextmenu="handleContextMenu(p)">
    <div class="export-card-head">
      <span class="export-state" :data-state="p.state">
        <PhCircleNotch v-if="p.state === 'Processing'" :size="15" weight="bold" class="jm-spin" />
        <PhWarningCircle v-else-if="p.state === 'Error'" :size="15" weight="bold" />
        <PhFileZip v-else :size="15" weight="bold" />
      </span>
      <strong class="export-title" :title="p.comicTitle">{{ p.comicTitle }}</strong>
      <button
        v-if="p.chapterExportDir !== undefined"
        type="button"
        class="export-dir"
        aria-label="打开导出目录"
        title="打开导出目录"
        @click="showChapterExportDirInFileManager">
        <PhFolderOpen :size="16" />
      </button>
    </div>

    <div class="export-status-row">
      <span class="export-indicator">{{ p.indicator }}</span>
      <span v-if="p.state === 'End'" class="export-done">已完成</span>
      <span v-else-if="p.state === 'Error'" class="export-failed">导出失败</span>
    </div>

    <n-progress
      v-if="p.state === 'Processing'"
      class="export-progress"
      :percentage="p.percentage"
      :show-indicator="false"
      :height="4"
      processing />
    <n-progress
      v-else-if="p.state === 'Error'"
      class="export-progress"
      status="error"
      :percentage="p.percentage"
      :show-indicator="false"
      :height="4" />

    <button
      v-if="isAndroid && p.state === 'End' && p.chapterExportDir"
      type="button"
      class="publish-btn"
      :disabled="publishing"
      @click="publish">
      {{ publishing ? '正在保存…' : '保存到系统下载' }}
    </button>
  </div>
</template>

<style scoped>
.export-card {
  padding: 11px 12px;
  border: 1px solid var(--jm-line);
  border-radius: var(--jm-r-md);
  background: var(--jm-surface);
  transition:
    border-color 160ms ease,
    background-color 160ms ease;
}

.export-card-head {
  display: flex;
  align-items: center;
  gap: 8px;
}

.export-state {
  display: grid;
  width: 24px;
  height: 24px;
  flex: 0 0 auto;
  place-items: center;
  border-radius: var(--jm-r-sm);
}

.export-state[data-state='Processing'] {
  color: var(--jm-accent);
  background: var(--jm-accent-soft);
}

.export-state[data-state='End'] {
  color: var(--jm-success);
  background: var(--jm-success-soft);
}

.export-state[data-state='Error'] {
  color: var(--jm-danger);
  background: var(--jm-danger-soft);
}

.export-title {
  min-width: 0;
  flex: 1;
  overflow: hidden;
  color: var(--jm-text);
  font-size: 12.5px;
  font-weight: 650;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.export-dir {
  display: grid;
  width: 30px;
  height: 30px;
  flex: 0 0 auto;
  place-items: center;
  padding: 0;
  color: var(--jm-text-2);
  border: 0;
  border-radius: var(--jm-r-sm);
  background: var(--jm-sunken);
  cursor: pointer;
}

.export-status-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  margin-top: 6px;
  padding-left: 32px;
}

.export-indicator {
  min-width: 0;
  overflow: hidden;
  color: var(--jm-text-3);
  font-family: ui-monospace, 'SF Mono', Menlo, monospace;
  font-size: 10px;
  font-variant-numeric: tabular-nums;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.export-done {
  flex: 0 0 auto;
  color: var(--jm-success);
  font-size: 10px;
  font-weight: 700;
}

.export-failed {
  flex: 0 0 auto;
  color: var(--jm-danger);
  font-size: 10px;
  font-weight: 700;
}

.export-progress {
  margin-top: 8px;
}

.publish-btn {
  width: 100%;
  height: 34px;
  margin-top: 9px;
  color: var(--jm-accent);
  border: 1px solid rgba(0, 174, 236, 0.24);
  border-radius: var(--jm-r-md);
  background: var(--jm-accent-soft);
  font-size: 11.5px;
  font-weight: 650;
  cursor: pointer;
}

.publish-btn:disabled {
  opacity: 0.55;
  cursor: wait;
}
</style>
