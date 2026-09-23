import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import { useSessionStore } from './session'

export interface ChatMessage {
  role: 'user' | 'assistant' | 'error' | 'system'
  content: string
  timestamp: string
  /** Producing model in `provider/name` form (assistant messages only).
   *  Rendered as a "供应商·模型名" badge; undefined for user/error/system or
   *  legacy messages persisted before the badge feature. */
  model?: string
  /** 集群续行归属（2026-09-23）：实际执行任务的 worker 节点名（receive 帧
   *  source_node / 历史行 source_node 透传）。渲染「节点 X」徽章，与模型
   *  徽章并列——干活的是远端节点，写字的是主节点模型。缺省 = 非集群回复。 */
  sourceNode?: string
  /** T8 多模态：该消息附带的图片数（本地回显 + 历史映射 m.images.length）。 */
  imageCount?: number
  /** M1b（devtool-upgrade 阶段 3）：本条 assistant 消息对应的工具调用卡片
   *  事件（M1a AgentEvent 通道实时收集，响应落地时 flush 挂载）。
   *  会话重同步（watchdog replaceMessages）后事件丢失——诚实降级为无卡片。 */
  toolEvents?: ToolEvent[]
  /** R1（2026-09-21）：本条 assistant 消息对应的中间轮正文（多步任务的
   *  每轮过程叙述，AgentEvent::RoundText 通道实时收集，回复落地时 flush
   *  挂载并折叠；watchdog 重灌后由环回放重建）。 */
  roundTexts?: RoundTextEntry[]
  /** R1：折叠条展开态（undefined = 折叠）。挂载后的 UI 状态，不参与同步。 */
  roundTextsOpen?: boolean
  /** M6（devtool-upgrade 阶段 7）：本条消息在后端 chat_log jsonl 里的行号
   *  （E3 rewind 的 message_index）。只有 user/assistant 行有值——error/
   *  system 消息是纯前端渲染，后端无对应行。watchdog replaceMessages 重建
   *  后不可信，置空诚实降级（菜单入口随之隐藏）。 */
  rowIndex?: number
  /** A1（2026-09-22 聊天切会话竞态）：本条消息在后端 chat_event_log 环里
   *  的会话内单调 seq。只有实时 receive 帧 / chat.sync 补拉路径产生
   *  （历史快照行不带 seq）。用于历史响应 last_seq 到达后剔除「先于
   *  快照渲染的重复 assistant 帧」。 */
  seq?: number
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

/** R1：单段中间轮正文（AgentEvent::RoundText 的 content，完整不截断）。 */
export interface RoundTextEntry {
  content: string
}

export const useChatStore = defineStore('chat', () => {
  const messages = ref<ChatMessage[]>([])
  const input = ref('')
  // D-3（2026-09-23 多会话并行清账）：发送占用态**按会话隔离**的 busy 表。
  // 旧实现是全局布尔 `streaming`——工作流「对话生成」等嵌入面板与主聊天页
  // 共用一个布尔，异会话 turn 的 busy 泄漏进本视图（发送被静默吞掉、
  // 占位误渲染）；「保存工作流后对话仍绑【新建工作流】」的跨会话串扰把
  // 这条放大成日常路径（BUG 2026-09-23_workflow-agentgen-session-crosstalk）。
  // 真相源 = busyBySid[sid]；`streaming` 是「当前选中会话」的投影（writable
  // computed，读写都落到表里），存量调用方/测试零改动。
  const busyBySid = ref<Record<string, boolean>>({})

  function isBusy(sessionId: string | null): boolean {
    return !!sessionId && busyBySid.value[sessionId] === true
  }

  function setBusy(sessionId: string | null, v: boolean) {
    if (!sessionId) return
    busyBySid.value = { ...busyBySid.value, [sessionId]: v }
  }

  const streaming = computed({
    get() {
      return isBusy(useSessionStore().currentId)
    },
    set(v: boolean) {
      setBusy(useSessionStore().currentId, v)
    },
  })
  // H2：当前会话的 todo 清单（TodoPanel 渲染；会话切换时 reset 清空）。
  const todos = ref<TodoItem[]>([])
  // M1b：进行中轮次的工具事件缓冲（响应落地时 flush 挂到 assistant 消息；
  // 会话切换 / watchdog 重同步时清空，防误挂到下一轮）。
  const pendingToolEvents = ref<ToolEvent[]>([])
  // R1：进行中轮次的中间正文缓冲（模型每轮过程叙述；回复落地时 flush
  // 折叠挂载；reset 清空——语义同 pendingToolEvents）。
  const pendingRoundTexts = ref<RoundTextEntry[]>([])
  // F1（devtool-upgrade 阶段 4）：plan/build 工作模式徽标。'build' 是保守
  // 初值——真实值进会话时经 chat.get_mode 对齐；ModeChanged push（/plan
  // /build slash 或 chat.set_mode）实时刷新。注意后端模式是 loop 级全局态
  // （非 per-session），徽标只做呈现。
  const agentMode = ref<'build' | 'plan'>('build')

  // B2（2026-09-22 聊天切会话竞态）：在飞 turn 登记——**不被 reset() 清掉**
  // （跨会话切换存活）。发送时 set，assistant/error 帧 / sync 拉到回复 /
  // 死轮 / cancel 时 clear。切回会话后即便 chat_log 尚无本轮 user 行（B1
  // 未覆盖的极早切回）或尾部不是悬空 user，detectPendingTurn 也能凭此
  // 启动占位 + 轮询，消除「纯空视图无任何反馈」的空窗。
  const inflightTurns = ref<Record<string, { sentAt: number; content: string }>>({})

  function markInflightTurn(sessionId: string | null, content: string) {
    if (!sessionId) return
    inflightTurns.value = {
      ...inflightTurns.value,
      [sessionId]: { sentAt: Date.now(), content },
    }
  }

  function clearInflightTurn(sessionId: string | null) {
    if (!sessionId || !inflightTurns.value[sessionId]) return
    const next = { ...inflightTurns.value }
    delete next[sessionId]
    inflightTurns.value = next
  }

  function inflightTurnOf(sessionId: string | null) {
    return sessionId ? inflightTurns.value[sessionId] : undefined
  }

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
    pendingRoundTexts.value = []
    // M6：行号锚随消息列表重推——watchdog 路径（fresh 对象无行号）归 null
    // 诚实降级；resync 路径（带 oldest_index 行号）重推出正确锚。
    recomputeNextRowIndex()
  }

  // --- A1/A2（2026-09-22 聊天切会话竞态）：历史快照与实时帧的合并语义 ---
  // 切会话 reset() 清空视图制造了无防护窗口：窗口内到达的实时帧先入列
  // （无尾部可比），随后历史响应 prependHistory 无条件前置拼接 → 同一条
  // 回复渲染两次。以下两个方法在 prepend 前清洗「实时帧先到的尾巴」。

  /** A1：剔除「先于历史快照渲染的 assistant 实时帧」——seq ≤ maxSeq 的
   *  assistant 行必已含于快照（assistant 入环点在 chat_log 落盘**之后**，
   *  历史读取又晚于落盘）。返回剔除条数。user 行入环早于落盘，「seq ≤
   *  last_seq ⟹ 已落盘」不成立，不在此剔除（由 dropTailIfSame 同文兜底
   *  + sync 通道既有尾行比对覆盖）。 */
  function dropAssistantBelowSeq(maxSeq: number): number {
    if (maxSeq <= 0) return 0
    const before = messages.value.length
    messages.value = messages.value.filter(
      m => !(m.role === 'assistant' && m.seq !== undefined && m.seq <= maxSeq),
    )
    if (messages.value.length !== before) recomputeNextRowIndex()
    return before - messages.value.length
  }

  /** A2：尾行同文兜底——列表尾部消息与历史批次尾部同 role 且同文则丢弃
   *  尾部（last_seq 缺失的旧网关路径 / seq 采样与落盘的极窄缝隙）。与
   *  receive 分支 isDuplicateRecovery 同语义同取舍（用户真发两条同文会
   *  误伤一条——既有代码已接受该取舍）。 */
  function dropTailIfSame(role: string, content: string): boolean {
    const last = messages.value[messages.value.length - 1]
    if (last && last.role === role && last.content === content) {
      messages.value.pop()
      recomputeNextRowIndex()
      return true
    }
    return false
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

  /** R1：收集中间轮正文（到达序追加；RoundText 无幂等键——实时通道
   *  broadcast 不重发，补拉通道有 seq ≤ 游标去重，不会重复入列）。 */
  function appendRoundText(content: string) {
    pendingRoundTexts.value.push({ content })
  }

  /** R1：取走 pending 中间正文（assistant 消息落地时折叠挂载到该消息）。 */
  function flushPendingRoundTexts(): RoundTextEntry[] {
    const out = pendingRoundTexts.value
    pendingRoundTexts.value = []
    return out
  }

  function clearInput() {
    input.value = ''
  }

  /**
   * Reset all conversation state. Used when ChatPanel mounts under a
   * non-default module (e.g., workflow_chat) so messages from a previous
   * chat session don't bleed into the new context.
   *
   * B2：**刻意不清 inflightTurns**——那是跨会话的在飞 turn 登记（发送后
   * 切走再切回要靠它恢复占位/轮询），清了等于白登记。生命周期归
   * markInflightTurn/clearInflightTurn 显式管理。
   */
  function reset() {
    messages.value = []
    input.value = ''
    // D-3：reset **不清 busy 表**——调用点都在 currentId 已切换之后，此时
    // 清 `streaming` 投影会误清新会话的占用态；busy 的生命周期归各会话自己
    // 的 assistant/error/断连帧收尾（setBusy(sid,false)），与 inflightTurns
    // 同一条纪律。
    todos.value = []
    pendingToolEvents.value = []
    pendingRoundTexts.value = []
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
    pendingRoundTexts,
    agentMode,
    commandDraft,
    inflightTurns,
    addMessage,
    prependHistory,
    replaceMessages,
    dropAssistantBelowSeq,
    dropTailIfSame,
    markInflightTurn,
    clearInflightTurn,
    inflightTurnOf,
    isBusy,
    setBusy,
    appendToolEvent,
    flushPendingToolEvents,
    appendRoundText,
    flushPendingRoundTexts,
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
