<script setup lang="ts">
import { computed } from 'vue'
import { PhHeart } from '@phosphor-icons/vue'
import { useMessage } from 'naive-ui'
import { useStore } from '../store.ts'

const props = withDefaults(
  defineProps<{
    comicId: number
    initialState?: boolean
    initialEpoch?: number
    /** icon：封面上的圆形收藏键；labeled：带文字的按钮（详情页用） */
    variant?: 'icon' | 'labeled'
  }>(),
  { variant: 'labeled' },
)

const store = useStore()
const message = useMessage()

const known = computed(() =>
  store.userProfile
    ? store.favoriteStatuses.has(props.comicId)
      ? store.favoriteStatuses.get(props.comicId)
      : props.initialEpoch === store.sessionEpoch
        ? props.initialState
        : undefined
    : undefined,
)

const pending = computed(() => store.favoritePending.has(props.comicId))
const selected = computed(() => known.value === true)
const label = computed(() =>
  !store.userProfile
    ? '登录后收藏'
    : selected.value
      ? '已收藏，点击取消'
      : known.value === false
        ? '收藏'
        : '收藏状态待确认',
)

async function act() {
  if (!store.userProfile) {
    store.openAccount()
    return
  }
  try {
    const result = await store.setFavorite(props.comicId, known.value !== true)
    if (result !== undefined) message.success(result ? '已收藏' : '已取消收藏')
  } catch (error) {
    message.error(String(error))
  }
}
</script>

<template>
  <button
    v-if="comicId > 0"
    type="button"
    class="favorite-button"
    :class="[`is-${variant}`, { 'is-selected': selected }]"
    :title="label"
    :aria-label="label"
    :aria-pressed="selected"
    :disabled="pending"
    @click.stop="act">
    <PhHeart :size="variant === 'icon' ? 18 : 21" :weight="selected ? 'fill' : 'regular'" />
    <span v-if="variant === 'labeled'">
      {{ pending ? '处理中' : selected ? '已收藏' : !store.userProfile ? '收藏' : known === undefined ? '确认收藏' : '收藏' }}
    </span>
  </button>
</template>

<style scoped>
.favorite-button {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 6px;
  color: var(--jm-text-2);
  border: 1px solid var(--jm-line);
  background: var(--jm-surface);
  cursor: pointer;
  transition:
    color 160ms ease,
    background-color 160ms ease,
    border-color 160ms ease,
    transform 120ms ease;
}

.favorite-button.is-labeled {
  min-height: 44px;
  padding: 0 14px;
  border-radius: var(--jm-r-md);
  font-size: var(--jm-fs-sm);
  font-weight: 600;
}

.favorite-button.is-icon {
  width: 32px;
  height: 32px;
  padding: 0;
  border-color: transparent;
  border-radius: var(--jm-r-pill);
  background: rgba(255, 255, 255, 0.86);
  box-shadow: 0 2px 8px rgba(23, 24, 28, 0.16);
  backdrop-filter: blur(6px);
}

.favorite-button.is-selected {
  color: var(--jm-brand);
  border-color: rgba(251, 114, 153, 0.42);
  background: var(--jm-brand-soft);
}

.favorite-button.is-icon.is-selected {
  background: rgba(255, 255, 255, 0.94);
  box-shadow: 0 2px 10px rgba(251, 114, 153, 0.35);
}

.favorite-button:hover {
  color: var(--jm-brand);
  border-color: rgba(251, 114, 153, 0.42);
}

.favorite-button:active {
  transform: scale(0.94);
}

.favorite-button:disabled {
  opacity: 0.55;
  cursor: wait;
}
</style>
