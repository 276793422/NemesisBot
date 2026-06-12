<script setup lang="ts">
import { ref, nextTick, onMounted, onUnmounted, watch } from 'vue'
import { useChatStore, type ChatMessage } from '../stores/chat'
import { useAppStore } from '../stores/app'
import { useAuthStore } from '../stores/auth'
import { connect, send, sendHistoryRequest, onMessage, removeMessageHandler, wsStatus } from '../composables/useWebSocket'
import { useWSAPI } from '../composables/useWSAPI'
import { marked } from 'marked'
import hljs from 'highlight.js/lib/core'
import javascript from 'highlight.js/lib/languages/javascript'
import typescript from 'highlight.js/lib/languages/typescript'
import python from 'highlight.js/lib/languages/python'
import rust from 'highlight.js/lib/languages/rust'
import bash from 'highlight.js/lib/languages/bash'
import json from 'highlight.js/lib/languages/json'
import xml from 'highlight.js/lib/languages/xml'
import css from 'highlight.js/lib/languages/css'
import sql from 'highlight.js/lib/languages/sql'
import yaml from 'highlight.js/lib/languages/yaml'
import markdown from 'highlight.js/lib/languages/markdown'
import 'highlight.js/styles/github-dark.min.css'

hljs.registerLanguage('javascript', javascript)
hljs.registerLanguage('typescript', typescript)
hljs.registerLanguage('python', python)
hljs.registerLanguage('rust', rust)
hljs.registerLanguage('bash', bash)
hljs.registerLanguage('json', json)
hljs.registerLanguage('xml', xml)
hljs.registerLanguage('html', xml)
hljs.registerLanguage('css', css)
hljs.registerLanguage('sql', sql)
hljs.registerLanguage('yaml', yaml)
hljs.registerLanguage('markdown', markdown)

const props = defineProps<{
  standalone?: boolean
}>()

const chatStore = useChatStore()
const appStore = useAppStore()
const auth = useAuthStore()
const { request } = useWSAPI()

// Voice toolbar state
const sttReady = ref(false)
const ttsReady = ref(false)
const voiceDictation = ref(false)
const voiceDialogue = ref(false)
const voicePlayback = ref(false)
const toolbarCollapsed = ref(false)
const silenceTimeout = ref(3.0)

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
  const now = new Date()
  const isToday = date.getFullYear() === now.getFullYear()
    && date.getMonth() === now.getMonth()
    && date.getDate() === now.getDate()
  if (isToday) {
    return date.toLocaleTimeString('zh-CN', {
      hour: '2-digit',
      minute: '2-digit',
      second: '2-digit',
      hour12: false,
    })
  }
  const y = date.getFullYear()
  const M = String(date.getMonth() + 1).padStart(2, '0')
  const d = String(date.getDate()).padStart(2, '0')
  const time = date.toLocaleTimeString('zh-CN', {
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
    hour12: false,
  })
  return `${y}-${M}-${d} ${time}`
}

function scrollToBottom() {
  if (chatMessages.value) {
    chatMessages.value.scrollTop = chatMessages.value.scrollHeight
  }
}

// Track whether user is near the bottom of the chat.
// If user scrolled up to read history, don't auto-scroll on new messages.
const userNearBottom = ref(true)

function checkUserNearBottom() {
  const el = chatMessages.value
  if (!el) return
  // Within 80px of bottom counts as "near bottom"
  userNearBottom.value = el.scrollHeight - el.scrollTop - el.clientHeight < 80
}

function scrollToBottomIfNear() {
  if (userNearBottom.value) {
    scrollToBottom()
  }
}

function onChatAreaClick() {
  chatInput.value?.focus()
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

        // TTS playback: if enabled, send AI response to backend for synthesis
        if (voicePlayback.value && ttsReady.value && data.data.role !== 'user' && data.data.content) {
          request('voice', 'tts_playback', { text: data.data.content }).catch(() => {})
        }
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

  // Voice push messages
  if (data.type === 'push' && data.module === 'voice') {
    if (data.cmd === 'stt_to_input' && data.data?.text) {
      chatStore.input += data.data.text
    } else if (data.cmd === 'stt_accumulate' && data.data?.text) {
      chatStore.input = data.data.text
    } else if (data.cmd === 'stt_auto_send' && data.data?.text) {
      chatStore.input = data.data.text
      sendMessage()
    } else if (data.cmd === 'engine_fault') {
      if (data.data?.engine === 'stt') {
        sttReady.value = false
        voiceDictation.value = false
        voiceDialogue.value = false
      }
      if (data.data?.engine === 'tts') {
        ttsReady.value = false
        voicePlayback.value = false
      }
    } else if (data.cmd === 'speaker_rejected') {
      chatStore.addMessage({
        role: 'error',
        content: '⚠ 声纹验证未通过，语音输入已忽略',
        timestamp: new Date().toISOString(),
      })
    }
  }

  nextTick(() => {
    scrollToBottomIfNear()
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

  // Send with voice_playback flag if playback is enabled
  send(content, voicePlayback.value)

  // If dialogue mode is active, reset the accumulation buffer to prevent duplicate send
  if (voiceDialogue.value) {
    request('voice', 'stt_dialogue_reset').catch(() => {})
  }

  nextTick(() => scrollToBottom())
  nextTick(() => {
    chatInput.value?.focus()
    userNearBottom.value = true
  })
}

function stopGeneration() {
  request('agent', 'cancel').then((res) => {
    if (res && res.cancelled > 0) {
      chatStore.streaming = false
      chatStore.addMessage({
        role: 'system',
        content: '已停止生成',
        timestamp: new Date().toISOString(),
      })
      nextTick(() => scrollToBottom())
    }
  }).catch(() => {
    chatStore.streaming = false
  })
}

// Voice toolbar toggle functions
async function toggleDictation() {
  if (voiceDictation.value) {
    await request('voice', 'stt_to_input_stop').catch(() => {})
    voiceDictation.value = false
  } else {
    if (!sttReady.value) return
    // Close dialogue if open
    if (voiceDialogue.value) {
      await request('voice', 'stt_dialogue_stop').catch(() => {})
      voiceDialogue.value = false
    }
    try {
      await request('voice', 'stt_to_input_start')
      voiceDictation.value = true
    } catch {}
  }
  saveVoiceConfig()
}

async function toggleDialogue() {
  if (voiceDialogue.value) {
    await request('voice', 'stt_dialogue_stop').catch(() => {})
    voiceDialogue.value = false
  } else {
    if (!sttReady.value) return
    // Close dictation if open
    if (voiceDictation.value) {
      await request('voice', 'stt_to_input_stop').catch(() => {})
      voiceDictation.value = false
    }
    try {
      await request('voice', 'stt_dialogue_start', { silence_timeout: silenceTimeout.value })
      voiceDialogue.value = true
    } catch {}
  }
  saveVoiceConfig()
}

async function togglePlayback() {
  if (voicePlayback.value) {
    await request('voice', 'tts_playback_stop').catch(() => {})
    voicePlayback.value = false
  } else {
    if (!ttsReady.value) return
    voicePlayback.value = true
  }
  saveVoiceConfig()
}

function toggleToolbar() {
  toolbarCollapsed.value = !toolbarCollapsed.value
  saveVoiceConfig()
}

async function saveVoiceConfig() {
  try {
    await request('voice', 'chat_config_set', {
      toolbar_collapsed: toolbarCollapsed.value,
      dictation_enabled: voiceDictation.value,
      dialogue_enabled: voiceDialogue.value,
      playback_enabled: voicePlayback.value,
    })
  } catch {}
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

async function initVoiceState() {
  try {
    const [config, engines, voiceCfg] = await Promise.all([
      request('voice', 'chat_config_get'),
      request('voice', 'engine_status'),
      request('voice', 'voice_config_get'),
    ])
    if (config) {
      toolbarCollapsed.value = config.toolbar_collapsed ?? false
      // Visual-only restore: buttons show enabled state but pipelines are NOT started
      voiceDictation.value = config.dictation_enabled ?? false
      voiceDialogue.value = config.dialogue_enabled ?? false
      voicePlayback.value = config.playback_enabled ?? false
      // Reset to false since pipelines aren't actually running
      voiceDictation.value = false
      voiceDialogue.value = false
    }
    if (engines) {
      sttReady.value = engines.stt_ready ?? false
      ttsReady.value = engines.tts_ready ?? false
    }
    if (voiceCfg) {
      silenceTimeout.value = voiceCfg.silence_timeout ?? 3.0
    }
  } catch {
    // Voice not available — keep buttons disabled
  }
}

// Scroll listener for history
let scrollHandler: (() => void) | null = null

function setupScrollListener() {
  scrollHandler = () => {
    const container = chatMessages.value
    if (!container) return
    checkUserNearBottom()
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
    if (val === 'connected') {
      initVoiceState()
    }
  }
})

onMounted(() => {
  onMessage(handleWSMessage)
  setupScrollListener()

  nextTick(() => {
    scrollToBottom()
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

  // Initialize voice toolbar state after WS is ready
  if (wsStatus.value === 'connected') {
    initVoiceState()
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
    <div ref="chatMessages" class="chat-messages" @click="onChatAreaClick">
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

    <!-- Toolbar -->
    <div v-if="!toolbarCollapsed" class="voice-toolbar">
      <button
        class="voice-btn"
        :class="{ active: voiceDictation }"
        :disabled="!sttReady"
        :title="sttReady ? '听写：说话内容追加到输入框' : '请先在语音通道页启用 STT 引擎'"
        @click="toggleDictation"
      >
        <svg class="voice-btn-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <path d="M17 3a2.85 2.83 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5Z"/>
          <path d="m15 5 4 4"/>
          <rect x="3" y="13" width="7" height="8" rx="1"/>
        </svg>
        听写
      </button>
      <button
        class="voice-btn"
        :class="{ active: voiceDialogue }"
        :disabled="!sttReady"
        :title="sttReady ? '语音对话：说话后自动发送给 AI' : '请先在语音通道页启用 STT 引擎'"
        @click="toggleDialogue"
      >
        <svg class="voice-btn-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <path d="M12 2a3 3 0 0 0-3 3v7a3 3 0 0 0 6 0V5a3 3 0 0 0-3-3Z"/>
          <path d="M19 10v2a7 7 0 0 1-14 0v-2"/>
          <line x1="12" x2="12" y1="19" y2="22"/>
        </svg>
        语音对话
      </button>
      <button
        class="voice-btn"
        :class="{ active: voicePlayback }"
        :disabled="!ttsReady"
        :title="ttsReady ? '语音播放：AI 回复自动朗读' : '请先在语音通道页启用 TTS 引擎'"
        @click="togglePlayback"
      >
        <svg class="voice-btn-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <polygon points="11 5 6 9 2 9 2 15 6 15 11 19 11 5"/>
          <path d="M15.54 8.46a5 5 0 0 1 0 7.07"/>
          <path d="M19.07 4.93a10 10 0 0 1 0 14.14"/>
        </svg>
        语音播放
      </button>
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
      <button v-if="chatStore.streaming" class="btn btn-stop" @click="stopGeneration" title="停止生成">
        <svg viewBox="0 0 24 24" fill="currentColor" width="16" height="16">
          <rect x="6" y="6" width="12" height="12" rx="2"/>
        </svg>
      </button>
      <button v-else class="btn btn-primary" @click="sendMessage" :disabled="!chatStore.input.trim()">
        发送
      </button>
      <button
        class="toolbar-toggle"
        :class="{ active: !toolbarCollapsed }"
        @click="toggleToolbar"
        :title="toolbarCollapsed ? '展开工具栏' : '收起工具栏'"
      >
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
          <polygon points="12,2 20.66,7 20.66,17 12,22 3.34,17 3.34,7"/>
          <circle cx="12" cy="12" r="3.5"/>
        </svg>
      </button>
    </div>
  </div>
</template>

<style scoped>
.voice-toolbar {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 6px 12px;
  background: var(--surface);
  min-height: 36px;
}
.voice-btn {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 6px 12px;
  font-size: 13px;
  border: 1px solid var(--border);
  border-radius: 6px;
  background: var(--bg-primary);
  color: var(--text-secondary);
  cursor: pointer;
  transition: all 0.15s;
  white-space: nowrap;
  line-height: 1;
}
.voice-btn-icon {
  width: 16px;
  height: 16px;
  flex-shrink: 0;
}
.voice-btn:hover:not(:disabled) {
  border-color: var(--accent);
  color: var(--accent);
}
.voice-btn.active {
  background: var(--accent);
  color: #fff;
  border-color: var(--accent);
}
.voice-btn:disabled {
  opacity: 0.4;
  cursor: not-allowed;
}
.toolbar-toggle {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  padding: 0.5rem 1rem;
  font-size: 0.8125rem;
  font-weight: 500;
  font-family: var(--font-sans);
  line-height: 1.5;
  border: 1px solid var(--accent);
  border-radius: var(--radius-md);
  background: transparent;
  color: var(--text-muted);
  cursor: pointer;
  transition: all 0.15s;
  flex-shrink: 0;
}
.toolbar-toggle svg {
  width: 18px;
  height: 18px;
}
.toolbar-toggle:hover {
  background: var(--accent-muted);
}
.toolbar-toggle.active {
  border-color: var(--accent);
  color: var(--accent);
}
.btn-stop {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  padding: 0.5rem 1rem;
  font-size: 0.8125rem;
  font-weight: 500;
  font-family: var(--font-sans);
  line-height: 1.5;
  border: 1px solid #dc3545;
  border-radius: var(--radius-md);
  background: #dc3545;
  color: #fff;
  cursor: pointer;
  transition: all 0.15s;
  flex-shrink: 0;
}
.btn-stop:hover {
  background: #c82333;
  border-color: #c82333;
}
.btn-stop svg {
  display: block;
}
</style>
