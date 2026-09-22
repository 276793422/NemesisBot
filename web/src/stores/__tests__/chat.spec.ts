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

// M1b（2026-09-05）：工具调用事件收集——upsert 去重（断线重连幂等）、
// flush 取走挂载、reset/replaceMessages 清 pending 防误挂下一轮。

import type { ToolEvent } from '../chat'

function started(callId: string, tool = 'exec'): ToolEvent {
  return { callId, tool, state: 'running', argsPreview: '{"cmd":"ls"}' }
}

describe('chat store tool events (M1b)', () => {
  it('starts empty', () => {
    const store = useChatStore()
    expect(store.pendingToolEvents).toEqual([])
  })

  it('appendToolEvent upserts by callId (Started→Finished merges in place)', () => {
    const store = useChatStore()
    store.appendToolEvent(started('c1'))
    store.appendToolEvent(started('c2', 'read_file'))
    expect(store.pendingToolEvents).toHaveLength(2)
    store.appendToolEvent({
      callId: 'c1',
      tool: 'exec',
      state: 'ok',
      durationMs: 1200,
      resultPreview: 'file-a\nfile-b',
    })
    expect(store.pendingToolEvents).toHaveLength(2)
    expect(store.pendingToolEvents[0]).toEqual({
      callId: 'c1',
      tool: 'exec',
      state: 'ok',
      argsPreview: '{"cmd":"ls"}',
      durationMs: 1200,
      resultPreview: 'file-a\nfile-b',
    })
    expect(store.pendingToolEvents[1].state).toBe('running')
  })

  it('duplicate Finished push is idempotent (no dup entry)', () => {
    const store = useChatStore()
    const fin: ToolEvent = { callId: 'c1', tool: 'grep', state: 'ok', durationMs: 5 }
    store.appendToolEvent(fin)
    store.appendToolEvent(fin)
    store.appendToolEvent(fin)
    expect(store.pendingToolEvents).toHaveLength(1)
  })

  it('flushPendingToolEvents takes the events and empties the buffer', () => {
    const store = useChatStore()
    store.appendToolEvent(started('c1'))
    const flushed = store.flushPendingToolEvents()
    expect(flushed).toHaveLength(1)
    expect(flushed[0].callId).toBe('c1')
    expect(store.pendingToolEvents).toEqual([])
  })

  it('replaceMessages (watchdog resync) clears pending', () => {
    const store = useChatStore()
    store.appendToolEvent(started('c1'))
    store.replaceMessages([{ role: 'assistant', content: 'recovered', timestamp: 't' }])
    expect(store.pendingToolEvents).toEqual([])
  })

  it('reset clears pending', () => {
    const store = useChatStore()
    store.appendToolEvent(started('c1'))
    store.reset()
    expect(store.pendingToolEvents).toEqual([])
  })
})

// M6（2026-09-07）：行号推导——E3 rewind 菜单的 message_index 数据源。
// chat_log 只存 user/assistant 行（与渲染消息 1:1），历史批次行号连续
// （oldest_index + j），live 行从锚顺序递增；error/system 是纯前端渲染
// 不占行。

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

// A1/A2（2026-09-22 聊天切会话竞态）：历史快照与实时帧的合并语义。
// 竞态：切会话 reset() 清空视图 → 实时帧先入列（无尾部可比）→ 历史响应
// prependHistory 无条件前置拼接 → 同一条回复渲染两次。修法 = prepend 前
// 清洗「实时帧先到的尾巴」：A1 seq 精确剔除（assistant）+ A2 尾行同文兜底。

describe('chat store A1/A2 history-vs-live merge', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
  })

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

  it('dropTailIfSame drops the tail only on same role+content', () => {
    const store = useChatStore()
    store.addMessage({ role: 'user', content: 'q', timestamp: 't' })
    store.addMessage({ role: 'assistant', content: 'dup', timestamp: 't' })
    expect(store.dropTailIfSame('assistant', 'dup')).toBe(true)
    expect(store.messages.map(m => m.content)).toEqual(['q'])
    expect(store.dropTailIfSame('assistant', 'dup')).toBe(false)
  })

  it('race replay: live assistant frame lands before snapshot → prepended exactly once', () => {
    const store = useChatStore()
    // T3：reset 后空视图，实时帧先到（现象 A 的竞态窗口）。
    store.reset()
    store.addMessage({ role: 'assistant', content: 'reply', timestamp: 't', seq: 7 })
    // T4：历史响应到达（last_seq=7 覆盖实时帧）→ seq 剔除 → prepend 快照。
    store.dropAssistantBelowSeq(7)
    store.prependHistory(
      [
        { role: 'user', content: 'q', timestamp: 't' },
        { role: 'assistant', content: 'reply', timestamp: 't' },
      ],
      0,
    )
    expect(store.messages.map(m => m.content)).toEqual(['q', 'reply'])
  })

  it('A2 fallback: legacy gateway without last_seq → tail-same dedup catches the dup', () => {
    const store = useChatStore()
    store.reset()
    store.addMessage({ role: 'assistant', content: 'reply', timestamp: 't', seq: 3 })
    store.dropAssistantBelowSeq(0) // 旧网关无 last_seq → 0 → 不剔
    store.dropTailIfSame('assistant', 'reply')
    store.prependHistory(
      [
        { role: 'user', content: 'q', timestamp: 't' },
        { role: 'assistant', content: 'reply', timestamp: 't' },
      ],
      0,
    )
    expect(store.messages.map(m => m.content)).toEqual(['q', 'reply'])
  })

  it('pagination prepend (older batch) is unaffected by both rules', () => {
    const store = useChatStore()
    store.addMessage({ role: 'user', content: 'q2', timestamp: 't' })
    store.addMessage({ role: 'assistant', content: 'a2', timestamp: 't' })
    // 翻页：更早的批次（尾部与列表尾部不同文）→ 不剔不丢，原样前置。
    store.dropAssistantBelowSeq(99)
    store.dropTailIfSame('user', 'q1')
    store.prependHistory(
      [
        { role: 'user', content: 'q1', timestamp: 't' },
        { role: 'assistant', content: 'a1', timestamp: 't' },
      ],
      0,
    )
    expect(store.messages.map(m => m.content)).toEqual(['q1', 'a1', 'q2', 'a2'])
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
