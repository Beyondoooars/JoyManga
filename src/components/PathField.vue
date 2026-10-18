<script setup lang="ts">
import { PhCopy } from '@phosphor-icons/vue'
import { useMessage } from 'naive-ui'

const props = defineProps<{
  label: string
}>()

const value = defineModel<string>('value', { required: true })
const message = useMessage()

async function copyPath() {
  try {
    await navigator.clipboard.writeText(value.value)
    message.success(`已复制${props.label}到剪贴板`)
  } catch {
    message.error('复制失败，请重试')
  }
}
</script>

<template>
  <div class="path-field">
    <span class="path-field-label">{{ label }}</span>
    <input
      class="path-field-value"
      :value="value"
      :title="value"
      :aria-label="props.label"
      readonly
      spellcheck="false" />
    <button
      type="button"
      class="path-field-action"
      :aria-label="`复制${props.label}`"
      :title="`复制${props.label}到剪贴板`"
      @click="copyPath">
      <PhCopy :size="19" />
    </button>
  </div>
</template>

<style scoped>
.path-field {
  display: grid;
  width: 100%;
  min-width: 0;
  flex-shrink: 0;
  height: 36px;
  grid-template-columns: 66px minmax(0, 1fr) 36px;
  overflow: hidden;
  border: 1px solid #e3e5e7;
  border-radius: var(--radius-control);
  background: #ffffff;
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
}

.path-field-value {
  padding: 0 10px;
  overflow: hidden;
  text-align: left;
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
  transition:
    color 150ms ease,
    background-color 150ms ease;
}

.path-field-action:hover,
.path-field-action:active {
  color: #fb7299;
  background: #fff0f5;
}

.path-field-action:focus-visible {
  outline: 2px solid rgba(251, 114, 153, 0.48);
  outline-offset: -3px;
}

/* 触屏：整行抬到 48dp，动作格至少 44dp */
@media (max-width: 768px) {
  .path-field {
    height: 48px;
    grid-template-columns: 72px minmax(0, 1fr) 48px;
  }

  .path-field-value {
    font-size: 16px;
  }
}
</style>
