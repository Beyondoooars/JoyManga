<script setup lang="ts">
import { PhFolder, PhFolderOpen, PhCopy } from '@phosphor-icons/vue'
import { ref, watch } from 'vue'
import { useMessage } from 'naive-ui'
import { useIsMobile } from '../composables/useIsMobile.ts'
import { useStore } from '../store.ts'
import PathEditDialog from '../dialogs/PathEditDialog.vue'
import { getErrorMessage } from '../utils/errors.ts'

const props = defineProps<{
  label: string
}>()

const value = defineModel<string>('value', { required: true })
const draft = ref(value.value)
const editModalShowing = ref(false)

const store = useStore()
const message = useMessage()
const { isMobile } = useIsMobile()

watch(value, (path) => {
  draft.value = path
})

// 输入过程中只改本地 draft：逐字提交会让防抖保存写入半截路径，
// 后端随即 create_dir_all 建出垃圾目录并切换书库。
function commitPath() {
  if (isMobile.value) return
  const path = draft.value.trim()
  if (path) {
    value.value = path
    emit('change', path)
  } else {
    draft.value = value.value
  }
}

function handleInputClick() {
  if (isMobile.value) {
    editModalShowing.value = true
  }
}

function handleChooseClick() {
  if (isMobile.value) {
    editModalShowing.value = true
  } else {
    emit('choose')
  }
}

async function handleActionClick() {
  if (isMobile.value) {
    try {
      await navigator.clipboard.writeText(value.value)
      message.success(`已复制${props.label}到剪贴板`)
    } catch {
      message.error('复制失败，请重试')
    }
  } else {
    emit('open')
  }
}

async function handleModalConfirm(newPath: string) {
  value.value = newPath
  draft.value = newPath
  emit('change', newPath)
  try {
    await store.saveConfigNow()
    message.success(`已更新${props.label}`)
  } catch (err) {
    message.error(`保存失败：${getErrorMessage(err)}`)
  }
}

function openModal() {
  editModalShowing.value = true
}

defineExpose({
  openModal,
})

const emit = defineEmits<{
  choose: []
  open: []
  change: [path: string]
}>()
</script>

<template>
  <div class="path-field" :class="{ 'is-mobile': isMobile }">
    <span class="path-field-label">{{ label }}</span>
    <input
      v-model="draft"
      class="path-field-value"
      :title="value"
      :aria-label="props.label"
      :readonly="isMobile"
      spellcheck="false"
      @click="handleInputClick"
      @blur="commitPath"
      @keydown.enter="commitPath" />
    <button
      type="button"
      class="path-field-action"
      :aria-label="`修改${props.label}`"
      :title="isMobile ? `修改${props.label}` : `选择${props.label}`"
      @click="handleChooseClick">
      <PhFolder :size="19" />
    </button>
    <button
      type="button"
      class="path-field-action"
      :aria-label="isMobile ? `复制${props.label}` : `打开${props.label}`"
      :title="isMobile ? `复制${props.label}到剪贴板` : `打开${props.label}`"
      @click="handleActionClick">
      <PhCopy v-if="isMobile" :size="19" />
      <PhFolderOpen v-else :size="19" weight="duotone" />
    </button>

    <PathEditDialog
      v-model:show="editModalShowing"
      :title="`修改${props.label}`"
      :current-path="value"
      :create-on-confirm="true"
      @confirm="handleModalConfirm" />
  </div>
</template>

<style scoped>
.path-field {
  display: grid;
  width: 100%;
  min-width: 0;
  flex-shrink: 0;
  height: 36px;
  grid-template-columns: 66px minmax(0, 1fr) 36px 36px;
  overflow: hidden;
  border: 1px solid #e3e5e7;
  border-radius: var(--radius-control);
  background: #ffffff;
  transition:
    border-color 160ms ease,
    box-shadow 160ms ease;
}

.path-field:focus-within {
  border-color: #fb7299;
  box-shadow: 0 0 0 2px rgba(251, 114, 153, 0.16);
}

.path-field.is-mobile .path-field-value {
  cursor: pointer;
}

.path-field-label {
  display: flex;
  align-items: center;
  justify-content: center;
  color: #61666d;
  border-right: 1px solid #e3e5e7;
  background: #f6f7f8;
  font-size: 11px;
  font-weight: 600;
  white-space: nowrap;
}

.path-field-value,
.path-field-action {
  min-width: 0;
  padding: 0;
  color: #18191c;
  border: 0;
  background: transparent;
  transition:
    color 150ms ease,
    background-color 150ms ease;
}

.path-field-value {
  padding: 0 10px;
  overflow: hidden;
  text-align: left;
  cursor: text;
  font-size: 12px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.path-field-action {
  display: grid;
  place-items: center;
  border-left: 1px solid #e3e5e7;
  color: #61666d;
  cursor: pointer;
}

.path-field-action:hover,
.path-field-action:active {
  color: #fb7299;
  background: #fff0f5;
}

.path-field-value:focus-visible,
.path-field-action:focus-visible {
  outline: 2px solid rgba(251, 114, 153, 0.48);
  outline-offset: -3px;
}

/* 触屏：整行抬到 48dp，两个动作格至少 44dp */
@media (max-width: 768px) {
  .path-field {
    height: 48px;
    grid-template-columns: 72px minmax(0, 1fr) 48px 48px;
  }

  .path-field-value {
    font-size: 16px;
  }
}
</style>
