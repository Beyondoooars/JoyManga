<template>
  <button type="button" class="icon-button cursor-pointer p-1 rounded-lg flex items-center justify-center text-[#61666D]">
    <slot />
  </button>
</template>

<style scoped>
.icon-button {
  min-width: 34px;
  min-height: 34px;
  border: 0;
  background: transparent;
  transition:
    color 160ms ease,
    background-color 160ms ease,
    transform 160ms ease;
}

.icon-button:hover {
  color: #fb7299;
  background: rgba(251, 114, 153, 0.12);
}

.icon-button:active {
  transform: scale(0.94);
}

.icon-button:focus-visible {
  outline: 2px solid rgba(251, 114, 153, 0.45);
  outline-offset: 2px;
}

.icon-button:disabled {
  cursor: not-allowed;
  opacity: 0.45;
  transform: none;
}

.icon-button:disabled:hover {
  color: #9499a0;
  background: transparent;
}

/* 触屏：图标按钮补足 44dp 可点区域 */
@media (max-width: 768px) {
  .icon-button {
    min-width: 44px;
    min-height: 44px;
  }
}
</style>
