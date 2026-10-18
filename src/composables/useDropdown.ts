import { nextTick, ref } from 'vue'

export function useDropdown() {
  const dropdownX = ref<number>(0)
  const dropdownY = ref<number>(0)
  const dropdownShowing = ref<boolean>(false)

  async function showDropdown(e: MouseEvent) {
    dropdownShowing.value = false
    await nextTick()
    dropdownShowing.value = true
    dropdownX.value = e.clientX
    dropdownY.value = e.clientY
  }

  function closeDropdown() {
    dropdownShowing.value = false
  }

  return {
    dropdownX,
    dropdownY,
    dropdownShowing,
    showDropdown,
    closeDropdown,
  }
}
