<script setup lang="ts">
import { ref, nextTick, onMounted, onUnmounted, watch, computed } from 'vue'
import { useChatStore, type ChatMessage } from '../stores/chat'
import { useAppStore } from '../stores/app'
import { useAuthStore } from '../stores/auth'
import { connect, send, sendHistoryRequest, onMessage, removeMessageHandler, wsStatus } from '../composables/useWebSocket'
import { useWSAPI } from '../composables/useWSAPI'
import { useInboxStatus } from '../composables/useInboxStatus'
import { useSlashCommands, filterSlashCommands, type SlashCommand } from '../composables/useSlashCommands'
import { useSessionStore } from '../stores/session'
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
  /** WS protocol module to send/receive on. Defaults to 'chat'. */
  module?: string
  /** Extra fields merged into each send + history_request data payload. */
  moduleData?: Record<string, unknown>
  /** Override the assistant welcome title / heading. */
  titleOverride?: string
  /** Override the textarea placeholder. */
  placeholderOverride?: string
}>()

const chatStore = useChatStore()
const appStore = useAppStore()
const auth = useAuthStore()
const { request } = useWSAPI()
const sessionStore = useSessionStore()

// Multi-session: in the default chat module, attach the active conversation
// id so the backend routes to `agent:main:session:{sid}` (server.rs/loop.rs).
const isDefaultChat = computed(() => (props.module ?? 'chat') === 'chat')
function activeModuleData(): Record<string, unknown> {
  const md: Record<string, unknown> = { ...(props.moduleData ?? {}) }
  if (isDefaultChat.value && sessionStore.currentId) {
    md.session_id = sessionStore.currentId
  }
  return md
}

// U7 inbox visibility (G1): queue/steer state of the active session.
const {
  status: inboxStatus,
  refresh: refreshInbox,
  startPolling: startInboxPolling,
  stopPolling: stopInboxPolling,
  steerEnabled,
  queueEnabled,
  queuedTotal,
  queueFull,
} = useInboxStatus()

/** Re-fetch the inbox mode snapshot (mount / session switch / reconnect). */
function syncInboxMode() {
  if (!isDefaultChat.value) return
  void refreshInbox(sessionStore.currentId || '')
}

/** busy 时发送是否仍然有效（默认 chat + queue/steer 模式）。 */
const canQueueWhileBusy = computed(() => isDefaultChat.value && queueEnabled.value)

/** 输入以 ! 开头且处于 steer 模式 → 提示将以插队发送。 */
const showSteerHint = computed(
  () => steerEnabled.value && /^[!！]/.test(chatStore.input.trimStart()),
)

/** 一键插队：给输入加 `!` 前缀（已有前缀则不动）。 */
function prefixSteer() {
  if (!/^[!！]/.test(chatStore.input.trimStart())) {
    chatStore.input = '! ' + chatStore.input
  }
  chatInput.value?.focus()
}

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

/** Format the model badge: "provider/name" → "provider · name".
 *  Bare name if there's no slash (e.g. continuation path stamps model_name only).
 *  Empty string when model is absent (caller's v-if hides the badge). */
function modelBadge(model: string | undefined): string {
  if (!model) return ''
  const idx = model.indexOf('/')
  if (idx <= 0) return model
  return model.slice(0, idx) + ' · ' + model.slice(idx + 1)
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
    const activeModule = props.module ?? 'chat'
    if (data.type === 'message' && data.module === activeModule) {
      if (data.cmd === 'receive') {
        const incomingRole = data.data.role || 'assistant'
        const incomingContent = data.data?.content
        const last = chatStore.messages[chatStore.messages.length - 1]
        // Skip addMessage if the watchdog already recovered this exact
        // response from session_log (late-arriving live frame, same tail).
        const isDuplicateRecovery =
          !!last &&
          last.role === incomingRole &&
          incomingContent != null &&
          last.content === incomingContent
        if (!isDuplicateRecovery) {
          chatStore.addMessage({
            role: incomingRole,
            content: data.data.content,
            timestamp: data.timestamp,
            model: data.data.model,
          })
        }
        chatStore.streaming = false
        clearWatchdog()

        // TTS playback: if enabled, send AI response to backend for synthesis
        if (voicePlayback.value && ttsReady.value && data.data.role !== 'user' && data.data.content) {
          request('voice', 'tts_playback', { text: data.data.content }).catch(() => {})
        }
      } else if (data.cmd === 'history_response') {
        handleHistoryResponse(data.data)
      } else if (data.cmd === 'history') {
        // Legacy chat history reply uses cmd 'history' (data shape is the same).
        handleHistoryResponse(data.data)
      } else if (data.cmd === 'error') {
        chatStore.addMessage({
          role: 'error',
          content: data.data.content || data.data,
          timestamp: data.timestamp,
        })
        chatStore.streaming = false
        clearWatchdog()
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

// --- Watchdog: recover from a lost live response frame ---
// If `streaming` stays true past WATCHDOG_MS with no receive/error frame, the
// WS frame was likely lost (e.g. half-open connection). The response is already
// persisted to session_log, so resync by reloading the latest page and
// REPLACING the message list (no dedup / stable-id needed). Default chat only
// — workflow_chat streaming is engine-driven, not in this session_log path.
const WATCHDOG_MS = 45000
const MAX_WATCHDOG_ATTEMPTS = 3
let watchdogTimer: ReturnType<typeof setTimeout> | null = null
let watchdogAttempts = 0
let pendingWatchdogReload = false
// # of assistant messages at send time — used to detect that the lost
// response has actually landed in session_log (vs. still running).
let assistantCountAtSend = 0

function clearWatchdog() {
  if (watchdogTimer) {
    clearTimeout(watchdogTimer)
    watchdogTimer = null
  }
}
function armWatchdog() {
  clearWatchdog()
  watchdogTimer = setTimeout(onWatchdog, WATCHDOG_MS)
}
function startWatchdog() {
  watchdogAttempts = 0
  pendingWatchdogReload = false
  assistantCountAtSend = chatStore.messages.filter(m => m.role === 'assistant').length
  armWatchdog()
}
function reloadLatest() {
  pendingWatchdogReload = true
  sendHistoryRequest('watchdog_' + Date.now(), 50, null, {
    module: props.module,
    moduleData: activeModuleData(),
  })
}
function onWatchdog() {
  watchdogTimer = null
  if (!chatStore.streaming) return
  if (!isDefaultChat.value) return
  watchdogAttempts++
  reloadLatest()
}

function handleHistoryResponse(data: any) {
  chatStore.historyLoading = false
  if (!data) return

  // Watchdog-driven resync: if a genuinely new assistant message is in
  // session_log (more than at send time), the lost response landed — replace
  // from source of truth and un-stick. Otherwise keep the current view and
  // re-check (never drop the just-sent user message).
  if (pendingWatchdogReload) {
    pendingWatchdogReload = false
    const rawMsgs: any[] = data.messages || []
    const latestAssistantCount = rawMsgs.filter((m: any) => m.role === 'assistant').length
    if (chatStore.streaming && latestAssistantCount > assistantCountAtSend) {
      chatStore.replaceMessages(
        rawMsgs.map((m: any) => ({
          role: m.role,
          content: m.content,
          timestamp: m.timestamp || new Date().toISOString(),
          model: m.model,
        })),
      )
      chatStore.streaming = false
      clearWatchdog()
      nextTick(() => scrollToBottom())
    } else if (chatStore.streaming && watchdogAttempts < MAX_WATCHDOG_ATTEMPTS) {
      // Response not landed yet (maybe still running) — re-check later.
      armWatchdog()
    } else {
      clearWatchdog()
    }
    return
  }

  const historyMessages = data.messages || []
  if (historyMessages.length > 0) {
    const container = chatMessages.value
    const oldScrollHeight = container ? container.scrollHeight : 0

    const newMessages: ChatMessage[] = historyMessages.map((m: any) => ({
      role: m.role,
      content: m.content,
      timestamp: m.timestamp || new Date().toISOString(),
      model: m.model,
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
  sendHistoryRequest(requestId, limit, chatStore.oldestIndex, {
    module: props.module,
    moduleData: activeModuleData(),
  })

  // Safety timeout: reset loading flag if no response in 10s
  setTimeout(() => {
    if (chatStore.historyLoading) {
      chatStore.historyLoading = false
    }
  }, 10000)
}

function sendMessage() {
  const content = chatStore.input.trim()
  if (!content) return
  // U7: queue/steer 模式下 busy 发送是合法操作（后端排队/插队）；reject 模式维持原样。
  if (chatStore.streaming && !canQueueWhileBusy.value) return

  chatStore.addMessage({
    role: 'user',
    content,
    timestamp: new Date().toISOString(),
  })

  chatStore.clearInput()
  chatStore.streaming = true
  startWatchdog()

  // Reset textarea height
  if (chatInput.value) chatInput.value.style.height = 'auto'

  // Send with voice_playback flag if playback is enabled
  send(content, voicePlayback.value, {
    module: props.module,
    moduleData: activeModuleData(),
  })

  // U7: busy 中排队/插队 → 立即拉一次队列快照并轮询，chip 才能出现。
  if (canQueueWhileBusy.value) {
    startInboxPolling(sessionStore.currentId || '')
  }

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
  // stopGeneration only applies to the default chat module (cancels the
  // agent loop). Workflow_chat streams are driven by the workflow engine,
  // not the agent loop, so agent.cancel is a no-op there — we hide the
  // stop button in that case via `showStopButton`.
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

const showStopButton = computed(() => {
  const activeModule = props.module ?? 'chat'
  return activeModule === 'chat'
})

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
    return
  }
  // slash 命令菜单打开时接管导航键（Enter/Tab 选中，↑↓ 移动，Esc 关闭）。
  if (handleSlashKeydown(e)) return
}

// ---------------------------------------------------------------------------
// 自定义 slash 命令补全（2026-08-29）：输入 / + 名称片段时弹出命令菜单。
// 选中只负责把 "/name 命令" 填进输入框；模板展开在后端 AgentLoop 入口
// （rewrite_custom_command），对所有通道生效。
// ---------------------------------------------------------------------------

const slash = useSlashCommands()
const slashItems = ref<SlashCommand[]>([])
const slashIndex = ref(0)
const slashOpen = computed(() => slashItems.value.length > 0)

watch(() => chatStore.input, input => {
  // 首次输入 / 时静默拉取命令表（失败无补全，不影响输入）。
  if (input.startsWith('/') && !slash.loaded.value) void slash.load()
  slashItems.value = filterSlashCommands(input, slash.commands.value)
  if (slashIndex.value >= slashItems.value.length) slashIndex.value = 0
})

function applySlashCommand(cmd: SlashCommand) {
  // 参数提示以灰字形式预填（用户替换为真实参数）；无参数提示则留一个空格。
  chatStore.input = `/${cmd.name}${cmd.argument_hint ? ' ' + cmd.argument_hint : ' '}`
  slashItems.value = []
  chatInput.value?.focus()
}

function handleSlashKeydown(e: KeyboardEvent): boolean {
  if (!slashOpen.value) return false
  if (e.key === 'ArrowDown') {
    e.preventDefault()
    slashIndex.value = (slashIndex.value + 1) % slashItems.value.length
    return true
  }
  if (e.key === 'ArrowUp') {
    e.preventDefault()
    slashIndex.value = (slashIndex.value - 1 + slashItems.value.length) % slashItems.value.length
    return true
  }
  if (e.key === 'Enter' || e.key === 'Tab') {
    e.preventDefault()
    const cmd = slashItems.value[slashIndex.value]
    if (cmd) applySlashCommand(cmd)
    return true
  }
  if (e.key === 'Escape') {
    slashItems.value = []
    return true
  }
  return false
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
      voicePlayback.value = false
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
      syncInboxMode()
    }
  }
})

// U7: streaming 结束 → 停轮询并刷新一次（队列里剩余条数清零/被消费）。
const unwatchStreaming = watch(() => chatStore.streaming, (s) => {
  if (!isDefaultChat.value) return
  if (!s) {
    stopInboxPolling()
    syncInboxMode()
  }
})

// Multi-session: when the active conversation id changes, reset the chat
// state and reload that conversation's history (backend routes by session_id).
const unwatchSession = watch(
  () => sessionStore.currentId,
  (newId, oldId) => {
    if (!isDefaultChat.value || newId === oldId) return
    chatStore.reset()
    if (newId && wsStatus.value === 'connected') {
      loadHistory()
    }
    syncInboxMode()
  },
)

onMounted(() => {
  onMessage(handleWSMessage)
  setupScrollListener()

  // Non-default module (e.g., workflow_chat) must NOT share conversation
  // state with a prior chat session in the same tab — reset before binding.
  const activeModule = props.module ?? 'chat'
  if (activeModule !== 'chat') {
    chatStore.reset()
  }

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

  // U7: 挂载时拉一次 inbox 模式（失败则保守按 reject 处理）。
  syncInboxMode()
})

onUnmounted(() => {
  if (chatMessages.value && scrollHandler) {
    chatMessages.value.removeEventListener('scroll', scrollHandler)
  }
  removeMessageHandler(handleWSMessage)
  unwatchStatus()
  unwatchSession()
  unwatchStreaming()
  // 离开 chat 页时停掉活跃的 STT 会话，避免后端 orphan 后再回来 "already running" 卡死
  // （组件重挂载后 voiceDictation 是新 ref=false，但后端会话还在跑 → 重启报错 → 永远起不来）
  if (voiceDictation.value) {
    request('voice', 'stt_to_input_stop').catch(() => {})
  }
  if (voiceDialogue.value) {
    request('voice', 'stt_dialogue_stop').catch(() => {})
  }
  if (voicePlayback.value) {
    request('voice', 'tts_playback_stop').catch(() => {})
  }
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
            <p>{{ props.titleOverride || '你好！我是 NemesisBot。有什么可以帮助你的吗？' }}</p>
          </div>
        </div>
      </div>

      <div v-for="(msg, idx) in chatStore.messages" :key="idx" class="message" :class="msg.role">
        <div class="message-avatar">{{ getAvatar(msg.role) }}</div>
        <div class="message-content">
          <div class="message-bubble">
            <div v-if="msg.role === 'assistant'" class="markdown-body" v-html="getRenderedHtml(msg)"></div>
            <div v-else class="message-text">{{ msg.content }}</div>
          </div>
          <div class="message-time">
            <span>{{ formatTime(msg.timestamp) }}</span>
            <span v-if="msg.role === 'assistant' && modelBadge(msg.model)" class="model-badge">{{ modelBadge(msg.model) }}</span>
          </div>
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
        v-if="steerEnabled"
        class="voice-btn steer-btn"
        title="一键插队：给输入加 ! 前缀，agent 忙碌时立即送达当前轮"
        @click="prefixSteer"
      >
        <span class="steer-mark">!</span>
        插队
      </button>
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

    <!-- U7 inbox visibility: queued/steer chip + steer input hint -->
    <div v-if="chatStore.streaming && queuedTotal > 0" class="queue-chip" :class="{ full: queueFull }">
      ⏳ agent 处理中，已排队 {{ queuedTotal }} 条（其中插队 {{ inboxStatus?.next_step ?? 0 }}）<template v-if="queueFull"> · 队列已满</template>
    </div>
    <div v-if="showSteerHint" class="steer-hint">
      ⚡ 将以插队（steer）模式发送，立即送达当前轮
    </div>

    <!-- Input -->
    <div class="chat-input-area">
      <!-- slash 命令补全菜单 -->
      <div v-if="slashOpen" class="slash-menu">
        <div
          v-for="(c, i) in slashItems"
          :key="c.name"
          class="slash-item"
          :class="{ active: i === slashIndex }"
          @mousedown.prevent="applySlashCommand(c)"
          @mouseenter="slashIndex = i"
        >
          <span class="slash-item-name">/{{ c.name }}</span>
          <span class="slash-item-desc">{{ c.description }}</span>
          <span v-if="c.argument_hint" class="slash-item-hint">{{ c.argument_hint }}</span>
        </div>
      </div>
      <textarea
        ref="chatInput"
        :placeholder="props.placeholderOverride || '输入消息... (Ctrl+Enter 发送)'"
        rows="1"
        v-model="chatStore.input"
        @keydown="handleKeydown"
        @input="handleInput"
        :disabled="chatStore.streaming && !canQueueWhileBusy"
      ></textarea>
      <button v-if="chatStore.streaming && showStopButton" class="btn btn-stop" @click="stopGeneration" title="停止生成">
        <svg viewBox="0 0 24 24" fill="currentColor" width="16" height="16">
          <rect x="6" y="6" width="12" height="12" rx="2"/>
        </svg>
      </button>
      <button v-if="!chatStore.streaming || canQueueWhileBusy" class="btn btn-primary" @click="sendMessage" :disabled="!chatStore.input.trim()">
        发送
      </button>
      <span v-else-if="!showStopButton" class="btn btn-primary btn-disabled-workflow" title="工作流执行中，无法中断">
        执行中...
      </span>
      <button
        class="toolbar-toggle"
        :class="{ active: sessionStore.showSidebar }"
        @click="sessionStore.toggleSidebar()"
        :title="sessionStore.showSidebar ? '隐藏会话列表' : '显示会话列表'"
      >
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" width="18" height="18">
          <rect x="3" y="4" width="18" height="16" rx="2" />
          <line x1="9" y1="4" x2="9" y2="20" />
        </svg>
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
/* U7 inbox visibility */
.queue-chip {
  padding: 4px 12px;
  font-size: var(--text-xs);
  color: var(--text-secondary);
  background: var(--surface);
  border-top: 1px solid var(--border);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.queue-chip.full {
  color: #dc3545;
}
.steer-hint {
  padding: 4px 12px;
  font-size: var(--text-xs);
  color: var(--accent);
  background: var(--surface);
  border-top: 1px solid var(--border);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.steer-btn {
  border-color: var(--accent);
  color: var(--accent);
}
.steer-mark {
  font-weight: 700;
}
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
.btn-disabled-workflow {
  opacity: 0.6;
  cursor: not-allowed;
  pointer-events: none;
}

/* slash 菜单锚定：chat-input-area 全局样式无定位，本组件内补 relative。 */
.chat-input-area {
  position: relative;
}

/* slash 命令补全菜单（2026-08-29） */
.slash-menu {
  position: absolute;
  bottom: 100%;
  left: 12px;
  right: 12px;
  max-height: 240px;
  overflow-y: auto;
  background: var(--bg-primary);
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
  box-shadow: 0 4px 16px rgba(0, 0, 0, 0.15);
  z-index: 50;
}
.slash-item {
  display: flex;
  gap: var(--space-2);
  align-items: baseline;
  padding: var(--space-2) var(--space-3);
  cursor: pointer;
  font-size: var(--text-sm);
}
.slash-item.active {
  background: var(--bg-hover, rgba(59, 130, 246, 0.1));
}
.slash-item-name {
  font-family: var(--font-mono);
  font-weight: 600;
  color: var(--text-primary);
  white-space: nowrap;
}
.slash-item-desc {
  flex: 1;
  color: var(--text-secondary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.slash-item-hint {
  color: var(--text-muted);
  font-size: var(--text-xs);
  font-family: var(--font-mono);
  white-space: nowrap;
}
</style>
