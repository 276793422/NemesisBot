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
