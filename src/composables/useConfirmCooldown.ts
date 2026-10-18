import { computed, onUnmounted, ref } from 'vue'

export function useConfirmCooldown(durationSeconds = 10) {
  const remainingSeconds = ref(0)
  const disabled = computed(() => remainingSeconds.value > 0)
  let interval: ReturnType<typeof setInterval> | undefined

  function stop() {
    if (interval !== undefined) {
      clearInterval(interval)
      interval = undefined
    }
  }

  function start() {
    stop()
    remainingSeconds.value = durationSeconds
    interval = setInterval(() => {
      remainingSeconds.value = Math.max(0, remainingSeconds.value - 1)
      if (remainingSeconds.value === 0) {
        stop()
      }
    }, 1000)
  }

  onUnmounted(stop)

  return { remainingSeconds, disabled, start }
}
