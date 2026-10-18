<script setup lang="ts">
import { computed, defineAsyncComponent, onMounted, ref } from 'vue'
import { commands } from '../../bindings.ts'
import { open } from '@tauri-apps/plugin-dialog'
import { PhGearSix } from '@phosphor-icons/vue'
import { useStore } from '../../store.ts'
const SettingsDialog = defineAsyncComponent(() => import('../../dialogs/SettingsDialog.vue'))
import UncompletedProgresses from './components/UncompletedProgresses.vue'
import CompletedProgresses from './components/CompletedProgresses.vue'
import ExportProgresses from './components/ExportProgresses.vue'
import PathField from '../../components/PathField.vue'
import { useMessage } from 'naive-ui'
import { useIsMobile } from '../../composables/useIsMobile.ts'

const store = useStore()
const message = useMessage()
const { isMobile } = useIsMobile()
const pathFieldRef = ref<InstanceType<typeof PathField>>()
const queueStats = computed(() => {
  const tasks = [...store.progresses.values()]
  return { downloading: tasks.filter((task) => task.state === 'Downloading' || task.state === 'Pending').length,
    paused: tasks.filter((task) => task.state === 'Paused').length,
    failed: tasks.filter((task) => task.state === 'Failed').length }
})

const settingsDialogShowing = ref<boolean>(false)

const taskLoadError = computed(() => store.taskLoadError)
const loadingTasks = computed(() => store.loadingTasks)
async function loadTaskSnapshots() { await store.reloadTasks() }
onMounted(() => {
  void store.initDownloadListener().catch((error) => message.error(String(error)))
  // 单独打开传输页时也要确保导出监听已就绪（initStateListeners 已注册过则为空操作）
  void store.initExportListener().catch((error) => message.error(String(error)))
})

async function showDownloadDirInFileManager() {
  if (store.config === undefined) {
    return
  }
  const result = await commands.showPathInFileManager(store.config.downloadDir)
  if (result.status === 'error') {
    console.error(result.error)
  }
}

async function selectDownloadDir() {
  if (store.config === undefined) {
    return
  }

  if (isMobile.value) {
    pathFieldRef.value?.openModal()
    return
  }

  try {
    const selectedDirPath = await open({ directory: true })
    if (selectedDirPath === null) {
      return
    }

    store.config.downloadDir = selectedDirPath
    await store.saveConfigNow()
    await store.refreshDownloadedComics()
    message.success('已修改下载目录')
  } catch (e) {
    console.warn('Native folder picker failed or unsupported, opening modal:', e)
    pathFieldRef.value?.openModal()
  }
}
</script>

<template>
  <div v-if="store.config !== undefined" class="progress-pane">
    <div class="transfer-overview">
      <div class="transfer-caption">为下次阅读做好准备</div>
      <div class="transfer-stats"><span><strong>{{ queueStats.downloading }}</strong> 下载与排队</span><span><strong>{{ queueStats.paused }}</strong> 已暂停</span><span class="failed-stat"><strong>{{ queueStats.failed }}</strong> 待重试</span></div>
    </div>
    <div class="progress-toolbar">
      <PathField
        ref="pathFieldRef"
        v-model:value="store.config.downloadDir"
        label="漫画书库"
        @choose="selectDownloadDir"
        @open="showDownloadDirInFileManager" />
      <n-button class="settings-button" size="small" aria-label="应用设置" @click="settingsDialogShowing = true">
        <template #icon>
          <n-icon size="20">
            <PhGearSix />
          </n-icon>
        </template>
      </n-button>
    </div>
    <div v-if="taskLoadError" class="px-3 pb-2" role="alert">
      <span>{{ taskLoadError }}</span>
      <n-button size="tiny" :loading="loadingTasks" @click="loadTaskSnapshots">重新读取队列</n-button>
    </div>
    <n-tabs class="progress-tabs" v-model:value="store.progressesPaneTabName" type="line" size="small">
      <n-tab-pane class="h-full p-0! overflow-auto" name="uncompleted" tab="未完成">
        <UncompletedProgresses />
      </n-tab-pane>
      <n-tab-pane class="h-full p-0! overflow-auto" name="completed" tab="已完成">
        <CompletedProgresses />
      </n-tab-pane>
      <n-tab-pane class="h-full p-0! overflow-auto" name="export" tab="导出进度" display-directive="show">
        <ExportProgresses />
      </n-tab-pane>

      <template #suffix>
        <span class="whitespace-nowrap text-ellipsis overflow-hidden">{{ store.downloadSpeed }}</span>
      </template>
    </n-tabs>
    <SettingsDialog v-if="settingsDialogShowing" v-model:showing="settingsDialogShowing" />
  </div>
</template>

<style scoped>
.transfer-overview { padding: 20px 15px 15px; background: linear-gradient(125deg, #fff5f9, #f7f7ff); border-radius: 14px; margin: 4px 10px 8px; }
.transfer-caption { font-size: 11px; color: #ab8196; margin-bottom: 15px; }
.transfer-stats { display: grid; grid-template-columns: repeat(3, 1fr); gap: 12px; color: #9991a0; font-size: 11px; }
.transfer-stats strong { display: block; color: #615269; font-size: 25px; font-weight: 700; margin-bottom: 3px; }
.transfer-stats .failed-stat strong { color: #c57f76; }
.progress-pane {
  display: flex;
  min-height: 0;
  flex: 1;
  flex-direction: column;
  overflow: hidden;
}

.progress-toolbar {
  display: grid;
  flex: 0 0 auto;
  grid-template-columns: minmax(0, 1fr) 36px;
  gap: 8px;
  padding: 13px 13px 8px;
}

.settings-button {
  width: 36px;
  height: 36px;
  padding: 0;
}

.progress-tabs {
  min-height: 0;
  flex: 1;
  overflow: hidden;
}

.progress-tabs :deep(.n-tabs-nav) {
  padding: 0 13px;
}

.progress-tabs :deep(.n-tabs-pane-wrapper) {
  height: 100%;
  min-height: 0;
}
</style>
