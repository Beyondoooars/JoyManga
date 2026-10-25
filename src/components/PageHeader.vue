<script setup lang="ts">
import { PhArrowLeft } from '@phosphor-icons/vue'

withDefaults(
  defineProps<{
    title: string
    subtitle?: string
    backLabel?: string
  }>(),
  { subtitle: '', backLabel: '返回' },
)

const emit = defineEmits<{ (e: 'back'): void }>()
</script>

<template>
  <header class="page-header">
    <button type="button" class="back-btn" :aria-label="backLabel" :title="backLabel" @click="emit('back')">
      <PhArrowLeft :size="20" weight="bold" aria-hidden="true" />
    </button>
    <div class="page-heading">
      <h1>{{ title }}</h1>
      <span v-if="subtitle">{{ subtitle }}</span>
    </div>
    <div class="page-actions">
      <slot name="actions" />
    </div>
  </header>
</template>

<style scoped>
.page-header {
  display: flex;
  min-height: 56px;
  flex: 0 0 auto;
  align-items: center;
  gap: 10px;
  padding: calc(6px + var(--safe-top)) var(--jm-gutter) 6px;
  border-bottom: 1px solid var(--jm-line);
  background: var(--jm-surface);
}

.back-btn {
  display: grid;
  width: 40px;
  height: 40px;
  flex: 0 0 auto;
  place-items: center;
  padding: 0;
  color: var(--jm-text);
  border: 1px solid var(--jm-line);
  border-radius: var(--jm-r-pill);
  background: var(--jm-sunken);
  cursor: pointer;
  transition:
    color 160ms ease,
    background-color 160ms ease,
    border-color 160ms ease,
    transform 120ms ease;
}

.back-btn:hover {
  color: var(--jm-brand);
  border-color: rgba(251, 114, 153, 0.35);
  background: var(--jm-brand-soft);
}

.back-btn:active {
  transform: scale(0.94);
}

.page-heading {
  display: flex;
  min-width: 0;
  flex: 1;
  flex-direction: column;
  gap: 1px;
}

.page-heading h1 {
  margin: 0;
  overflow: hidden;
  color: var(--jm-text);
  font-size: var(--jm-fs-title);
  font-weight: 750;
  letter-spacing: -0.015em;
  line-height: 1.25;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.page-heading span {
  overflow: hidden;
  color: var(--jm-text-3);
  font-size: var(--jm-fs-xs);
  line-height: 1.3;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.page-actions {
  display: flex;
  flex: 0 0 auto;
  align-items: center;
  gap: 6px;
}

@media (pointer: coarse) {
  .back-btn {
    width: 44px;
    height: 44px;
  }
}
</style>
