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
import { useApprovals } from '../composables/useApprovals'
import { useEditorMode } from '../composables/useEditorMode'
// 皮肤骨架槽位（主页启动器）：skinState.id 非空才渲染
import { skinState } from '../composables/useSkin'
// H2 (2026-09-05): todo 清单面板（todowrite 工具的实时渲染）。
import TodoPanel from './chat/TodoPanel.vue'

import ShareModal from './ShareModal.vue'
// M1b (2026-09-05): 工具调用卡片（M1a AgentEvent 通道的实时渲染）。
import ToolCallCard from './chat/ToolCallCard.vue'
import type { ToolEvent } from '../stores/chat'
// M5 (2026-09-05): 会话级 context/cost 常驻条——cost 格式化与侧栏共用。
import { fmtCost } from '../composables/useUsageFormat'
import { renderMarkdownHtml } from '../utils/markdown'
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
  /** D-3（2026-09-23 多会话并行清账）：嵌入宿主钉死的会话 id（工作流
   *  「对话生成」等）。传入后本面板的所有收发/轮询/占用态都锚定该会话，
   *  **绝不跟随也不抢占全局选中**（currentId）；缺省 = 跟随全局（旧行为）。 */
  sessionId?: string
}>()

const chatStore = useChatStore()
const appStore = useAppStore()
const auth = useAuthStore()
const { request } = useWSAPI()
const sessionStore = useSessionStore()

// Multi-session: in the default chat module, attach the active conversation
// id so the backend routes to `agent:main:session:{sid}` (server.rs/loop.rs).
const isDefaultChat = computed(() => (props.module ?? 'chat') === 'chat')

// 皮肤骨架槽位 3/3：主页启动器（空会话时品牌 + 场景标签）。皮肤未激活
// （skinState.id 空）或非默认聊天模块时零渲染——默认观感零变化。
const launcherMode = computed(
  () =>
    !!skinState.id &&
    isDefaultChat.value &&
    chatStore.messages.length === 0 &&
    !chatStore.historyLoading &&
    !historyLoadFailed.value
)

/** 场景标签点击 → 预填输入（场景名前缀，用户补全具体诉求）并聚焦。 */
function applyScene(scene: string) {
  chatStore.input = `${scene}：`
  nextTick(() => chatInput.value?.focus())
}

// D-3：本面板实际显示的会话 id——全部会话寻址（收发、历史、补拉、占位轮询、
// inbox/usage 查询、占用表）的唯一入口。旧代码散落 ~40 处直引
// `sessionStore.currentId`，嵌入面板（agent-gen）因此被迫抢写全局选中——
// 跨会话串扰的直接根源（BUG 2026-09-23_workflow-agentgen-session-crosstalk）。
const effectiveSid = computed(() => props.sessionId ?? sessionStore.currentId)

// 本会话的发送占用态：busy 表按会话隔离（chat.busyBySid），旧的全局布尔
// 曾让异会话 turn 的 busy 泄漏进本视图（发送被静默吞掉 / 假占位）。
const streaming = computed(() => chatStore.isBusy(effectiveSid.value))

function activeModuleData(): Record<string, unknown> {
  const md: Record<string, unknown> = { ...(props.moduleData ?? {}) }
  if (isDefaultChat.value && effectiveSid.value) {
    md.session_id = effectiveSid.value
  }
  return md
}

// L6++（2026-09-08）：项目 chip——当前会话归属项目时，欢迎语上方显示
// 「⟦项目名⟧」（注册表联结显示名；已移除/未知 pid 不显示，不误导）。
const activeProjectName = computed(() => {
  const pid = sessionStore.sessions.find(s => s.id === effectiveSid.value)?.projectId
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
  void refreshInbox(effectiveSid.value || '')
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
  const sid = effectiveSid.value
  if (!sid) return
  request('chat', 'get_mode', { session_id: sid })
    .then((data) => {
      if (effectiveSid.value !== sid) return
      if (data?.mode === 'plan' || data?.mode === 'build') chatStore.setAgentMode(data.mode)
    })
    .catch(() => {})
}

/** 点击徽标切换模式（await 回包后更新；失败 toast 不翻转）。 */
async function toggleAgentMode() {
  const target = chatStore.agentMode === 'plan' ? 'build' : 'plan'
  try {
    const data = await request('chat', 'set_mode', {
      session_id: effectiveSid.value || '',
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

// --- Full Access 编辑器放行开关（2026-09-20 用户裁决）---
// 模块级单例（useEditorMode）：聊天框旁按钮与设置页【编辑器】TAB 同源。
// 点击语义：关 FA 必须随关 ext（服务端联动会把 (false, true) 折回
// full=true，前端显式双关才能真关掉）；开 ext 时确保 full 开。

const { fullAccess, externalWrite, editorAvailable, setEditorAccess } = useEditorMode()

async function toggleFullAccess() {
  if (fullAccess.value) {
    await setEditorAccess(false, false)
  } else {
    await setEditorAccess(true, externalWrite.value)
  }
}

async function toggleExternalWrite() {
  // ext 依赖 full：按钮仅在 fullAccess 时可点（disabled），此处仍显式带 full。
  await setEditorAccess(true, !externalWrite.value)
}

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
  const sid = effectiveSid.value
  if (!sid) return
  // 响应可能晚于会话切换——回包时校验还是当前会话（同 TodoPanel 纪律）。
  request('chat', 'context_status', { session_id: sid })
    .then((data) => {
      if (effectiveSid.value !== sid) return
      ctxPct.value = typeof data?.context?.pct === 'number' ? data.context.pct : null
    })
    .catch(() => {})
  request('logs', 'session_usage', { session_id: sid })
    .then((data) => {
      if (effectiveSid.value !== sid) return
      sessCost.value = typeof data?.total_cost_usd === 'number' ? data.total_cost_usd : null
    })
    .catch(() => {})
}

const unwatchUsage = watch(
  effectiveSid,
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

function renderMarkdown(text: string): string {
  // 代码高亮不在此处：marked v15 已移除 highlight 选项（传了也是静默
  // no-op），统一由 renderCodeBlocks() 在 DOM 插入后对 pre code 跑
  // hljs.highlightElement。
  try {
    return renderMarkdownHtml(text, { breaks: true })
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

// P1（2026-09-21）：tool_event 载荷 → store 状态的唯一转换。实时 push
// （handleWSMessage）与 chat.sync 回放（syncMissedChat / primeSeqBaseline）
// 共用——回放端语义与实时端完全一致，修「补拉/回放路径工具事件丢失」
// 时不出现第二份转换逻辑漂移。appendToolEvent 按 callId upsert 幂等。
function applyToolEventPayload(ev: any) {
  const p = ev?.data ?? {}
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
    // 徽标实时刷新。会话过滤由调用方完成。
    if (p.mode === 'plan' || p.mode === 'build') chatStore.setAgentMode(p.mode)
  } else if (ev?.kind === 'RoundText') {
    // R1（2026-09-21）：中间轮正文（模型每轮过程叙述）——执行中 pending
    // 区展开显示，最终回复落地时 flush 折叠挂载到该消息。
    if (typeof p.content === 'string' && p.content) chatStore.appendRoundText(p.content)
  }
}

function handleWSMessage(data: any) {
  // M1b: tool_event push（M1a AgentEvent 通道；无 module 字段的 push 帧）。
  // 按当前会话过滤。2026-09-20 BUG-A：帧内层 chat_id 是连接级 id
  // （`web:{连接id}`，session.rs 连接建立时派生），与前端会话 id 不同域，
  // 恒不等 → 全部实时帧被丢弃（工具卡/任务清单/模式徽标零反应）。现
  // 优先用 web pump 注入的 `session_id`（会话 id，session_key 末段，与
  // currentId 同域）；旧帧无该字段时回退旧 chat_id 逻辑（兼容）。
  if (data.type === 'push' && data.cmd === 'tool_event') {
    const ev = data.data
    const p = ev?.data ?? {}
    if (typeof p.session_id === 'string' && p.session_id.length > 0) {
      // 无会话锚（effectiveSid 空 = standalone/未选中/降级 legacy 路径）时
      // 保持接受——与下方 receive 分支的 legacy 语义对齐；否则新建会话的
      // 实时工具帧会因「空锚恒不等」全量丢弃（2026-09-26 BUG 第二形态，
      // 与 2026-09-20 BUG-A 的 chat_id 域不等同构）。
      if (effectiveSid.value && p.session_id !== effectiveSid.value) return
    } else {
      const expected = effectiveSid.value ? `web:${effectiveSid.value}` : null
      if (expected ? p.chat_id !== expected : !String(p.chat_id ?? '').startsWith('web:')) return
    }
    applyToolEventPayload(ev)
    // P8 精修：tool 帧带环内 seq（pump 注入）——与 chat 行统一推进补拉
    // 游标。否则本端实时收到工具事件而游标不动，chat.activity（seq 更大）
    // 会把本端误判成「落后端」触发无谓的全量刷新。
    if (typeof ev?.seq === 'number') lastChatSeq = Math.max(lastChatSeq, ev.seq)
    return
  }
  if (data.module !== undefined) {
    const activeModule = props.module ?? 'chat'
    if (data.type === 'message' && data.module === activeModule) {
      if (data.cmd === 'receive') {
        // L6++：帧带 agent 会话 id 时按本面板会话过滤——异会话的回复不进当前
        // 视图（后端已持久化，切到该会话时从磁盘加载），否则切走后晚到的
        // 回复会串进当前视图。无该字段 = 旧路径帧，保持接受（legacy 兼容）。
        const frameSid = data.data?.session_id
        if (frameSid && effectiveSid.value && frameSid !== effectiveSid.value) {
          // D-3：异会话帧先落账再过滤——busy 表按会话隔离后，异会话回复的
          // 完成语义必须照常清算（该会话的占用态 + B2 在飞登记），否则嵌入
          // 面板卸载窗口/无视图归属的会话 busy 永挂。assistant 帧才是完成
          // 信号（user 回声帧不是——与下方完成语义同口径）。
          if (data.data.role === 'assistant') {
            chatStore.setBusy(frameSid, false)
            chatStore.clearInflightTurn(frameSid)
          }
          return
        }
        // 无 session_id 帧（旧路径/legacy 广播）或本面板无会话锚
        // （standalone/未选中）→ 保持接受（legacy 兼容语义）。
        // SB（2026-09-17）兜底：帧的会话不在列表中（session.created 事件
        // 丢失 / B 端路径无事件 / SSE 断窗）→ force 刷新列表（同 sid 3s
        // 节流）。事件丢失时这是列表可见性的第二道闸。
        if (frameSid && !sessionStore.sessions.some(s => s.id === frameSid)) {
          maybeRefreshSessionsFor(frameSid)
        }
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
          // R1：中间轮正文同轮收尾——折叠挂载（roundTextsOpen 缺省收起）。
          const toolEvents =
            incomingRole === 'assistant' && chatStore.pendingToolEvents.length
              ? chatStore.flushPendingToolEvents()
              : undefined
          const roundTexts =
            incomingRole === 'assistant' && chatStore.pendingRoundTexts.length
              ? chatStore.flushPendingRoundTexts()
              : undefined
          chatStore.addMessage({
            role: incomingRole,
            content: data.data.content,
            timestamp: data.timestamp,
            model: data.data.model,
            // 集群续行归属（2026-09-23）：worker 节点名 →「节点 X」徽章。
            sourceNode: data.data.source_node,
            // A1：环 seq 随消息存档——历史响应 last_seq 到达后据此剔除
            // 「先于快照渲染」的重复 assistant 帧。
            seq: typeof data.data?.seq === 'number' ? data.data.seq : undefined,
            toolEvents,
            roundTexts,
          })
        }
        // P8 补全（2026-09-21）：user 回声帧（后端入环回推，多端一致）不是
        // 完成信号——不关 streaming、不清 watchdog（本地发送态保持到
        // assistant 回复到场）；完成语义只属于 assistant 帧。
        if (incomingRole === 'assistant') {
          chatStore.setBusy(effectiveSid.value, false)
          // B2：本会话回复到场——在飞登记完成使命。
          chatStore.clearInflightTurn(effectiveSid.value)
          clearWatchdog()
          // M5：turn 完成（历史已落 store）→ 刷新 context/cost 常驻条。
          refreshUsage()
          // P2（2026-09-21）：回复已显示 → 立即 detect 停轮清占位。此前
          // receive 分支不调 detectPendingTurn，且上方已把补拉游标推进到
          // 回复自身 seq（此后 sync 恒 n=0）——「sync 拉到新行才停轮」
          // 路径被 receive 自己堵死，占位只能等 busy=false 兜底节奏消失
          //（实测残留 4.6-6.8s）。assistant 到场即 detect：尾部已是回复
          // → 停轮，占位即时消失。
          detectPendingTurn()
        }

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
        chatStore.setBusy(effectiveSid.value, false)
        // B2：错误帧同样终结在飞轮次（后端拒绝/装配失败等）。
        chatStore.clearInflightTurn(effectiveSid.value)
        clearWatchdog()
      }
    } else if (data.type === 'system' && data.module === 'error' && data.cmd === 'notify') {
      chatStore.addMessage({
        role: 'error',
        content: data.data.content || data.data,
        timestamp: data.timestamp,
      })
      chatStore.setBusy(effectiveSid.value, false)
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

// SB（2026-09-17）兜底：receive 帧的会话不在列表 → force 刷新（同 sid 3s
// 节流，防风暴）。这是 session.created SSE 事件丢失时的第二道闸。
const sessionRefreshAt = new Map<string, number>()
function maybeRefreshSessionsFor(sid: string) {
  const now = Date.now()
  const last = sessionRefreshAt.get(sid) ?? 0
  if (now - last < 3000) return
  sessionRefreshAt.set(sid, now)
  void sessionStore.fetchList(true)
}

// 历史载入后对齐 seq 基线 + 回放尾部工具流程：
// 1) 基线（原 L2 逻辑）：历史帧（chat_log 路径）不带 seq，不锚基线的话
//    首次重连补拉会从 0 起重放出已载历史。只取最新游标不渲染；本轮已有
//    活帧（lastChatSeq>0）或拿不到基线（gap/失败）则保持现状——重连补拉
//    的 gap→重载兜底链仍然成立。fire-and-forget，失败静默。
// 2) P1c（2026-09-21）：chat_log 只有 user/assistant 文本行，工具事件只
//    存在于 chat_event_log——reset+loadHistory 全量重载（SSE resync /
//    watchdog / 换会话 / 重挂载补偿）后工具卡全部消失（用户实测「中间的
//    流程本来也该在，但是都没了」）。此处按「倒数第二条 assistant 行之后」
//    回放 tool 条目：覆盖最后一轮完成段（挂回历史 assistant 消息）+ 进行
//    中段（留 pendingToolEvents 区，占位旁渲染，收尾由 receive/sync flush）。
//    更早轮次的工具卡不重建（历史窗口有限，重放成本随深度膨胀；最近一轮
//    + 进行中是「中间流程」的主要可见诉求）。
// 3) 2026-09-21 watchdog 重灌复用：全量重灌分支（replaceMessages）同样
//    只有文本行——提炼 replayToolsFromRing 供 primeSeqBaseline（挂载基线）
//    与 watchdog 重灌后共用,重灌后最后一轮工具卡不再丢失。

/** 从环回放最近一轮工具事件并挂卡（F5/重灌后「中间流程」恢复的共用主体）。
 * 前提:视图已由 chat_log 历史渲染(user/assistant 文本行);本函数只负责
 * tool 条目→挂载。fire-and-forget,失败静默。 */
async function replayToolsFromRing() {
  const sid = effectiveSid.value
  if (!sid) return
  const res = await request('chat', 'sync', { session_id: sid, after_seq: 0 })
  // 会话围栏（2026-09-24）：回放在飞期间切走 → 旧会话的工具卡/游标推进
  // 不得落到新会话视图（同 syncMissedChat 纪律）。
  if (effectiveSid.value !== sid) return
  if (!res?.gap && Array.isArray(res?.events) && res.events.length) {
    const events = res.events
    const tail = events[events.length - 1]
    if (typeof tail?.seq === 'number') lastChatSeq = Math.max(lastChatSeq, tail.seq)
    // 回放窗口起点：倒数第二条 assistant 行（含）——其后的第一个
    // assistant 是最后一轮回复，其前的 tool 属于它；其后剩余 tool 属于
    // 进行中轮次。assistant 不足两条则从 0 起全回放（短历史无损）。
    const aPositions = events
      .map((e: any, i: number) => (e.kind !== 'tool' && e.role === 'assistant' ? i : -1))
      .filter((i: number) => i >= 0)
    const from = aPositions.length >= 2 ? aPositions[aPositions.length - 2] : 0
    const replayAssistants = aPositions.filter((i: number) => i >= from)
    let seen = 0
    for (let i = from; i < events.length; i++) {
      const ev = events[i]
      if (ev.kind === 'tool' && ev.tool) {
        applyToolEventPayload(ev.tool)
        continue
      }
      if (ev.role === 'assistant') {
        // 该 assistant 行已由 chat_log 历史渲染——把此前累积的工具事件
        // 挂回列表中对应位置的 assistant（回放段第 j 个 assistant ↔
        // 列表倒数第 k-j 个）；列表 assistant 数不足（窗口滑出）时兜底
        // 挂到最后一条。R1：中间轮正文同位挂载（重灌后折叠块不丢）。
        if (chatStore.pendingToolEvents.length || chatStore.pendingRoundTexts.length) {
          const assistants = chatStore.messages.filter((m: any) => m.role === 'assistant')
          const k = replayAssistants.length
          const target =
            assistants[assistants.length - (k - seen)] ?? assistants[assistants.length - 1]
          if (target) {
            if (chatStore.pendingToolEvents.length) {
              target.toolEvents = chatStore.flushPendingToolEvents()
            }
            if (chatStore.pendingRoundTexts.length) {
              target.roundTexts = chatStore.flushPendingRoundTexts()
            }
          }
        }
        seen++
      }
    }
  }
}

async function primeSeqBaseline() {
  if (!isDefaultChat.value || lastChatSeq > 0) return
  try {
    await replayToolsFromRing()
  } catch { /* 基线拿不到就保持 0 */ }
}

async function syncMissedChat() {
  if (!isDefaultChat.value) return
  const sid = effectiveSid.value
  if (!sid || chatStore.historyLoading) return
  try {
    const res = await request('chat', 'sync', { session_id: sid, after_seq: lastChatSeq })
    // 会话围栏（2026-09-24，与 refreshUsage/syncAgentMode 同纪律）：在飞期间
    // 切走 → 本响应属于旧会话，整包丢弃。切换链已自行 reset+重拉；不拦的
    // 话旧会话事件会追加进新会话视图，还会把旧会话 seq 推进补拉游标。
    if (effectiveSid.value !== sid) return
    if (res?.gap) {
      // 旧在飞登记随 reset 作废——否则其迟到 timeout 误判新请求。
      inFlightHistory.clear()
      chatStore.reset()
      lastChatSeq = 0
      loadHistory()
      return
    }
    let added = false
    let gotAssistant = false
    for (const ev of res?.events ?? []) {
      // 重放与活帧的赛窗：sync 在途时新帧可能已从 live 通道到达并推进游标
      //——seq ≤ 游标的重放帧跳过，避免双渲染。
      if (typeof ev.seq === 'number' && ev.seq <= lastChatSeq) continue
      if (ev.kind === 'tool' && ev.tool) {
        // P1a（2026-09-21）：补拉的 tool 条目转回工具事件（与实时帧同一
        // 转换）——断线重连窗口内的工具流程不再丢：ToolStarted 落补拉
        // 窗口时后续 assistant 行到场所 flush 挂载；assistant 已过、轮次
        // 进行中则在 pendingToolEvents 区渲染（占位旁）。
        applyToolEventPayload(ev.tool)
        if (typeof ev.seq === 'number') lastChatSeq = Math.max(lastChatSeq, ev.seq)
        added = true
        continue
      }
      // 尾行同文比对（与 receive 分支 isDuplicateRecovery 同语义）：发送方
      // 极端时序下自己的 user 行可能已由回声帧先落视图，补拉不重复渲染。
      const tail = chatStore.messages[chatStore.messages.length - 1]
      if (ev.role === 'user' && tail && tail.role === 'user' && tail.content === ev.content) {
        if (typeof ev.seq === 'number') lastChatSeq = Math.max(lastChatSeq, ev.seq)
        continue
      }
      // P1a：assistant 行落地时挂载本轮累积的工具事件（与 receive 分支
      // 同语义；此前补拉路径 addMessage 无 flush——重连补进的回复永远
      // 裸奔，内存 pending 工具事件挂不上）。R1：中间轮正文同轮收尾。
      const toolEvents =
        ev.role === 'assistant' && chatStore.pendingToolEvents.length
          ? chatStore.flushPendingToolEvents()
          : undefined
      const roundTexts =
        ev.role === 'assistant' && chatStore.pendingRoundTexts.length
          ? chatStore.flushPendingRoundTexts()
          : undefined
      chatStore.addMessage({
        role: ev.role,
        content: ev.content,
        // HD（2026-09-17）：优先用后端记录时刻——重放载荷此前无时间戳，
        // 补拉消息显示为拉取时刻而非真实发生时刻；旧条目无 ts 回退本地钟。
        timestamp: ev.ts || new Date().toISOString(),
        model: ev.model,
        // 集群续行归属：环帧同样带节点名（send_to_session 落环时已写键）。
        sourceNode: ev.source_node,
        // A1：环 seq 存档（与 receive 分支同字段——历史快照到货后剔除用）。
        seq: typeof ev.seq === 'number' ? ev.seq : undefined,
        toolEvents,
        roundTexts,
      })
      if (ev.role === 'assistant') gotAssistant = true
      if (typeof ev.seq === 'number') lastChatSeq = Math.max(lastChatSeq, ev.seq)
      added = true
    }
    if (added) {
      nextTick(() => scrollToBottomIfNear())
      // 切页恢复：补拉到新行后重新评估「尾部悬空 user」——assistant 行
      // 到达则停轮清占位；补拉后仍悬空则维持/重启占位轮询。
      detectPendingTurn()
    }
    // B2：补拉到 assistant 行 = 该会话在飞轮次已收尾。
    if (gotAssistant) {
      chatStore.clearInflightTurn(sid)
    }
  } catch {
    // sync 失败不炸 UI——watchdog / 下轮重连兜底
  }
}

// SSE resync 提示（缺口滑出重放窗口/网关重启）→ 全量重载兜底。
// BUG 2026-09-21 ③：历史加载失败态（historyLoaded=false）下不得拦截——
// resync 信号本身就是「重试一次全量拉取」的天然时机。
function onSSEResync() {
  if (!isDefaultChat.value) return
  if (chatStore.historyLoading) return
  // 旧在飞登记随 reset 作废——否则其迟到 timeout 误判新请求。
  inFlightHistory.clear()
  chatStore.reset()
  lastChatSeq = 0
  loadHistory()
}

// P8（2026-09-21）：多端同会话——任一端写入新帧（chat 行/工具事件）时
// 后端广播 SSE `chat.activity {session_id, seq, kind}`。本端自己的写入由
// WS 实时帧先行推进 lastChatSeq → 信号 seq ≤ 游标即「自己或已追平」，零
// 开销短路；落后（lastChatSeq < seq）才响应。P8 补全（2026-09-21）：user
// 行入环（process_messages 入站点 record）后，user/assistant/tool 三类条
// 目都在补拉窗口里——增量 sync 全覆盖，不再按 kind 分流（gap 缺口滑出窗
// 口由 syncMissedChat 内部 reset+loadHistory 兜底）。此前 kind="chat" 走
// 全量刷新的根因是 user 行只在 chat_log、增量补不到；且帧路由经
// broadcast(chat_id) 是连接级——第二标签完全收不到本轮（user 行与回复都
// 不出现，表现为「另一端死了」）。
let activityLagSeq = 0
let activityDebounce: number | null = null
let activityLastRefreshAt = 0
function onChatActivity(payload: any) {
  if (!isDefaultChat.value) return
  if (payload?.session_id !== effectiveSid.value) return
  const seq = typeof payload?.seq === 'number' ? payload.seq : 0
  if (seq <= lastChatSeq) return
  activityLagSeq = Math.max(activityLagSeq, seq)
  // 防抖：高频信号合并为一次响应；5s 冷却防风暴循环。
  if (activityDebounce !== null) clearTimeout(activityDebounce)
  activityDebounce = window.setTimeout(() => {
    activityDebounce = null
    if (activityLagSeq <= lastChatSeq) return // 响应间隙实时帧已追平
    activityLagSeq = 0
    if (streaming.value) return // 本端发送中：流式态自管理，不打断
    if (Date.now() - activityLastRefreshAt < 5000) return
    activityLastRefreshAt = Date.now()
    // 增量补齐（user/assistant/tool 同通道）；gap → 全量兜底在函数内部。
    syncMissedChat()
  }, 800)
}

// --- Watchdog: recover from a lost live response frame ---
// If `streaming` stays true past WATCHDOG_MS with no receive/error frame, the
// WS frame was likely lost (e.g. half-open connection). The response is already
// persisted to session_log, so resync by reloading the latest page and
// REPLACING the message list (no dedup / stable-id needed). Default chat only
// — workflow_chat streaming is engine-driven, not in this session_log path.
//
// 2026-09-21 修复(真机复现:exec 审批挂起 45s → 视图被磁盘 50 条重灌,工具
// 卡/占位全冲掉):watchdog 只认 receive 帧,不认识「轮次活着但静默」的
// 中段——CRITICAL 审批挂起(WebApproval 阻塞等 respond,最长 300s)与长工具
// 执行期间天然无 receive 帧。onWatchdog 触发时先看活跃迹象(本会话审批
// 挂起 / 本轮工具事件未收尾),有则只续期不重灌;活跃续期单独上限
// (12 次 ≈ 9 分钟,覆盖 300s 审批超时 + 模型处理拒绝的余量),超限走原
// 恢复路径。审批超时自动拒绝后链路自洽:拒绝理由回灌模型 → 模型产出最终
// 回复 → receive 帧关 streaming;残留的 running 工具事件由该回复的
// flushPendingToolEvents 挂载收尾。
const WATCHDOG_MS = 45000
const MAX_WATCHDOG_ATTEMPTS = 3
const MAX_WATCHDOG_ACTIVE_EXTENDS = 12
let watchdogTimer: ReturnType<typeof setTimeout> | null = null
let watchdogAttempts = 0
let watchdogActiveExtends = 0
let pendingWatchdogReload = false
// 本轮发送文本——watchdog 恢复判定「回复已落」的语义锚(见响应处理分支
// pendingWatchdogReload 的 landed 判定):从磁盘历史尾部向前找本轮 user 行,
// 其间出现 assistant 行即已落。计数式判定(磁盘 50 条 vs 发送时视图 ~20
// 条的 assistant 数)窗口错位,磁盘计数天然偏大,轮次还在跑也被判已落 →
// 误重灌,已废弃。
let watchdogSentContent = ''

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
function startWatchdog(sentContent = '') {
  watchdogAttempts = 0
  watchdogActiveExtends = 0
  pendingWatchdogReload = false
  watchdogSentContent = sentContent
  armWatchdog()
}
function reloadLatest() {
  pendingWatchdogReload = true
  const requestId = 'watchdog_' + Date.now()
  inFlightHistory.set(requestId, { kind: 'watchdog' })
  sendHistoryRequest(requestId, 50, null, {
    module: props.module,
    moduleData: activeModuleData(),
  })
}
function onWatchdog() {
  watchdogTimer = null
  if (!streaming.value) return
  if (!isDefaultChat.value) return
  // 轮次活跃迹象 → 续期不重灌:审批挂起(全局单例 pendingApprovals,含
  // 其他会话的挂起——多等一个周期无害)或本轮工具事件未 flush(回复未到
  // 即未收尾,含 running/finished 残留——它们都要等回复到场才挂载清空)。
  if (pendingApprovals.length > 0 || chatStore.pendingToolEvents.length > 0) {
    watchdogActiveExtends++
    if (watchdogActiveExtends <= MAX_WATCHDOG_ACTIVE_EXTENDS) {
      armWatchdog()
      return
    }
  }
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
  const requestId = 'resync_' + Date.now()
  inFlightHistory.set(requestId, { kind: 'resync' })
  sendHistoryRequest(requestId, 100, null, {
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
  const sid = effectiveSid.value
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
  const sid = effectiveSid.value
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

// HD（2026-09-17）：历史请求在飞登记——request_id 路由围栏（对齐
// useWSAPI 的 reqId 关联范式）。此前 handleHistoryResponse 从不比对
// request_id：会话切换 reset() 清掉 historyLoading 守卫后，两个在飞
// 请求的响应先后到达各前插一次 → 消息成对重复（U,NB,U,NB）；迟到的
// hist_ 响应还会冒领 pendingResync / pendingWatchdogReload 标志，拿错误
// 请求的 payload 整段替换。围栏语义：响应按 request_id 匹配在飞登记，
// 不匹配（迟到/并行/他请求）直接丢弃；会话切换时整表作废。
const inFlightHistory = new Map<string, { kind: 'page' | 'resync' | 'watchdog'; paginated?: boolean }>()

function handleHistoryResponse(data: any) {
  // 围栏第一道：request_id 必须匹配一个在飞请求，否则丢弃（不前插、
  // 不清 historyLoading、不冒领任何标志）。
  const inflight = data?.request_id ? inFlightHistory.get(data.request_id) : undefined
  if (!inflight) {
    return
  }
  inFlightHistory.delete(data.request_id)
  // 围栏第二道：会话归属——响应携带 session_id 且与本面板会话不符（快速
  // 切换会话时旧会话响应迟到 / 嵌入面板异会话响应）→ 丢弃，防串台。
  if (data?.session_id && effectiveSid.value && data.session_id !== effectiveSid.value) {
    return
  }

  chatStore.historyLoading = false
  if (!data) return

  // P2（2026-09-11）：历史到手 = 未读已见 → 清零未送达徽标（本地立即置 0 +
  // 后端 sidecar 计数清除；无未读时本地短路零开销）。放在所有早退分支之前，
  // resync/watchdog 路径同样视为已读。
  if (effectiveSid.value) sessionStore.markDelivered(effectiveSid.value)
  // P4：本响应已通过会话归属围栏 → 视图归属当前会话，钉住（见
  // loadedHistorySid 声明）。resync/watchdog 分支的 replace 与翻页 prepend
  // 同样是完整视图语义，统一在此设置。
  loadedHistorySid = effectiveSid.value ?? null
  loadingHistorySid = null

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
        sourceNode: m.source_node,
        imageCount: Array.isArray(m.images) ? m.images.length : undefined,
        rowIndex: oldest !== null ? oldest + j : undefined,
      })),
    )
    chatStore.setBusy(effectiveSid.value, false)
    clearWatchdog()
    nextTick(() => scrollToBottom())
    return
  }

  // Watchdog-driven resync: if a genuinely new assistant message is in
  // session_log, the lost response landed — replace from source of truth and
  // un-stick. Otherwise keep the current view and re-check (never drop the
  // just-sent user message).
  if (pendingWatchdogReload) {
    pendingWatchdogReload = false
    const rawMsgs: any[] = data.messages || []
    // 「回复已落」语义化判定:磁盘历史尾部向前,遇到本轮发送文本的 user
    // 行之前是否出现 assistant 行(行序即落盘序)。原判定
    // latestAssistantCount > assistantCountAtSend 拿磁盘 50 条窗口的
    // assistant 计数与发送时视图(~20 条)比较——窗口错位,磁盘计数天然
    // 偏大,轮次还在跑(审批挂起/长工具)也被判已落 → 误重灌(真机
    // 2026-09-21:审批挂起 45s 触发,n=22→50,工具卡/占位全冲)。
    // 同文重发:只看最后一条同文 user 之后——更早的同文行不影响。
    let landed = false
    for (let i = rawMsgs.length - 1; i >= 0; i--) {
      const m = rawMsgs[i]
      if (m.role === 'assistant') { landed = true; break }
      if (m.role === 'user') {
        if ((m.content || '') === watchdogSentContent) break // 追到本轮 user,其后无 assistant → 未落
        continue // 更早 user 行(同文重发等),继续向前
      }
    }
    if (streaming.value && landed) {
      chatStore.replaceMessages(
        rawMsgs.map((m: any) => ({
          role: m.role,
          content: m.content,
          timestamp: m.timestamp || new Date().toISOString(),
          model: m.model,
          sourceNode: m.source_node,
          imageCount: Array.isArray(m.images) ? m.images.length : undefined,
        })),
      )
      chatStore.setBusy(effectiveSid.value, false)
      clearWatchdog()
      // 重灌只有文本行——从环回放挂回最后一轮工具卡(与 F5/切回的
      // primeSeqBaseline 同语义;重灌丢卡 = P1 修复的同族问题)。
      void replayToolsFromRing().catch(() => {})
      nextTick(() => scrollToBottom())
    } else if (streaming.value && watchdogAttempts < MAX_WATCHDOG_ATTEMPTS) {
      // Response not landed yet (maybe still running) — re-check later.
      armWatchdog()
    } else {
      // 放弃臂（W5，2026-09-23）：重试耗尽仍未在磁盘看到回复——视图停在
      // 旧数据是诚实降级（真相源没有新内容可灌），但 streaming 旗标与
      // B2 在飞登记必须复位，否则输入框永久锁死、B2 占位恢复逻辑永挂。
      // 后到的真实回复帧仍可经 receive 正常入列（streaming=false 不拦）。
      if (streaming.value) {
        chatStore.setBusy(effectiveSid.value, false)
        chatStore.clearInflightTurn(effectiveSid.value)
      }
      clearWatchdog()
    }
    return
  }

  const historyMessages = data.messages || []

  // A1/A2（2026-09-22 聊天切会话竞态）：prepend 前清洗「实时帧先到的尾巴」。
  // 切会话 reset() 清空视图的窗口内，assistant 实时帧先入列（无尾部可比），
  // 随后历史快照前置拼接会把它重复一遍。清洗规则（顺序敏感）：
  // ① A1 seq 剔除：历史已含回复 ⟹ 后端读取晚于其落盘 ⟹ last_seq ≥ 其环
  //    seq（assistant 入环在落盘后）——先剔精确的；last_seq 缺省（旧网关/
  //    未注入）自然跳过。
  // ② A2 尾行同文兜底：剔除后的列表尾部与历史批次尾部同 role 同文则丢
  //    尾部——覆盖 user 回声帧重复与 ① 的采样缝隙。
  // W1 修正（2026-09-22 审计修复）：原注释「翻页时两规则天然不命中」对 ①
  // 不成立——后端翻页响应同样携带**全会话最新** last_seq（采样不区分
  // before_index），dropAssistantBelowSeq 会把视图内所有带 seq 的 assistant
  // 实时帧删掉，而这些帧不在更旧的翻页批次里 → 直接消失，且行号重算连带
  // 错算 rewind 锚。故翻页请求（在飞登记 paginated 钉子）跳过两规则：翻页
  // 批次严格更旧、与尾部实时帧零交集，清洗既无必要也有害（宁可不删不错删）。
  if (!inflight.paginated) {
    const lastSeq = typeof data.last_seq === 'number' ? data.last_seq : 0
    chatStore.dropAssistantBelowSeq(lastSeq)
    if (historyMessages.length > 0) {
      const histTail = historyMessages[historyMessages.length - 1]
      chatStore.dropTailIfSame(String(histTail.role ?? ''), String(histTail.content ?? ''))
    }
  }

  if (historyMessages.length > 0) {
    const container = chatMessages.value
    const oldScrollHeight = container ? container.scrollHeight : 0

    const newMessages: ChatMessage[] = historyMessages.map((m: any) => ({
      role: m.role,
      content: m.content,
      timestamp: m.timestamp || new Date().toISOString(),
      model: m.model,
      sourceNode: m.source_node,
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
  // BUG 2026-09-21 ③：本会话历史成功落地——清失败态与自动重试账目。
  historyLoadFailed.value = false
  historyRetryCount = 0
  clearHistoryRetryTimer()
  // L2：历史落地后对齐补拉基线（只取游标，不渲染；详见 primeSeqBaseline）。
  primeSeqBaseline()
  // 切页恢复：历史落地后检测「尾部悬空 user 行」→ 处理中占位 + 轮询
  // （assistant 行落盘后下一轮重拉自动补上并停轮）。
  detectPendingTurn()

  if (chatStore.oldestIndex === 0 || !data.has_more) {
    chatStore.hasMoreHistory = false
    nextTick(() => scrollToBottom())
  }
}

// P4（2026-09-21）：全量历史的「已载会话 / 在飞目标会话」钉子——
// loadedHistorySid：视图当前完整载入的会话（handleHistoryResponse 会话
// 围栏通过后设置）；loadingHistorySid：在飞全量拉取的目标会话（loadHistory
// 发起时设置，响应落地转正）。currentId watch 据此跳过「同会话重复
// reset+重拉」：切回刚看过的会话视图即最新（保留工具卡/占位轮询状态），
// 登录序列「WS 建连 × 默认会话选中」双触发也只剩一次真拉取。切到其他
// 会话时两枚钉子随新会话的历史响应改写，再切回正常重拉，无陈旧风险。
let loadedHistorySid: string | null = null
let loadingHistorySid: string | null = null

// BUG 2026-09-21 ③：历史加载失败不再静默。失败判定 = 10s safety timeout
// （请求蒸发/响应丢失，如重启断连窗口）；成功（handleHistoryResponse 围栏
// 通过）即清。失败态可见 + 有限自动重试（≤2 次退避，断连中不消耗次数、
// 交给重连补偿）；超次数后显示手动「重新加载历史」。
const historyLoadFailed = ref(false)
let historyRetryCount = 0
let historyRetryTimer: number | null = null
const HISTORY_RETRY_MAX = 2

function clearHistoryRetryTimer() {
  if (historyRetryTimer !== null) {
    clearTimeout(historyRetryTimer)
    historyRetryTimer = null
  }
}

/** 换会话 / 重挂载 / 手动重载时复位失败态（旧会话的失败不带到新会话）。 */
function resetHistoryLoadFailure() {
  historyLoadFailed.value = false
  historyRetryCount = 0
  clearHistoryRetryTimer()
}

/** 有限自动重试：2s/4s 退避；WS 未连时不消耗次数（重连补偿会认失败态）。 */
function scheduleHistoryRetry() {
  if (historyRetryTimer !== null) return
  if (historyRetryCount >= HISTORY_RETRY_MAX) return
  historyRetryCount += 1
  const delay = 2000 * historyRetryCount
  historyRetryTimer = window.setTimeout(() => {
    historyRetryTimer = null
    if (wsStatus.value === 'connected') {
      if (!chatStore.historyLoaded) loadHistory()
    } else {
      historyRetryCount -= 1
      // 断连中：不再排下一拍——重连 watch 的失败态分支接管。
    }
  }, delay)
}

/** 手动重载（失败态按钮）——复位失败态后走常规 loadHistory。 */
function manualReloadHistory() {
  resetHistoryLoadFailure()
  loadHistory()
}

function loadHistory() {
  if (chatStore.historyLoading) return
  chatStore.historyLoading = true
  loadingHistorySid = effectiveSid.value
  const requestId = 'hist_' + Date.now()
  inFlightHistory.set(requestId, {
    kind: 'page',
    // W1（2026-09-22 审计修复）：发请求时钉「本次是否翻页」。oldestIndex
    // 初始 null（chat.ts）、切会话 reset 归 null、每次历史响应后回写——
    // 发请求时非 null ⟺ 在翻页。翻页批次严格更旧，A1/A2 尾部清洗必须
    // 跳过（见 handleHistoryResponse 的 W1 注释）。
    paginated: chatStore.oldestIndex != null,
  })
  const limit = 20
  sendHistoryRequest(requestId, limit, chatStore.oldestIndex, {
    module: props.module,
    moduleData: activeModuleData(),
  })

  // Safety timeout: reset loading flag if no response in 10s（超时同时作废
  // 在飞登记——迟到的响应不再被围栏放行）。per-request 围栏：只有本请求
  // 仍在登记表（未被响应、未被会话切换作废）才允许动全局状态——否则
  // 更早请求的迟到 timer 会误杀当前在飞请求（清掉别人的 loading、用别人
  // 的 10s 到期给本请求判死），真机挂起场景实测复现。
  setTimeout(() => {
    if (!inFlightHistory.has(requestId)) return
    inFlightHistory.delete(requestId)
    chatStore.historyLoading = false
    // BUG 2026-09-21 ③：历史未落地 + 请求蒸发 → 失败态可见 + 有限自动
    // 重试。此前静默清 flag，重连补偿看到「在飞」假象跳过重拉，视图
    // 永远空壳且与「会话本来就空」不可区分。
    if (!chatStore.historyLoaded && isDefaultChat.value) {
      historyLoadFailed.value = true
      scheduleHistoryRetry()
    }
  }, 10000)
}

// ---------------------------------------------------------------------------
// 切页恢复（2026-09-21 切标签页丢消息修复）：后端 user 行已在 turn 开始时
// 落盘，但 assistant 行要等 turn 完成才落——切走再切回（loadHistory）若
// AI 仍在处理，历史尾部是「悬空 user 行」。pendingTurn 是本组件对该形态
// 的呈现态（不入 store）：复用 typing-indicator 显示「处理中」占位 + 定时
// 重拉历史，assistant 行落盘后自动出现并停轮。busy=false（agent.inbox_status，
// 与发送同规则的会话键）连续 2 次重拉仍无回复 → 轮次已死（重启/异常），
// 诚实停轮、悬空 user 行保留展示（与 steer 悬空形态一致）；15 分钟硬上限
// 兜底防无限轮询。
// ---------------------------------------------------------------------------

const pendingTurn = ref(false)
// BUG 2026-09-21 ①：占位区的限流重试实时态（agent.retry_status 轮询）——
// 切走切回后用户看到「第 N/M 次重试」而非哑转圈；重试不落盘（既有裁决
// 进度只走实时帧），这里是唯一的过程可见面。
const retryStatus = ref<{ retry: number; max_retries: number; wait_secs: number; model: string } | null>(null)
let pendingTurnTimer: number | null = null
let pendingTurnDeadPolls = 0
let pendingTurnStartedAt = 0
const PENDING_TURN_POLL_MS = 4000
const PENDING_TURN_MAX_DEAD_POLLS = 2
const PENDING_TURN_MAX_MS = 15 * 60 * 1000

function stopPendingTurnPolling() {
  if (pendingTurnTimer !== null) {
    clearInterval(pendingTurnTimer)
    pendingTurnTimer = null
  }
  if (pendingTurn.value) pendingTurn.value = false
  retryStatus.value = null
  pendingTurnDeadPolls = 0
}

/** 历史加载完成后检测「尾部悬空 user 行」（非本地发送态）——命中则启动
 *  处理中占位 + 轮询；尾部已是 assistant（或空/加载中）则停轮。幂等。
 *  B2（2026-09-22）：检测条件放宽——除悬空 user 行外，本会话有「在飞
 *  turn 登记」（发送后切走再切回，chat_log 可能连 user 行都还没落）同样
 *  启动占位 + 轮询，消除「纯空视图无任何反馈」的空窗。尾部已是
 *  assistant = 回复已到场（历史渲染或实时帧），登记完成使命一并清除。 */
function detectPendingTurn() {
  const msgs = chatStore.messages
  const last = msgs[msgs.length - 1]
  const sid = effectiveSid.value
  // 尾部 assistant = 回复已到场——无论历史渲染还是实时帧，在飞登记清账。
  // D-3：busy 一并清算——重挂载补偿的 reset 不再清 busy 表（按会话隔离），
  // 卸载窗口内完成的轮次（完成帧丢失）靠此处收尾，否则输入框永锁。
  if (last && last.role === 'assistant') {
    chatStore.setBusy(sid, false)
    chatStore.clearInflightTurn(sid)
    stopPendingTurnPolling()
    return
  }
  const inflight = !!chatStore.inflightTurnOf(sid)
  const dangling = (!!last && last.role === 'user') || inflight
  if (!dangling) {
    stopPendingTurnPolling()
    return
  }
  // 本地发送态（streaming）不接管——占位已由 streaming 渲染。
  if (streaming.value) {
    stopPendingTurnPolling()
    return
  }
  if (pendingTurnTimer === null) {
    pendingTurnDeadPolls = 0
    pendingTurnStartedAt = Date.now()
    pendingTurn.value = true
    pendingTurnTimer = window.setInterval(() => {
      void pollPendingTurn()
    }, PENDING_TURN_POLL_MS)
  }
}

async function pollPendingTurn() {
  if (!pendingTurn.value) return
  if (Date.now() - pendingTurnStartedAt > PENDING_TURN_MAX_MS) {
    chatStore.clearInflightTurn(effectiveSid.value) // B2：硬上限到期清账
    stopPendingTurnPolling()
    return
  }
  // busy 快照（查询失败 available:false → busy=false → 走死轮计数，
  // 不会卡死轮询）。
  await refreshInbox(effectiveSid.value || '')
  const busy = inboxStatus.value?.busy === true
  // BUG 2026-09-21 ①：同拍查限流重试态——retrying 时占位区显示进度文案。
  // 查询失败静默（retryStatus 保持旧值/空，不影响停轮判定）。
  try {
    const rs = await request('agent', 'retry_status', { session_id: effectiveSid.value || '' })
    retryStatus.value = rs?.retrying
      ? { retry: rs.retry, max_retries: rs.max_retries, wait_secs: rs.wait_secs, model: rs.model }
      : null
  } catch {
    /* 后端不可用/非 default chat——占位退化为转圈，无害 */
  }
  // 重拉走 L2 增量补拉（chat.sync after_seq，append 语义）而不是
  // loadHistory——后者是 prepend 翻页通道，重拉全量会重复插行。assistant
  // 行落盘后 sync 返回该事件 append 到尾部，syncMissedChat 内部的
  // detectPendingTurn 停轮清占位。
  await syncMissedChat()
  if (!busy) {
    pendingTurnDeadPolls += 1
    if (pendingTurnDeadPolls >= PENDING_TURN_MAX_DEAD_POLLS) {
      // B2：轮次已死（重启/异常）——登记一并清账，悬空 user 行保留展示
      // （诚实停轮，与 steer 悬空形态一致）。
      chatStore.clearInflightTurn(effectiveSid.value)
      stopPendingTurnPolling()
    }
  } else {
    pendingTurnDeadPolls = 0
  }
}
// ---------------------------------------------------------------------------
// T8 多模态（2026-09-03）：图片附件（上传端点 /api/upload/image → chat.send
// media）。三种入口：📎 选择文件、粘贴（clipboard files）、拖拽到输入区。
// 上传成功后持 id 等待随消息发送；不做 canvas 压缩，超限由前置校验与后端
// 同一口径拒绝。
// ---------------------------------------------------------------------------

const toast = useToast()
// 2026-09-21 watchdog 活跃续期判据:审批挂起(WebApproval 阻塞等 respond
// 期间无 receive 帧,45s 静默不该触发恢复)。全局单例的挂起列表,含其他
// 会话的审批——误续期一个周期无害,不做会话过滤。
const { pendingApprovals } = useApprovals()
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
  if (streaming.value && !canQueueWhileBusy.value) return

  sendValidated(content, media)
}

/** 输入校验通过后的实际发送（sendMessage 的 async 续体；2026-09-26 起
 *  无会话锚时先建会话——见 sendValidated 内注释）。 */
async function sendValidated(content: string, media: { id: string }[]) {
  // 先本地回显用户消息（即时反馈），再做会话锚定——顺序不可倒：watch 是
  // flush:'pre' 微任务，create() 内 switchTo 排队的 watcher 触发早于本
  // 函数 await 续体，此刻回显若未发生，切换链会先把视图 reset 掉。
  chatStore.addMessage({
    role: 'user',
    // 纯图无文字时回显占位（发送内容保持原样，不污染提示词）。
    content: content || (media.length ? '[图片]' : ''),
    timestamp: new Date().toISOString(),
    imageCount: media.length || undefined,
  })

  // 皮肤 launcher / 首用未选中态发送（2026-09-26 用户实测 BUG 根修）：
  // 此前 chat.send 不带 session_id → 后端自动落连接级会话 → 回复帧的
  // session_id 与本地空锚恒不等 → tool_event 实时帧全被「异会话」过滤
  // 丢弃、回复落地也无进行中指示，用户体验为「右侧啥都没有，过一会突然
  // 蹦一句话，点会话才看到工具记录」。先建会话锚定，全链帧同域。standalone
  // 面板不建（无 session store 语义，保持 legacy 接受路径）；create 失败
  // 同样退 legacy 无锚发送（tool_event 过滤对无锚态豁免）。
  if (!effectiveSid.value && !props.standalone && (props.module ?? 'chat') === 'chat') {
    await sessionStore.create(undefined, undefined, { markJustCreated: true })
  }

  chatStore.clearInput()
  pendingImages.value = []
  // I4: 粘贴折叠状态一并清空（占位符已全部还原，映射/展开态/计数器重置）。
  pastedTexts.value = new Map()
  expandedPastes.value = new Set()
  pasteSeq = 0
  chatStore.setBusy(effectiveSid.value, true)
  // B2：登记在飞 turn（不被会话切换 reset 清掉）——切走再切回时占位/
  // 轮询凭此恢复；assistant/error/sync 收尾时清除。
  chatStore.markInflightTurn(effectiveSid.value, content)
  // 本地发送接管占位——清掉可能残留的切页恢复轮询（streaming 态由
  // watchdog 负责，两套机制不叠加）。
  stopPendingTurnPolling()
  // 传入本轮文本:watchdog 恢复判定「回复已落」的语义锚(见响应处理
  // 分支 pendingWatchdogReload 的 landed 判定)。
  startWatchdog(content)

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
    startInboxPolling(effectiveSid.value || '')
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
      chatStore.setBusy(effectiveSid.value, false)
      // B2：主动停止 = 在飞轮次终结。
      chatStore.clearInflightTurn(effectiveSid.value)
      chatStore.addMessage({
        role: 'system',
        content: '已停止生成',
        timestamp: new Date().toISOString(),
      })
      nextTick(() => scrollToBottom())
    } else {
      // W5（2026-09-23）：cancelled=0 = 后端没有在跑的轮次——本地点击
      // 的停止意图仍然生效，UI 的 streaming/在飞登记是陈旧态（回复丢失
      // 或早已完成而前端漏了收尾），必须复位，否则停止按钮永远不消失。
      // 不加「已停止生成」系统行——后端确认无在跑轮次，加行是假话。
      chatStore.setBusy(effectiveSid.value, false)
      chatStore.clearInflightTurn(effectiveSid.value)
    }
  }).catch(() => {
    chatStore.setBusy(effectiveSid.value, false)
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
    if (val === 'connected' && (!chatStore.historyLoaded || historyLoadFailed.value)) {
      loadHistory()
    } else if (val === 'connected' && chatStore.historyLoaded) {
      // L2：重连（非首连）→ 断线补拉而非整页重载。
      syncMissedChat()
    }
    if (val === 'disconnected' && streaming.value) {
      chatStore.setBusy(effectiveSid.value, false)
    }
    return
  }
  appStore.connected = val === 'connected'
  // BUG 2026-09-21 ③：失败态（timeout 清了 historyLoading 的在飞假象）也
  // 触发重连重拉——此前条件只看 historyLoaded，重连时若原请求蒸发，
  // 这里的 loadHistory 会被入口守卫挡掉（historyLoading 假象残留）或因
  // 条件不满足而不发，空壳视图没有自愈入口。
  if (val === 'connected' && (!chatStore.historyLoaded || historyLoadFailed.value)) {
    loadHistory()
  } else if (val === 'connected' && chatStore.historyLoaded) {
    // L2：重连（非首连）→ 断线补拉而非整页重载。
    syncMissedChat()
  }
  // Reset streaming flag on disconnect to prevent stuck UI
  if (val === 'disconnected' && streaming.value) {
    chatStore.setBusy(effectiveSid.value, false)
  }
  // 断连期间轮询重拉无意义——停掉；重连后 loadHistory 会重新 detect。
  if (val === 'disconnected' && pendingTurn.value) {
    stopPendingTurnPolling()
  }
  if (val === 'connected') {
    initVoiceState()
    syncInboxMode()
    syncAgentMode()
  }
})

// U7: streaming 结束 → 停轮询并刷新一次（队列里剩余条数清零/被消费）。
const unwatchStreaming = watch(streaming, (s) => {
  if (!isDefaultChat.value) return
  if (!s) {
    stopInboxPolling()
    syncInboxMode()
  }
})

// Multi-session: when the active conversation id changes, reset the chat
// state and reload that conversation's history (backend routes by session_id).
// D-3：监听 effectiveSid 而非裸 currentId——嵌入宿主（sessionId 钉死）时
// 全局选中切换不再触发本面板 reset/重拉（视图锚定自己的会话）；props 变化
// （宿主重绑，如 draft_apply 后落到正式会话）同样走这条切换链。
const unwatchSession = watch(
  effectiveSid,
  (newId, oldId) => {
    if (!isDefaultChat.value || newId === oldId) return
    // P4（2026-09-21）：同会话免重拉——已完整载入（newId === loadedHistorySid
    // 且 historyLoaded）或在飞的正是本会话历史（loadingHistorySid === newId，
    // 响应围栏与渲染目标都是 newId）：跳过 reset+重拉。会话相关的模式
    // 徽标仍要对齐。此时不动 inFlightHistory（在飞的正是本会话，要放行）。
    if (
      newId &&
      ((newId === loadedHistorySid && chatStore.historyLoaded) ||
        newId === loadingHistorySid ||
        // 刚由本面板发送链 create() 的会话（launcher/未选中态发送即锚定，
        // 2026-09-26）：服务端历史在首条消息落盘前必为空，reset 会毁掉
        // 尚未落盘的本地回显，loadHistory 纯多余——无条件跳过。消费即清，
        // 防陈旧标记误杀日后对该会话的正常重拉。
        newId === sessionStore.justCreatedSid)
    ) {
      if (newId === sessionStore.justCreatedSid) sessionStore.justCreatedSid = null
      syncInboxMode()
      syncAgentMode()
      return
    }
    // HD：换会话 → 作废全部在飞历史请求（旧会话响应迟到时被围栏丢弃）。
    inFlightHistory.clear()
    // 换会话 → 旧会话的切页恢复轮询随之作废（新会话历史落地后重新 detect）。
    stopPendingTurnPolling()
    // BUG 2026-09-21 ③：旧会话的加载失败态不带到新会话。
    resetHistoryLoadFailure()
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
  // P8：多端同会话感知信号 → 落后端防抖全量刷新（见 onChatActivity）。
  onSSE('chat.activity', onChatActivity)

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
  // justCreatedSid 只服务于「挂载中面板」发送链的 create→watcher 交接；
  // 若会话是在面板卸载窗口内经侧栏新建的，重挂载时本函数的初始加载会正常
  // 拉历史——陈旧标记必须在此消费掉，否则日后切走再切回该会话会被
  // watcher 误判为「刚建空会话」而跳过重拉（2026-09-26 修复件）。
  sessionStore.justCreatedSid = null
  // F-B：历史首拉对两种模式一致——standalone 的 connect 由 auth store 在
  // 登录时完成，挂载时可能已 connected（watcher 不回放旧值，这里直接查）。
  if (wsStatus.value === 'connected' && !chatStore.historyLoaded && !chatStore.historyLoading) {
    loadHistory()
  } else if (wsStatus.value === 'connected' && chatStore.historyLoaded && isDefaultChat.value) {
    // 路由卸载重挂载补偿：离开聊天页（如切到定时/Skills 再回来）期间
    // ChatPanel 被卸载，messageHandler 随之移除——卸载窗口内晚到的
    // receive 帧无人接收而丢失（后端 session_log 照常落盘）。Pinia store
    // 是全局单例、historyLoaded 残留 true，重挂载若跳过重载，视图就停留
    // 在旧数据（直到手动 F5 整页刷新）。重挂载即强制全量重拉（与
    // onSSEResync 同链：reset + 补拉游标归零 + loadHistory），丢帧窗口闭合。
    resetHistoryLoadFailure()
    chatStore.reset()
    lastChatSeq = 0
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
  offSSE('chat.activity', onChatActivity)
  // BUG 2026-09-21 ③：卸载清自动重试定时器（残留会在下个实例外开火）。
  clearHistoryRetryTimer()
  if (activityDebounce !== null) {
    clearTimeout(activityDebounce)
    activityDebounce = null
  }
  unwatchStatus()
  unwatchSession()
  unwatchStreaming()
  unwatchUsage()
  // 切页恢复轮询随组件卸载终止（重挂载时 loadHistory 重新 detect）。
  stopPendingTurnPolling()
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
  <div class="page-chat" :class="{ 'nb-launcher-mode': launcherMode }">
    <!-- H2: todo 清单面板（todowrite 实时刷新 + 进会话拉取）。会话锚定本面板
         effectiveSid（2026-09-24 串扰修复：嵌入面板钉死会话，不得显示全局
         选中会话的清单——曾因此渲染进工作流「对话生成」）。 -->
    <TodoPanel v-if="isDefaultChat" :session-id="effectiveSid" />

    <!-- Messages -->
    <div ref="chatMessages" class="chat-messages" @click="onChatAreaClick">
      <!-- History loading indicator -->
      <div v-if="chatStore.historyLoading" class="history-loading" style="text-align: center; padding: 8px; color: var(--text-muted); font-size: var(--text-xs);">
        <span class="spinner" style="width:14px;height:14px;border-width:2px;vertical-align:middle;"></span>
        <span style="vertical-align:middle;"> 加载历史消息...</span>
      </div>

      <!-- 皮肤骨架槽位：主页启动器（空会话时品牌 + 场景标签；WB home 形态） -->
      <div v-if="launcherMode" class="nb-launcher">
        <div class="nb-launcher-brand">
          <i class="nb-brand-mark nb-brand-mark-lg" aria-hidden="true"></i>
          <span>{{ skinState.meta?.brand || skinState.id }}</span>
        </div>
        <div v-if="skinState.meta?.scenes?.length" class="nb-launcher-scenes">
          <button
            v-for="s in skinState.meta.scenes"
            :key="s"
            class="nb-scene-chip"
            type="button"
            @click="applyScene(s)"
          >{{ s }}</button>
        </div>
      </div>

      <!-- Welcome message -->
      <!-- BUG 2026-09-21 ③：历史加载失败态优先于欢迎语——空视图与「会话本
           来就空」不可区分是本案空壳视图的直接成因；显式给失败态 + 手动
           重试入口（自动重试见 loadHistory 的 timeout 分支）。 -->
      <div v-if="chatStore.messages.length === 0 && historyLoadFailed" class="message assistant">
        <div class="message-avatar">NB</div>
        <div class="message-content">
          <div class="message-bubble">
            <p>⚠️ 历史消息加载失败（网络或服务暂不可达）。数据并未丢失。</p>
            <button class="btn btn-secondary btn-sm" type="button" @click="manualReloadHistory">重新加载历史</button>
          </div>
        </div>
      </div>
      <div v-else-if="chatStore.messages.length === 0 && !launcherMode" class="message assistant">
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
          <!-- R1: 本条消息关联的中间轮正文（多步任务的每轮过程叙述）。
               最终回复已落地 → 默认折叠为计数条，点击展开回看。 -->
          <template v-if="msg.roundTexts && msg.roundTexts.length">
            <button
              class="round-text-toggle"
              type="button"
              @click="msg.roundTextsOpen = !msg.roundTextsOpen"
            >
              <span class="tool-group-icon">💬</span>
              过程 · {{ msg.roundTexts.length }} 段
              <span class="tool-group-caret">{{ msg.roundTextsOpen ? '▾' : '▸' }}</span>
            </button>
            <div v-if="msg.roundTextsOpen" class="round-text-body">
              <div v-for="(rt, ri) in msg.roundTexts" :key="ri" class="round-text">{{ rt.content }}</div>
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
            <!-- 集群续行归属（2026-09-23）：干活的是远端 worker 节点——徽章与
                 模型徽章并列（模型徽章说的是转述文本由哪个主节点模型生成）。
                 复用 model-badge 低对比基调 + 降不透明度区分。 -->
            <span v-if="msg.role === 'assistant' && msg.sourceNode" class="model-badge node-badge">节点 {{ msg.sourceNode }}</span>
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
      <!-- pendingTurn：切页恢复的「处理中」占位——历史尾部悬空 user 行时复用
           同款 typing-indicator（AI 仍在处理，assistant 行落盘后自动补上）。
           P8：工具卡区与 typing 气泡解耦（外层条件并入 pendingToolEvents）——
           多端场景下本端未发起轮次（另一端在跑工具）也能实时看到工具卡，
           否则实时 tool_event 只进 store 不渲染，直到回复落地才闪现。 -->
      <div
        v-if="streaming || pendingTurn || chatStore.pendingToolEvents.length || chatStore.pendingRoundTexts.length"
        class="message assistant"
      >
        <div class="message-avatar">NB</div>
        <div class="message-content">
          <!-- R1: 进行中轮次的中间正文（模型每轮过程叙述，实时展开显示；
               回复落地时 flush 折叠挂载到该回复消息）。 -->
          <template v-if="chatStore.pendingRoundTexts.length">
            <div class="round-text-body">
              <div
                v-for="(rt, ri) in chatStore.pendingRoundTexts"
                :key="ri"
                class="round-text"
              >{{ rt.content }}</div>
            </div>
          </template>
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
          <div v-if="streaming || pendingTurn" class="message-bubble">
            <!-- BUG 2026-09-21 ①：pendingTurn（切回会话）时限流重试实时态
                 可见——文案与实时帧同口径；无快照（非限流/查询不可用）退化为
                 转圈。streaming（本地发送）不显示——重试进度已有实时帧。 -->
            <template v-if="pendingTurn && retryStatus">
              <div class="retry-status-text">
                ⏳ 上游限流（{{ retryStatus.model }}），第 {{ retryStatus.retry }}/{{ retryStatus.max_retries }} 次重试，等待 {{ retryStatus.wait_secs }} 秒…
              </div>
            </template>
            <div class="typing-indicator"><span></span><span></span><span></span></div>
          </div>
        </div>
      </div>
    </div>

    <!-- Toolbar（launcher 启动器模式不渲染——首屏只留品牌 + 输入盒；
         归模板管而非皮肤 CSS display:none：scoped display:flex 与 :where 基线
         同特异性且后加载会赢回，2026-09-26 实录） -->
    <div v-if="!toolbarCollapsed && !launcherMode" class="voice-toolbar">
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
      <!-- Full Access 放行开关（2026-09-20 用户裁决；运行时态，
           Agent 重启后自动关闭）——与设置页【编辑器】TAB 同一状态 -->
      <button
        v-if="isDefaultChat"
        class="voice-btn editor-full-btn"
        :class="{ active: fullAccess }"
        :disabled="!editorAvailable"
        title="Full Access（运行时开关，重启失效）：项目目录内文件操作全放行；项目目录外读/执行/网络/系统放行；项目外写删仍需审批；自毁硬拦不被绕过"
        @click="toggleFullAccess"
      >
        ⚡ Full Access
      </button>
      <button
        v-if="isDefaultChat"
        class="voice-btn editor-ext-btn"
        :class="{ active: externalWrite }"
        :disabled="!fullAccess"
        title="外部写删放行（依赖 Full Access）：项目目录外的写入/删除也放行。两开关全开 = 真·全放（真沙盒仍兜底）"
        @click="toggleExternalWrite"
      >
        外部写删
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
    <div v-if="streaming && queuedTotal > 0" class="queue-chip" :class="{ full: queueFull }">
      ⏳ agent 处理中，已排队 {{ queuedTotal }} 条（其中插队 {{ inboxStatus?.next_step ?? 0 }}）<template v-if="queueFull"> · 队列已满</template>
    </div>
    <!-- F1: 计划模式常驻条（工具栏可折叠，安全相关状态需要始终可见） -->
    <div v-if="isDefaultChat && chatStore.agentMode === 'plan'" class="plan-strip">
      📋 计划模式：文件修改类工具已停用（plans/ 目录写入放行）— 点击上方徽标或发送 /build 切回
    </div>
    <!-- Full Access 放行常驻条（安全相关状态始终可见，plan-strip 同款） -->
    <div v-if="fullAccess" class="editor-strip">
      <template v-if="externalWrite">⚡ Full Access + 外部写删：全放行中（exec 由真沙盒兜底；自毁硬拦仍生效）— 点击上方按钮或设置页【编辑器】关闭</template>
      <template v-else>⚡ Full Access：项目内全放行 + 项目外读/执行/网络/系统放行（项目外写删仍审批）— Agent 重启后自动关闭</template>
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
        :disabled="streaming && !canQueueWhileBusy"
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
        :disabled="streaming && !canQueueWhileBusy"
      ></textarea>
      <button v-if="streaming && showStopButton" class="btn btn-stop" @click="stopGeneration" title="停止生成">
        <svg viewBox="0 0 24 24" fill="currentColor" width="16" height="16">
          <rect x="6" y="6" width="12" height="12" rx="2"/>
        </svg>
      </button>
      <button v-if="!streaming || canQueueWhileBusy" class="btn btn-primary" @click="sendMessage" :disabled="(!chatStore.input.trim() && !pendingImages.length) || uploadingImages > 0">
        发送
      </button>
      <span v-else-if="!showStopButton" class="btn btn-primary btn-disabled-workflow" title="工作流执行中，无法中断">
        执行中...
      </span>
      <!-- 皮肤激活时会话列表常驻 SkinSidebar，此开关无意义 -->
      <button
        v-if="!skinState.id"
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
      v-if="showShare && effectiveSid"
      :session-id="effectiveSid"
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
/* Full Access 放行常驻条（安全相关状态始终可见，plan-strip 同款） */
.editor-strip {
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

/* R1（2026-09-21）：中间轮正文（模型每轮过程叙述）——折叠条复用
   tool-group 同款胶囊风格；正文块浅字缩进，段间细分隔线。 */
.round-text-toggle {
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
.round-text-toggle:hover {
  color: var(--text);
  border-color: var(--accent);
}
.round-text-body {
  margin-bottom: 6px;
  padding: 6px 10px;
  border-left: 2px solid var(--border);
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.round-text {
  color: var(--text-muted);
  font-size: var(--text-sm, 13px);
  line-height: 1.5;
  white-space: pre-wrap;
  word-break: break-word;
}
.round-text + .round-text {
  padding-top: 6px;
  border-top: 1px dashed var(--border);
}

/* BUG 2026-09-21 ①：占位区限流重试进度文案（pendingTurn 态，切回会话后
   与实时帧同口径的过程可见面）。 */
.retry-status-text {
  color: var(--text-muted);
  font-size: var(--text-sm, 13px);
  line-height: 1.5;
  margin-bottom: 6px;
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
