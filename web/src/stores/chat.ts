import { defineStore } from 'pinia'
import { ref } from 'vue'

export interface ChatMessage {
  role: 'user' | 'assistant' | 'error' | 'system'
  content: string
  timestamp: string
}

export const useChatStore = defineStore('chat', () => {
  const messages = ref<ChatMessage[]>([])
  const input = ref('')
  const streaming = ref(false)

  // History state
  const historyLoading = ref(false)
  const hasMoreHistory = ref(true)
  const oldestIndex = ref<number | null>(null)
  const historyLoaded = ref(false)

  function addMessage(msg: ChatMessage) {
    messages.value.push(msg)
  }

  function prependHistory(history: ChatMessage[]) {
    messages.value = [...history, ...messages.value]
  }

  function clearInput() {
    input.value = ''
  }

  return {
    messages,
    input,
    streaming,
    historyLoading,
    hasMoreHistory,
    oldestIndex,
    historyLoaded,
    addMessage,
    prependHistory,
    clearInput,
  }
})
