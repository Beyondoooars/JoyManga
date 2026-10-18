import { onMounted, onUnmounted, readonly, ref } from 'vue'

export const isAndroid = typeof navigator !== 'undefined' && /Android/i.test(navigator.userAgent)

const isMobile = ref(isAndroid || (typeof window !== 'undefined' ? window.innerWidth <= 768 : false))
let listenerCount = 0

function handleResize() {
  if (typeof window !== 'undefined') {
    isMobile.value = isAndroid || window.innerWidth <= 768
  }
}

export function useIsMobile() {
  onMounted(() => {
    if (listenerCount === 0 && typeof window !== 'undefined') {
      window.addEventListener('resize', handleResize)
      handleResize()
    }
    listenerCount++
  })

  onUnmounted(() => {
    listenerCount = Math.max(0, listenerCount - 1)
    if (listenerCount === 0 && typeof window !== 'undefined') {
      window.removeEventListener('resize', handleResize)
    }
  })

  return {
    isMobile: readonly(isMobile),
    isAndroid,
  }
}
