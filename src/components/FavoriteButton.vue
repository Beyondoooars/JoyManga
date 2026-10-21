<script setup lang="ts">
import { computed } from 'vue'
import { PhHeart } from '@phosphor-icons/vue'
import { useMessage } from 'naive-ui'
import { useStore } from '../store.ts'
const props = defineProps<{ comicId: number; initialState?: boolean; initialEpoch?: number }>()
const store = useStore()
const message = useMessage()
const known = computed(() => store.userProfile
  ? (store.favoriteStatuses.has(props.comicId) ? store.favoriteStatuses.get(props.comicId) : (props.initialEpoch === store.sessionEpoch ? props.initialState : undefined))
  : undefined)
const label = computed(() => !store.userProfile ? '登录后收藏' : known.value === true ? '已收藏，点击取消' : known.value === false ? '收藏' : '收藏状态待确认')
async function act() {
  if (!store.userProfile) { store.openAccount(); return }
  try {
    const result = await store.setFavorite(props.comicId, known.value !== true)
    if (result !== undefined) message.success(result ? '已收藏' : '已取消收藏')
  } catch (error) { message.error(String(error)) }
}
</script>
<template>
  <button v-if="comicId > 0" type="button" class="favorite-button" :class="{ selected: known === true }"
    :title="label" :aria-label="label" :aria-pressed="known === true"
    :disabled="store.favoritePending.has(comicId)" @click.stop="act">
    <PhHeart :size="22" :weight="known === true ? 'fill' : 'regular'" />
    <span>{{ store.favoritePending.has(comicId) ? '处理中' : known === true ? '已收藏' : !store.userProfile ? '收藏' : known === undefined ? '确认收藏' : '收藏' }}</span>
  </button>
</template>
<style scoped>
.favorite-button { display: inline-flex; align-items: center; gap: 5px; min-height: 40px; padding: 6px 10px; border: 1px solid #e3e5e7; border-radius: 9px; background: #fff; color: #655b66; cursor: pointer; }
.favorite-button.selected { color: #d94d7c; border-color: #f1b4c7; background: #fff3f7; }
.favorite-button:disabled { opacity: .6; cursor: wait; }
</style>
