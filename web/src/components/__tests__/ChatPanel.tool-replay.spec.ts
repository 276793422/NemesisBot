import { describe, it, expect, vi, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount, flushPromises } from '@vue/test-utils'

// 2026-09-21 全面修复批次回归钉（P1a/P1c/P2/P4/P8）：
// - P2：receive assistant 帧到场 → detectPendingTurn 立即停轮清占位
//   （此前占位只能等 busy=false 兜底节奏，实测残留 4.6-6.8s）。
// - P1a：chat.sync 补拉路径的 tool 条目转回工具事件、assistant 行落地时
//   flush 挂载（此前补拉回复永远裸奔，pending 工具事件挂不上）。
// - P1c：全量重载后 primeSeqBaseline 按「倒数第二条 assistant 之后」回放
//   工具卡——最后一轮完成段挂回历史 assistant 消息、进行中段留占位区。
// - P4：currentId watch 识别「在飞的正是本会话历史」→ 跳过 reset+重拉。
// - P8：SSE chat.activity 落后端（seq > lastChatSeq）防抖全量刷新；
//   本端已追平（seq ≤ lastChatSeq）零开销短路。

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

// useSSE：捕获订阅 handler（P8 信号从测试直接派发）。
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
import { useSessionStore } from '../../stores/session'

function sseHandlers(): Record<string, any[]> {
  return (globalThis as any).__sseHandlers ?? {}
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
  delete sseHandlers()['chat.activity']
})

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
  const calls = vi.mocked(sendHistoryRequest).mock.calls
  return calls[calls.length - 1][0] as string
}

function prepareSession() {
  const sessionStore = useSessionStore()
  sessionStore.sessions = [
    { id: 's1', channel: 'web', startTime: '', lastTime: '', messageCount: 0, firstMessage: 'A', model: '' },
  ] as any
  sessionStore.currentId = 's1'
}

function countHistoryRequests(): number {
  return vi.mocked(sendHistoryRequest).mock.calls.length
}

/** tool 条目（chat_event_log kind="tool" 形态：载荷为帧 data 原样）。 */
function toolEvent(seq: number, kind: string, data: any) {
  return { seq, kind: 'tool', role: '', content: '', tool: { kind, data } }
}

describe('P2：receive assistant 到场 → 占位即时消失', () => {
  it('悬空 user 占位轮询中，receive assistant 帧到达即停轮（不等兜底节奏）', async () => {
    prepareSession()
    vi.useFakeTimers()
    const w = mount(ChatPanel)
    await flushPromises()

    feedHistory(lastRequestId(), [{ role: 'user', content: 'long task' }])
    await flushPromises()
    expect(w.find('.typing-indicator').exists()).toBe(true)

    // 回复帧到达（带 seq 推进游标 + session_id 归属当前会话）→ 占位立即消失。
    mountedHandler()({
      type: 'message',
      module: 'chat',
      cmd: 'receive',
      data: { role: 'assistant', content: 'all done', seq: 3, session_id: 's1' },
    })
    await flushPromises()
    expect(w.find('.typing-indicator').exists()).toBe(false)
    expect(w.text()).toContain('all done')

    // 轮询已停：推进两个周期无新请求。
    const reqs = countHistoryRequests()
    await vi.advanceTimersByTimeAsync(8000)
    expect(countHistoryRequests()).toBe(reqs)
    w.unmount()
  })
})

describe('P1a：sync 补拉 tool 条目转回工具事件 + assistant flush 挂载', () => {
  it('补拉窗口的 tool 对 + assistant 行 → 工具卡挂到回复消息上', async () => {
    prepareSession()
    let syncN = 0
    requestMock.mockImplementation((_mod: string, cmd: string) => {
      if (cmd === 'inbox_status') return Promise.resolve({ available: true, busy: true })
      if (cmd === 'sync') {
        syncN += 1
        if (syncN === 1) return Promise.resolve({}) // primeSeqBaseline 基线（空）
        return Promise.resolve({
          events: [
            toolEvent(2, 'ToolStarted', { call_id: 'c1', tool: 'exec', args_preview: 'ls' }),
            toolEvent(3, 'ToolFinished', { call_id: 'c1', ok: true, duration_ms: 120 }),
            { seq: 4, role: 'assistant', content: 'recovered!', ts: '2026-09-21T00:00:00Z' },
          ],
        })
      }
      return Promise.resolve({})
    })
    vi.useFakeTimers()
    const w = mount(ChatPanel)
    await flushPromises()

    feedHistory(lastRequestId(), [{ role: 'user', content: 'reconnect case' }])
    await flushPromises()
    expect(w.find('.typing-indicator').exists()).toBe(true)

    // 轮询 tick：sync 补回 tool 对 + assistant → 卡挂到回复上、占位消失。
    await vi.advanceTimersByTimeAsync(4000)
    await flushPromises()
    expect(w.text()).toContain('recovered!')
    expect(w.find('.typing-indicator').exists()).toBe(false)
    // 消息级工具卡存在（flush 挂载），pending 区已清空（不再重复渲染）。
    const msgCards = w.findAll('.message .tool-card')
    expect(msgCards.length).toBeGreaterThanOrEqual(1)
    // 完成态卡渲染 args_preview + 耗时（ToolCallCard 契约）。
    expect(msgCards[0].text()).toContain('ls')
    w.unmount()
  })
})

describe('P1c：全量重载后 primeSeqBaseline 回放尾部工具流程', () => {
  it('最后一轮完成段挂回历史 assistant；进行中段留占位区渲染', async () => {
    prepareSession()
    requestMock.mockImplementation((_mod: string, cmd: string) => {
      if (cmd === 'inbox_status') return Promise.resolve({ available: true, busy: true })
      if (cmd === 'sync') {
        // primeSeqBaseline（after=0 全量）：倒数第二条 assistant 之后 =
        // 旧轮 assistant → T1 对 → a1（最后一轮回复）→ T2（进行中）。
        return Promise.resolve({
          events: [
            { seq: 1, role: 'assistant', content: 'ancient', ts: 'x' },
            toolEvent(2, 'ToolStarted', { call_id: 't1', tool: 'read', args_preview: 'main.rs' }),
            toolEvent(3, 'ToolFinished', { call_id: 't1', ok: true, duration_ms: 5 }),
            { seq: 4, role: 'assistant', content: 'a1 done', ts: 'x' },
            toolEvent(5, 'ToolStarted', { call_id: 't2', tool: 'exec' }),
          ],
        })
      }
      return Promise.resolve({})
    })
    vi.useFakeTimers()
    const w = mount(ChatPanel)
    await flushPromises()

    // 历史载入：user q1 / assistant a1 / user q2（悬空 → 占位）。
    feedHistory(lastRequestId(), [
      { role: 'user', content: 'q1' },
      { role: 'assistant', content: 'a1 done' },
      { role: 'user', content: 'q2 running' },
    ])
    await flushPromises()
    expect(w.find('.typing-indicator').exists()).toBe(true)

    // 回放后：T1 挂在 a1 消息上（不再裸奔），T2 在 pending 区（占位旁）。
    const a1 = w.findAll('.message.assistant').find(d => d.text().includes('a1 done'))
    expect(a1).toBeTruthy()
    expect(a1!.findAll('.tool-card').length).toBe(1)
    expect(a1!.find('.tool-card').text()).toContain('main.rs')
    // pending 区：T2（callId t2）直接渲染（<3 张卡不折叠），且未误挂消息。
    const allCards = w.findAll('.tool-card')
    expect(allCards.length).toBe(2) // a1 挂 1 张（main.rs）+ pending 区 1 张
    const pendingCards = allCards.filter(d => d.text().includes('exec'))
    expect(pendingCards.length).toBe(1)
    w.unmount()
  })
})

describe('P4：currentId watch 同会话免重拉', () => {
  it('在飞的正是本会话历史（loadingHistorySid === newId）→ 跳过 reset+重拉', async () => {
    prepareSession()
    const sessionStore = useSessionStore()
    vi.useFakeTimers()
    const w = mount(ChatPanel)
    await flushPromises()
    // 挂载首拉已发起（wsStatus connected + 未载入），目标会话 s1 在飞。
    expect(countHistoryRequests()).toBe(1)

    // 登录序列形态：currentId 抖动（'' → 's1'）——在飞的就是 s1 的历史，
    // 不再发第二发（P4 前：reset+重拉，双 history_request）。
    sessionStore.currentId = ''
    await flushPromises()
    sessionStore.currentId = 's1'
    await flushPromises()
    expect(countHistoryRequests()).toBe(1)
    w.unmount()
  })

  it('切到其他会话仍正常重拉（跳过条件不误伤真切换）', async () => {
    prepareSession()
    const sessionStore = useSessionStore()
    const syncN = { n: 0 }
    requestMock.mockImplementation((_mod: string, cmd: string) => {
      if (cmd === 'inbox_status') return Promise.resolve({ available: true, busy: true })
      if (cmd === 'sync') {
        syncN.n += 1
        return Promise.resolve({})
      }
      return Promise.resolve({})
    })
    vi.useFakeTimers()
    const w = mount(ChatPanel)
    await flushPromises()
    feedHistory(lastRequestId(), [
      { role: 'user', content: 'q' },
      { role: 'assistant', content: 'a' },
    ])
    await flushPromises()
    expect(countHistoryRequests()).toBe(1)

    // 切到新会话 s2：loadedHistorySid 是 s1 → 正常 reset+重拉。
    sessionStore.sessions = [
      { id: 's1', channel: 'web', startTime: '', lastTime: '', messageCount: 0, firstMessage: 'A', model: '' },
      { id: 's2', channel: 'web', startTime: '', lastTime: '', messageCount: 0, firstMessage: 'B', model: '' },
    ] as any
    sessionStore.currentId = 's2'
    await flushPromises()
    expect(countHistoryRequests()).toBe(2)
    w.unmount()
  })
})

describe('P8：SSE chat.activity 落后端防抖全量刷新', () => {
  function fireActivity(payload: any) {
    for (const h of sseHandlers()['chat.activity'] ?? []) h(payload)
  }

  it('落后信号（seq > 游标）→ 防抖后增量 sync；gap=true 才 reset+全量重载兜底', async () => {
    prepareSession()
    let syncN = 0
    requestMock.mockImplementation((_mod: string, cmd: string) => {
      if (cmd === 'inbox_status') return Promise.resolve({ available: true, busy: true })
      if (cmd === 'sync') {
        syncN += 1
        // syncN=1 是 mount 时 primeSeqBaseline 基线（空）；syncN=2 是落后
        // 信号触发的增量——P8 补全：user 行入环后增量全覆盖，缺口滑出窗
        // 口（gap）才全量。
        if (syncN === 2) return Promise.resolve({ gap: true })
        return Promise.resolve({})
      }
      return Promise.resolve({})
    })
    vi.useFakeTimers()
    const w = mount(ChatPanel)
    await flushPromises()
    feedHistory(lastRequestId(), [
      { role: 'user', content: 'q' },
      { role: 'assistant', content: 'a' },
    ])
    await flushPromises()

    // 另一端写入（本端没收到实时帧 → 游标落后于信号 seq）。
    fireActivity({ session_id: 's1', seq: 42, kind: 'chat' })
    // 防抖窗口内不刷新。
    await vi.advanceTimersByTimeAsync(500)
    expect(countHistoryRequests()).toBe(1)
    // 800ms 到 → sync 发现 gap → reset+loadHistory 全量兜底。
    await vi.advanceTimersByTimeAsync(500)
    expect(syncN).toBeGreaterThanOrEqual(1)
    expect(countHistoryRequests()).toBe(2)
    w.unmount()
  })

  it('P8 补全：kind=chat 落后信号 → 增量 sync 补 user+assistant 行（不整页重拉）', async () => {
    prepareSession()
    let syncN = 0
    requestMock.mockImplementation((_mod: string, cmd: string) => {
      if (cmd === 'inbox_status') return Promise.resolve({ available: true, busy: true })
      if (cmd === 'sync') {
        syncN += 1
        // 基线（after=0）返回空；落后信号触发的增量（after>0）返回另一端
        // 的 user 行 + 回复行（入环后 sync 可达，无需全量）。
        if (syncN === 1) return Promise.resolve({})
        return Promise.resolve({
          events: [
            { seq: 6, role: 'user', content: 'from tab1', ts: 'x' },
            { seq: 7, role: 'assistant', content: 'reply', ts: 'x' },
          ],
        })
      }
      return Promise.resolve({})
    })
    vi.useFakeTimers()
    const w = mount(ChatPanel)
    await flushPromises()
    feedHistory(lastRequestId(), [
      { role: 'user', content: 'q' },
      { role: 'assistant', content: 'a' },
    ])
    await flushPromises()

    fireActivity({ session_id: 's1', seq: 7, kind: 'chat' })
    await vi.advanceTimersByTimeAsync(1000)
    await flushPromises()
    // 增量通道生效：user 与回复都补进来，history_request（全量）未增加。
    expect(w.text()).toContain('from tab1')
    expect(w.text()).toContain('reply')
    expect(countHistoryRequests()).toBe(1)
    w.unmount()
  })

  it('P8 补全：user 回声帧本地已有同文行 → 不重复渲染且不打断发送态', async () => {
    prepareSession()
    const { useChatStore } = await import('../../stores/chat')
    vi.useFakeTimers()
    const w = mount(ChatPanel)
    await flushPromises()
    feedHistory(lastRequestId(), [
      { role: 'user', content: 'q' },
      { role: 'assistant', content: 'a' },
    ])
    await flushPromises()

    // 模拟本地发送：user 行 + streaming 态。
    const chat = useChatStore()
    chat.addMessage({ role: 'user', content: 'my own msg', timestamp: 'x' })
    chat.streaming = true

    // 服务端入环回声帧到达（同文）→ 尾行去重不重复添加，发送态保持。
    mountedHandler()({
      type: 'message',
      module: 'chat',
      cmd: 'receive',
      data: { role: 'user', content: 'my own msg', seq: 5, session_id: 's1' },
    })
    await flushPromises()
    const userRows = w.findAll('.message.user').filter(d => d.text().includes('my own msg'))
    expect(userRows.length).toBe(1)
    expect(chat.streaming).toBe(true)
    expect(chat.messages[chat.messages.length - 1].content).toBe('my own msg')
    w.unmount()
  })

  it('P8 补全：user 回声帧本地无该行（另一端发的）→ 正常添加', async () => {
    prepareSession()
    vi.useFakeTimers()
    const w = mount(ChatPanel)
    await flushPromises()
    feedHistory(lastRequestId(), [
      { role: 'user', content: 'q' },
      { role: 'assistant', content: 'a' },
    ])
    await flushPromises()

    mountedHandler()({
      type: 'message',
      module: 'chat',
      cmd: 'receive',
      data: { role: 'user', content: 'from other tab', seq: 6, session_id: 's1' },
    })
    await flushPromises()
    expect(w.text()).toContain('from other tab')
    // 尾行是回声 user 行——不触发完成语义（streaming 未被误关的场景由
    // 上一用例覆盖；这里断言添加本身）。
    w.unmount()
  })

  it('本端已追平（seq ≤ 游标）→ 零开销短路，不刷新', async () => {
    prepareSession()
    let syncN = 0
    requestMock.mockImplementation((_mod: string, cmd: string) => {
      if (cmd === 'inbox_status') return Promise.resolve({ available: true, busy: true })
      if (cmd === 'sync') {
        syncN += 1
        // primeSeqBaseline 基线返回尾 seq=10 → lastChatSeq=10。
        return Promise.resolve({
          events: [{ seq: 10, role: 'assistant', content: 'a', ts: 'x' }],
        })
      }
      return Promise.resolve({})
    })
    vi.useFakeTimers()
    const w = mount(ChatPanel)
    await flushPromises()
    feedHistory(lastRequestId(), [
      { role: 'user', content: 'q' },
      { role: 'assistant', content: 'a' },
    ])
    await flushPromises()
    expect(syncN).toBeGreaterThanOrEqual(1)

    // 自己的写入（实时帧已推进游标到 10）：seq=10 信号 → 短路。
    fireActivity({ session_id: 's1', seq: 10 })
    await vi.advanceTimersByTimeAsync(2000)
    expect(countHistoryRequests()).toBe(1)
    w.unmount()
  })

  it('异会话信号与本端发送态（streaming）均不触发刷新', async () => {
    prepareSession()
    const { useChatStore } = await import('../../stores/chat')
    vi.useFakeTimers()
    const w = mount(ChatPanel)
    await flushPromises()
    feedHistory(lastRequestId(), [
      { role: 'user', content: 'q' },
      { role: 'assistant', content: 'a' },
    ])
    await flushPromises()

    // 异会话信号：忽略。
    fireActivity({ session_id: 'other', seq: 99 })
    await vi.advanceTimersByTimeAsync(2000)
    expect(countHistoryRequests()).toBe(1)

    // 本会话落后信号但正在本地发送：不打断流式态。
    const chat = useChatStore()
    chat.streaming = true
    fireActivity({ session_id: 's1', seq: 99 })
    await vi.advanceTimersByTimeAsync(2000)
    expect(countHistoryRequests()).toBe(1)
    expect(chat.streaming).toBe(true)
    w.unmount()
  })

  it('P8 精修：本端 tool_event 实时帧推进游标 → 同 seq 信号短路（无谓刷新消除）', async () => {
    prepareSession()
    vi.useFakeTimers()
    const w = mount(ChatPanel)
    await flushPromises()
    feedHistory(lastRequestId(), [
      { role: 'user', content: 'q' },
      { role: 'assistant', content: 'a' },
    ])
    await flushPromises()

    // 本端实时收到 tool_event（seq=9，pump 注入）→ 游标推进到 9。
    mountedHandler()({
      type: 'push',
      cmd: 'tool_event',
      data: {
        kind: 'ToolStarted',
        seq: 9,
        data: { call_id: 'c9', tool: 'exec', session_id: 's1' },
      },
    })
    await flushPromises()
    expect(w.findAll('.tool-card').length).toBeGreaterThanOrEqual(1)

    // 对应 chat.activity（seq=9）到达：游标已追平 → 短路，不刷新不重拉。
    fireActivity({ session_id: 's1', seq: 9, kind: 'tool' })
    await vi.advanceTimersByTimeAsync(2000)
    expect(countHistoryRequests()).toBe(1)
    w.unmount()
  })

  it('P8 精修：kind=tool 落后信号 → 增量 sync 补工具卡（不整页重拉）', async () => {
    prepareSession()
    let syncN = 0
    requestMock.mockImplementation((_mod: string, cmd: string) => {
      if (cmd === 'inbox_status') return Promise.resolve({ available: true, busy: true })
      if (cmd === 'sync') {
        syncN += 1
        // 基线（after=0）返回空；落后信号触发的增量（after>0）返回 tool。
        if (syncN === 1) return Promise.resolve({})
        return Promise.resolve({
          events: [toolEvent(7, 'ToolStarted', { call_id: 'c7', tool: 'exec', session_id: 's1' })],
        })
      }
      return Promise.resolve({})
    })
    vi.useFakeTimers()
    const w = mount(ChatPanel)
    await flushPromises()
    feedHistory(lastRequestId(), [
      { role: 'user', content: 'q' },
      { role: 'assistant', content: 'a' },
    ])
    await flushPromises()

    fireActivity({ session_id: 's1', seq: 7, kind: 'tool' })
    await vi.advanceTimersByTimeAsync(1000)
    await flushPromises()
    // 增量通道生效：工具卡补进来，但 history_request（全量）未增加。
    expect(w.findAll('.tool-card').length).toBeGreaterThanOrEqual(1)
    expect(countHistoryRequests()).toBe(1)
    w.unmount()
  })
})
