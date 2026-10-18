<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import {
  NButton,
  NInput,
  NModal,
  NSpin,
  useMessage,
} from 'naive-ui'
import {
  PhFolder,
  PhFolderPlus,
  PhArrowUp,
  PhCheck,
  PhFile,
} from '@phosphor-icons/vue'
import { commands, type DirectoryListing } from '../bindings.ts'
import { useBackHandler } from '../composables/useAndroidBack.ts'
import { getErrorMessage } from '../utils/errors.ts'

const props = withDefaults(
  defineProps<{
    title?: string
    currentPath: string
    mode?: 'directory' | 'file'
    fileExtensions?: string[]
    createOnConfirm?: boolean
  }>(),
  {
    title: '修改目录',
    mode: 'directory',
    fileExtensions: () => [],
    createOnConfirm: false,
  },
)

const emit = defineEmits<{
  (e: 'confirm', path: string): void
}>()

const show = defineModel<boolean>('show', { required: true })
const message = useMessage()

const inputPath = ref('')
const browsingPath = ref('')
const listing = ref<DirectoryListing | null>(null)
const loading = ref(false)
const showCreateFolder = ref(false)
const newFolderName = ref('')
const creating = ref(false)
const errorMessage = ref('')
let directoryGeneration = 0
const isFileMode = computed(() => props.mode === 'file')
const canConfirm = computed(() => {
  const path = inputPath.value.trim()
  return !!path && !loading.value && (!isFileMode.value || (path !== browsingPath.value && (props.fileExtensions.length === 0 || props.fileExtensions.some((ext) => path.toLowerCase().endsWith(ext.toLowerCase())))))
})

useBackHandler(() => {
  if (show.value) {
    if (showCreateFolder.value) {
      showCreateFolder.value = false
      return true
    }
    show.value = false
    return true
  }
  return false
}, 60)

watch(
  () => show.value,
  (val) => {
    if (val) {
      inputPath.value = isFileMode.value ? '' : props.currentPath || ''
      browsingPath.value = props.currentPath || ''
      showCreateFolder.value = false
      newFolderName.value = ''
      void loadDirectory(props.currentPath || null)
    }
  },
  { immediate: true },
)

async function loadDirectory(targetPath: string | null) {
  const generation = ++directoryGeneration
  loading.value = true; errorMessage.value = ''
  try {
    const res = await commands.listDirectoryContents(targetPath, isFileMode.value)
    if (generation !== directoryGeneration || !show.value) return
    if (res.status === 'error') { errorMessage.value = res.error.err_message; listing.value = null; return }
    listing.value = res.data; browsingPath.value = res.data.currentPath
  } catch (error) {
    if (generation === directoryGeneration) { errorMessage.value = getErrorMessage(error); listing.value = null }
  } finally { if (generation === directoryGeneration) loading.value = false }
}

function handleEnterSubdir(subdir: string) {
  const base = browsingPath.value.replace(/[/\\]+$/, '')
  const separator = base.includes('\\') ? '\\' : '/'
  const newPath = `${base}${separator}${subdir}`
  if (!isFileMode.value) inputPath.value = newPath
  void loadDirectory(newPath)
}

function handleGoParent() {
  if (listing.value?.parentPath) {
    const parent = listing.value.parentPath
    if (!isFileMode.value) inputPath.value = parent
    void loadDirectory(parent)
  }
}

function handleUseCurrentBrowsing() {
  if (browsingPath.value) {
    inputPath.value = browsingPath.value
  }
}

async function handleCreateSubfolder() {
  const name = newFolderName.value.trim()
  if (!name) {
    message.warning('请输入新文件夹名称')
    return
  }
  const base = browsingPath.value.replace(/[/\\]+$/, '')
  const separator = base.includes('\\') ? '\\' : '/'
  const targetDir = `${base}${separator}${name}`

  creating.value = true
  try {
    const res = await commands.createDirectory(targetDir)
    if (res.status === 'ok') {
      message.success(`创建文件夹「${name}」成功`)
      newFolderName.value = ''
      showCreateFolder.value = false
      inputPath.value = targetDir
      await loadDirectory(targetDir)
    } else {
      message.error(`创建失败：${res.error.err_message}`)
    }
  } catch (err) {
    message.error(`创建失败：${getErrorMessage(err)}`)
  } finally {
    creating.value = false
  }
}

const filteredFiles = computed(() => {
  if (!listing.value?.files) return []
  if (!props.fileExtensions || props.fileExtensions.length === 0) {
    return listing.value.files
  }
  const exts = props.fileExtensions.map((e) => e.toLowerCase())
  return listing.value.files.filter((f) => exts.some((ext) => f.toLowerCase().endsWith(ext)))
})

function isFileSelected(fileName: string): boolean {
  const base = browsingPath.value.replace(/[/\\]+$/, '')
  const separator = base.includes('\\') ? '\\' : '/'
  return inputPath.value === `${base}${separator}${fileName}`
}

function handleSelectFile(fileName: string) {
  const base = browsingPath.value.replace(/[/\\]+$/, '')
  const separator = base.includes('\\') ? '\\' : '/'
  inputPath.value = `${base}${separator}${fileName}`
}

async function handleConfirm() {
  if (!canConfirm.value) { message.error(isFileMode.value ? '请选择符合格式的文件' : '请选择目录'); return }
  const chosen = inputPath.value.trim()
  loading.value = true
  try {
    const result = props.mode === 'directory' && props.createOnConfirm
      ? await commands.createDirectory(chosen)
      : await commands.validateSelectedPath(chosen, props.mode)
    if (result.status === 'error') { message.error(result.error.err_message); return }
    emit('confirm', chosen); show.value = false
  } catch (error) { message.error(getErrorMessage(error)) }
  finally { loading.value = false }
}

const pathBreadcrumbs = computed(() => {
  if (!browsingPath.value) return []
  const isWindows = browsingPath.value.includes('\\')
  const parts = browsingPath.value.split(/[/\\]/).filter(Boolean)
  const result: { label: string; fullPath: string }[] = []
  let accum = isWindows ? '' : ''
  for (let i = 0; i < parts.length; i++) {
    if (isWindows) {
      accum = accum ? `${accum}\\${parts[i]}` : parts[i]
    } else {
      accum = `${accum}/${parts[i]}`
    }
    result.push({
      label: parts[i],
      fullPath: accum,
    })
  }
  return result
})
</script>

<template>
  <n-modal
    v-model:show="show"
    preset="card"
    :title="title"
    class="path-edit-modal"
    :mask-closable="true"
    :auto-focus="false">
    <div class="modal-content">
      <!-- 路径输入框与状态 -->
      <div class="input-section">
        <div class="input-label">目标路径：</div>
        <n-input
          v-model:value="inputPath"
          type="text"
          placeholder="输入完整路径，或从下方选择"
          clearable
          size="medium"
          @keydown.enter="handleConfirm" />
      </div>

      <n-alert v-if="errorMessage" type="error">{{ errorMessage }}</n-alert>
      <!-- 目录浏览区 -->
      <div class="browser-section">
        <div class="browser-header">
          <div class="section-title">
            <PhFolder :size="16" />
            <span>浏览设备目录</span>
          </div>
          <div class="browser-header-actions">
            <n-button
              v-if="listing?.parentPath"
              size="tiny"
              secondary
              @click="handleGoParent">
              <template #icon><PhArrowUp :size="14" /></template>
              上一级
            </n-button>
            <n-button
              size="tiny"
              type="primary"
              secondary
              v-if="!isFileMode" @click="showCreateFolder = !showCreateFolder">
              <template #icon><PhFolderPlus :size="14" /></template>
              新建文件夹
            </n-button>
          </div>
        </div>

        <!-- 面包屑路径条 -->
        <div class="breadcrumb-bar">
          <span
            class="crumb-root"
            @click="loadDirectory(null)">
            存储
          </span>
          <span
            v-for="(crumb, idx) in pathBreadcrumbs"
            :key="crumb.fullPath"
            class="crumb-item"
            :class="{ current: idx === pathBreadcrumbs.length - 1 }"
            @click="loadDirectory(crumb.fullPath)">
            <span class="crumb-sep">/</span>
            <span class="crumb-text">{{ crumb.label }}</span>
          </span>
        </div>

        <!-- 新建文件夹输入 -->
        <div v-if="showCreateFolder" class="create-folder-bar">
          <n-input
            v-model:value="newFolderName"
            size="small"
            placeholder="新文件夹名称"
            @keydown.enter="handleCreateSubfolder" />
          <n-button
            size="small"
            type="primary"
            :loading="creating"
            @click="handleCreateSubfolder">
            创建
          </n-button>
          <n-button
            size="small"
            @click="showCreateFolder = false">
            取消
          </n-button>
        </div>

        <!-- 子目录与文件列表 -->
        <div class="subdirs-container">
          <n-spin :show="loading">
            <div v-if="listing?.subdirectories?.length || (mode === 'file' && filteredFiles.length)" class="subdirs-grid">
              <button
                v-for="sub in listing?.subdirectories || []"
                :key="'dir-' + sub"
                type="button"
                class="subdir-chip"
                @click="handleEnterSubdir(sub)">
                <PhFolder :size="16" class="subdir-icon" />
                <span class="subdir-name">{{ sub }}/</span>
              </button>
              <template v-if="mode === 'file'">
                <button
                  v-for="f in filteredFiles"
                  :key="'file-' + f"
                  type="button"
                  :class="['subdir-chip file-chip', { selected: isFileSelected(f) }]"
                  @click="handleSelectFile(f)">
                  <PhFile :size="16" class="file-icon" />
                  <span class="subdir-name">{{ f }}</span>
                </button>
              </template>
            </div>
            <div v-else-if="!loading" class="empty-subdirs">
              <span>{{ mode === 'file' ? '此目录下没有符合条件的文件' : '没有子文件夹' }}</span>
              <n-button
                v-if="mode === 'directory' && browsingPath !== inputPath"
                size="tiny"
                type="primary"
                text
                @click="handleUseCurrentBrowsing">
                使用当前目录
              </n-button>
            </div>
          </n-spin>
        </div>
      </div>
    </div>

    <template #footer>
      <div class="modal-footer">
        <n-button @click="show = false">取消</n-button>
        <n-button
          type="primary"
          :loading="loading"
          @click="handleConfirm" :disabled="!canConfirm">
          <template #icon><PhCheck :size="16" /></template>
          {{ mode === 'file' ? '选择此文件' : '保存' }}
        </n-button>
      </div>
    </template>
  </n-modal>
</template>

<style scoped>
.path-edit-modal {
  width: 92vw;
  max-width: 540px;
  border-radius: 14px;
}

.modal-content {
  display: flex;
  flex-direction: column;
  gap: 14px;
}

.input-section {
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.input-label {
  font-size: 13px;
  font-weight: 600;
  color: #333639;
}

.section-title {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: 13px;
  font-weight: 600;
  color: #555;
}

.browser-section {
  display: flex;
  flex-direction: column;
  gap: 8px;
  border: 1px solid #f0f0f4;
  border-radius: 10px;
  padding: 10px;
  background: #fbfbfc;
}

.browser-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
}

.browser-header-actions {
  display: flex;
  gap: 6px;
}

.breadcrumb-bar {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 2px;
  font-size: 11px;
  background: #fff;
  border: 1px solid #ebebee;
  border-radius: 6px;
  padding: 4px 8px;
  max-height: 48px;
  overflow-y: auto;
}

.crumb-root,
.crumb-text {
  cursor: pointer;
  color: #fb7299;
}

.crumb-root:hover,
.crumb-text:hover {
  text-decoration: underline;
}

.crumb-item.current .crumb-text {
  color: #333;
  font-weight: 600;
  cursor: default;
}

.crumb-item.current .crumb-text:hover {
  text-decoration: none;
}

.crumb-sep {
  margin: 0 3px;
  color: #bbb;
}

.create-folder-bar {
  display: flex;
  gap: 6px;
  align-items: center;
  background: #fff;
  padding: 6px;
  border-radius: 6px;
  border: 1px dashed #fb7299;
}

.subdirs-container {
  max-height: 140px;
  min-height: 50px;
  overflow-y: auto;
}

.subdirs-grid {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
}

.subdir-chip {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  padding: 4px 9px;
  font-size: 11px;
  border: 1px solid #e3e5e7;
  border-radius: 6px;
  background: #fff;
  cursor: pointer;
  transition: all 0.15s ease;
  max-width: 100%;
}

.subdir-chip:hover,
.subdir-chip:active {
  border-color: #fb7299;
  background-color: #fff0f5;
  color: #fb7299;
}

.subdir-icon {
  color: #e6a23c;
  flex-shrink: 0;
}

.file-chip {
  border-color: #d1d5db;
  background-color: #fafafa;
}

.file-chip:hover,
.file-chip:active {
  border-color: #fb7299;
  background-color: #fff0f5;
  color: #fb7299;
}

.file-chip.selected {
  border-color: #fb7299;
  background-color: #fff0f5;
  color: #fb7299;
  font-weight: 600;
}

.file-icon {
  color: #3b82f6;
  flex-shrink: 0;
}

.subdir-name {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.empty-subdirs {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 6px;
  padding: 16px;
  font-size: 12px;
  color: #999;
}

.modal-footer {
  display: flex;
  justify-content: flex-end;
  gap: 10px;
}
</style>
