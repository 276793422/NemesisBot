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
  /** N5（2026-10-13 机会项）：本 turn 的交付物清单（present_files 工具登记，
   *  历史行 deliverables 透传）。渲染交付物卡片（「该看什么」产物指引，
   *  与 file_changes 变更映射并列不重叠）。缺省 = 无交付物/旧条目。 */
  deliverables?: DeliverableFile[]
  /** Work 时间线（2026-10-30 ChatPanel 长任务时间线改造）：本条 assistant
   *  消息对应的「中间轮叙述+工具调用」交错时间线。历史行 `"work"` 键透传
   *  （已完成任务）/ 实时摄取 flush 挂载（live 落地）。直答与旧条目缺省。 */
  work?: WorkTimeline
  /** M6（devtool-upgrade 阶段 7）：本条消息在后端 chat_log jsonl 里的行号
   *  （E3 rewind 的 message_index）。后端行经历史/重同步批次装载时编号——
   *  user/assistant 之外，后端也会落盘 system 行（busy 排队回执/compact
   *  注记，BUG-20261106-037），同样占一行。纯前端渲染的 error/system 消息
   *  无对应行，不带行号。watchdog replaceMessages 重建后不可信，置空诚实
   *  降级（菜单入口随之隐藏）。 */
  rowIndex?: number
  /** A1（2026-09-22 聊天切会话竞态）：本条消息在后端 chat_event_log 环里
   *  的会话内单调 seq。只有实时 receive 帧 / chat.sync 补拉路径产生
   *  （历史快照行不带 seq）。用于历史响应 last_seq 到达后剔除「先于
   *  快照渲染的重复 assistant 帧」。 */
  seq?: number
}

/** N5：单条交付物（与后端 chat_log PresentedFile serde 字段对齐）。 */
export interface DeliverableFile {
  path: string
  title: string
  note: string
}

/** H1/H2（2026-09-05）：单条 todo（与后端 TodoItem serde snake_case 对齐）。 */
export interface TodoItem {
  content: string
  status: 'pending' | 'in_progress' | 'completed'
}

/** 2026-10-30 Work 时间线类型族：与后端 nemesis-agent work_timeline.rs 的
 *  serde 形态**逐字段镜像（snake_case 原样）**，不做 camel 转换——嵌套结构
 *  三路入口（历史行 work 键 / chat.work_current 活查询 / 实时事件摄取）
 *  统一原样透传，避免每路各写一份转换漂移。形状单一真相源 = 后端
 *  WorkToolRecord/WorkRound/WorkTimeline serde。 */
export interface WorkToolEntry {
  tool: string
  call_id: string
  /** 成败。running=true 时占位 false 不参考。 */
  ok: boolean
  duration_ms: number
  args_preview?: string
  result_preview?: string
  /** 仍在执行（Started 已收 Finished 未收）。落盘形态省略。 */
  running?: boolean
  /** 执行中输出尾窗（2026-11-08 ToolOutput 实时流；仅 running 条目携带，
   *  Finished 时清除——终态以 result_preview 为准，避免双文本打架）。 */
  live_tail?: string
  /** 节流窗累计输出字节（与 live_tail 同源）。 */
  live_total_bytes?: number
}

export interface WorkRoundEntry {
  /** 模型该轮过程叙述。空串 = 纯工具轮 / thinking-only 轮（叙述空）。 */
  text: string
  /** 该轮思考链（后端 reasoning_content 归一形态；空/旧数据省略）。 */
  reasoning?: string
  tools: WorkToolEntry[]
}

export interface WorkTimeline {
  started_at_ms: number
  ended_at_ms: number
  rounds: WorkRoundEntry[]
  /** 轮数超 MAX_WORK_ROUNDS 被截断。false 时字段省略。 */
  truncated?: boolean
}

export const useChatStore = defineStore('chat', () => {
  const messages = ref<ChatMessage[]>([])
  const input = ref('')
  // 皮肤结构引擎 fill-input 动作的聚焦信号（v2）：场景 chip 预填后
  // ChatPanel watch 此 nonce 聚焦输入框（皮肤包无 JS，聚焦只能宿主驱动）。
  const focusInputNonce = ref(0)
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
  // 2026-10-30 Work 时间线（ChatPanel 长任务时间线改造）：进行中 turn 的
  // 活时间线单一缓冲——取代旧 pendingToolEvents + pendingRoundTexts 两个
  // 平行缓冲。「叙述+工具按轮交错」的组织责任从渲染层移进摄取层（与后端
  // WorkRecorder 同一组分组规则），实时 push / chat.sync 补拉 /
  // chat.work_current 活查询三路共用同一摄取与同一 flush。会话过滤在
  // ChatPanel 帧入口完成（异会话帧不入缓冲，同旧纪律）；reset 清空。
  const pendingWork = ref<WorkTimeline | null>(null)
  // R575（BUG-20261106-005）：RoundText 幂等集。环回放（primeSeqBaseline /
  // watchdog 重灌的 replayToolsFromRing）与实时 WS 帧在 sync 在飞窗口内会
  // 双通道投递同一事件，RoundText 无 call_id 类天然幂等键（Started/Finished
  // 有），重复摄取 = 时间线轮重复渲染。seq 会话内全局单调、三路投递一致 →
  // 以 seq 去重（无 seq 的旧网关帧不去重，保持旧行为）。生命周期与
  // pendingWork 缓冲严格同步——四处整缓冲清空点（flush / adopt /
  // replaceMessages / reset）同步 clear：换会话必须清（seq 按会话单调，
  // 不清会拿旧会话的大 seq 误抑新会话的轮），adopt 必须清（快照轮来自
  // 后端活累加器、与环 seq 无映射，接管后重新放行）。
  const roundTextSeqs = new Set<number>()
  // B3（2026-11-09）流式思考增量：进行中 LLM 调用的思考实时缓冲（provider
  // 流 think_start/think_delta/think_end 相位帧——ephemeral，不参与断线补拉
  // 环，断线窗口内的思考只能靠轮发布后的 rounds 回看）。live 时间线内渲染为
  // 增长中的思考卡（边想边长出来）；轮发布（RoundText 落地，含终答轮
  // `("", reasoning)` 形态）即清——同一思考已随轮进入时间线（轮内折叠卡），
  // live 卡让位，不双份展示。上限 8000 字符保留尾部（防超长思考撑爆响应式
  // 缓冲；完整全文随轮落地后可见，live 卡只是增长中的过程视图）。清空点与
  // pendingWork 同族：ingestWorkRoundText / flushPendingWork /
  // adoptWorkSnapshot / replaceMessages / reset。
  const liveThinking = ref('')
  const LIVE_THINKING_CAP = 8000
  function ingestLiveThinkingDelta(text: string) {
    if (!text) return
    // 首个思考片段顺带建 live 时间线槽（思考-only 轮此前无任何块可挂）——
    // 空轮 flush 时按 rounds.length 判空不挂载，语义与工具事件建槽同构。
    ensurePendingWork()
    let s = liveThinking.value + text
    if (s.length > LIVE_THINKING_CAP) s = s.slice(s.length - LIVE_THINKING_CAP)
    liveThinking.value = s
  }
  function clearLiveThinking() {
    if (liveThinking.value) liveThinking.value = ''
  }
  // N5（2026-10-13 机会项）：进行中轮次的交付物缓冲（present_files 登记
  // 事件先于 assistant 回复帧到达——落盘后发布、回复帧最后发布，顺序由
  // 后端单 turn 串行保证；回复落地时 flush 挂载；reset 清空——语义同
  // pendingToolEvents）。ChatPanel 侧按 session_id 过滤后入列（异会话
  // 的交付物不入缓冲，历史映射兜底）。
  const pendingDeliverables = ref<DeliverableFile[]>([])
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

  // W3（2026-10-14 审计）：按会话清账。busy 与在飞 turn 登记都刻意不被
  // reset() 清、靠各会话自己的收尾帧（assistant/error/sync/stop/cancel）
  // 清除——会话被删除/清空或登出后永无收尾帧，条目就成了永久泄漏
  // （inflightTurns 还会让会话 id 复用时出幽灵占位）。调用点：session
  // remove/clear、auth logout（dropAllSessionState）、ChatPanel watchdog
  // 放弃分支。
  // 会话级输入草稿表（2026-11-07 充分测试战役 webkit 全量竞速根修）。
  // reset() 是后台恢复路径（SSE resync / gap 全量重载 / 路由重挂载补偿）与
  // 切会话共用的状态重建入口——后台路径不该抹用户正在输入的草稿（多标签页
  // 另一端同会话写入 → chat.activity 防抖全刷 → 草稿被清，实录 webkit 全量
  // 4 例「fill 后发送钮恒 disabled」）。因此 reset() 不再碰 input；input 的
  // 生命周期改为显式管理：切会话走 carryInput（旧会话草稿入表、新会话草稿
  // 出表），发送链走 clearInput（既有），模块边界（workflow_chat 装配）显式
  // clearInput，会话删除随 dropSessionState 清表项。
  const draftBySid = new Map<string, string>()

  function carryInput(oldSid: string | null | undefined, newSid: string | null | undefined) {
    if (oldSid) {
      draftBySid.set(oldSid, input.value)
      input.value = (newSid && draftBySid.get(newSid)) || ''
    } else if (newSid && draftBySid.has(newSid)) {
      // oldSid=null（bootstrap 初始选定）：用户在无会话态已打的草稿不属于
      // 任何会话，保留现值；仅当目标会话恰有历史存稿时恢复之。旧语义
      // （无条件空白起步）在 ChatView 初始 switchTo 晚于用户输入的竞速窗
      // 口里把草稿洗掉 → 发送钮恒 disabled（2026-11-07 充分测试战役
      // webkit 全量实录，chat.spec fill→click 10s 超时 + 截图输入框空）。
      input.value = draftBySid.get(newSid) ?? ''
    }
  }

  function dropSessionState(sessionId: string | null) {
    if (!sessionId) return
    draftBySid.delete(sessionId)
    if (busyBySid.value[sessionId] !== undefined) {
      const next = { ...busyBySid.value }
      delete next[sessionId]
      busyBySid.value = next
    }
    clearInflightTurn(sessionId)
  }

  /** 新建会话落地前的在建草稿归属（2026-11-07 FF chat e2e 全量实录）：
   *  sessions.create 往返窗口内用户输入的文本意图发进新会话——session
   *  store 在 create 落地 switchTo 前登记进草稿表，carryInput 原样恢复
   *  （不登记则被当旧会话草稿洗掉 → 发送钮恒 disabled）。 */
  function stageDraft(sid: string, text: string) {
    draftBySid.set(sid, text)
  }

  /** W3：登出等「一次清掉所有会话」场景的全量版。 */
  function dropAllSessionState() {
    busyBySid.value = {}
    inflightTurns.value = {}
    draftBySid.clear()
  }

  // M6（devtool-upgrade 阶段 7）：下一行 live 消息的后端 chat_log 行号锚。
  // null = 尚无锚（历史未加载），此时 live 消息不编号（诚实降级）。派生式
  // 见 recomputeNextRowIndex——占行判定按「是否对应后端 jsonl 行」：
  // user/assistant 恒占行，后端落盘的 system 行（排队回执/compact 注记）
  // 也占行，纯前端渲染的 error/system 不占行（BUG-20261106-037）。
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
    // M6：live user/assistant 行按 nextRowIndex 顺序编号（客户端 error/
    // system 是纯前端渲染不占行——后端落盘的 system 行只随历史/重同步批
    // 次进视图，锚重推时已在 recomputeNextRowIndex 计入）。无锚（历史未
    // 加载）不编号，诚实降级。
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

  /** M6：重推 nextRowIndex 锚 = 首条带行号消息的行号 + 其后**占行**消息
   *  总数。占行判定（BUG-20261106-037）：带 rowIndex 的行 = 后端 jsonl 行
   *  （含 system——排队回执/compact 注记是后端落盘的合法 system 行，各占
   *  一行）；不带 rowIndex 的 user/assistant = watchdog 重灌的 fresh 对象
   *  /本地回显（占行但行号未知）；不带 rowIndex 的 error/system = 纯前端
   *  渲染（不占行）。无任何行号锚 → null。 */
  function recomputeNextRowIndex() {
    nextRowIndex = null
    let rows = 0
    let anchor: number | null = null
    for (const m of messages.value) {
      const occupies =
        m.rowIndex !== undefined || m.role === 'user' || m.role === 'assistant'
      if (!occupies) continue
      if (anchor === null && m.rowIndex !== undefined) anchor = m.rowIndex
      rows++
    }
    if (anchor !== null) nextRowIndex = anchor + rows
  }

  /** M6-fix（2026-10-21）：发送链刚 create 的全新会话建立行号锚。watcher 的
   *  justCreatedSid 分支有意跳过 reset+loadHistory（服务端 jsonl 在首条消息
   *  落盘前必为空，且 reset 会毁掉尚未落盘的本地回显）——代价是锚永远停在
   *  null，本地回显与后续 live 行全部不编号，回退/重新生成按钮组
   *  （rewindActions 要求 rowIndex）在该会话首轮恒不渲染。此函数由该分支
   *  调用：新会话服务端历史为空是事实，视图内 user/assistant 行就是 jsonl
   *  的第 0..n-1 行——按序回填编号并把锚推到行尾。error/system 行不占行，
   *  与 addMessage/prependHistory 语义一致。 */
  function anchorFreshSession() {
    let n = 0
    for (const m of messages.value) {
      if (m.role !== 'user' && m.role !== 'assistant') continue
      m.rowIndex = n++
    }
    nextRowIndex = n
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
    // 2026-10-30：重同步后进行中时间线不可信，清空防止误挂到下一轮
    //（进行中内容由 work_current 活查询 + 环回放重建）。
    pendingWork.value = null
    roundTextSeqs.clear()
    // B3：重同步后 live 思考不可信（同 pendingWork——进行中内容由
    // work_current 活查询 + 环回放重建）。
    clearLiveThinking()
    pendingDeliverables.value = []
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

  // --- 2026-10-30 Work 时间线摄取（三路共用：实时 push / chat.sync 补拉 /
  // chat.work_current 活查询）---

  /** 确保缓冲存在（惰性初始化——直答零开销，首个中间事件才建槽）。 */
  function ensurePendingWork(): WorkTimeline {
    if (!pendingWork.value) {
      pendingWork.value = { started_at_ms: Date.now(), ended_at_ms: Date.now(), rounds: [] }
    }
    return pendingWork.value
  }

  /** 摄取中间轮输出（RoundText：叙述 + 思考链，2026-10-31 批次二）。
   *  一条输出开一轮——其后的工具批次落该轮（与后端 emit_round_output
   *  发布序同构：叙述/思考 → Started/Finished×k）。皆空不建轮。
   *  R575（BUG-20261106-005）：带 seq 时按幂等集去重——环回放与实时帧
   *  双通道投递同一事件只成一轮；无 seq（旧网关）不去重。 */
  function ingestWorkRoundText(content: string, reasoning?: string, seq?: number) {
    // B3：轮发布即思考卡的让位信号——流式思考已随轮进入时间线（无论本帧
    // 是否被 seq 去重抑制，live 缓冲都该清；重复帧到来时 live 早已清空）。
    clearLiveThinking()
    if (!content && !reasoning) return
    if (typeof seq === 'number') {
      if (roundTextSeqs.has(seq)) return
      roundTextSeqs.add(seq)
    }
    const w = ensurePendingWork()
    w.rounds.push({ text: content, reasoning: reasoning || undefined, tools: [] })
    w.ended_at_ms = Date.now()
  }

  /** 摄取工具开始。分组规则与后端 ensure_tool_round 同构：末轮有叙述或
   *  思考且尚无工具 = 本批次的输出轮；否则开纯工具轮（皆空轮不发输出，
   *  其批次不得误挂上一轮）。同 call_id 幂等（重复 Started 只刷新 args）。 */
  function ingestWorkToolStart(callId: string, tool: string, argsPreview?: string) {
    const w = ensurePendingWork()
    for (const r of w.rounds) {
      const t = r.tools.find(t => t.call_id === callId)
      if (t) {
        t.args_preview = argsPreview || undefined
        return
      }
    }
    const last = w.rounds[w.rounds.length - 1]
    let round =
      last && (last.text || last.reasoning) && last.tools.length === 0 ? last : null
    if (!round) {
      round = { text: '', tools: [] }
      w.rounds.push(round)
    }
    round.tools.push({
      tool,
      call_id: callId,
      ok: false,
      duration_ms: 0,
      args_preview: argsPreview || undefined,
      running: true,
    })
    w.ended_at_ms = Date.now()
  }

  /** 摄取工具结束。倒序找 call_id 原地更新终态（与后端 tool_finished 同
   *  构）；无匹配 Started（截断/异常）诚实忽略。 */
  function ingestWorkToolFinish(callId: string, ok: boolean, durationMs: number, resultPreview?: string) {
    const w = pendingWork.value
    if (!w) return
    for (let i = w.rounds.length - 1; i >= 0; i--) {
      const t = w.rounds[i].tools.find(t => t.call_id === callId)
      if (t) {
        t.ok = ok
        t.duration_ms = durationMs
        t.result_preview = resultPreview || undefined
        delete t.running
        delete t.live_tail
        delete t.live_total_bytes
        w.ended_at_ms = Date.now()
        return
      }
    }
  }

  /** 摄取执行中输出尾窗（2026-11-08 ToolOutput：exec 流式尾窗实时推送，
   *  替代「sleep+background_output 轮询」旧玩法）。call_id 置空（工具层
   *  不可见 dispatch id）→ 倒序找**最后一个 running 条目**挂载（并行 exec
   *  归属最新启动行，与旧 read-后台-输出语义一致）。无 running 条目诚实
   *  忽略（事件早于 Started/晚于 Finished 的窗口缝隙）。 */
  function ingestWorkToolOutput(tail: string, totalBytes: number) {
    const w = pendingWork.value
    if (!w) return
    for (let i = w.rounds.length - 1; i >= 0; i--) {
      const tools = w.rounds[i].tools
      for (let j = tools.length - 1; j >= 0; j--) {
        const t = tools[j]
        if (t.running) {
          t.live_tail = tail
          t.live_total_bytes = totalBytes
          w.ended_at_ms = Date.now()
          return
        }
      }
    }
  }

  /** 取走 pending 时间线（assistant 回复落地时挂载到该消息）。无中间轮
   *  （直答 / 已取走）= undefined，消息不带 work 字段。 */
  function flushPendingWork(): WorkTimeline | undefined {
    const w = pendingWork.value
    pendingWork.value = null
    roundTextSeqs.clear()
    // B3：回复落地 = 本 turn 流式面收尾（异常路径 error 后无 RoundText 时
    // live 思考卡在此兜底清空，正常路径已被轮发布清过——幂等）。
    clearLiveThinking()
    return w && w.rounds.length ? w : undefined
  }

  /** chat.work_current 活查询回包：整体接管 pending 时间线（切走再切回的
   *  全量恢复——后端活累加器无环滑窗限制）。work 为 null / 无轮 = 清空
   *  现值（无进行中 turn 是事实）；调用方负责迟到快照的作废闸。 */
  function adoptWorkSnapshot(work: WorkTimeline | null | undefined) {
    pendingWork.value = work && work.rounds.length ? work : null
    // R575：快照轮来自后端活累加器，与环 seq 无映射——接管后幂等集必须
    // 重新放行（不清会拿旧集合误抑后续合法轮）。
    roundTextSeqs.clear()
    // B3：快照不含 live 思考（sink 缓冲在 WorkRecorder 之外）——切走再切回
    // 后旧 live 卡不可信，清空；流仍在跑则下一 think_delta 重建。
    clearLiveThinking()
  }

  /** N5：收集交付物（`deliverables` push 帧，ChatPanel 按 session 过滤后
   *  入列；登记事件单 turn 至多一次、无幂等键，重复推送不存在）。 */
  function appendPendingDeliverables(files: DeliverableFile[]) {
    pendingDeliverables.value.push(...files)
  }

  /** N5：取走 pending 交付物（assistant 消息落地时挂载到该消息）。 */
  function flushPendingDeliverables(): DeliverableFile[] {
    const out = pendingDeliverables.value
    pendingDeliverables.value = []
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
    // input 不在此清（见 draftBySid 注释）：后台全量重建路径不得抹用户草稿，
    // 清空职责归 carryInput（切会话）/clearInput（发送链、模块边界）。
    // D-3：reset **不清 busy 表**——调用点都在 currentId 已切换之后，此时
    // 清 `streaming` 投影会误清新会话的占用态；busy 的生命周期归各会话自己
    // 的 assistant/error/断连帧收尾（setBusy(sid,false)），与 inflightTurns
    // 同一条纪律。
    todos.value = []
    pendingWork.value = null
    roundTextSeqs.clear()
    // B3：会话切换丢掉上一会话的 live 思考（与 pendingWork 同生命周期）。
    clearLiveThinking()
    pendingDeliverables.value = []
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
    focusInputNonce,
    streaming,
    historyLoading,
    hasMoreHistory,
    oldestIndex,
    historyLoaded,
    todos,
    pendingWork,
    pendingDeliverables,
    liveThinking,
    agentMode,
    commandDraft,
    inflightTurns,
    addMessage,
    prependHistory,
    anchorFreshSession,
    stageDraft,
    replaceMessages,
    dropAssistantBelowSeq,
    dropTailIfSame,
    markInflightTurn,
    clearInflightTurn,
    inflightTurnOf,
    dropSessionState,
    dropAllSessionState,
    isBusy,
    setBusy,
    ingestWorkRoundText,
    ingestWorkToolStart,
    ingestWorkToolFinish,
    /** B3：流式思考增量摄取 + 清空（ChatPanel think_* 相位帧入口）。 */
    ingestLiveThinkingDelta,
    clearLiveThinking,
    ingestWorkToolOutput,
    flushPendingWork,
    adoptWorkSnapshot,
    appendPendingDeliverables,
    flushPendingDeliverables,
    clearInput,
    carryInput,
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
