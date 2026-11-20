<script setup lang="ts">
import { computed, ref } from 'vue'
import { InputInst, InputProps } from 'naive-ui'

withDefaults(
  defineProps<{
    label: string
    size?: InputProps['size']
    type?: InputProps['type']
    clearable?: InputProps['clearable']
  }>(),
  {
    size: 'medium',
    type: 'text',
    clearable: false,
  },
)

const value = defineModel<InputProps['value']>('value', { required: true })

const focused = ref(false)
const NInputRef = ref<InputInst>()

const floating = computed(() => (value.value != null && value.value !== '') || focused.value)

const emit = defineEmits<{
  focus: [e: FocusEvent]
  blur: [e: FocusEvent]
}>()

function onFocus(e: FocusEvent) {
  focused.value = true
  emit('focus', e)
}

function onBlur(e: FocusEvent) {
  focused.value = false
  emit('blur', e)
}

defineExpose({ NInputRef })
</script>

<template>
  <n-input
    ref="NInputRef"
    :size="size"
    :type="type"
    :clearable="clearable"
    :show-password-on="type === 'password' ? 'click' : undefined"
    :aria-label="label"
    placeholder=""
    v-model:value="value"
    @focus="onFocus"
    @blur="onBlur">
    <template #prefix>
      <n-el
        tag="span"
        :class="[
          'float-label',
          size ? `size-${size}` : '',
          { 'is-floating': floating },
        ]">
        {{ label }}
      </n-el>
    </template>
  </n-input>
</template>

<style scoped>
.float-label {
  padding-inline: 0.25rem;
  background: transparent;
  color: var(--jm-text-3);
  pointer-events: none;
  transition: all 200ms ease-in-out;
}

.float-label.is-floating {
  font-size: 0.75rem;
}

.float-label.is-floating.size-tiny {
  transform: translateY(-90%);
}

.float-label.is-floating.size-small {
  transform: translateY(-120%);
}

.float-label.is-floating.size-medium {
  transform: translateY(-140%);
}

.float-label.is-floating.size-large {
  transform: translateY(-160%);
}

@media (prefers-reduced-motion: reduce) {
  .float-label {
    transition: none !important;
  }
}

:deep(.n-input-wrapper) {
  display: flex;
  align-items: center;
  overflow: visible;
}

:deep(.n-input__prefix) {
  position: absolute;
  z-index: 2;
  line-height: 1;
}

:deep(.n-input__input-el) {
  position: relative;
  z-index: 3;
}

:deep(.n-input--focus) .float-label,
:deep(.n-input:hover) .float-label {
  color: var(--jm-brand);
}
</style>
