<script setup lang="ts">
import { ref, nextTick, onMounted, onUnmounted, watch, computed } from 'vue'
import { useChatStore, type ChatMessage } from '../stores/chat'
import { useAppStore } from '../stores/app'
import { useAuthStore } from '../stores/auth'
import { connect, send, sendHistoryRequest, onMessage, removeMessageHandler, wsStatus } from '../composables/useWebSocket'
import { useWSAPI } from '../composables/useWSAPI'
// L2（devtool-upgrade 阶段 6）：SSE resync 提示 → 会话全量刷新兜底。
import { on as onSSE, off as offSSE } from '../composables/useSSE'
import { useInboxStatus } from '../composables/useInboxStatus'
import { useSlashCommands, filterSlashCommands, type SlashCommand } from '../composables/useSlashCommands'
import { useSessionStore } from '../stores/session'
import { uploadImage, validateImageFile, type UploadedImage } from '../composables/useImageUpload'
import { useToast } from '../composables/useToast'
// H2 (2026-09-05): todo 清单面板（todowrite 工具的实时渲染）。
import TodoPanel from './chat/TodoPanel.vue'

import ShareModal from './ShareModal.vue'
// M1b (2026-09-05): 工具调用卡片（M1a AgentEvent 通道的实时渲染）。
import ToolCallCard from './chat/ToolCallCard.vue'
import type { ToolEvent } from '../stores/chat'
// M5 (2026-09-05): 会话级 context/cost 常驻条——cost 格式化与侧栏共用。
import { fmtCost } from '../composables/useUsageFormat'
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
// M2 (2026-09-05): diff 语言高亮——A2/A5 工具回灌的 ```diff 代码块获得行级 +/- 高亮。
import diff from 'highlight.js/lib/languages/diff'
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
hljs.registerLanguage('diff', diff)

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

// L6++（2026-09-08）：项目 chip——当前会话归属项目时，欢迎语上方显示
// 「⟦项目名⟧」（注册表联结显示名；已移除/未知 pid 不显示，不误导）。
const activeProjectName = computed(() => {
  const pid = sessionStore.sessions.find(s => s.id === sessionStore.currentId)?.projectId
  return pid ? sessionStore.projectNameOf(pid) : null
})

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

// --- F1: plan/build 模式徽标（chat.get_mode 对齐 + chat.set_mode 切换 +
// ModeChanged push 实时刷新；后端模式是 loop 级全局态，徽标只做呈现） ---

/** 进会话 / 重连时对齐一次真实模式（失败保持当前值，不炸 UI）。
 *  与 refreshUsage 同款守卫：无活跃会话不发请求（徽标保持 build 默认，
 *  ModeChanged push 对任意 web: 会话兜底刷新）。 */
function syncAgentMode() {
  if (!isDefaultChat.value) return
  const sid = sessionStore.currentId
  if (!sid) return
  request('chat', 'get_mode', { session_id: sid })
    .then((data) => {
      if (sessionStore.currentId !== sid) return
      if (data?.mode === 'plan' || data?.mode === 'build') chatStore.setAgentMode(data.mode)
    })
    .catch(() => {})
}

/** 点击徽标切换模式（await 回包后更新；失败 toast 不翻转）。 */
async function toggleAgentMode() {
  const target = chatStore.agentMode === 'plan' ? 'build' : 'plan'
  try {
    const data = await request('chat', 'set_mode', {
      session_id: sessionStore.currentId || '',
      mode: target,
    })
    if (data?.mode === 'plan' || data?.mode === 'build') chatStore.setAgentMode(data.mode)
  } catch (e) {
    toast.error(String((e as Error)?.message ?? e))
  }
}

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

// --- M1b: 工具卡片「已运行 N 个工具」折叠状态 ---
// key = 组内首个 callId（稳定唯一）；>=3 个默认折叠，点击展开/收起。
const toolGroupExpanded = ref<Record<string, boolean>>({})

function toolGroupCollapsed(evs: ToolEvent[]): boolean {
  const key = evs[0]?.callId ?? ''
  return toolGroupExpanded.value[key] ?? evs.length >= 3
}

function toggleToolGroup(evs: ToolEvent[]) {
  const key = evs[0]?.callId ?? ''
  toolGroupExpanded.value[key] = !toolGroupCollapsed(evs)
}

// --- M5: 会话级 context/cost 常驻条 ---
// context% 来自 chat.context_status（与压缩压力同口径的尾部 token 估算）；
// cost 来自 logs.session_usage（request_logs 按 session_key 聚合）。
// 刷新时机：进会话 / 响应落地（turn 完成）/ 30s 兜底轮询。
const ctxPct = ref<number | null>(null)
const sessCost = ref<number | null>(null)

function refreshUsage() {
  if (!isDefaultChat.value) return
  const sid = sessionStore.currentId
  if (!sid) return
  // 响应可能晚于会话切换——回包时校验还是当前会话（同 TodoPanel 纪律）。
  request('chat', 'context_status', { session_id: sid })
    .then((data) => {
      if (sessionStore.currentId !== sid) return
      ctxPct.value = typeof data?.context?.pct === 'number' ? data.context.pct : null
    })
    .catch(() => {})
  request('logs', 'session_usage', { session_id: sid })
    .then((data) => {
      if (sessionStore.currentId !== sid) return
      sessCost.value = typeof data?.total_cost_usd === 'number' ? data.total_cost_usd : null
    })
    .catch(() => {})
}

const unwatchUsage = watch(
  () => sessionStore.currentId,
  () => {
    ctxPct.value = null
    sessCost.value = null
    refreshUsage()
  },
)

let usagePollTimer: ReturnType<typeof setInterval> | null = null

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

/**
 * E2: 并发回执徽章——识别 AgentLoop 的排队/插话回执（前缀见 loop.rs
 * gate_inbound 的 busy 收据文案；⏳ 还有 /compact /clear 忙时拒绝回执，
 * 语义是"已忽略"不是"已排队"，所以匹配具体前缀而非裸 emoji）。
 * 后端文案改动时徽章退化为不显示，正文永远完整可见——纯前端装饰。
 */
function concurrencyBadge(msg: { role: string; content: string }): { cls: string; label: string } | null {
  if (msg.role !== 'assistant') return null
  if (msg.content.startsWith('⚡ 已接收为紧急插话')) return { cls: 'badge-warning', label: '插话' }
  if (msg.content.startsWith('⏳ 当前正在处理上一条消息')) return { cls: 'badge-info', label: '已排队' }
  if (msg.content.startsWith('⏳ 排队已满')) return { cls: 'badge-error', label: '排队满' }
  return null
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
  // M1b: tool_event push（M1a AgentEvent 通道；无 module 字段的 push 帧）。
  // 按当前会话 chat_id 过滤（`web:{session_id}`，M1a pump 路由键）；
  // 无活跃会话（standalone 单会话）时接受任意 web: 前缀事件。
  if (data.type === 'push' && data.cmd === 'tool_event') {
    const ev = data.data
    const p = ev?.data ?? {}
    const expected = sessionStore.currentId ? `web:${sessionStore.currentId}` : null
    if (expected ? p.chat_id !== expected : !String(p.chat_id ?? '').startsWith('web:')) return
    if (ev?.kind === 'ToolStarted') {
      chatStore.appendToolEvent({
        callId: p.call_id,
        tool: p.tool,
        state: 'running',
        argsPreview: p.args_preview,
      })
    } else if (ev?.kind === 'ToolFinished') {
      chatStore.appendToolEvent({
        callId: p.call_id,
        tool: p.tool,
        state: p.ok ? 'ok' : 'error',
        durationMs: p.duration_ms,
        resultPreview: p.result_preview,
      })
    } else if (ev?.kind === 'ModeChanged') {
      // F1：模式切换事件（/plan /build slash 或 chat.set_mode 发布）——
      // 徽标实时刷新。chat_id 过滤已在上方完成（含无活跃会话的 web: 放行）。
      if (p.mode === 'plan' || p.mode === 'build') chatStore.setAgentMode(p.mode)
    }
    return
  }
  if (data.module !== undefined) {
    const activeModule = props.module ?? 'chat'
    if (data.type === 'message' && data.module === activeModule) {
      if (data.cmd === 'receive') {
        // L6++：帧带 agent 会话 id 时按当前会话过滤——异会话的回复不进当前
        // 视图（后端已持久化，切到该会话时从磁盘加载），否则切走后晚到的
        // 回复会串进当前视图。无该字段 = 旧路径帧，保持接受（legacy 兼容）。
        const frameSid = data.data?.session_id
        if (frameSid && sessionStore.currentId && frameSid !== sessionStore.currentId) return
        const incomingRole = data.data.role || 'assistant'
        const incomingContent = data.data?.content
        // L2：推送帧带会话内单调 seq——更新补拉游标（sync 去重 + 重连续传）。
        if (typeof data.data?.seq === 'number') {
          lastChatSeq = Math.max(lastChatSeq, data.data.seq)
        }
        const last = chatStore.messages[chatStore.messages.length - 1]
        // Skip addMessage if the watchdog already recovered this exact
        // response from session_log (late-arriving live frame, same tail).
        const isDuplicateRecovery =
          !!last &&
          last.role === incomingRole &&
          incomingContent != null &&
          last.content === incomingContent
        if (!isDuplicateRecovery) {
          // M1b：assistant 响应落地时，把本轮累积的工具事件挂载到该消息
          // （收集自 M1a push 通道；flush 即取走，避免误挂下一轮）。
          const toolEvents =
            incomingRole === 'assistant' && chatStore.pendingToolEvents.length
              ? chatStore.flushPendingToolEvents()
              : undefined
          chatStore.addMessage({
            role: incomingRole,
            content: data.data.content,
            timestamp: data.timestamp,
            model: data.data.model,
            toolEvents,
          })
        }
        chatStore.streaming = false
        clearWatchdog()
        // M5：turn 完成（历史已落 store）→ 刷新 context/cost 常驻条。
        if (incomingRole === 'assistant') refreshUsage()

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

// --- L2（devtool-upgrade 阶段 6）：WS chat 帧断线补拉 ---
// 后端为每个 chat 推送帧盖会话内单调 seq（chat_event_log 环形缓冲，窗口 200）。
// 重连成功且历史已载 → chat.sync {after_seq: lastChatSeq} 补齐缺口；
// gap=true（缺口滑出窗口/网关重启 seq 重置）→ reset + 全量重载兜底。
// 只对默认 chat 模块生效（workflow_chat 引擎自管，帧不在补拉通道里）。
let lastChatSeq = 0

// 历史载入后对齐 seq 基线：历史帧（chat_log 路径）不带 seq，不锚基线的话
// 首次重连补拉会从 0 起重放出已载历史。只取最新游标不渲染；本轮已有活帧
//（lastChatSeq>0）或拿不到基线（gap/失败）则保持现状——重连补拉的
// gap→重载兜底链仍然成立。fire-and-forget，失败静默。
async function primeSeqBaseline() {
  if (!isDefaultChat.value || lastChatSeq > 0) return
  const sid = sessionStore.currentId
  if (!sid) return
  try {
    const res = await request('chat', 'sync', { session_id: sid, after_seq: 0 })
    if (!res?.gap && Array.isArray(res?.events) && res.events.length) {
      const tail = res.events[res.events.length - 1]
      if (typeof tail?.seq === 'number') lastChatSeq = Math.max(lastChatSeq, tail.seq)
    }
  } catch { /* 基线拿不到就保持 0 */ }
}

async function syncMissedChat() {
  if (!isDefaultChat.value) return
  const sid = sessionStore.currentId
  if (!sid || chatStore.historyLoading) return
  try {
    const res = await request('chat', 'sync', { session_id: sid, after_seq: lastChatSeq })
    if (res?.gap) {
      chatStore.reset()
      lastChatSeq = 0
      loadHistory()
      return
    }
    let added = false
    for (const ev of res?.events ?? []) {
      // 重放与活帧的赛窗：sync 在途时新帧可能已从 live 通道到达并推进游标
      //——seq ≤ 游标的重放帧跳过，避免双渲染。
      if (typeof ev.seq === 'number' && ev.seq <= lastChatSeq) continue
      chatStore.addMessage({
        role: ev.role,
        content: ev.content,
        timestamp: new Date().toISOString(),
        model: ev.model,
      })
      if (typeof ev.seq === 'number') lastChatSeq = Math.max(lastChatSeq, ev.seq)
      added = true
    }
    if (added) nextTick(() => scrollToBottomIfNear())
  } catch {
    // sync 失败不炸 UI——watchdog / 下轮重连兜底
  }
}

// SSE resync 提示（缺口滑出重放窗口/网关重启）→ 全量重载兜底。
function onSSEResync() {
  if (!isDefaultChat.value) return
  if (!chatStore.historyLoaded || chatStore.historyLoading) return
  chatStore.reset()
  lastChatSeq = 0
  loadHistory()
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

// ---------------------------------------------------------------------------
// M6（devtool-upgrade 阶段 7）：消息级回退——E3 rewind/redo 的首个前端入口。
// message_index = 后端 chat_log jsonl 行号（ChatMessage.rowIndex：历史批次
// 连续推导 + live 递增；error/system 是纯前端渲染不占行）。rewind 截断语义
// 按 turn 边界（后端从目标行之后扫到下一 user 行），checkpoint 锚存在时
// 尽力回滚文件。仅主聊天模块可用——sessions.rewind_to_message 后端固定编
// 址 agent:main:session:{sid}，workflow_chat 等模块会话键不同。
// ---------------------------------------------------------------------------

const rewinding = ref(false)
/** L4：会话分享弹窗开关（工具栏 🔗 按钮）。 */
const showShare = ref(false)

/** rewind/redo 后全量重同步：无条件以 chat_log 为真相源 replace 重建视图，
 *  并用响应 oldest_index 重建行号（与 watchdog 分支的区别：不依赖 streaming
 *  条件——回退发生在非 streaming 态）。 */
let pendingResync = false
function resyncFromLog() {
  pendingResync = true
  sendHistoryRequest('resync_' + Date.now(), 100, null, {
    module: props.module,
    moduleData: activeModuleData(),
  })
}

function rewindToastSummary(resp: any) {
  const parts: string[] = [`撤掉 ${resp?.removed_count ?? 0} 条消息`]
  if (resp?.file_restore === 'applied') {
    parts.push(
      `文件回滚：写 ${resp?.written?.length ?? 0} / 删 ${resp?.deleted?.length ?? 0}`,
    )
  } else if (resp?.file_restore_note) {
    parts.push(String(resp.file_restore_note))
  } else {
    parts.push('文件未动')
  }
  toast.success(`已回退：${parts.join('；')}`)
}

/** 回退到 messageIndex 行之后（行号语义见 rewindActions 各按钮）。 */
async function doRewind(messageIndex: number) {
  if (rewinding.value || !isDefaultChat.value) return
  const sid = sessionStore.currentId
  if (!sid) return
  rewinding.value = true
  try {
    const resp = await request('sessions', 'rewind_to_message', {
      session_id: sid,
      message_index: messageIndex,
    })
    rewindToastSummary(resp)
    resyncFromLog()
  } catch (e: any) {
    toast.error(e?.message || String(e))
  }
  rewinding.value = false
}

/** E3 redo：弹本会话 undo 栈顶反向恢复（栈在网关内存态，空栈后端诚实报错）。 */
async function doRedo() {
  if (rewinding.value || !isDefaultChat.value) return
  const sid = sessionStore.currentId
  if (!sid) return
  rewinding.value = true
  try {
    await request('sessions', 'redo', { session_id: sid })
    toast.success('已重做上一次回退')
    resyncFromLog()
  } catch (e: any) {
    toast.error(e?.message || String(e))
  }
  rewinding.value = false
}

/** 消息气泡的回退动作组（行号不可用=诚实不给入口）。 */
function rewindActions(msg: ChatMessage) {
  if (!isDefaultChat.value) return []
  if (msg.role !== 'user' && msg.role !== 'assistant') return []
  if (msg.rowIndex === undefined) return []
  const acts: { label: string; title: string; run: () => void }[] = []
  if (msg.role === 'user') {
    acts.push({
      label: '↻ 重新生成',
      title: '撤掉这条提问的回复（保留提问）',
      run: () => doRewind(msg.rowIndex!),
    })
    if (msg.rowIndex > 0) {
      acts.push({
        label: '⏪ 撤销此问',
        title: '删除这条提问及其回复（文件改动尽力回滚）',
        run: () => doRewind(msg.rowIndex! - 1),
      })
    }
  } else {
    // assistant 行的前一行是本轮 user 行（chat_log 只存 user/assistant 且
    // 连续）：回退到 user 行 = 重新生成；回退到 user 前一行 = 删除整轮。
    acts.push({
      label: '↻ 重新生成',
      title: '撤掉这条回复（保留提问）',
      run: () => doRewind(msg.rowIndex! - 1),
    })
    if (msg.rowIndex > 1) {
      acts.push({
        label: '⏪ 删除整轮',
        title: '删除本轮提问与回复（文件改动尽力回滚）',
        run: () => doRewind(msg.rowIndex! - 2),
      })
    }
  }
  return acts
}

// M6：命令面板「插入不发送」——草稿追加进输入框并聚焦（消费后清空）。
watch(
  () => chatStore.commandDraft,
  (d) => {
    if (!d) return
    chatStore.input = chatStore.input ? chatStore.input.replace(/\s*$/, '') + ' ' + d : d
    chatStore.commandDraft = ''
    nextTick(() => chatInput.value?.focus())
  },
)

function handleHistoryResponse(data: any) {
  chatStore.historyLoading = false
  if (!data) return

  // M6：rewind/redo 后的重同步（无条件 replace + oldest_index 重建行号）。
  if (pendingResync) {
    pendingResync = false
    const rawMsgs: any[] = data.messages || []
    const oldest = typeof data.oldest_index === 'number' ? data.oldest_index : null
    chatStore.replaceMessages(
      rawMsgs.map((m: any, j: number) => ({
        role: m.role,
        content: m.content,
        timestamp: m.timestamp || new Date().toISOString(),
        model: m.model,
        imageCount: Array.isArray(m.images) ? m.images.length : undefined,
        rowIndex: oldest !== null ? oldest + j : undefined,
      })),
    )
    chatStore.streaming = false
    clearWatchdog()
    nextTick(() => scrollToBottom())
    return
  }

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
          imageCount: Array.isArray(m.images) ? m.images.length : undefined,
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
      imageCount: Array.isArray(m.images) ? m.images.length : undefined,
    }))
    // M6：批次行号连续——oldest_index 传给 store 逐条编号（E3 rewind 定位）。
    chatStore.prependHistory(
      newMessages,
      typeof data.oldest_index === 'number' ? data.oldest_index : null,
    )

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
  // L2：历史落地后对齐补拉基线（只取游标，不渲染；详见 primeSeqBaseline）。
  primeSeqBaseline()

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

// ---------------------------------------------------------------------------
// T8 多模态（2026-09-03）：图片附件（上传端点 /api/upload/image → chat.send
// media）。三种入口：📎 选择文件、粘贴（clipboard files）、拖拽到输入区。
// 上传成功后持 id 等待随消息发送；不做 canvas 压缩，超限由前置校验与后端
// 同一口径拒绝。
// ---------------------------------------------------------------------------

const toast = useToast()
type PendingImage = UploadedImage & { name: string }
const pendingImages = ref<PendingImage[]>([])
const uploadingImages = ref(0)
const imageFileInput = ref<HTMLInputElement | null>(null)
const dragOver = ref(false)
// 前置上限与后端真相源一致：image_path_detector::MAX_IMAGES_PER_MESSAGE = 8
// （D6，2026-09-03 用户定值；此前前端写 4 与后端 8 不一致）。
const MAX_IMAGES_PER_MESSAGE = 8

/** 收文件 → 逐个前置校验 + 异步上传（并发安全：计数器占位防超量）。 */
function addImageFiles(files: FileList | File[] | null) {
  if (!files) return
  for (const f of Array.from(files)) {
    if (pendingImages.value.length + uploadingImages.value >= MAX_IMAGES_PER_MESSAGE) {
      toast.warn(`每条消息最多附带 ${MAX_IMAGES_PER_MESSAGE} 张图片`)
      break
    }
    const invalid = validateImageFile(f)
    if (invalid) {
      toast.warn(invalid)
      continue
    }
    uploadingImages.value++
    uploadImage(f)
      .then(up => {
        pendingImages.value.push({ ...up, name: f.name })
      })
      .catch(e => {
        toast.error(String((e as Error)?.message ?? e))
      })
      .finally(() => {
        uploadingImages.value--
      })
  }
}

function removePendingImage(idx: number) {
  pendingImages.value.splice(idx, 1)
}

function pickImages() {
  imageFileInput.value?.click()
}

function onImageFilesChosen(e: Event) {
  const input = e.target as HTMLInputElement
  addImageFiles(input.files)
  input.value = ''
}

function onPaste(e: ClipboardEvent) {
  // 只在有图片文件时接管；纯文本粘贴走默认行为。
  const files = e.clipboardData?.files
  if (files && files.length > 0) {
    e.preventDefault()
    addImageFiles(files)
    return
  }
  // I4（devtool-upgrade 阶段 4）：超长纯文本粘贴折叠为占位符（原文暂存本地，
  // 发送时还原全文）。短文本不干预。
  const text = e.clipboardData?.getData('text/plain') ?? ''
  if (text.length > PASTE_FOLD_THRESHOLD) {
    e.preventDefault()
    insertPastePlaceholder(text)
  }
}

// --- I4: 超长粘贴折叠 -------------------------------------------------------
// >2000 字符的纯文本粘贴 → 输入区只留占位符 `[Pasted ~N lines #p1]`，原文存
// pastedTexts 映射；chips 行点击展开预览；发送时占位符还原为全文（上行与本
// 地回显都是全量）。纯前端，后端协议不变。

const PASTE_FOLD_THRESHOLD = 2000

/** 占位符文本的识别正则（插入与还原共用同一格式——单一真相源在 pastePlaceholder）。 */
const PASTE_PLACEHOLDER_RE = /\[Pasted ~\d+ (?:lines|chars) #(p\d+)\]/g

/** 占位符 → 原文映射。p 计数器组件生命周期内递增；发送后清空重来。 */
const pastedTexts = ref(new Map<string, string>())
const expandedPastes = ref(new Set<string>())
let pasteSeq = 0

/** 尺寸标签：多行显示行数，单行显示字符数。 */
function pasteSizeLabel(text: string): string {
  const lines = text.split('\n').length
  return lines > 1 ? `~${lines} lines` : `~${text.length} chars`
}

function pastePlaceholder(id: string, text: string): string {
  return `[Pasted ${pasteSizeLabel(text)} #${id}]`
}

/** 当前输入里仍存在的占位符 id（chips 只显示这些——手动删掉占位符，chip 即消失）。 */
const activePasteIds = computed(() => {
  const ids: string[] = []
  for (const m of chatStore.input.matchAll(PASTE_PLACEHOLDER_RE)) {
    if (pastedTexts.value.has(m[1]) && !ids.includes(m[1])) ids.push(m[1])
  }
  return ids
})

/** 占位符 → 原文还原；映射中不存在的 id（用户手打的同形文本）原样保留。 */
function expandPastedPlaceholders(text: string): string {
  return text.replace(PASTE_PLACEHOLDER_RE, (m, id: string) => pastedTexts.value.get(id) ?? m)
}

/** 把占位符插入光标处（替换选区），原文存入映射。 */
function insertPastePlaceholder(text: string) {
  const id = `p${++pasteSeq}`
  pastedTexts.value.set(id, text)
  const placeholder = pastePlaceholder(id, text)
  const el = chatInput.value
  const input = chatStore.input
  const start = Math.min(el?.selectionStart ?? input.length, input.length)
  const end = Math.min(el?.selectionEnd ?? input.length, input.length)
  const from = Math.min(start, end)
  const to = Math.max(start, end)
  chatStore.input = input.slice(0, from) + placeholder + input.slice(to)
  if (el) {
    // 与 handleInput 同款高度自适应（折叠让输入框变矮）。
    el.style.height = 'auto'
    el.style.height = Math.min(el.scrollHeight, 150) + 'px'
    nextTick(() => {
      el.focus()
      const pos = from + placeholder.length
      try {
        el.setSelectionRange(pos, pos)
      } catch {
        /* jsdom 等环境可能不支持——光标位置非关键路径 */
      }
    })
  }
}

function togglePasteExpand(id: string) {
  const next = new Set(expandedPastes.value)
  if (next.has(id)) next.delete(id)
  else next.add(id)
  expandedPastes.value = next
}

function onDragOver(e: DragEvent) {
  e.preventDefault()
  dragOver.value = true
}

function onDragLeave() {
  dragOver.value = false
}

function onDrop(e: DragEvent) {
  e.preventDefault()
  dragOver.value = false
  addImageFiles(e.dataTransfer?.files ?? null)
}

function sendMessage() {
  // I4: 发送前把折叠占位符还原为全文（上行与本地回显都是全量）。
  const content = expandPastedPlaceholders(chatStore.input).trim()
  const media = pendingImages.value.map(p => ({ id: p.id }))
  if (!content && media.length === 0) return
  // LO1（2026-09-04 四轮盲审）：上传未完成时发送被拒——旧行为静默 return，
  // 用户按 Ctrl+Enter 毫无反馈（以为已发出）。toast 明示原因。
  if (uploadingImages.value > 0) {
    toast.warn('图片仍在上传中，请稍候再发送')
    return
  }
  // U7: queue/steer 模式下 busy 发送是合法操作（后端排队/插队）；reject 模式维持原样。
  if (chatStore.streaming && !canQueueWhileBusy.value) return

  chatStore.addMessage({
    role: 'user',
    // 纯图无文字时回显占位（发送内容保持原样，不污染提示词）。
    content: content || (media.length ? '[图片]' : ''),
    timestamp: new Date().toISOString(),
    imageCount: media.length || undefined,
  })

  chatStore.clearInput()
  pendingImages.value = []
  // I4: 粘贴折叠状态一并清空（占位符已全部还原，映射/展开态/计数器重置）。
  pastedTexts.value = new Map()
  expandedPastes.value = new Set()
  pasteSeq = 0
  chatStore.streaming = true
  startWatchdog()

  // Reset textarea height
  if (chatInput.value) chatInput.value.style.height = 'auto'

  // Send with voice_playback flag if playback is enabled
  send(content, voicePlayback.value, {
    module: props.module,
    moduleData: activeModuleData(),
    media,
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
  // @文件引用补全菜单打开时同样接管（与 slash 互斥：'/' 开头 vs '@' 词首）。
  if (handleAtKeydown(e)) return
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

// ---------------------------------------------------------------------------
// @文件引用补全（I2，devtool-upgrade 阶段 3）：输入尾部 `@片段` 时弹 workspace
// 路径列表（后端 fs.complete_path，与 fs_watcher 共用忽略表，≤20 条）。
// 选中把 @token 原位替换为 `@相对路径 `（目录带尾斜杠，续打下一层）；
// #L 行号语法由用户手打（@src/main.rs#L10-20，后端切片）。
// ---------------------------------------------------------------------------

const atItems = ref<string[]>([])
const atIndex = ref(0)
const atTruncated = ref(false)
const atOpen = computed(() => atItems.value.length > 0)
let atDebounce: ReturnType<typeof setTimeout> | null = null

/// 光标前的 `@片段` token（词首 @ 才触发：前一字符是空白/行首——邮箱
/// user@x 不算）。与后端 token 语义同形（非空白非 @ 非反引号）。
/// 未聚焦（含测试环境）视作光标在文本尾。
function currentAtToken(): { prefix: string; start: number } | null {
  const input = chatStore.input
  const el = chatInput.value
  const focused = !!el && document.activeElement === el
  const pos = (focused && el.selectionStart != null) ? el.selectionStart : input.length
  const m = input.slice(0, pos).match(/(^|\s)@([^\s@`]*)$/)
  if (!m) return null
  return { prefix: m[2], start: pos - m[2].length - 1 }
}

watch(() => chatStore.input, () => {
  const tok = currentAtToken()
  if (!tok) {
    atItems.value = []
    return
  }
  if (atDebounce) clearTimeout(atDebounce)
  atDebounce = setTimeout(async () => {
    try {
      const out = await request('fs', 'complete_path', { prefix: tok.prefix })
      atItems.value = (out?.paths as string[] | undefined) ?? []
      atTruncated.value = !!(out?.truncated)
      atIndex.value = 0
    } catch {
      atItems.value = [] // 补全失败静默——不影响输入
    }
  }, 150)
})

function applyAtCompletion(path: string) {
  const tok = currentAtToken()
  if (!tok) {
    atItems.value = []
    return
  }
  const input = chatStore.input
  const end = tok.start + 1 + tok.prefix.length
  chatStore.input = input.slice(0, tok.start) + '@' + path + ' ' + input.slice(end)
  atItems.value = []
  nextTick(() => {
    chatInput.value?.focus()
    const caret = tok.start + 1 + path.length + 1
    chatInput.value?.setSelectionRange(caret, caret)
  })
}

function handleAtKeydown(e: KeyboardEvent): boolean {
  if (!atOpen.value) return false
  if (e.key === 'ArrowDown') {
    e.preventDefault()
    atIndex.value = (atIndex.value + 1) % atItems.value.length
    return true
  }
  if (e.key === 'ArrowUp') {
    e.preventDefault()
    atIndex.value = (atIndex.value - 1 + atItems.value.length) % atItems.value.length
    return true
  }
  if (e.key === 'Enter' || e.key === 'Tab') {
    e.preventDefault()
    const p = atItems.value[atIndex.value]
    if (p) applyAtCompletion(p)
    return true
  }
  if (e.key === 'Escape') {
    atItems.value = []
    return true
  }
  return false
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
  // F-B（2026-09-04 四轮盲审）：standalone 只是自管连接状态展示，不等于
  // 不要历史——旧分支把整个 body 跳过，独立聊天页永远是空会话（仅新消息
  // 可见）。连接建立拉历史 + 断连复位 streaming 两态通用。
  if (props.standalone) {
    if (val === 'connected' && !chatStore.historyLoaded && !chatStore.historyLoading) {
      loadHistory()
    } else if (val === 'connected' && chatStore.historyLoaded) {
      // L2：重连（非首连）→ 断线补拉而非整页重载。
      syncMissedChat()
    }
    if (val === 'disconnected' && chatStore.streaming) {
      chatStore.streaming = false
    }
    return
  }
  appStore.connected = val === 'connected'
  if (val === 'connected' && !chatStore.historyLoaded) {
    loadHistory()
  } else if (val === 'connected' && chatStore.historyLoaded) {
    // L2：重连（非首连）→ 断线补拉而非整页重载。
    syncMissedChat()
  }
  // Reset streaming flag on disconnect to prevent stuck UI
  if (val === 'disconnected' && chatStore.streaming) {
    chatStore.streaming = false
  }
  if (val === 'connected') {
    initVoiceState()
    syncInboxMode()
    syncAgentMode()
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
    lastChatSeq = 0 // L2：换会话 → 补拉游标归零（seq 是会话内单调的）
    if (newId && wsStatus.value === 'connected') {
      loadHistory()
    }
    syncInboxMode()
    syncAgentMode()
  },
)

onMounted(() => {
  onMessage(handleWSMessage)
  setupScrollListener()
  // L2：SSE resync 提示 → 会话全量刷新兜底（缺口滑出重放窗口/网关重启）。
  onSSE('resync', onSSEResync)

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
  }
  // F-B：历史首拉对两种模式一致——standalone 的 connect 由 auth store 在
  // 登录时完成，挂载时可能已 connected（watcher 不回放旧值，这里直接查）。
  if (wsStatus.value === 'connected' && !chatStore.historyLoaded && !chatStore.historyLoading) {
    loadHistory()
  }

  // Initialize voice toolbar state after WS is ready
  if (wsStatus.value === 'connected') {
    initVoiceState()
  }

  // U7: 挂载时拉一次 inbox 模式（失败则保守按 reject 处理）。
  syncInboxMode()

  // F1: 挂载时对齐一次真实模式（徽标初值 build 是保守猜测）。
  syncAgentMode()

  // M5: 常驻条初拉 + 30s 兜底轮询（refreshUsage 内部自带 default-chat 守卫）。
  refreshUsage()
  usagePollTimer = setInterval(refreshUsage, 30000)
})

onUnmounted(() => {
  if (chatMessages.value && scrollHandler) {
    chatMessages.value.removeEventListener('scroll', scrollHandler)
  }
  removeMessageHandler(handleWSMessage)
  offSSE('resync', onSSEResync)
  unwatchStatus()
  unwatchSession()
  unwatchStreaming()
  unwatchUsage()
  if (usagePollTimer) {
    clearInterval(usagePollTimer)
    usagePollTimer = null
  }
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
    <!-- H2: todo 清单面板（默认 chat 模块；todowrite 实时刷新 + 进会话拉取） -->
    <TodoPanel v-if="isDefaultChat" :is-default-chat="isDefaultChat" />

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
            <!-- L6++：项目会话欢迎语 chip（组头同源联结的项目名） -->
            <div v-if="activeProjectName" class="project-chip">⟦{{ activeProjectName }}⟧</div>
            <p>{{ props.titleOverride || '你好！我是 NemesisBot。有什么可以帮助你的吗？' }}</p>
          </div>
        </div>
      </div>

      <div v-for="(msg, idx) in chatStore.messages" :key="idx" class="message" :class="msg.role">
        <div class="message-avatar">{{ getAvatar(msg.role) }}</div>
        <div class="message-content">
          <!-- M1b: 本条消息关联的工具调用卡片（>=3 个默认折叠为计数条）。 -->
          <template v-if="msg.toolEvents && msg.toolEvents.length">
            <button
              v-if="msg.toolEvents.length >= 3"
              class="tool-group-toggle"
              type="button"
              @click="toggleToolGroup(msg.toolEvents)"
            >
              <span class="tool-group-icon">⚒</span>
              已运行 {{ msg.toolEvents.length }} 个工具
              <span class="tool-group-caret">{{ toolGroupCollapsed(msg.toolEvents) ? '▸' : '▾' }}</span>
            </button>
            <div
              v-if="msg.toolEvents.length < 3 || !toolGroupCollapsed(msg.toolEvents)"
              class="tool-cards"
            >
              <ToolCallCard v-for="ev in msg.toolEvents" :key="ev.callId" :event="ev" />
            </div>
          </template>
          <div class="message-bubble">
            <div v-if="msg.role === 'assistant'" class="markdown-body" v-html="getRenderedHtml(msg)"></div>
            <div v-else class="message-text">{{ msg.content }}</div>
            <span v-if="msg.imageCount" class="msg-image-chip" title="消息附带图片">📷 {{ msg.imageCount }}</span>
          </div>
          <div class="message-time">
            <span>{{ formatTime(msg.timestamp) }}</span>
            <span v-if="msg.role === 'assistant' && modelBadge(msg.model)" class="model-badge">{{ modelBadge(msg.model) }}</span>
            <!-- E2: 并发回执徽章（排队/插话），紧跟模型徽章。
                 用 .badge 基类不用 .model-badge——后者源码序靠后会盖掉徽章配色。 -->
            <span v-if="concurrencyBadge(msg)" class="badge concurrency-badge" :class="concurrencyBadge(msg)!.cls">{{ concurrencyBadge(msg)!.label }}</span>
            <!-- M6：消息级回退入口（E3 rewind）。行号不可用=诚实不给入口。 -->
            <span v-if="rewindActions(msg).length" class="msg-actions">
              <button
                v-for="a in rewindActions(msg)"
                :key="a.label"
                class="msg-action-btn"
                :title="a.title"
                :disabled="rewinding"
                @click="a.run()"
              >
                {{ a.label }}
              </button>
            </span>
          </div>
        </div>
      </div>

      <!-- Typing indicator -->
      <div v-if="chatStore.streaming" class="message assistant">
        <div class="message-avatar">NB</div>
        <div class="message-content">
          <!-- M1b: 进行中轮次的工具卡片（响应落地前实时可见）。 -->
          <template v-if="chatStore.pendingToolEvents.length">
            <button
              v-if="chatStore.pendingToolEvents.length >= 3"
              class="tool-group-toggle"
              type="button"
              @click="toggleToolGroup(chatStore.pendingToolEvents)"
            >
              <span class="tool-group-icon">⚒</span>
              已运行 {{ chatStore.pendingToolEvents.length }} 个工具
              <span class="tool-group-caret">{{ toolGroupCollapsed(chatStore.pendingToolEvents) ? '▸' : '▾' }}</span>
            </button>
            <div
              v-if="chatStore.pendingToolEvents.length < 3 || !toolGroupCollapsed(chatStore.pendingToolEvents)"
              class="tool-cards"
            >
              <ToolCallCard v-for="ev in chatStore.pendingToolEvents" :key="ev.callId" :event="ev" />
            </div>
          </template>
          <div class="message-bubble">
            <div class="typing-indicator"><span></span><span></span><span></span></div>
          </div>
        </div>
      </div>
    </div>

    <!-- Toolbar -->
    <div v-if="!toolbarCollapsed" class="voice-toolbar">
      <button
        v-if="isDefaultChat"
        class="voice-btn mode-btn"
        :class="{ 'mode-plan': chatStore.agentMode === 'plan' }"
        :title="chatStore.agentMode === 'plan'
          ? '计划模式：文件修改类工具已停用（plans/ 目录写入放行）。点击切回构建模式（/build）'
          : '构建模式：全量工具。点击切换到计划模式（/plan），AI 只读代码出计划'"
        @click="toggleAgentMode"
      >
        <span class="mode-mark">{{ chatStore.agentMode === 'plan' ? '📋' : '🛠' }}</span>
        {{ chatStore.agentMode === 'plan' ? '计划' : '构建' }}
      </button>
      <button
        v-if="isDefaultChat"
        class="voice-btn redo-btn"
        title="重做上一次回退（恢复被撤销的消息，文件尽力前向恢复；undo 栈在网关内存，重启即清）"
        :disabled="rewinding"
        @click="doRedo"
      >
        ↪ 重做
      </button>
      <button
        v-if="isDefaultChat"
        class="voice-btn share-btn"
        title="分享本会话（只读链接，token 即凭据，可随时撤销）"
        :disabled="rewinding"
        @click="showShare = true"
      >
        🔗 分享
      </button>
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
    <!-- F1: 计划模式常驻条（工具栏可折叠，安全相关状态需要始终可见） -->
    <div v-if="isDefaultChat && chatStore.agentMode === 'plan'" class="plan-strip">
      📋 计划模式：文件修改类工具已停用（plans/ 目录写入放行）— 点击上方徽标或发送 /build 切回
    </div>
    <div v-if="showSteerHint" class="steer-hint">
      ⚡ 将以插队（steer）模式发送，立即送达当前轮
    </div>

    <!-- M5: 会话级 context/cost 常驻条（与压缩压力同口径；turn 完成即刷 + 30s 兜底） -->
    <div v-if="isDefaultChat && ctxPct !== null" class="usage-strip">
      <span class="usage-ctx" :class="{ hot: ctxPct >= 80 }">{{ ctxPct }}% context</span>
      <template v-if="sessCost !== null && sessCost > 0"> · <span class="usage-cost">{{ fmtCost(sessCost) }}</span></template>
    </div>

    <!-- Input -->
    <div
      class="chat-input-area"
      :class="{ 'drag-over': dragOver }"
      @dragover="onDragOver"
      @dragleave="onDragLeave"
      @drop="onDrop"
    >
      <!-- T8 多模态：待发送图片附件 chips -->
      <div v-if="pendingImages.length || uploadingImages > 0" class="attach-chips">
        <span v-for="(p, i) in pendingImages" :key="p.id" class="attach-chip" :title="p.path">
          🖼 {{ p.name }}
          <button class="attach-chip-x" title="移除" @click="removePendingImage(i)">×</button>
        </span>
        <span v-if="uploadingImages > 0" class="attach-chip uploading">
          <span class="spinner" style="width:12px;height:12px;border-width:2px;"></span>
          上传中…
        </span>
      </div>
      <!-- I4: 折叠粘贴 chips（点击展开预览原文；发送时占位符还原为全文） -->
      <div v-if="activePasteIds.length" class="paste-chips">
        <span
          v-for="id in activePasteIds"
          :key="id"
          class="paste-chip"
          :class="{ open: expandedPastes.has(id) }"
          :title="expandedPastes.has(id) ? '点击收起' : '点击展开查看粘贴内容'"
          @click="togglePasteExpand(id)"
        >
          📋 Pasted {{ pasteSizeLabel(pastedTexts.get(id) ?? '') }} #{{ id }}
        </span>
      </div>
      <template v-for="id in activePasteIds" :key="'pv-' + id">
        <pre v-if="expandedPastes.has(id)" class="paste-preview">{{ pastedTexts.get(id) }}</pre>
      </template>
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
      <!-- @文件引用补全菜单（I2）：复用 slash-menu 样式 -->
      <div v-if="atOpen" class="slash-menu">
        <div
          v-for="(p, i) in atItems"
          :key="p"
          class="slash-item"
          :class="{ active: i === atIndex }"
          @mousedown.prevent="applyAtCompletion(p)"
          @mouseenter="atIndex = i"
        >
          <span class="slash-item-name">{{ p }}</span>
          <span v-if="atTruncated && i === atItems.length - 1" class="slash-item-hint">更多未列出…</span>
        </div>
      </div>
      <button
        class="attach-btn"
        title="附带图片（也可粘贴或拖拽进来）"
        :disabled="chatStore.streaming && !canQueueWhileBusy"
        @click="pickImages"
      >
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" width="18" height="18">
          <path d="m21.44 11.05-9.19 9.19a6 6 0 0 1-8.49-8.49l8.57-8.57A4 4 0 1 1 18 8.84l-8.59 8.57a2 2 0 0 1-2.83-2.83l8.49-8.48"/>
        </svg>
      </button>
      <input
        ref="imageFileInput"
        type="file"
        accept="image/png,image/jpeg,image/webp,image/gif"
        multiple
        hidden
        @change="onImageFilesChosen"
      >
      <textarea
        ref="chatInput"
        :placeholder="props.placeholderOverride || '输入消息... (Ctrl+Enter 发送)'"
        rows="1"
        v-model="chatStore.input"
        @keydown="handleKeydown"
        @input="handleInput"
        @paste="onPaste"
        :disabled="chatStore.streaming && !canQueueWhileBusy"
      ></textarea>
      <button v-if="chatStore.streaming && showStopButton" class="btn btn-stop" @click="stopGeneration" title="停止生成">
        <svg viewBox="0 0 24 24" fill="currentColor" width="16" height="16">
          <rect x="6" y="6" width="12" height="12" rx="2"/>
        </svg>
      </button>
      <button v-if="!chatStore.streaming || canQueueWhileBusy" class="btn btn-primary" @click="sendMessage" :disabled="(!chatStore.input.trim() && !pendingImages.length) || uploadingImages > 0">
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
    <!-- L4 会话分享弹窗 -->
    <ShareModal
      v-if="showShare && sessionStore.currentId"
      :session-id="sessionStore.currentId"
      @close="showShare = false"
    />
  </div>
</template>

<style scoped>
/* L6++：项目 chip（欢迎语上方；弱化徽标，不做高亮以免与消息气泡争焦点） */
.project-chip {
  display: inline-block;
  font-size: var(--text-xs);
  color: var(--text-secondary);
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: 10px;
  padding: 1px 8px;
  margin-bottom: 6px;
}
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
/* F1: plan/build 模式徽标 + 计划模式常驻条 */
.mode-btn.mode-plan {
  color: #e6a23c;
  border-color: #e6a23c;
}
.plan-strip {
  padding: 4px 12px;
  font-size: var(--text-xs);
  color: #e6a23c;
  background: var(--surface);
  border-top: 1px solid var(--border);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
/* M5: 会话级 context/cost 常驻条 */
.usage-strip {
  padding: 3px 12px;
  font-size: var(--text-xs);
  color: var(--text-muted);
  background: var(--surface);
  border-top: 1px solid var(--border);
  text-align: center;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.usage-strip .usage-ctx.hot {
  color: #e6a23c;
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

/* T8 多模态：拖拽高亮 + 附件 chips + 📎 按钮 */
.chat-input-area.drag-over {
  outline: 2px dashed var(--accent);
  outline-offset: -2px;
}
.attach-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  align-self: flex-end;
  padding: 0 10px;
  height: 38px;
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
  background: var(--bg-primary);
  color: var(--text-secondary);
  cursor: pointer;
  transition: all 0.15s;
  flex-shrink: 0;
}
.attach-btn:hover:not(:disabled) {
  border-color: var(--accent);
  color: var(--accent);
}
.attach-btn:disabled {
  opacity: 0.4;
  cursor: not-allowed;
}
.attach-chips {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  padding: 6px 12px 0;
}
.attach-chip {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  max-width: 220px;
  padding: 3px 8px;
  font-size: var(--text-xs);
  border: 1px solid var(--border);
  border-radius: 999px;
  background: var(--surface);
  color: var(--text-secondary);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.attach-chip.uploading {
  color: var(--text-muted);
}
.attach-chip-x {
  border: none;
  background: none;
  color: var(--text-muted);
  font-size: 14px;
  line-height: 1;
  cursor: pointer;
  padding: 0 2px;
}
.attach-chip-x:hover {
  color: #dc3545;
}
/* I4: 折叠粘贴 chips + 展开预览 */
.paste-chips {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  padding: 6px 12px 0;
}
.paste-chip {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 3px 8px;
  font-size: var(--text-xs);
  border: 1px solid var(--border);
  border-radius: 999px;
  background: var(--surface);
  color: var(--text-secondary);
  cursor: pointer;
  white-space: nowrap;
}
.paste-chip:hover,
.paste-chip.open {
  color: var(--text);
  border-color: var(--text-muted);
}
.paste-preview {
  margin: 6px 12px 0;
  padding: 8px 10px;
  max-height: 200px;
  overflow: auto;
  font-size: var(--text-xs);
  line-height: 1.5;
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: 6px;
  white-space: pre-wrap;
  word-break: break-word;
  color: var(--text-secondary);
}
.msg-image-chip {
  display: inline-block;
  margin-top: 4px;
  padding: 1px 8px;
  font-size: var(--text-xs);
  border-radius: 999px;
  background: rgba(255, 255, 255, 0.12);
  color: inherit;
  opacity: 0.85;
}

/* E2: 并发回执徽章（时间行内，弱化不抢内容） */
.concurrency-badge {
  padding: 0 8px;
  font-size: var(--text-xs);
  line-height: 1.5;
  opacity: 0.9;
}

/* M6: 消息级回退入口（时间行内，hover 显现不抢视觉） */
.msg-actions {
  display: inline-flex;
  gap: 6px;
  margin-left: 8px;
  opacity: 0;
  transition: opacity 0.12s ease;
}
.message:hover .msg-actions {
  opacity: 1;
}
.msg-action-btn {
  padding: 0 8px;
  font-size: var(--text-xs);
  line-height: 1.6;
  color: var(--text-muted);
  background: transparent;
  border: 1px solid var(--border);
  border-radius: 999px;
  cursor: pointer;
}
.msg-action-btn:hover:not(:disabled) {
  color: var(--text-primary, inherit);
  background: var(--bg-elev, rgba(128, 128, 128, 0.1));
}
.msg-action-btn:disabled {
  opacity: 0.5;
  cursor: wait;
}

/* M1b: 工具调用卡片组（消息上方 / typing 指示上方） */
.tool-cards {
  display: flex;
  flex-direction: column;
  gap: 4px;
  margin-bottom: 6px;
}
.tool-group-toggle {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  align-self: flex-start;
  margin-bottom: 6px;
  padding: 3px 10px;
  border: 1px solid var(--border);
  border-radius: 999px;
  background: var(--bg-elev, rgba(128, 128, 128, 0.06));
  color: var(--text-muted);
  font-size: var(--text-xs, 12px);
  cursor: pointer;
}
.tool-group-toggle:hover {
  color: var(--text);
  border-color: var(--accent);
}
.tool-group-icon {
  font-size: 11px;
}
.tool-group-caret {
  font-size: 10px;
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
