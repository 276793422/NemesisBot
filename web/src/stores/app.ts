import { defineStore } from 'pinia'
import { ref, watch } from 'vue'
import { type WSStatus, wsStatus } from '../composables/useWebSocket'

export const useAppStore = defineStore('app', () => {
  const sidebarCollapsed = ref(false)
  const focusMode = ref(false)
  const showMobileSidebar = ref(false)

  const connected = ref(false)

  // Sync WS status to connected
  watch(wsStatus, (val) => {
    connected.value = val === 'connected'
  })

  function toggleSidebar() {
    sidebarCollapsed.value = !sidebarCollapsed.value
  }

  function toggleMobileSidebar() {
    showMobileSidebar.value = !showMobileSidebar.value
  }

  return {
    sidebarCollapsed,
    focusMode,
    showMobileSidebar,
    connected,
    toggleSidebar,
    toggleMobileSidebar,
  }
})
