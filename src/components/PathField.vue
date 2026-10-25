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
  height: 40px;
  grid-template-columns: 62px minmax(0, 1fr) 40px;
  overflow: hidden;
  border: 1px solid var(--jm-line);
  border-radius: var(--jm-r-md);
  background: var(--jm-surface);
}

.path-field-label {
  display: flex;
  align-items: center;
  justify-content: center;
  color: var(--jm-text-3);
  border-right: 1px solid var(--jm-line);
  background: var(--jm-sunken);
  font-size: 10.5px;
  font-weight: 650;
  white-space: nowrap;
}

.path-field-value,
.path-field-action {
  min-width: 0;
  padding: 0;
  color: var(--jm-text-2);
  border: 0;
  background: transparent;
}

.path-field-value {
  padding: 0 10px;
  overflow: hidden;
  text-align: left;
  font-size: 11.5px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.path-field-action {
  display: grid;
  color: var(--jm-text-3);
  place-items: center;
  border-left: 1px solid var(--jm-line);
  cursor: pointer;
  transition:
    color 150ms ease,
    background-color 150ms ease;
}

.path-field-action:hover,
.path-field-action:active {
  color: var(--jm-brand);
  background: var(--jm-brand-soft);
}

/* 触屏：整行抬到 48dp，动作格至少 44dp */
@media (max-width: 768px) {
  .path-field {
    height: 44px;
    grid-template-columns: 68px minmax(0, 1fr) 44px;
  }

  .path-field-value {
    font-size: 12.5px;
  }
}
</style>
