import { describe, it, expect, vi, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount, flushPromises } from '@vue/test-utils'

// R1（2026-09-21，2026-10-30 随 work 时间线改造更新）：中间轮正文
// （AgentEvent::RoundText）前端链路回归钉。背景：LLM 循环里带工具调用的
// 中间轮，其正文（模型过程叙述）此前只进 history——前端只见工具卡与最终
// 回复。修复后归入 work 时间线（叙述开轮 + 工具批次交错）：
// - 实时：RoundText tool_event 帧 → ingestWorkRoundText 开轮 → pendingWork
//   缓冲 → live 块展开渲染（只有叙述、没有工具的轮次也要出 pending 区）；
// - 收尾：assistant 回复落地时 flush 折叠挂载到该消息（WorkTimelineBlock
//   默认收起成「用时 X」胶囊头，点击展开回看）；
// - 回放：chat.sync 补拉的 kind=tool / RoundText 条目 → 同一转换 →
//   assistant 行落地时同轮挂载（切页/重连不丢）。

const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
}))

vi.mock('../../composables/useWebSocket', async () => {
  const { ref } = await import('vue')
  return {
    connect: vi.fn(),
    send: vi.fn(),
    sendHistoryRequest: vi.fn(),
    onMessage: vi.fn(),
    addMessageHandler: vi.fn(),
    removeMessageHandler: vi.fn(),
    wsStatus: ref('connected'),
  }
})

vi.mock('../../composables/useSSE', () => {
  const handlers: Record<string, any[]> = {}
  ;(globalThis as any).__sseHandlers = handlers
  return {
    on: (t: string, h: any) => {
      ;(handlers[t] ??= []).push(h)
    },
    off: (t: string, h: any) => {
      handlers[t] = (handlers[t] ?? []).filter((x: any) => x !== h)
    },
  }
})

import { onMessage, sendHistoryRequest } from '../../composables/useWebSocket'
import ChatPanel from '../ChatPanel.vue'
import { useChatStore } from '../../stores/chat'
import { useSessionStore } from '../../stores/session'

function mountedHandler(): (msg: any) => void {
  return vi.mocked(onMessage).mock.calls[0][0] as any
}

function feedHistory(requestId: string, messages: any[]) {
  mountedHandler()({
    type: 'message',
    module: 'chat',
    cmd: 'history_response',
    data: { request_id: requestId, messages, has_more: false, oldest_index: 0 },
  })
}

function lastRequestId(): string {
  return vi.mocked(sendHistoryRequest).mock.calls[
    vi.mocked(sendHistoryRequest).mock.calls.length - 1
  ][0] as string
}

function prepareSession() {
  const sessionStore = useSessionStore()
  sessionStore.sessions = [
    { id: 's1', channel: 'web', startTime: '', lastTime: '', messageCount: 0, firstMessage: 'A', model: '' },
  ] as any
  sessionStore.currentId = 's1'
}

/** RoundText 实时 push 帧（web pump 注入 session_id 后的生产形态）。 */
function roundTextFrame(seq: number, content: string) {
  return {
    type: 'push',
    cmd: 'tool_event',
    data: {
      kind: 'RoundText',
      seq,
      data: { session_id: 's1', chat_id: 'web:conn-x', content },
    },
  }
}

/** sync 补拉的 kind=tool 条目（RoundText 形态，tool 内层即实时帧 ev）。 */
function syncRoundText(seq: number, content: string) {
  return {
    seq,
    kind: 'tool',
    role: '',
    content: '',
    tool: { kind: 'RoundText', data: { session_id: 's1', chat_id: 'web:conn-x', content } },
  }
}

beforeEach(() => {
  setActivePinia(createPinia())
  requestMock.mockReset()
  requestMock.mockImplementation((_mod: string, cmd: string) => {
    if (cmd === 'inbox_status') return Promise.resolve({ available: true, busy: true })
    return Promise.resolve({})
  })
  vi.mocked(sendHistoryRequest).mockClear()
  vi.mocked(onMessage).mockClear()
})

describe('R1：实时 RoundText → live 块展开渲染', () => {
  it('RoundText 帧进缓冲并在 live 块显示（无需工具/streaming 在场）', async () => {
    prepareSession()
    const w = mount(ChatPanel)
    await flushPromises()
    feedHistory(lastRequestId(), [
      { role: 'user', content: 'q1' },
      { role: 'assistant', content: 'a1' },
    ])
    await flushPromises()

    mountedHandler()(roundTextFrame(10, '第一步：先看配置。'))
    mountedHandler()(roundTextFrame(11, '第二步：改完再验证。'))
    await flushPromises()

    const chat = useChatStore()
    // 每条叙述开一轮（ingestWorkRoundText 单轮语义）。
    expect(chat.pendingWork?.rounds.map(r => r.text)).toEqual([
      '第一步：先看配置。',
      '第二步：改完再验证。',
    ])
    // 只有叙述、没有工具的轮次也要出 live 块（外层条件含 pendingWork）。
    const liveTexts = w.findAll('.wt-round-text')
    expect(liveTexts.length).toBe(2)
    expect(w.text()).toContain('第一步：先看配置。')
    w.unmount()
  })

  it('异会话 RoundText 帧被过滤（session_id ≠ currentId）', async () => {
    prepareSession()
    const w = mount(ChatPanel)
    await flushPromises()
    mountedHandler()({
      type: 'push',
      cmd: 'tool_event',
      data: {
        kind: 'RoundText',
        seq: 12,
        data: { session_id: 'other', chat_id: 'web:conn-x', content: '异会话的叙述' },
      },
    })
    await flushPromises()
    const chat = useChatStore()
    expect(chat.pendingWork).toBeNull()
    expect(w.findAll('.wt-round-text').length).toBe(0)
    w.unmount()
  })
})

describe('R1：回复落地 → 折叠挂载，点击展开回看', () => {
  it('assistant 落地时 flush 挂载 work，默认折叠为「用时」胶囊头', async () => {
    prepareSession()
    const w = mount(ChatPanel)
    await flushPromises()
    feedHistory(lastRequestId(), [{ role: 'user', content: 'q1' }])
    await flushPromises()

    mountedHandler()(roundTextFrame(10, '第一步。'))
    mountedHandler()(roundTextFrame(11, '第二步。'))
    await flushPromises()

    mountedHandler()({
      type: 'message',
      module: 'chat',
      cmd: 'receive',
      data: { role: 'assistant', content: '全部完成。', seq: 12, session_id: 's1' },
    })
    await flushPromises()

    const chat = useChatStore()
    expect(chat.pendingWork).toBeNull()
    const last = chat.messages[chat.messages.length - 1]
    expect(last.role).toBe('assistant')
    expect(last.work?.rounds.map(r => r.text)).toEqual(['第一步。', '第二步。'])

    // 默认折叠：胶囊头存在、正文不渲染；live 块已随 flush 卸载。
    const blocks = w.findAll('.wt-block:not(.live)')
    expect(blocks.length).toBe(1)
    expect(blocks[0].find('.wt-header').text()).toContain('用时')
    expect(blocks[0].find('.wt-body').exists()).toBe(false)

    // 点击展开 → 正文可见；再点收起。
    await blocks[0].find('.wt-header').trigger('click')
    expect(w.find('.wt-block:not(.live) .wt-body').exists()).toBe(true)
    expect(w.text()).toContain('第一步。')
    await blocks[0].find('.wt-header').trigger('click')
    expect(w.find('.wt-block:not(.live) .wt-body').exists()).toBe(false)
    w.unmount()
  })
})

describe('R1：sync 补拉 RoundText 条目 → assistant 行落地时同轮挂载', () => {
  it('补拉窗口的 RoundText + assistant 行 → 折叠块挂到回复消息', async () => {
    prepareSession()
    let syncN = 0
    requestMock.mockImplementation((_mod: string, cmd: string) => {
      if (cmd === 'inbox_status') return Promise.resolve({ available: true, busy: true })
      if (cmd === 'sync') {
        syncN += 1
        if (syncN === 1) return Promise.resolve({}) // primeSeqBaseline 基线（空）
        return Promise.resolve({
          events: [
            syncRoundText(5, '重放的第一步。'),
            syncRoundText(6, '重放的第二步。'),
            { seq: 7, role: 'assistant', content: 'recovered!', ts: '2026-09-21T00:00:00Z' },
          ],
        })
      }
      return Promise.resolve({})
    })
    // fake timers 须先于 mount——轮询定时器在历史应用后才注册，
    // 与 tool-replay P1a 用例同形态。
    vi.useFakeTimers()
    const w = mount(ChatPanel)
    await flushPromises()
    feedHistory(lastRequestId(), [{ role: 'user', content: 'reconnect case' }])
    await flushPromises()

    // detectPendingTurn 轮询 tick 触发 sync 补拉。
    await vi.advanceTimersByTimeAsync(4100)
    await flushPromises()
    vi.useRealTimers()

    const chat = useChatStore()
    const last = chat.messages[chat.messages.length - 1]
    expect(last.content).toBe('recovered!')
    expect(last.work?.rounds.map(r => r.text)).toEqual(['重放的第一步。', '重放的第二步。'])
    const block = w.find('.wt-block:not(.live)')
    expect(block.exists()).toBe(true)
    expect(block.find('.wt-header').text()).toContain('用时')
    w.unmount()
  })
})
