import { describe, it, expect, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { useChatStore, type TodoItem } from '../chat'

// H1/H2（2026-09-05）：todo 清单状态——TodoPanel 的数据源。
// setTodos 全量替换（TodoUpdated 载荷内联）+ reset 随会话切换清空。

const sample: TodoItem[] = [
  { content: 'task a', status: 'completed' },
  { content: 'task b', status: 'in_progress' },
  { content: 'task c', status: 'pending' },
]

beforeEach(() => {
  setActivePinia(createPinia())
})

describe('chat store todos', () => {
  it('starts empty', () => {
    const store = useChatStore()
    expect(store.todos).toEqual([])
  })

  it('setTodos replaces the whole list', () => {
    const store = useChatStore()
    store.setTodos(sample)
    expect(store.todos).toHaveLength(3)
    store.setTodos([{ content: 'only one', status: 'completed' }])
    expect(store.todos).toHaveLength(1)
    expect(store.todos[0].content).toBe('only one')
  })

  it('setTodos copies (external mutation does not leak in)', () => {
    const store = useChatStore()
    const list: TodoItem[] = [{ content: 'a', status: 'pending' }]
    store.setTodos(list)
    list.push({ content: 'b', status: 'completed' })
    expect(store.todos).toHaveLength(1)
  })

  it('reset clears todos', () => {
    const store = useChatStore()
    store.setTodos(sample)
    store.reset()
    expect(store.todos).toEqual([])
  })
})

// 2026-10-30（ChatPanel 长任务时间线改造）：work 时间线单一缓冲——摄取
// （ingestWork*，分组规则与后端 WorkRecorder 同构）、flush 取走挂载、
// adoptWorkSnapshot 活查询接管、reset/replaceMessages 清 pending。

import type { WorkTimeline } from '../chat'

function snap(rounds: WorkTimeline['rounds']): WorkTimeline {
  return { started_at_ms: 1, ended_at_ms: 2, rounds }
}

describe('chat store work timeline (2026-10-30)', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
  })

  it('starts null (直答零开销)', () => {
    const store = useChatStore()
    expect(store.pendingWork).toBeNull()
    expect(store.flushPendingWork()).toBeUndefined()
  })

  it('ingestWorkRoundText opens a round; tool batch carries the narrative round', () => {
    const store = useChatStore()
    store.ingestWorkRoundText('我先看下配置')
    store.ingestWorkToolStart('c1', 'exec', 'ls')
    store.ingestWorkToolFinish('c1', true, 42, 'ok')
    expect(store.pendingWork?.rounds).toHaveLength(1)
    expect(store.pendingWork?.rounds[0].text).toBe('我先看下配置')
    expect(store.pendingWork?.rounds[0].tools[0]).toMatchObject({
      call_id: 'c1',
      ok: true,
      duration_ms: 42,
      result_preview: 'ok',
    })
    expect(store.pendingWork?.rounds[0].tools[0].running).toBeUndefined()
  })

  it('tool without narrative starts a tools-only round (不误挂上一轮)', () => {
    const store = useChatStore()
    store.ingestWorkRoundText('第一轮有叙述')
    store.ingestWorkToolStart('c1', 'exec', '')
    store.ingestWorkToolFinish('c1', true, 1, '')
    // 第二轮空正文（不发叙述）→ 新纯工具轮。
    store.ingestWorkToolStart('c2', 'grep', 'pat')
    expect(store.pendingWork?.rounds).toHaveLength(2)
    expect(store.pendingWork?.rounds[1].text).toBe('')
    expect(store.pendingWork?.rounds[1].tools[0].tool).toBe('grep')
    expect(store.pendingWork?.rounds[1].tools[0].running).toBe(true)
  })

  it('Started→Finished merges in place by call_id; duplicate Started is idempotent', () => {
    const store = useChatStore()
    store.ingestWorkRoundText('t')
    store.ingestWorkToolStart('c1', 'exec', 'a=1')
    store.ingestWorkToolStart('c1', 'exec', 'a=2') // 重复 Started 只刷新 args
    store.ingestWorkToolFinish('c1', false, 5, 'err')
    store.ingestWorkToolFinish('c1', false, 5, 'err') // 重复 Finished 不新增
    const rounds = store.pendingWork?.rounds ?? []
    expect(rounds).toHaveLength(1)
    expect(rounds[0].tools).toHaveLength(1)
    expect(rounds[0].tools[0]).toMatchObject({ ok: false, duration_ms: 5, result_preview: 'err' })
  })

  // --- B3（2026-11-09）流式思考增量 ---
  it('liveThinking: delta 累积 + 首 delta 建 live 槽；flush 判空不挂载', () => {
    const store = useChatStore()
    expect(store.liveThinking).toBe('')
    store.ingestLiveThinkingDelta('想')
    store.ingestLiveThinkingDelta('第二步')
    expect(store.liveThinking).toBe('想第二步')
    // 首 delta 顺带建 live 时间线槽（思考-only 轮此前无块可挂）。
    expect(store.pendingWork).not.toBeNull()
    expect(store.pendingWork?.rounds).toHaveLength(0)
    // 纯思考直答 flush：零轮 = 不挂载 + live 清空。
    expect(store.flushPendingWork()).toBeUndefined()
    expect(store.liveThinking).toBe('')
    expect(store.pendingWork).toBeNull()
  })

  it('liveThinking: 轮发布（RoundText）即清——思考随轮进时间线不双份', () => {
    const store = useChatStore()
    store.ingestLiveThinkingDelta('流式思考')
    expect(store.liveThinking).toBe('流式思考')
    store.ingestWorkRoundText('叙述', '流式思考')
    expect(store.liveThinking).toBe('')
    expect(store.pendingWork?.rounds[0]).toMatchObject({ text: '叙述', reasoning: '流式思考' })
  })

  it('liveThinking: 上限 8000 字符保留尾部', () => {
    const store = useChatStore()
    store.ingestLiveThinkingDelta('a'.repeat(7990))
    store.ingestLiveThinkingDelta('b'.repeat(100))
    expect(store.liveThinking.length).toBeLessThanOrEqual(8000)
    expect(store.liveThinking.startsWith('a')).toBe(true)
    expect(store.liveThinking.endsWith('b'.repeat(100))).toBe(true)
  })

  it('liveThinking: think_start 语义（clear）+ reset 清空', () => {
    const store = useChatStore()
    store.ingestLiveThinkingDelta('旧流的思考')
    store.clearLiveThinking() // think_start：新流 reset
    expect(store.liveThinking).toBe('')
    store.ingestLiveThinkingDelta('新流')
    store.reset()
    expect(store.liveThinking).toBe('')
    expect(store.pendingWork).toBeNull()
  })

  // --- 插话窗正文流预览（2026-11-25 插话中间过程补全）---

  it('steerLiveText: 窗口旗标随注记开 / 随回应关，delta 仅窗口内摄取（调用方闸门约定）', () => {
    const store = useChatStore()
    // 窗口未开（无注记）——ChatPanel 侧按 steerWindowActive 闸门不调用
    // ingestSteerBodyDelta；store 侧的摄取与旗标解耦，此断言只锁旗标。
    expect(store.steerWindowActive).toBe(false)
    store.ingestSteerNote('⚡ 插话已注入本轮思考：补充边界')
    expect(store.steerWindowActive).toBe(true)
    store.ingestSteerBodyDelta('正在生成')
    store.ingestSteerBodyDelta('回应内容')
    expect(store.steerLiveText).toBe('正在生成回应内容')
    store.ingestSteerResponse('完整回应')
    // 回应到场 = 窗口关闸 + 预览让位（落地的回应段接棒，不双份）。
    expect(store.steerWindowActive).toBe(false)
    expect(store.steerLiveText).toBe('')
    const rounds = store.pendingWork?.rounds ?? []
    const noteRound = rounds.find(r => r.steer_note)
    expect(noteRound?.steer_response).toBe('完整回应')
  })

  it('steerLiveText: 换流重开（clearSteerLive）清旧流尾巴——预览恒对准当前流', () => {
    const store = useChatStore()
    store.ingestSteerNote('⚡ 插话已注入本轮思考：换思路')
    store.ingestSteerBodyDelta('叙述流残尾')
    store.clearSteerLive() // start 相位：新流 reset（与 think_start 同源语义）
    expect(store.steerLiveText).toBe('')
    store.ingestSteerBodyDelta('回应流')
    expect(store.steerLiveText).toBe('回应流')
    // 窗口未关——clearSteerLive 只清文本不动旗标。
    expect(store.steerWindowActive).toBe(true)
  })

  it('steerLiveText: RoundText 发布即清预览（叙述随轮进时间线）+ flush 收口关窗', () => {
    const store = useChatStore()
    store.ingestSteerNote('⚡ 插话已注入本轮思考：先查再答')
    store.ingestSteerBodyDelta('叙述流正文')
    store.ingestWorkRoundText('叙述', '思考')
    expect(store.steerLiveText).toBe('')
    expect(store.steerWindowActive).toBe(true)
    // turn 收尾 flush：整缓冲取走 + 窗口必关（漏关会吃下一 turn 主终答流）。
    const w = store.flushPendingWork()
    expect(w?.rounds.some(r => r.steer_note)).toBe(true)
    expect(store.steerWindowActive).toBe(false)
    expect(store.steerLiveText).toBe('')
  })

  it('steerLiveText: 上限 8000 字符保留尾部 + reset 跨会话不泄漏', () => {
    const store = useChatStore()
    store.ingestSteerNote('⚡ 插话已注入本轮思考：长文')
    store.ingestSteerBodyDelta('a'.repeat(7990))
    store.ingestSteerBodyDelta('b'.repeat(100))
    expect(store.steerLiveText.length).toBeLessThanOrEqual(8000)
    expect(store.steerLiveText.endsWith('b'.repeat(100))).toBe(true)
    // 换会话 reset：窗口旗标与预览一并清（旧窗不得吃新会话正文流）。
    store.reset()
    expect(store.steerWindowActive).toBe(false)
    expect(store.steerLiveText).toBe('')
  })

  it('Finished without Started is honestly ignored', () => {
    const store = useChatStore()
    store.ingestWorkRoundText('t')
    store.ingestWorkToolFinish('ghost', true, 5, 'r')
    expect(store.pendingWork?.rounds[0].tools).toHaveLength(0)
  })

  it('flushPendingWork takes the timeline and empties the buffer', () => {
    const store = useChatStore()
    store.ingestWorkRoundText('t')
    const w = store.flushPendingWork()
    expect(w?.rounds).toHaveLength(1)
    expect(store.pendingWork).toBeNull()
    expect(store.flushPendingWork()).toBeUndefined()
  })

  // --- 批次二（2026-10-31）：思考链摄取（reasoning 可选字段）---

  it('thinking-only round: empty content + reasoning opens a round (thinking-only 轮照建)', () => {
    const store = useChatStore()
    store.ingestWorkRoundText('', '先想清楚改哪里再动手')
    expect(store.pendingWork?.rounds).toHaveLength(1)
    expect(store.pendingWork?.rounds[0]).toMatchObject({
      text: '',
      reasoning: '先想清楚改哪里再动手',
      tools: [],
    })
  })

  it('both empty is ignored (皆空不建轮；旧网关无 reasoning 键同形态)', () => {
    const store = useChatStore()
    store.ingestWorkRoundText('', undefined)
    store.ingestWorkRoundText('', '')
    expect(store.pendingWork).toBeNull()
    // 有叙述的轮照常。
    store.ingestWorkRoundText('叙述', '')
    expect(store.pendingWork?.rounds[0].text).toBe('叙述')
    expect(store.pendingWork?.rounds[0].reasoning).toBeUndefined()
  })

  it('reasoning round carries the following tool batch (工具批次挂思考轮)', () => {
    const store = useChatStore()
    store.ingestWorkRoundText('', '推敲方案')
    store.ingestWorkToolStart('c1', 'exec', 'ls')
    store.ingestWorkToolFinish('c1', true, 5, 'out')
    expect(store.pendingWork?.rounds).toHaveLength(1)
    expect(store.pendingWork?.rounds[0].reasoning).toBe('推敲方案')
    expect(store.pendingWork?.rounds[0].tools[0]).toMatchObject({ call_id: 'c1', ok: true })
  })

  it('thinking round does not capture the next batch after tools exist (与叙述轮同规)', () => {
    const store = useChatStore()
    store.ingestWorkRoundText('', '第一轮思考')
    store.ingestWorkToolStart('c1', 'exec', '')
    store.ingestWorkToolFinish('c1', true, 1, '')
    // 第二轮纯工具（无输出）→ 新纯工具轮，不误挂思考轮。
    store.ingestWorkToolStart('c2', 'grep', 'pat')
    expect(store.pendingWork?.rounds).toHaveLength(2)
    expect(store.pendingWork?.rounds[0].tools.map(t => t.call_id)).toEqual(['c1'])
    expect(store.pendingWork?.rounds[1].tools.map(t => t.call_id)).toEqual(['c2'])
  })

  it('adoptWorkSnapshot replaces wholesale (work_current 活查询恢复)', () => {
    const store = useChatStore()
    store.adoptWorkSnapshot(snap([{ text: '恢复的叙述', tools: [] }]))
    expect(store.pendingWork?.rounds[0].text).toBe('恢复的叙述')
    // null / 空轮 = 清空（无进行中 turn 是事实）。
    store.adoptWorkSnapshot(null)
    expect(store.pendingWork).toBeNull()
    store.adoptWorkSnapshot(snap([]))
    expect(store.pendingWork).toBeNull()
  })

  // 2026-11-25 观看端闪屏修复：占位轮询周期拍的 adopt 若无条件清空 live
  // 流缓冲，思考卡/插话预览每拍被抹、下一个 delta 才重建 = 4s 周期闪。
  it('adoptWorkSnapshot preserveLiveStreams keeps live thinking & open steer window', () => {
    const store = useChatStore()
    store.ingestLiveThinkingDelta('正在推敲边界情况')
    store.ingestSteerNote('插话注记轮')
    // 快照注记轮未收口（无回应段）= 窗口事实上开着 → 不关。
    const steerWork = snap([{ text: '', tools: [] }])
    steerWork.rounds[0].steer_note = '插话注记轮'
    store.adoptWorkSnapshot(steerWork, { preserveLiveStreams: true })
    expect(store.liveThinking).toBe('正在推敲边界情况')
    expect(store.steerWindowActive).toBe(true)
    // 注记轮已收口（带回应段）= 窗口事实上结束 → 照常关闸；思考缓冲仍保留。
    store.ingestSteerNote('另一条插话')
    store.ingestSteerResponse('已处理插话')
    const closedWork = snap([{ text: '', tools: [] }])
    closedWork.rounds[0].steer_note = '另一条插话'
    closedWork.rounds[0].steer_response = '已处理插话'
    store.ingestLiveThinkingDelta('继续思考')
    store.adoptWorkSnapshot(closedWork, { preserveLiveStreams: true })
    // delta 追加语义：保留的缓冲含全部累积。
    expect(store.liveThinking).toBe('正在推敲边界情况继续思考')
    expect(store.steerWindowActive).toBe(false)
  })

  it('adoptWorkSnapshot without option clears live thinking & steer window (旧行为不变)', () => {
    const store = useChatStore()
    store.ingestLiveThinkingDelta('残尾')
    store.ingestSteerNote('注记')
    store.adoptWorkSnapshot(snap([{ text: '恢复的叙述', tools: [] }]))
    expect(store.liveThinking).toBe('')
    expect(store.steerWindowActive).toBe(false)
  })

  it('replaceMessages (watchdog resync) clears pending', () => {
    const store = useChatStore()
    store.ingestWorkRoundText('t')
    store.replaceMessages([{ role: 'assistant', content: 'recovered', timestamp: 't' }])
    expect(store.pendingWork).toBeNull()
  })

  it('reset clears pending', () => {
    const store = useChatStore()
    store.ingestWorkRoundText('t')
    store.reset()
    expect(store.pendingWork).toBeNull()
  })
})

// M6（2026-09-07）：行号推导——E3 rewind 菜单的 message_index 数据源。
// chat_log 与渲染消息按「占行」对应（BUG-20261106-037 修正语义；2026-11-11
// 三态化）：帧带 occupies 键以键为准（true=落盘占行含 system 注记；false=
// 瞬态回执不占行——⏳ busy/⚡ 插话/slash 确认/维护收据/直派收据）；缺省
// 走旧规则（user/assistant 占行，纯前端 error/system 不占行；历史批次里
// 带 rowIndex 的后端 system 行天然占行）。历史批次行号连续
// （oldest_index + j），live 行从锚顺序递增。

function msg(role: 'user' | 'assistant' | 'error' | 'system', content: string, rowIndex?: number) {
  return { role, content, timestamp: 't', ...(rowIndex !== undefined ? { rowIndex } : {}) }
}

describe('chat store rowIndex (M6)', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
  })

  it('prependHistory numbers the batch oldest_index + j', () => {
    const store = useChatStore()
    store.prependHistory(
      [msg('user', 'q1'), msg('assistant', 'a1'), msg('user', 'q2')],
      7,
    )
    expect(store.messages.map(m => m.rowIndex)).toEqual([7, 8, 9])
  })

  it('prependHistory without oldest_index leaves rows unnumbered (honest degradation)', () => {
    const store = useChatStore()
    store.prependHistory([msg('user', 'q1'), msg('assistant', 'a1')], null)
    expect(store.messages.map(m => m.rowIndex)).toEqual([undefined, undefined])
  })

  it('live user/assistant messages number from the anchor; error/system skip', () => {
    const store = useChatStore()
    store.prependHistory([msg('user', 'q1'), msg('assistant', 'a1')], 0)
    // 历史占 0/1 两行 → live 从 2 开始。
    store.addMessage(msg('error', 'boom'))
    store.addMessage(msg('system', 'note'))
    store.addMessage(msg('user', 'q2'))
    store.addMessage(msg('assistant', 'a2'))
    expect(store.messages[2].rowIndex).toBeUndefined()
    expect(store.messages[3].rowIndex).toBeUndefined()
    expect(store.messages[4].rowIndex).toBe(2)
    expect(store.messages[5].rowIndex).toBe(3)
  })

  it('backend system row in history occupies a raw row (BUG-20261106-037)', () => {
    const store = useChatStore()
    // busy 排队回执/compact 注记是后端落盘的 system 行、占 raw 行——历史
    // 批次编号 0..4（含 system@2），live 锚必须推到 5。修复前锚只数
    // user/assistant（=4）→ live 行标签 4/5 与历史 a2@4 撞号，rewind 按
    // 标签切会把历史轮误删（或被后端「不是 user 行」拒绝）。
    store.prependHistory(
      [
        msg('user', 'q1'),
        msg('assistant', 'a1'),
        msg('system', '排队回执'),
        msg('user', 'q2'),
        msg('assistant', 'a2'),
      ],
      0,
    )
    expect(store.messages.map(m => m.rowIndex)).toEqual([0, 1, 2, 3, 4])
    store.addMessage(msg('user', 'q3'))
    store.addMessage(msg('assistant', 'a3'))
    expect(store.messages[5].rowIndex).toBe(5)
    expect(store.messages[6].rowIndex).toBe(6)
  })

  it('resync batch with backend system row keeps live numbering aligned', () => {
    const store = useChatStore()
    store.replaceMessages([
      msg('user', 'q1', 0),
      msg('assistant', 'a1', 1),
      msg('system', 'compact 注记', 2),
    ])
    store.addMessage(msg('user', 'q2'))
    expect(store.messages[3].rowIndex).toBe(3)
  })

  it('load-older batch re-derives the anchor (older rows prepended, live continues)', () => {
    const store = useChatStore()
    store.prependHistory([msg('user', 'q2'), msg('assistant', 'a2')], 2)
    store.addMessage(msg('user', 'q3')) // 行号 4
    // 更早的批次到达：q1/a1 占 0/1，整列行号应为 0..4 连续。
    store.prependHistory([msg('user', 'q1'), msg('assistant', 'a1')], 0)
    expect(store.messages.map(m => m.rowIndex)).toEqual([0, 1, 2, 3, 4])
  })

  it('replaceMessages re-derives the anchor (resync path keeps rows; watchdog path degrades)', () => {
    const store = useChatStore()
    // resync 路径：带行号的 fresh 列表 → 锚重推正确，live 接着编号。
    store.replaceMessages([msg('user', 'q1', 0), msg('assistant', 'a1', 1)])
    store.addMessage(msg('user', 'q2'))
    expect(store.messages[2].rowIndex).toBe(2)

    // watchdog 路径：fresh 对象无行号 → 锚归 null，live 不编号（诚实降级）。
    store.replaceMessages([msg('user', 'q1'), msg('assistant', 'a1')])
    store.addMessage(msg('user', 'q3'))
    expect(store.messages[2].rowIndex).toBeUndefined()
  })

  it('reset drops the anchor (no cross-session bleed)', () => {
    const store = useChatStore()
    store.prependHistory([msg('user', 'q1')], 40)
    store.reset()
    store.addMessage(msg('user', 'new session'))
    expect(store.messages[0].rowIndex).toBeUndefined()
  })

  it('reset clears commandDraft (M6 palette draft delivery)', () => {
    const store = useChatStore()
    store.commandDraft = '/compact '
    store.reset()
    expect(store.commandDraft).toBe('')
  })
})

// 2026-11-11 行号锚漂移根修：occupies 三态。修复前 live 路径按 role
// 二值计数——瞬态 gate 回执（⏳ busy 排队/⚡ 插话/slash 确认，role=
// assistant 但不落盘）被误计锚超前 +1；落盘 system 注记（压缩完成/
// snooze 恢复行，role=system）live 到达不被计数锚落后 −1。rewind/
// regenerate 的 message_index 双向漂移 → 删错行。后端 gate 盖章 +
// 帧键 occupies 全链透传，前端三态计数。
describe('chat store rowIndex (occupies 三态, 2026-11-11)', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
  })

  function frame(
    role: 'user' | 'assistant' | 'error' | 'system',
    content: string,
    occupies: boolean | undefined,
  ) {
    return { role, content, timestamp: 't', ...(occupies !== undefined ? { occupies } : {}) }
  }

  it('transient receipt (occupies=false, role=assistant) skips numbering — anchor stays exact', () => {
    const store = useChatStore()
    store.prependHistory([msg('user', 'q1'), msg('assistant', 'a1')], 0)
    // ⏳ busy 排队回执：role=assistant 但瞬态不落盘。修复前被计锚 →
    // 下一条 live user 拿 3（实际 jsonl 下一行是 2）→ rewind 删错行。
    store.addMessage(frame('assistant', '⏳ 正在处理上一条…', false))
    expect(store.messages[2].rowIndex).toBeUndefined()
    store.addMessage(msg('user', 'q2'))
    store.addMessage(msg('assistant', 'a2'))
    expect(store.messages[3].rowIndex).toBe(2)
    expect(store.messages[4].rowIndex).toBe(3)
  })

  it('persisted system note (occupies=true) occupies a live row', () => {
    const store = useChatStore()
    store.prependHistory([msg('user', 'q1'), msg('assistant', 'a1')], 0)
    // 压缩完成注记/snooze 恢复行：role=system 但后端落盘占行。修复前
    // live 到达不被计数 → 锚落后 1，后续行号全体 −1 撞已落盘行。
    store.addMessage(frame('system', '[自动压缩] 已折叠…', true))
    expect(store.messages[2].rowIndex).toBe(2)
    store.addMessage(msg('user', 'q2'))
    expect(store.messages[3].rowIndex).toBe(3)
  })

  it('mixed receipts + notes: live numbering tracks jsonl rows exactly', () => {
    const store = useChatStore()
    store.prependHistory([msg('user', 'q1'), msg('assistant', 'a1')], 0)
    store.addMessage(frame('assistant', '⏳ 正在处理上一条…', false)) // 不落盘
    store.addMessage(frame('assistant', '/help', false)) // slash 静态确认，不落盘
    store.addMessage(msg('user', 'q2')) // jsonl 行 2
    store.addMessage(msg('assistant', 'a2')) // jsonl 行 3
    store.addMessage(frame('system', '[自动压缩] 已折叠…', true)) // 落盘行 4
    expect(store.messages.map(m => m.rowIndex)).toEqual([0, 1, undefined, undefined, 2, 3, 4])
    store.addMessage(msg('user', 'q3')) // jsonl 行 5
    expect(store.messages[7].rowIndex).toBe(5)
  })

  it('explicit false wins over role; undefined keeps the legacy rule (wire compat)', () => {
    const store = useChatStore()
    store.prependHistory([msg('user', 'q1'), msg('assistant', 'a1')], 0)
    // 旧网关/本地回显不带键 → 旧规则：assistant 占行、system 不占。
    store.addMessage(msg('assistant', 'legacy assistant'))
    store.addMessage(msg('system', 'legacy system'))
    // 新网关瞬态回执带 false → 即使 role=assistant 也不占。
    store.addMessage(frame('assistant', '⚡ 已插话', false))
    expect(store.messages[2].rowIndex).toBe(2)
    expect(store.messages[3].rowIndex).toBeUndefined()
    expect(store.messages[4].rowIndex).toBeUndefined()
    store.addMessage(msg('user', 'q2'))
    expect(store.messages[5].rowIndex).toBe(3)
  })

  it('resync recompute counts occupies frames consistently', () => {
    const store = useChatStore()
    // 混合列表重推锚：带行号历史（含 system@2）+ 无行号 live 占行（q2/
    // 注记——watchdog 路径降级形态）+ 瞬态回执。锚 = 首行号 + 占行总数：
    // 占行 5 帧（q1/a1/sys/q2/note）→ 锚 5，a2 编 5。
    store.replaceMessages([
      msg('user', 'q1', 0),
      msg('assistant', 'a1', 1),
      msg('system', '排队回执', 2),
      frame('assistant', '⚡ 已插话', false),
      msg('user', 'q2'),
      frame('system', '[自动压缩] 已折叠…', true),
    ])
    store.addMessage(msg('assistant', 'a2'))
    expect(store.messages[6].rowIndex).toBe(5)
  })

  it('anchorFreshSession numbers persisted notes and skips transient receipts', () => {
    const store = useChatStore()
    // 新会话首轮：本地回显 + 瞬态回执 + 落盘注记 + 回复。
    store.addMessage(msg('user', 'q1'))
    store.addMessage(frame('assistant', '⏳ 正在处理…', false))
    store.addMessage(frame('system', '[自动压缩] 已折叠…', true))
    store.addMessage(msg('assistant', 'a1'))
    store.anchorFreshSession()
    // q1=0、瞬态不占、注记=1、a1=2 —— 与 jsonl 行一致。
    expect(store.messages.map(m => m.rowIndex)).toEqual([0, undefined, 1, 2])
    expect(store.messages.length).toBe(4)
  })
})

// A1/A2→②③（2026-09-22 竞态 / 2026-11-16 用户消息重复渲染修复）：历史
// 快照与实时帧的合并语义。竞态：切会话 reset() 清空视图 → 实时帧先入列 →
// 历史响应无条件前置拼接 → 同一条回复渲染两次。修法 = 前置前尾段对齐合并
// （mergeHistoryTail，②——旧 A2 单行比对覆盖不了批量重复）+ sync 补拉乐观
// 行认领（consumeReplayRow，③）+ A1 seq 精确剔除（合并**之后**跑——先 A1
// 会删掉视图侧重复 assistant 行令合并行走提前断，夹层 user 乐观行漏认领）。

describe('chat store A1/A2/②③ history-vs-live merge', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
  })

  function frame(
    role: 'user' | 'assistant' | 'error' | 'system',
    content: string,
    occupies: boolean | undefined,
  ) {
    return { role, content, timestamp: 't', ...(occupies !== undefined ? { occupies } : {}) }
  }

  it('dropAssistantBelowSeq removes only assistant rows with seq <= maxSeq', () => {
    const store = useChatStore()
    store.addMessage({ role: 'user', content: 'q', timestamp: 't', seq: 1 })
    store.addMessage({ role: 'assistant', content: 'a', timestamp: 't', seq: 2 })
    store.addMessage({ role: 'assistant', content: 'no-seq', timestamp: 't' })
    store.addMessage({ role: 'assistant', content: 'future', timestamp: 't', seq: 9 })
    const removed = store.dropAssistantBelowSeq(5)
    expect(removed).toBe(1)
    // user 行不剔（入环早于落盘，推断不成立）；无 seq / 超前 seq 保留。
    expect(store.messages.map(m => m.content)).toEqual(['q', 'no-seq', 'future'])
  })

  it('dropAssistantBelowSeq with 0 (no last_seq) is a no-op', () => {
    const store = useChatStore()
    store.addMessage({ role: 'assistant', content: 'a', timestamp: 't', seq: 2 })
    expect(store.dropAssistantBelowSeq(0)).toBe(0)
    expect(store.messages).toHaveLength(1)
  })

  it('② full-tail claim (path β): batch copies rebuild the view exactly once', () => {
    const store = useChatStore()
    // 路径 β：historyLoaded=false 会话重连全量重拉——视图已渲染
    // 乐观/live 行（带 seq / optimistic 标记），历史批次含同样内容。
    store.addMessage({ role: 'user', content: 'q1', timestamp: 't', optimistic: true })
    store.addMessage({ role: 'assistant', content: 'a1', timestamp: 't', seq: 3 })
    store.addMessage({ role: 'user', content: 'q2', timestamp: 't', optimistic: true })
    const claimed = store.mergeHistoryTail(
      [
        { role: 'user', content: 'q1', timestamp: 'srv' },
        { role: 'assistant', content: 'a1', timestamp: 'srv' },
        { role: 'user', content: 'q2', timestamp: 'srv' },
      ],
      null,
    )
    expect(claimed).toBe(3)
    // 整段替换成批次副本：无重复、顺序保持、服务端时间戳胜出。
    expect(store.messages.map(m => m.content)).toEqual(['q1', 'a1', 'q2'])
    expect(store.messages.map(m => m.timestamp)).toEqual(['srv', 'srv', 'srv'])
  })

  it('② partial tail claim: batch prefix (older rows) prepends between', () => {
    const store = useChatStore()
    store.addMessage({ role: 'user', content: 'q1', timestamp: 't', optimistic: true })
    store.addMessage({ role: 'assistant', content: 'a1', timestamp: 't', seq: 3 })
    const claimed = store.mergeHistoryTail(
      [
        { role: 'user', content: 'q0', timestamp: 'srv' },
        { role: 'assistant', content: 'a0', timestamp: 'srv' },
        { role: 'user', content: 'q1', timestamp: 'srv' },
        { role: 'assistant', content: 'a1', timestamp: 'srv' },
      ],
      null,
    )
    expect(claimed).toBe(2)
    // 视图没有的更旧前缀（q0/a0）插入认领段之前；认领段换批次副本。
    expect(store.messages.map(m => m.content)).toEqual(['q0', 'a0', 'q1', 'a1'])
  })

  it('② zero-match (snapshot stale vs live tail) → classic prepend, view untouched', () => {
    const store = useChatStore()
    // 保守回落：快照读取早于视图尾部行落盘（残余赛窗）——批次整体
    // 比视图尾更旧，旧行为前置拼接，视图行原样保留。
    store.addMessage({ role: 'user', content: 'q-live', timestamp: 't', optimistic: true })
    store.mergeHistoryTail(
      [
        { role: 'user', content: 'q0', timestamp: 'srv' },
        { role: 'assistant', content: 'a0', timestamp: 'srv' },
      ],
      null,
    )
    expect(store.messages.map(m => m.content)).toEqual(['q0', 'a0', 'q-live'])
  })

  it('② transient receipts inside the claimed region are preserved (no loss)', () => {
    const store = useChatStore()
    store.addMessage({ role: 'user', content: 'q1', timestamp: 't', optimistic: true })
    store.addMessage(frame('assistant', '⏳ 正在处理…', false))
    store.addMessage({ role: 'assistant', content: 'a1', timestamp: 't', seq: 3 })
    store.mergeHistoryTail(
      [
        { role: 'user', content: 'q1', timestamp: 'srv' },
        { role: 'assistant', content: 'a1', timestamp: 'srv' },
      ],
      null,
    )
    // 认领段重建按视图原序重放：瞬态回执夹在两行之间原样保留。
    expect(store.messages.map(m => m.content)).toEqual(['q1', '⏳ 正在处理…', 'a1'])
  })

  it('② batch-side skip (disconnect window): view prefix of batch tail renders missed rows once', () => {
    const store = useChatStore()
    // 断线窗口形态：视图只有乐观 q1（回复 a1 断线期间落盘没渲染），
    // 批次 [q1, a1]——a1 经批次侧跳过照常渲染，q1 不重复。
    store.addMessage({ role: 'user', content: 'q1', timestamp: 't', optimistic: true })
    const claimed = store.mergeHistoryTail(
      [
        { role: 'user', content: 'q1', timestamp: 'srv' },
        { role: 'assistant', content: 'a1', timestamp: 'srv' },
      ],
      null,
    )
    expect(claimed).toBe(2)
    expect(store.messages.map(m => m.content)).toEqual(['q1', 'a1'])
    expect(store.messages.filter(m => m.content === 'q1')).toHaveLength(1)
  })

  it('② batch-side skip renders missed rows at batch order (interleaved span)', () => {
    const store = useChatStore()
    // 视图缺连续尾段 [u2, a2]：校验行 a1 命中后整段跳过，按批次原序落位。
    store.addMessage({ role: 'user', content: 'q1', timestamp: 't' })
    store.addMessage({ role: 'assistant', content: 'a1', timestamp: 't', seq: 3 })
    store.mergeHistoryTail(
      [
        { role: 'user', content: 'q1', timestamp: 'srv' },
        { role: 'assistant', content: 'a1', timestamp: 'srv' },
        { role: 'user', content: 'u2', timestamp: 'srv' },
        { role: 'assistant', content: 'a2', timestamp: 'srv' },
      ],
      null,
    )
    expect(store.messages.map(m => m.content)).toEqual(['q1', 'a1', 'u2', 'a2'])
  })

  it('② batch-side skip without validator (true divergence) stops conservatively', () => {
    const store = useChatStore()
    // 批次尾与视图尾同 role 但不同文、且批次更深处无校验行 = 真分叉
    // （残余赛窗）——零认领走保守前置，视图不动。
    store.addMessage({ role: 'user', content: 'q-live', timestamp: 't', optimistic: true })
    store.mergeHistoryTail(
      [
        { role: 'user', content: 'q0', timestamp: 'srv' },
        { role: 'assistant', content: 'a0', timestamp: 'srv' },
      ],
      null,
    )
    expect(store.messages.map(m => m.content)).toEqual(['q0', 'a0', 'q-live'])
  })

  it('② rows get batch numbering when oldestIdx is provided', () => {
    const store = useChatStore()
    store.addMessage({ role: 'user', content: 'q1', timestamp: 't', optimistic: true })
    store.mergeHistoryTail(
      [
        { role: 'user', content: 'q1', timestamp: 'srv' },
        { role: 'assistant', content: 'a1', timestamp: 'srv' },
      ],
      5,
    )
    // 批内行号 = oldestIdx + 批内序号（q1=5, a1=6，跳过行也按批次位编号）。
    expect(store.messages.map(m => m.rowIndex)).toEqual([5, 6])
  })

  it('race replay (updated call order): merge first, then A1 → rendered exactly once', () => {
    const store = useChatStore()
    // reset 后空视图，实时帧先到（竞态窗口）；新流程 ChatPanel 先
    // merge（视图单行被批次副本认领替换）再 A1（无残行可剔）。
    store.reset()
    store.addMessage({ role: 'assistant', content: 'reply', timestamp: 't', seq: 7 })
    store.mergeHistoryTail(
      [
        { role: 'user', content: 'q', timestamp: 'srv' },
        { role: 'assistant', content: 'reply', timestamp: 'srv' },
      ],
      0,
    )
    store.dropAssistantBelowSeq(7)
    expect(store.messages.map(m => m.content)).toEqual(['q', 'reply'])
  })

  it('A1-after-merge ordering: A1 first would strand the interleaved optimistic user row', () => {
    const store = useChatStore()
    // 路径 β 全匹配形态回放：合并先行 → assistant 行参与对齐被批次
    // 副本替换 → user 乐观行同批认领 → 无重复。（反面教材：若先 A1
    // 删掉视图 a1，合并行走只匹配到 u2，u1 残留重复。）
    store.addMessage({ role: 'user', content: 'u1', timestamp: 't', optimistic: true })
    store.addMessage({ role: 'assistant', content: 'a1', timestamp: 't', seq: 1 })
    store.addMessage({ role: 'user', content: 'u2', timestamp: 't', optimistic: true })
    const claimed = store.mergeHistoryTail(
      [
        { role: 'user', content: 'u1', timestamp: 'srv' },
        { role: 'assistant', content: 'a1', timestamp: 'srv' },
        { role: 'user', content: 'u2', timestamp: 'srv' },
      ],
      null,
    )
    expect(claimed).toBe(3)
    expect(store.messages.filter(m => m.content === 'u1')).toHaveLength(1)
    expect(store.messages.filter(m => m.content === 'u2')).toHaveLength(1)
    store.dropAssistantBelowSeq(1)
    expect(store.messages.map(m => m.content)).toEqual(['u1', 'a1', 'u2'])
  })

  it('pagination prepend (older batch) is unaffected by all rules', () => {
    const store = useChatStore()
    store.addMessage({ role: 'user', content: 'q2', timestamp: 't' })
    store.addMessage({ role: 'assistant', content: 'a2', timestamp: 't' })
    // 翻页：更早的批次（尾部与列表尾部不同文）→ 零认领走保守前置，
    // 原样 prepend（ChatPanel inflight.paginated 钉子：翻页不调用
    // mergeHistoryTail 的替换面，行为与 prependHistory 等价）。
    store.mergeHistoryTail(
      [
        { role: 'user', content: 'q1', timestamp: 't' },
        { role: 'assistant', content: 'a1', timestamp: 't' },
      ],
      0,
    )
    expect(store.messages.map(m => m.content)).toEqual(['q1', 'a1', 'q2', 'a2'])
  })

  // --- ③ consumeReplayRow：sync 补拉的环事件认领本地未确认形态 ---

  it('③ user replay event claims optimistic unconfirmed row and stamps seq', () => {
    const store = useChatStore()
    store.addMessage({ role: 'user', content: 'q1', timestamp: 't', optimistic: true })
    expect(store.consumeReplayRow('user', 'q1', 42)).toBe(true)
    expect(store.messages).toHaveLength(1)
    expect(store.messages[0].seq).toBe(42)
  })

  it('③ user replay event does not claim history/live rows (no optimistic mark)', () => {
    const store = useChatStore()
    // 历史区同文行（无 optimistic 标记）不是候选——误吞会白白发一轮。
    store.addMessage({ role: 'user', content: 'q1', timestamp: 't', seq: 3 })
    expect(store.consumeReplayRow('user', 'q1', 42)).toBe(false)
    expect(store.messages).toHaveLength(1)
    expect(store.messages[0].seq).toBe(3)
  })

  it('③ user scan passes confirmed rows (α cascade: replay assistant landed first)', () => {
    const store = useChatStore()
    // 路径 α：断线窗口 ≥2 条用户消息，补拉批次先渲染了漏掉的
    // assistant（已盖章），user 乐观行在其更早的位置——扫描穿越
    // 已盖章行到达乐观行（乐观标记钉死误吞面）。
    store.addMessage({ role: 'user', content: 'u1', timestamp: 't', optimistic: true })
    store.addMessage({ role: 'assistant', content: 'a1', timestamp: 't', seq: 7 })
    store.addMessage({ role: 'user', content: 'u2', timestamp: 't', optimistic: true })
    expect(store.consumeReplayRow('user', 'u2', 9)).toBe(true)
    expect(store.messages[2].seq).toBe(9)
    // u1 也照常认领（批次顺序无要求）。
    expect(store.consumeReplayRow('user', 'u1', 8)).toBe(true)
    expect(store.messages[0].seq).toBe(8)
  })

  it('③ assistant replay event claims unconfirmed row; seq-defined row stops the scan', () => {
    const store = useChatStore()
    // legacy 帧（无 seq）在视图里可认领。
    store.addMessage({ role: 'assistant', content: 'a1', timestamp: 't' })
    expect(store.consumeReplayRow('assistant', 'a1', 7)).toBe(true)
    expect(store.messages[0].seq).toBe(7)
    // 确认边界：扫到首个已盖章 assistant 行即停——不向更深处（历史区）
    // 越界匹配，防误吞落盘行（历史行不带 seq，误吞会扩大 A1 淘汰面）。
    store.addMessage({ role: 'user', content: 'u', timestamp: 't' })
    store.addMessage({ role: 'assistant', content: 'a1', timestamp: 't', seq: 5 })
    expect(store.consumeReplayRow('assistant', 'a1', 9)).toBe(false)
  })

  it('③ role must match exactly (user event never claims assistant row)', () => {
    const store = useChatStore()
    store.addMessage({ role: 'assistant', content: 'same', timestamp: 't' })
    expect(store.consumeReplayRow('user', 'same', 1)).toBe(false)
  })
})

// B2（2026-09-22）：在飞 turn 登记——**不被 reset 清掉**（跨会话切换存活，
// 发送后切走再切回要靠它恢复占位/轮询）；assistant/error/sync 收尾/死轮
// /cancel 各路径显式清账。

describe('chat store inflight turns (B2)', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
  })

  it('markInflightTurn survives reset (session switch keeps the registry)', () => {
    const store = useChatStore()
    store.markInflightTurn('s1', 'hello')
    store.reset() // 切会话
    expect(store.inflightTurnOf('s1')).toBeDefined()
  })

  it('clearInflightTurn removes the entry; inflightTurnOf null-safe', () => {
    const store = useChatStore()
    store.markInflightTurn('s1', 'hello')
    expect(store.inflightTurnOf('s1')).toEqual({
      sentAt: expect.any(Number),
      content: 'hello',
    })
    store.clearInflightTurn('s1')
    expect(store.inflightTurnOf('s1')).toBeUndefined()
    expect(store.inflightTurnOf(null)).toBeUndefined()
  })

  it('mark with null sessionId is a no-op (no legacy-chat pollution)', () => {
    const store = useChatStore()
    store.markInflightTurn(null, 'x')
    expect(Object.keys(store.inflightTurns)).toHaveLength(0)
  })

  it('multiple sessions register independently', () => {
    const store = useChatStore()
    store.markInflightTurn('s1', 'one')
    store.markInflightTurn('s2', 'two')
    store.clearInflightTurn('s1') // s1 回复到场
    expect(store.inflightTurnOf('s1')).toBeUndefined()
    expect(store.inflightTurnOf('s2')?.content).toBe('two')
  })
})

// W3（2026-10-14 审计）：按会话清账——busy 表与在飞 turn 登记在会话删除/
// 清空/登出/watchdog 放弃等「收尾帧永不再来」的路径上显式清除，防慢性
// 泄漏与会话 id 复用时的幽灵占位（二者都刻意不被 reset 清，全靠显式清账）。

describe('chat store dropSessionState (W3)', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
  })

  it('drops both busy state and the inflight registry entry', () => {
    const store = useChatStore()
    store.setBusy('s1', true)
    store.markInflightTurn('s1', 'hello')
    store.dropSessionState('s1')
    expect(store.isBusy('s1')).toBe(false)
    expect(store.inflightTurnOf('s1')).toBeUndefined()
  })

  it('only touches the target session', () => {
    const store = useChatStore()
    store.setBusy('s1', true)
    store.setBusy('s2', true)
    store.markInflightTurn('s2', 'two')
    store.dropSessionState('s1')
    expect(store.isBusy('s2')).toBe(true)
    expect(store.inflightTurnOf('s2')?.content).toBe('two')
  })

  it('session id can be used again after dropping (no stale state)', () => {
    const store = useChatStore()
    store.setBusy('s1', true)
    store.markInflightTurn('s1', 'old ghost')
    store.dropSessionState('s1')
    // 同 id 复用（新会话）：旧在飞登记不得复活占位。
    expect(store.inflightTurnOf('s1')).toBeUndefined()
    store.setBusy('s1', true)
    expect(store.isBusy('s1')).toBe(true)
  })

  it('dropAllSessionState clears every session (logout path)', () => {
    const store = useChatStore()
    store.setBusy('s1', true)
    store.setBusy('s2', true)
    store.markInflightTurn('s1', 'one')
    store.markInflightTurn('s2', 'two')
    store.dropAllSessionState()
    expect(store.isBusy('s1')).toBe(false)
    expect(store.isBusy('s2')).toBe(false)
    expect(Object.keys(store.inflightTurns)).toHaveLength(0)
  })
})

// ---------------------------------------------------------------------------
// M6-fix（2026-10-21）：发送链 create 的全新会话建立行号锚。此前 justCreatedSid
// 分支跳过 reset+loadHistory（防毁本地回显）但锚停在 null → 本地回显与后续
// live 行全部无 rowIndex → 回退/重新生成按钮组在该会话首轮恒不渲染。
describe('chat store anchorFreshSession (M6-fix)', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
  })

  it('numbers unanchored user/assistant rows from 0 and continues for later live rows', () => {
    const store = useChatStore()
    // 模拟发送链时序：本地回显先行（无锚不编号）→ create 后补锚。
    store.addMessage(msg('user', 'q1'))
    store.anchorFreshSession()
    expect(store.messages[0].rowIndex).toBe(0)
    // 后续 live 行接锚续号（assistant 回复、第二轮）。
    store.addMessage(msg('assistant', 'a1'))
    store.addMessage(msg('user', 'q2'))
    expect(store.messages.map(m => m.rowIndex)).toEqual([0, 1, 2])
  })

  it('error/system rows do not occupy lines (consistent with addMessage)', () => {
    const store = useChatStore()
    store.addMessage(msg('user', 'q1'))
    store.addMessage(msg('error', 'boom'))
    store.addMessage(msg('system', 'note'))
    store.anchorFreshSession()
    store.addMessage(msg('assistant', 'a1'))
    expect(store.messages[0].rowIndex).toBe(0)
    expect(store.messages[1].rowIndex).toBeUndefined()
    expect(store.messages[2].rowIndex).toBeUndefined()
    expect(store.messages[3].rowIndex).toBe(1)
  })

  it('re-anchoring an already-anchored fresh view renumbers from 0 (server log is empty)', () => {
    const store = useChatStore()
    store.prependHistory([msg('user', 'stale'), msg('assistant', 'rows')], 40)
    store.addMessage(msg('user', 'echo'))
    // justCreated 分支语义：新会话服务端 jsonl 必为空——视图内行就是 0..n-1。
    store.anchorFreshSession()
    expect(store.messages.map(m => m.rowIndex)).toEqual([0, 1, 2])
  })
})

// 2026-11-07 充分测试战役（webkit 全量实录根修）：会话级输入草稿表。
// reset() 是后台恢复路径（SSE resync / gap 全量重载 / 路由重挂载补偿）与
// 切会话共用的状态重建入口——后台路径不得抹用户正在输入的草稿（实录：
// 前序 spec 遗留的 gateway 侧异步工作完成 → chat.activity → 防抖全刷 →
// reset → fill 后发送钮恒 disabled，webkit 全量 4 例）。reset 不碰 input；
// input 生命周期显式化：carryInput 切会话（旧草稿入表/新草稿出表）、
// clearInput 发送链与模块边界、dropSessionState/dropAllSessionState 清表。
describe('chat store session draft map (2026-11-07)', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
  })

  it('reset preserves input (background rebuild must not wipe the draft)', () => {
    const store = useChatStore()
    store.input = '正在输入的草稿'
    store.reset()
    expect(store.input).toBe('正在输入的草稿')
  })

  it('carryInput parks the old draft and restores the target draft', () => {
    const store = useChatStore()
    store.input = 'A 会话的草稿'
    store.carryInput('sid-a', 'sid-b')
    expect(store.input).toBe('') // sid-b 无历史草稿 → 空白起步
    store.input = 'B 会话的草稿'
    store.carryInput('sid-b', 'sid-a')
    expect(store.input).toBe('A 会话的草稿') // 切回 A 草稿复现
    store.carryInput('sid-a', 'sid-b')
    expect(store.input).toBe('B 会话的草稿')
  })

  it('carryInput tolerates null session ids (bootstrap / logout edges)', () => {
    const store = useChatStore()
    store.input = 'some draft'
    store.carryInput(null, 'sid-1')
    // null 旧会话不入表（无处可落）；初始选定不得洗用户已打的草稿——
    // ChatView 初始 switchTo 晚于用户输入时，旧语义把草稿洗成空白 →
    // 发送钮恒 disabled（2026-11-07 webkit 全量竞速实录根修）
    expect(store.input).toBe('some draft')
    store.carryInput('sid-1', null)
    expect(store.input).toBe('')
  })

  it('carryInput with null oldSid restores a parked draft if the target has one', () => {
    const store = useChatStore()
    store.carryInput('sid-1', 'sid-2')
    store.input = 'sid-2 存稿'
    store.carryInput('sid-2', 'sid-1')
    // 回到无会话态再初始选定 sid-2：恢复其存稿
    store.carryInput(null, 'sid-2')
    expect(store.input).toBe('sid-2 存稿')
  })

  it('dropSessionState clears the draft so a recreated session id starts clean', () => {
    const store = useChatStore()
    store.input = 'draft for sid-x'
    store.carryInput('sid-x', 'sid-y')
    store.dropSessionState('sid-x')
    store.input = 'draft for sid-y'
    store.carryInput('sid-y', 'sid-x')
    expect(store.input).toBe('') // 复用 id 不得复活已删会话的草稿
  })

  it('dropAllSessionState clears every draft (logout)', () => {
    const store = useChatStore()
    store.input = 'draft a'
    store.carryInput('sid-a', 'sid-b')
    store.input = 'draft b'
    store.carryInput('sid-b', 'sid-a')
    store.input = 'live draft'
    store.dropAllSessionState()
    store.carryInput('sid-a', 'sid-b')
    expect(store.input).toBe('')
  })
})

// 缺陷 D 修复（2026-11-23 插队时间线消失家族审计）：replaceMessages
// keepLiveState 选项——busy 中 resync（subagent_reply 落在下一轮中途）以
// 磁盘 replace 重建视图时保留活 turn 时间线（默认形态维持 watchdog WIPE
// 的清空语义不回归）。
describe('chat store replaceMessages keepLiveState (缺陷 D)', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
  })

  function seedLiveState(store: ReturnType<typeof useChatStore>) {
    store.setBusy('s1', true)
    store.ingestWorkRoundText('正在分析…')
    store.ingestWorkToolStart('c1', 'sleep', '30')
    store.appendPendingDeliverables([{ path: 'out.txt', kind: 'file' } as any])
  }

  it('默认形态清空活状态（watchdog WIPE 语义不回归）', () => {
    const store = useChatStore()
    seedLiveState(store)
    expect(store.pendingWork).not.toBeNull()
    store.replaceMessages([
      { role: 'user', content: 'q', timestamp: '' },
      { role: 'assistant', content: 'a', timestamp: '' },
    ])
    expect(store.pendingWork).toBeNull()
    expect(store.pendingDeliverables).toHaveLength(0)
  })

  it('keepLiveState 保留活时间线与待挂交付物（busy 中 resync 语义）', () => {
    const store = useChatStore()
    seedLiveState(store)
    store.replaceMessages(
      [
        { role: 'user', content: 'q', timestamp: '' },
        { role: 'assistant', content: 'a', timestamp: '' },
        { role: 'assistant', content: '子代理回复', timestamp: '' },
      ],
      { keepLiveState: true },
    )
    // 活时间线原样保留（同一对象，轮内工具卡不丢）。
    expect(store.pendingWork).not.toBeNull()
    expect(store.pendingWork!.rounds.some(r => r.tools.some(t => t.call_id === 'c1'))).toBe(true)
    expect(store.pendingDeliverables).toHaveLength(1)
    // 行照常替换（subagent 回复行进视图）。
    expect(store.messages).toHaveLength(3)
    expect(store.messages[2].content).toBe('子代理回复')
  })
})
