<script setup lang="ts">
import { ref, nextTick, onMounted, onUnmounted, watch } from 'vue'
import { useChatStore, type ChatMessage } from '../stores/chat'
import { useAppStore } from '../stores/app'
import { useAuthStore } from '../stores/auth'
import { connect, send, sendHistoryRequest, onMessage, removeMessageHandler, wsStatus } from '../composables/useWebSocket'
import { marked } from 'marked'
import hljs from 'highlight.js'
import 'highlight.js/styles/github-dark.min.css'

const props = defineProps<{
  standalone?: boolean
}>()

const chatStore = useChatStore()
const appStore = useAppStore()
const auth = useAuthStore()

const chatMessages = ref<HTMLDivElement | null>(null)
const chatInput = ref<HTMLTextAreaElement | null>(null)

// Configure marked
marked.setOptions({
  breaks: true,
  gfm: true,
})

function renderMarkdown(text: string): string {
  try {
    return (marked as any).parse(text, {
      highlight(code: string, lang: string) {
        if (lang && hljs.getLanguage(lang)) {
          try { return hljs.highlight(code, { language: lang }).value } catch {}
        }
        // Skip highlightAuto — too expensive for large code blocks.
        // renderCodeBlocks() will handle untagged blocks after DOM insertion.
        return code
      },
    })
  } catch {
    return text.replace(/\n/g, '<br>')
  }
}

// Cache rendered HTML to avoid re-computing markdown on every Vue re-render.
const renderedHtmlCache = new WeakMap<ChatMessage, string>()

function getRenderedHtml(msg: ChatMessage): string {
  if (!renderedHtmlCache.has(msg)) {
    renderedHtmlCache.set(msg, renderMarkdown(msg.content))
  }
  return renderedHtmlCache.get(msg)!
}

function getAvatar(role: string): string {
  if (role === 'user') return 'U'
  return 'NB'
}

function formatTime(timestamp: string): string {
  const date = new Date(timestamp)
  return date.toLocaleTimeString('zh-CN', {
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
    hour12: false,
  })
}

function scrollToBottom() {
  if (chatMessages.value) {
    chatMessages.value.scrollTop = chatMessages.value.scrollHeight
  }
}

function handleWSMessage(data: any) {
  if (data.module !== undefined) {
    if (data.type === 'message' && data.module === 'chat') {
      if (data.cmd === 'receive') {
        chatStore.addMessage({
          role: data.data.role || 'assistant',
          content: data.data.content,
          timestamp: data.timestamp,
        })
        chatStore.streaming = false
      } else if (data.cmd === 'history') {
        handleHistoryResponse(data.data)
      }
    } else if (data.type === 'system' && data.module === 'error' && data.cmd === 'notify') {
      chatStore.addMessage({
        role: 'error',
        content: data.data.content || data.data,
        timestamp: data.timestamp,
      })
      chatStore.streaming = false
    }
  }

  nextTick(() => {
    scrollToBottom()
    renderCodeBlocks()
  })
}

function handleHistoryResponse(data: any) {
  chatStore.historyLoading = false
  if (!data) return

  const historyMessages = data.messages || []
  if (historyMessages.length > 0) {
    const container = chatMessages.value
    const oldScrollHeight = container ? container.scrollHeight : 0

    const newMessages: ChatMessage[] = historyMessages.map((m: any) => ({
      role: m.role,
      content: m.content,
      timestamp: m.timestamp || new Date().toISOString(),
    }))
    chatStore.prependHistory(newMessages)

    nextTick(() => {
      if (container) {
        const newScrollHeight = container.scrollHeight
        container.scrollTop = newScrollHeight - oldScrollHeight
      }
    })
  }

  chatStore.hasMoreHistory = data.has_more || false
  chatStore.oldestIndex = data.oldest_index
  chatStore.historyLoaded = true

  if (chatStore.oldestIndex === 0 || !data.has_more) {
    chatStore.hasMoreHistory = false
    nextTick(() => scrollToBottom())
  }
}

function loadHistory() {
  if (chatStore.historyLoading) return
  chatStore.historyLoading = true
  const requestId = 'hist_' + Date.now()
  const limit = 20
  sendHistoryRequest(requestId, limit, chatStore.oldestIndex)

  // Safety timeout: reset loading flag if no response in 10s
  setTimeout(() => {
    if (chatStore.historyLoading) {
      chatStore.historyLoading = false
    }
  }, 10000)
}

function sendMessage() {
  const content = chatStore.input.trim()
  if (!content || chatStore.streaming) return

  chatStore.addMessage({
    role: 'user',
    content,
    timestamp: new Date().toISOString(),
  })

  chatStore.clearInput()
  chatStore.streaming = true

  // Reset textarea height
  if (chatInput.value) chatInput.value.style.height = 'auto'

  send(content)
  nextTick(() => scrollToBottom())
}

function handleKeydown(e: KeyboardEvent) {
  if (e.ctrlKey && e.key === 'Enter') {
    e.preventDefault()
    sendMessage()
  }
}

function handleInput(e: Event) {
  const el = e.target as HTMLTextAreaElement
  el.style.height = 'auto'
  el.style.height = Math.min(el.scrollHeight, 150) + 'px'
}

function renderCodeBlocks() {
  nextTick(() => {
    if (chatMessages.value) {
      chatMessages.value.querySelectorAll('pre code:not(.hljs)').forEach((block) => {
        hljs.highlightElement(block as HTMLElement)
      })
    }
  })
}

// Scroll listener for history
let scrollHandler: (() => void) | null = null

function setupScrollListener() {
  scrollHandler = () => {
    const container = chatMessages.value
    if (!container) return
    if (container.scrollTop <= 50 && chatStore.hasMoreHistory && !chatStore.historyLoading && chatStore.historyLoaded) {
      loadHistory()
    }
  }
}

// Watch WS status
const unwatchStatus = watch(wsStatus, (val) => {
  if (props.standalone) {
    // standalone handles its own connection status
  } else {
    appStore.connected = val === 'connected'
    if (val === 'connected' && !chatStore.historyLoaded) {
      loadHistory()
    }
    // Reset streaming flag on disconnect to prevent stuck UI
    if (val === 'disconnected' && chatStore.streaming) {
      chatStore.streaming = false
    }
  }
})

onMounted(() => {
  onMessage(handleWSMessage)
  setupScrollListener()

  nextTick(() => {
    if (chatMessages.value && scrollHandler) {
      chatMessages.value.addEventListener('scroll', scrollHandler)
    }
  })

  // If not standalone, check if we need to connect
  if (!props.standalone) {
    const token = auth.token
    if (token) {
      connect(null, token)
    }
    // Auth store may have already connected WS before this component mounted.
    // The watcher only fires on value changes, so check current status directly.
    if (wsStatus.value === 'connected' && !chatStore.historyLoaded && !chatStore.historyLoading) {
      loadHistory()
    }
  }
})

onUnmounted(() => {
  if (chatMessages.value && scrollHandler) {
    chatMessages.value.removeEventListener('scroll', scrollHandler)
  }
  removeMessageHandler(handleWSMessage)
  unwatchStatus()
})
</script>

<template>
  <div class="page-chat">
    <!-- Messages -->
    <div ref="chatMessages" class="chat-messages">
      <!-- History loading indicator -->
      <div v-if="chatStore.historyLoading" class="history-loading" style="text-align: center; padding: 8px; color: var(--text-muted); font-size: var(--text-xs);">
        <span class="spinner" style="width:14px;height:14px;border-width:2px;vertical-align:middle;"></span>
        <span style="vertical-align:middle;"> 加载历史消息...</span>
      </div>

      <!-- Welcome message -->
      <div v-if="chatStore.messages.length === 0" class="message assistant">
        <div class="message-avatar">NB</div>
        <div class="message-content">
          <div class="message-bubble">
            <p>你好！我是 NemesisBot。有什么可以帮助你的吗？</p>
          </div>
        </div>
      </div>

      <div v-for="(msg, idx) in chatStore.messages" :key="idx" class="message" :class="msg.role">
        <div class="message-avatar">{{ getAvatar(msg.role) }}</div>
        <div class="message-content">
          <div class="message-bubble">
            <div v-if="msg.role === 'assistant'" class="markdown-body" v-html="getRenderedHtml(msg)"></div>
            <span v-else>{{ msg.content }}</span>
          </div>
          <div class="message-time">{{ formatTime(msg.timestamp) }}</div>
        </div>
      </div>

      <!-- Typing indicator -->
      <div v-if="chatStore.streaming" class="message assistant">
        <div class="message-avatar">NB</div>
        <div class="message-content">
          <div class="message-bubble">
            <div class="typing-indicator"><span></span><span></span><span></span></div>
          </div>
        </div>
      </div>
    </div>

    <!-- Input -->
    <div class="chat-input-area">
      <textarea
        ref="chatInput"
        placeholder="输入消息... (Ctrl+Enter 发送)"
        rows="1"
        v-model="chatStore.input"
        @keydown="handleKeydown"
        @input="handleInput"
        :disabled="chatStore.streaming"
      ></textarea>
      <button class="btn btn-primary" @click="sendMessage" :disabled="!chatStore.input.trim() || chatStore.streaming">
        发送
      </button>
    </div>
  </div>
</template>
