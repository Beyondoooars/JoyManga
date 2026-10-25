<template>
  <button type="button" class="icon-button">
    <slot />
  </button>
</template>

<style scoped>
.icon-button {
  display: inline-flex;
  min-width: 40px;
  min-height: 40px;
  flex: 0 0 auto;
  align-items: center;
  justify-content: center;
  padding: 0;
  color: var(--jm-text-2);
  border: 0;
  border-radius: var(--jm-r-sm);
  background: transparent;
  cursor: pointer;
  transition:
    color 160ms ease,
    background-color 160ms ease,
    transform 160ms ease;
}

.icon-button:hover {
  color: var(--jm-brand);
  background: var(--jm-brand-soft);
}

.icon-button:active {
  transform: scale(0.94);
}

.icon-button:focus-visible {
  outline: 2px solid var(--jm-brand-ring);
  outline-offset: 2px;
}

.icon-button:disabled {
  cursor: not-allowed;
  opacity: 0.45;
  transform: none;
}

.icon-button:disabled:hover {
  color: var(--jm-text-3);
  background: transparent;
}

/* 触屏：图标按钮补足 44dp 可点区域 */
@media (pointer: coarse) {
  .icon-button {
    min-width: 44px;
    min-height: 44px;
  }
}
</style>
