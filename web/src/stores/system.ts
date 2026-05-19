import { defineStore } from 'pinia'
import { ref } from 'vue'

export const useSystemStore = defineStore('system', () => {
  const status = ref<Record<string, any>>({})

  const logs = ref<any[]>([])
  const scannerEngines = ref<any[]>([])

  return { status, logs, scannerEngines }
})
