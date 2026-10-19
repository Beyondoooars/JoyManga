<script setup lang="ts">
import { onBeforeUnmount, ref, watch } from 'vue'
import { PhArrowLeft, PhFolder, PhFolderPlus, PhX } from '@phosphor-icons/vue'
import { useMessage } from 'naive-ui'
import { commands, type DirectoryEntry } from '../bindings.ts'
import { formatCommandError, getErrorMessage } from '../utils/errors.ts'
import { useBackHandler } from '../composables/useAndroidBack.ts'

/** Android 公共存储根目录，浏览器从这里开始 */
const STORAGE_ROOT = '/storage/emulated/0'

const show = defineModel<boolean>('show', { required: true })
const emit = defineEmits<{ select: [canonicalPath: string] }>()

const message = useMessage()
const currentPath = ref(STORAGE_ROOT)
const entries = ref<DirectoryEntry[]>([])
const loading = ref(false)
const manualPath = ref('')
const folderInputShowing = ref(false)
const folderName = ref('')
const folderCreating = ref(false)
const selecting = ref(false)
let generation = 0
let disposed = false

useBackHandler(() => {
  if (!show.value) return false
  show.value = false
  return true
}, 60)

watch(show, (visible) => {
  if (visible) void navigate(STORAGE_ROOT)
})

onBeforeUnmount(() => { disposed = true; generation++ })

function parentPath(path: string): string {
  const trimmed = path.replace(/\/+$/, '')
  const index = trimmed.lastIndexOf('/')
  return index <= 0 ? '/' : trimmed.slice(0, index)
}

/** 拉取目录列表并进入；失败（不存在/无权限）时停留在原目录 */
async function navigate(path: string): Promise<boolean> {
  const target = path.trim()
  if (!target || loading.value) return false
  const current = ++generation
  loading.value = true
  try {
    const result = await commands.listDirectoryEntries(target)
    if (disposed || current !== generation) return false
    if (result.status === 'error') {
      message.error(formatCommandError(result.error))
      return false
    }
    entries.value = [...result.data].sort((a, b) => a.name.localeCompare(b.name))
    currentPath.value = target
    manualPath.value = ''
    return true
  } catch (error) {
    if (current === generation) message.error(getErrorMessage(error))
    return false
  } finally {
    if (current === generation) loading.value = false
  }
}

/** 校验当前目录可作为书库；后端会创建缺失目录并返回规范路径 */
async function chooseCurrent() {
  if (loading.value || selecting.value) return
  selecting.value = true
  try {
    const result = await commands.validateDownloadDir(currentPath.value)
    if (disposed) return
    if (result.status === 'error') {
      message.error(formatCommandError(result.error))
      return
    }
    emit('select', result.data.canonicalPath)
    show.value = false
  } catch (error) {
    message.error(getErrorMessage(error))
  } finally {
    selecting.value = false
  }
}

function toggleFolderInput() {
  folderInputShowing.value = !folderInputShowing.value
  folderName.value = ''
}

/** 在当前目录下新建文件夹：validateDownloadDir 会创建目录，成功后进入 */
async function createFolder() {
  const name = folderName.value.trim()
  if (!name || folderCreating.value) return
  if (name.includes('/')) {
    message.error('文件夹名称不能包含 /')
    return
  }
  folderCreating.value = true
  try {
    const target = `${currentPath.value.replace(/\/+$/, '')}/${name}`
    const result = await commands.validateDownloadDir(target)
    if (disposed) return
    if (result.status === 'error') {
      message.error(formatCommandError(result.error))
      return
    }
    folderInputShowing.value = false
    folderName.value = ''
    message.success('文件夹已创建')
    await navigate(result.data.canonicalPath)
  } catch (error) {
    message.error(getErrorMessage(error))
  } finally {
    folderCreating.value = false
  }
}
</script>

<template>
  <n-modal v-model:show="show" :auto-focus="true" :trap-focus="true">
    <section class="storage-shell" role="dialog" aria-modal="true" aria-labelledby="storage-title">
      <header class="storage-header">
        <h2 id="storage-title">选择漫画保存位置</h2>
        <button class="storage-close" type="button" aria-label="关闭目录选择" @click="show = false"><PhX :size="20" aria-hidden="true" /></button>
      </header>
      <div class="storage-scroll" role="region" aria-label="目录浏览" tabindex="0">
        <div class="storage-toolbar">
          <button class="storage-back" type="button" :disabled="currentPath === STORAGE_ROOT || loading" aria-label="返回上一级目录" @click="navigate(parentPath(currentPath))">
            <PhArrowLeft :size="18" aria-hidden="true" /><span>上一级</span>
          </button>
          <p class="storage-path" aria-label="当前所在目录">{{ currentPath }}</p>
        </div>
        <div class="storage-jump">
          <n-input v-model:value="manualPath" aria-label="手动输入目录路径" placeholder="粘贴绝对路径，如 /storage/emulated/0/Download" :disabled="loading" @keyup.enter="navigate(manualPath)" />
          <n-button secondary :disabled="loading || !manualPath.trim()" @click="navigate(manualPath)">跳转</n-button>
        </div>
        <ul v-if="entries.length" class="storage-list" aria-label="子目录列表">
          <li v-for="entry in entries" :key="entry.path">
            <button type="button" class="storage-entry" :disabled="loading" :aria-label="`进入文件夹 ${entry.name}`" @click="navigate(entry.path)">
              <PhFolder :size="20" aria-hidden="true" />
              <span>{{ entry.name }}</span>
            </button>
          </li>
        </ul>
        <p v-else-if="!loading" class="storage-empty">此目录下没有子文件夹</p>
      </div>
      <footer class="storage-footer">
        <div v-if="folderInputShowing" class="storage-create">
          <n-input v-model:value="folderName" aria-label="新文件夹名称" placeholder="输入文件夹名称" :disabled="folderCreating" @keyup.enter="createFolder" />
          <n-button secondary :loading="folderCreating" :disabled="!folderName.trim()" @click="createFolder">创建</n-button>
          <n-button quaternary :disabled="folderCreating" @click="toggleFolderInput">取消</n-button>
        </div>
        <div class="storage-actions">
          <n-button secondary :disabled="loading || folderCreating" @click="toggleFolderInput">
            <template #icon><PhFolderPlus :size="16" aria-hidden="true" /></template>
            新建文件夹
          </n-button>
          <n-button type="primary" color="#b73963" text-color="#ffffff" :loading="selecting" :disabled="loading" @click="chooseCurrent">选择此目录</n-button>
        </div>
      </footer>
    </section>
  </n-modal>
</template>

<style scoped>
.storage-shell { --storage-text: var(--text-primary, #18191c); --storage-muted: #5e6672; --storage-border: var(--border-color, #e3e5e7); width: min(560px, calc(100vw - 40px)); height: min(640px, calc(100vh - 48px)); height: min(640px, calc(100dvh - 48px)); min-height: 0; display: flex; flex-direction: column; overflow: hidden; color: var(--storage-text); background: var(--bg-card, #fff); border: 1px solid var(--storage-border); border-radius: var(--radius-panel); box-shadow: 0 16px 56px rgb(0 0 0 / 16%); }
.storage-header { flex: 0 0 auto; display: flex; align-items: center; justify-content: space-between; padding: 16px 24px; border-bottom: 1px solid var(--storage-border); }
.storage-header h2 { margin: 0; font-size: 20px; font-weight: 650; }
.storage-close { display: grid; place-items: center; width: 44px; height: 44px; padding: 0; border: 0; border-radius: 10px; background: transparent; color: var(--storage-muted); cursor: pointer; }
.storage-close:hover { background: var(--bg-subtle); color: var(--storage-text); }
.storage-close:focus-visible { outline: 2px solid var(--bili-pink-active); outline-offset: -2px; }
.storage-scroll { flex: 1 1 auto; min-height: 0; overflow-y: auto; overscroll-behavior: contain; touch-action: pan-y; scrollbar-gutter: stable; padding: 16px 24px; }
.storage-toolbar { display: flex; align-items: center; gap: 12px; margin-bottom: 12px; }
.storage-back { display: flex; align-items: center; gap: 6px; flex: 0 0 auto; min-height: 40px; padding: 0 14px; border: 1px solid var(--storage-border); border-radius: 10px; background: transparent; color: var(--storage-text); cursor: pointer; font-size: 14px; font-weight: 550; }
.storage-back:disabled { color: var(--storage-muted); opacity: 0.5; cursor: default; }
.storage-back:not(:disabled):hover { background: var(--bg-subtle); }
.storage-back:focus-visible { outline: 2px solid var(--bili-pink-active); outline-offset: -2px; }
.storage-path { flex: 1 1 auto; min-width: 0; margin: 0; color: var(--storage-muted); font-size: 13px; line-height: 1.5; overflow-wrap: anywhere; }
.storage-jump { display: grid; grid-template-columns: minmax(0, 1fr) auto; gap: 8px; margin-bottom: 16px; }
.storage-jump :deep(.n-input) { min-height: 42px; }
.storage-list { display: grid; gap: 2px; margin: 0; padding: 0; list-style: none; }
.storage-entry { display: flex; align-items: center; gap: 10px; width: 100%; min-height: 48px; padding: 0 12px; border: 0; border-radius: 10px; background: transparent; color: var(--storage-text); cursor: pointer; font-size: 14px; text-align: left; }
.storage-entry span { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.storage-entry svg { flex: 0 0 auto; color: var(--bili-pink-active); }
.storage-entry:not(:disabled):hover { background: var(--bg-subtle); }
.storage-entry:disabled { cursor: default; }
.storage-entry:focus-visible { outline: 2px solid var(--bili-pink-active); outline-offset: -2px; }
.storage-empty { margin: 8px 0; color: var(--storage-muted); font-size: 14px; }
.storage-footer { flex: 0 0 auto; display: grid; gap: 12px; padding: 16px 24px; border-top: 1px solid var(--storage-border); background: var(--bg-card); }
.storage-create { display: grid; grid-template-columns: minmax(0, 1fr) auto auto; gap: 8px; }
.storage-create :deep(.n-input) { min-height: 42px; }
.storage-actions { display: flex; flex-wrap: wrap; justify-content: space-between; gap: 12px; }
.storage-actions :deep(.n-button) { min-height: 40px; }
@media (max-width: 600px) {
  .storage-shell { width: 100vw; height: 100vh; height: 100dvh; max-height: 100dvh; border: 0; border-radius: 0; }
  .storage-header { padding: calc(12px + env(safe-area-inset-top, 0px)) 16px 12px; }
  .storage-scroll { padding: 12px 16px; }
  .storage-footer { padding: 12px 16px calc(12px + env(safe-area-inset-bottom, 0px)); }
  .storage-actions { display: grid; grid-template-columns: 1fr; }
  .storage-actions :deep(.n-button), .storage-back { min-height: 48px; }
  .storage-create :deep(.n-input), .storage-jump :deep(.n-input) { min-height: 48px; }
}
</style>
