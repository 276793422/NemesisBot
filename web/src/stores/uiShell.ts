import { defineStore } from 'pinia'
import { computed, ref, watch } from 'vue'
import {
  type UiShellMode,
  readUiShellMode,
  toggleUiShellMode,
  writeUiShellMode,
} from '../lib/uiShell'

export const useUiShellStore = defineStore('uiShell', () => {
  const mode = ref<UiShellMode>(readUiShellMode())

  const isFriendly = computed(() => mode.value === 'friendly')
  const isClassic = computed(() => mode.value === 'classic')

  function setMode(next: UiShellMode) {
    mode.value = next
  }

  function toggle() {
    mode.value = toggleUiShellMode(mode.value)
  }

  watch(
    mode,
    (m) => {
      writeUiShellMode(m)
      if (typeof document !== 'undefined') {
        document.documentElement.dataset.uiShell = m
      }
    },
    { immediate: true },
  )

  return {
    mode,
    isFriendly,
    isClassic,
    setMode,
    toggle,
  }
})
