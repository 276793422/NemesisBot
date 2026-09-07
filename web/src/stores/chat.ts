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
  /** M1b（devtool-upgrade 阶段 3）：本条 assistant 消息对应的工具调用卡片
   *  事件（M1a AgentEvent 通道实时收集，响应落地时 flush 挂载）。
   *  会话重同步（watchdog replaceMessages）后事件丢失——诚实降级为无卡片。 */
  toolEvents?: ToolEvent[]
  /** M6（devtool-upgrade 阶段 7）：本条消息在后端 chat_log jsonl 里的行号
   *  （E3 rewind 的 message_index）。只有 user/assistant 行有值——error/
   *  system 消息是纯前端渲染，后端无对应行。watchdog replaceMessages 重建
   *  后不可信，置空诚实降级（菜单入口随之隐藏）。 */
  rowIndex?: number
}

/** H1/H2（2026-09-05）：单条 todo（与后端 TodoItem serde snake_case 对齐）。 */
export interface TodoItem {
  content: string
  status: 'pending' | 'in_progress' | 'completed'
}

/** M1b：单条工具调用事件（与后端 AgentEvent ToolStarted/ToolFinished
 *  serde snake_case 字段对齐；同一 callId 先 running 后终态，upsert 去重
 *  ——断线重连重复推送幂等）。 */
export interface ToolEvent {
  callId: string
  tool: string
  state: 'running' | 'ok' | 'error'
  argsPreview?: string
  durationMs?: number
  resultPreview?: string
}

export const useChatStore = defineStore('chat', () => {
  const messages = ref<ChatMessage[]>([])
  const input = ref('')
  const streaming = ref(false)
  // H2：当前会话的 todo 清单（TodoPanel 渲染；会话切换时 reset 清空）。
  const todos = ref<TodoItem[]>([])
  // M1b：进行中轮次的工具事件缓冲（响应落地时 flush 挂到 assistant 消息；
  // 会话切换 / watchdog 重同步时清空，防误挂到下一轮）。
  const pendingToolEvents = ref<ToolEvent[]>([])
  // F1（devtool-upgrade 阶段 4）：plan/build 工作模式徽标。'build' 是保守
  // 初值——真实值进会话时经 chat.get_mode 对齐；ModeChanged push（/plan
  // /build slash 或 chat.set_mode）实时刷新。注意后端模式是 loop 级全局态
  // （非 per-session），徽标只做呈现。
  const agentMode = ref<'build' | 'plan'>('build')

  // M6（devtool-upgrade 阶段 7）：下一行 live 消息的后端 chat_log 行号锚。
  // null = 尚无锚（历史未加载），此时 live 消息不编号（诚实降级）。派生式
  // 见 recomputeNextRowIndex——历史批次与 live 行都只含 user/assistant，
  // chat_log 与渲染消息 1:1 连续。
  let nextRowIndex: number | null = null
  // M6：Ctrl+K 命令面板选中命令后经此投递给 ChatPanel 输入框（跨组件；
  // ChatPanel watch 消费后清空）。命令面板「插入不发送」，参数型命令由
  // 用户补参后回车，K3 后端改写链照常生效。
  const commandDraft = ref('')

  // History state
  const historyLoading = ref(false)
  const hasMoreHistory = ref(true)
  const oldestIndex = ref<number | null>(null)
  const historyLoaded = ref(false)

  function addMessage(msg: ChatMessage) {
    // M6：live user/assistant 行按 nextRowIndex 顺序编号（error/system 不
    // 占行——后端 chat_log 无对应行）。无锚（历史未加载）不编号，诚实降级。
    if (
      msg.rowIndex === undefined &&
      nextRowIndex !== null &&
      (msg.role === 'user' || msg.role === 'assistant')
    ) {
      msg.rowIndex = nextRowIndex++
    }
    messages.value.push(msg)
  }

  function prependHistory(history: ChatMessage[], oldestIdx?: number | null) {
    // M6：历史批次与后端 jsonl 行连续——批内第 j 条的行号 = oldestIdx + j。
    const numbered = history.map((m, j) =>
      typeof oldestIdx === 'number' ? { ...m, rowIndex: oldestIdx + j } : m,
    )
    messages.value = [...numbered, ...messages.value]
    recomputeNextRowIndex()
  }

  /** M6：重推 nextRowIndex 锚 = 首条带行号消息的行号 + 其后 user/assistant
   *  消息总数（error/system 不占行）。无任何行号锚 → null。 */
  function recomputeNextRowIndex() {
    nextRowIndex = null
    let rows = 0
    let anchor: number | null = null
    for (const m of messages.value) {
      if (m.role !== 'user' && m.role !== 'assistant') continue
      if (anchor === null && m.rowIndex !== undefined) anchor = m.rowIndex
      rows++
    }
    if (anchor !== null) nextRowIndex = anchor + rows
  }

  /** Replace the whole message list — used by the chat watchdog to resync
   *  from session_log when a live response frame is suspected lost. Replacing
   *  (not merging) avoids any dedup / stable-id requirement.
   *
   *  M6：watchdog 路径的调用方映射 fresh 对象（不带 rowIndex）→ 行号天然
   *  缺失诚实降级；M6 resync 路径（rewind/redo 后刷新）的调用方带
   *  oldest_index 推导的 rowIndex，原样保留。 */
  function replaceMessages(msgs: ChatMessage[]) {
    messages.value = [...msgs]
    // M1b：重同步后事件通道状态不可信，清空 pending 防止误挂到下一轮。
    pendingToolEvents.value = []
    // M6：行号锚随消息列表重推——watchdog 路径（fresh 对象无行号）归 null
    // 诚实降级；resync 路径（带 oldest_index 行号）重推出正确锚。
    recomputeNextRowIndex()
  }

  /** M1b：收集工具事件（按 callId upsert——同一调用 Started→Finished 原地
   *  更新状态；重复推送幂等去重）。 */
  function appendToolEvent(ev: ToolEvent) {
    const idx = pendingToolEvents.value.findIndex(e => e.callId === ev.callId)
    if (idx >= 0) {
      pendingToolEvents.value[idx] = { ...pendingToolEvents.value[idx], ...ev }
    } else {
      pendingToolEvents.value.push(ev)
    }
  }

  /** M1b：取走 pending 事件（assistant 消息落地时挂载到该消息）。 */
  function flushPendingToolEvents(): ToolEvent[] {
    const out = pendingToolEvents.value
    pendingToolEvents.value = []
    return out
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
    todos.value = []
    pendingToolEvents.value = []
    agentMode.value = 'build'
    historyLoading.value = false
    hasMoreHistory.value = true
    oldestIndex.value = null
    historyLoaded.value = false
    commandDraft.value = ''
    // M6：会话切换丢掉旧行号锚——新会话历史未加载前 live 消息不编号，
    // 防止上一会话的锚串场（历史加载后 prependHistory 重推）。
    nextRowIndex = null
  }

  return {
    messages,
    input,
    streaming,
    historyLoading,
    hasMoreHistory,
    oldestIndex,
    historyLoaded,
    todos,
    pendingToolEvents,
    agentMode,
    commandDraft,
    addMessage,
    prependHistory,
    replaceMessages,
    appendToolEvent,
    flushPendingToolEvents,
    clearInput,
    /** F1：ModeChanged push / chat.set_mode 回包 / get_mode 对齐共用。 */
    setAgentMode(m: 'build' | 'plan') {
      agentMode.value = m
    },
    setTodos(newTodos: TodoItem[]) {
      todos.value = [...newTodos]
    },
    reset,
  }
})
