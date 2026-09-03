import { defineStore } from 'pinia'
import { ref } from 'vue'

export interface ChatMessage {
  role: 'user' | 'assistant' | 'error' | 'system'
  content: string
  timestamp: string
  /** Producing model in `provider/name` form (assistant messages only).
   *  Rendered as a "供应商·模型名" badge; undefined for user/error/system or
   *  legacy messages persisted before the badge feature. */
  model?: string
  /** T8 多模态：该消息附带的图片数（本地回显 + 历史映射 m.images.length）。 */
  imageCount?: number
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

  /** Replace the whole message list — used by the chat watchdog to resync
   *  from session_log when a live response frame is suspected lost. Replacing
   *  (not merging) avoids any dedup / stable-id requirement. */
  function replaceMessages(msgs: ChatMessage[]) {
    messages.value = [...msgs]
  }

  function clearInput() {
    input.value = ''
  }

  /**
   * Reset all conversation state. Used when ChatPanel mounts under a
   * non-default module (e.g., workflow_chat) so messages from a previous
   * chat session don't bleed into the new context.
   */
  function reset() {
    messages.value = []
    input.value = ''
    streaming.value = false
    historyLoading.value = false
    hasMoreHistory.value = true
    oldestIndex.value = null
    historyLoaded.value = false
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
    replaceMessages,
    clearInput,
    reset,
  }
})
