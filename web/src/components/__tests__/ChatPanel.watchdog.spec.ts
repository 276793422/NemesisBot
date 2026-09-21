import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount, flushPromises } from '@vue/test-utils'

// 2026-09-21 watchdog 三修复回归钉（真机 BUG：exec 审批挂起 45s → watchdog
// 误触发恢复 → 窗口错位误判「回复已落」→ 磁盘 50 条重灌视图，工具卡/占位
// 全冲掉，n=22→50）：
// - 修复①：onWatchdog 触发时先看轮次活跃迹象（审批挂起 pendingApprovals /
//   本轮工具事件未 flush）→ 只续期不重灌；活跃续期 12 次上限后走原恢复路径。
// - 修复②：「回复已落」判定语义化——磁盘历史尾部向前，本轮发送文本的
//   user 行之前出现 assistant 行才算已落。原计数式判定（磁盘 50 条 vs 发送
//   时视图 ~20 条的 assistant 数）窗口错位，磁盘计数天然偏大，轮次还在跑
//   也被判已落 → 误重灌。
// - 修复③：重灌只有文本行——replayToolsFromRing 从环回放挂回最后一轮
//   工具卡（primeSeqBaseline 同语义提炼共用）。

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

import { onMessage, sendHistoryRequest, send as wsSend } from '../../composables/useWebSocket'
import ChatPanel from '../ChatPanel.vue'
import { useChatStore } from '../../stores/chat'
import { useSessionStore } from '../../stores/session'
import {
  useApprovals,
  _resetApprovalsForTest,
} from '../../composables/useApprovals'

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

function countHistoryRequests(): number {
  return vi.mocked(sendHistoryRequest).mock.calls.length
}

function prepareSession() {
  const sessionStore = useSessionStore()
  sessionStore.sessions = [
    { id: 's1', channel: 'web', startTime: '', lastTime: '', messageCount: 0, firstMessage: 'A', model: '' },
  ] as any
  sessionStore.currentId = 's1'
}

/** 挂载 + 喂入基线历史（q1/a1 一轮完整）→ 返回 wrapper。 */
async function mountWithHistory() {
  const w = mount(ChatPanel)
  await flushPromises()
  feedHistory(lastRequestId(), [
    { role: 'user', content: 'q1' },
    { role: 'assistant', content: 'a1' },
  ])
  await flushPromises()
  return w
}

/** 走真实 sendMessage 路径武装 watchdog（startWatchdog 记录本轮文本锚）。
 * 输入区 Ctrl+Enter——比按钮 click 稳（面板多弹层下 btn-primary 的 click
 * 在 jsdom 里不总是触达 handler）。 */
async function sendFromUI(w: ReturnType<typeof mount>, content: string) {
  const chat = useChatStore()
  const ta = w.find('textarea')
  await ta.setValue(content)
  await ta.trigger('keydown', { key: 'Enter', ctrlKey: true })
  await flushPromises()
  expect(chat.streaming, '发送后进入流式态').toBe(true)
}

/** 模拟 CRITICAL 审批挂起（SSE approval-requested 的等价本地态）。 */
function hangApproval(requestId = 'appr-1') {
  const { pendingApprovals } = useApprovals()
  pendingApprovals.push({
    request_id: requestId,
    operation: 'process_exec',
    target: 'echo x',
    risk_level: 'CRITICAL',
    reason: 'test',
    timeout_secs: 300,
    expiresAt: Date.now() + 300_000,
  } as any)
}

beforeEach(() => {
  setActivePinia(createPinia())
  requestMock.mockReset()
  requestMock.mockImplementation((_mod: string, cmd: string) => {
    if (cmd === 'inbox_status') return Promise.resolve({ available: true, busy: true })
    return Promise.resolve({})
  })
  vi.mocked(sendHistoryRequest).mockClear()
  vi.mocked(wsSend).mockClear()
  vi.mocked(onMessage).mockClear()
  _resetApprovalsForTest()
})

afterEach(() => {
  _resetApprovalsForTest()
  vi.useRealTimers()
})

describe('修复①：审批挂起 / 工具未收尾 → watchdog 续期不重灌', () => {
  it('审批挂起中 45s 触发 → 不发恢复请求、streaming/视图保持', async () => {
    prepareSession()
    vi.useFakeTimers()
    const w = await mountWithHistory()
    const chat = useChatStore()
    await sendFromUI(w, 'run echo V1')
    expect(chat.streaming).toBe(true)
    hangApproval()

    const before = countHistoryRequests()
    const msgCount = chat.messages.length
    await vi.advanceTimersByTimeAsync(45_000)
    await flushPromises()

    // 未发 reloadLatest（历史请求数不变）、流式态与本地视图原样。
    expect(countHistoryRequests()).toBe(before)
    expect(chat.streaming).toBe(true)
    expect(chat.messages.length).toBe(msgCount)
    w.unmount()
  })

  it('本轮工具事件未 flush（回复未到）→ 同样续期', async () => {
    prepareSession()
    vi.useFakeTimers()
    const w = await mountWithHistory()
    const chat = useChatStore()
    await sendFromUI(w, 'run echo V2')
    // 工具开始未结束（exec 执行中 / 审批中 SDK 不发 Finished 的残留同理）。
    mountedHandler()({
      type: 'push',
      cmd: 'tool_event',
      data: { kind: 'ToolStarted', seq: 9, data: { call_id: 'c1', tool: 'exec', session_id: 's1' } },
    })
    await flushPromises()

    const before = countHistoryRequests()
    await vi.advanceTimersByTimeAsync(45_000)
    await flushPromises()
    expect(countHistoryRequests()).toBe(before)
    expect(chat.streaming).toBe(true)
    // 工具卡还在（未被重灌冲掉）。
    expect(w.findAll('.tool-card').length).toBeGreaterThanOrEqual(1)
    w.unmount()
  })

  it('活跃续期有上限：12 次后走原恢复路径（reloadLatest 发出）', async () => {
    prepareSession()
    vi.useFakeTimers()
    const w = await mountWithHistory()
    await sendFromUI(w, 'run echo V3')
    hangApproval()

    const before = countHistoryRequests()
    // 12 次续期内不发；第 13 个周期（12*45s 之后）走 reloadLatest。
    await vi.advanceTimersByTimeAsync(12 * 45_000)
    expect(countHistoryRequests()).toBe(before)
    await vi.advanceTimersByTimeAsync(45_000)
    await flushPromises()
    expect(countHistoryRequests()).toBe(before + 1)
    expect(lastRequestId().startsWith('watchdog_')).toBe(true)
    w.unmount()
  })

  it('无活跃迹象（纯静默）→ 45s 即走原恢复路径（既有行为不回归）', async () => {
    prepareSession()
    vi.useFakeTimers()
    const w = await mountWithHistory()
    await sendFromUI(w, 'run echo V4')

    const before = countHistoryRequests()
    await vi.advanceTimersByTimeAsync(45_000)
    await flushPromises()
    expect(countHistoryRequests()).toBe(before + 1)
    w.unmount()
  })
})

describe('修复②：「回复已落」语义化 landed 判定（替代窗口错位计数比较）', () => {
  it('磁盘窗口 assistant 计数偏大（旧轮次在前）但尾部是本轮 user → 未落：不重灌、狗重试', async () => {
    prepareSession()
    vi.useFakeTimers()
    const w = await mountWithHistory()
    const chat = useChatStore()
    await sendFromUI(w, 'run echo V5')

    await vi.advanceTimersByTimeAsync(45_000)
    await flushPromises()
    // reloadLatest 已发出（无活跃迹象），requestId 是 watchdog_ 前缀。
    expect(lastRequestId().startsWith('watchdog_')).toBe(true)

    // 磁盘 50 条：大量旧轮次 assistant（计数远超发送时视图——原计数式
    // 判定在此误判「已落」立即重灌），尾部是本轮 user 行、无回复。
    const disk: any[] = []
    for (let i = 0; i < 12; i++) disk.push({ role: 'user', content: `old q ${i}` })
    for (let i = 0; i < 12; i++) disk.push({ role: 'assistant', content: `old a ${i}` })
    disk.push({ role: 'user', content: 'run echo V5' })
    feedHistory(lastRequestId(), disk)
    await flushPromises()

    // 未落 → 视图保持（不被 50 条重灌）、streaming 保持、狗重试（再推进
    // 45s 又发一次 reloadLatest）。
    expect(chat.messages.length).toBeLessThan(disk.length)
    expect(chat.messages[chat.messages.length - 1].content).toBe('run echo V5')
    expect(chat.streaming).toBe(true)
    await vi.advanceTimersByTimeAsync(45_000)
    await flushPromises()
    expect(countHistoryRequests()).toBeGreaterThanOrEqual(2)
    w.unmount()
  })

  it('本轮 user 行之后磁盘有 assistant → 已落：重灌生效、streaming 关、回复可见', async () => {
    prepareSession()
    vi.useFakeTimers()
    const w = await mountWithHistory()
    const chat = useChatStore()
    await sendFromUI(w, 'run echo V6')

    await vi.advanceTimersByTimeAsync(45_000)
    await flushPromises()

    feedHistory(lastRequestId(), [
      { role: 'user', content: 'q1' },
      { role: 'assistant', content: 'a1' },
      { role: 'user', content: 'run echo V6' },
      { role: 'assistant', content: 'V6 输出完成' },
    ])
    await flushPromises()

    // 已落 → replaceMessages（回复出现在视图）、streaming 关、狗清。
    expect(chat.streaming).toBe(false)
    expect(w.text()).toContain('V6 输出完成')
    expect(chat.messages[chat.messages.length - 1].role).toBe('assistant')
    w.unmount()
  })
})

describe('修复③：重灌后从环回放挂回工具卡', () => {
  it('landed 重灌 → chat.sync 回放的 tool 条目挂到最后 assistant 消息', async () => {
    prepareSession()
    requestMock.mockImplementation((_mod: string, cmd: string) => {
      if (cmd === 'inbox_status') return Promise.resolve({ available: true, busy: true })
      if (cmd === 'sync') {
        return Promise.resolve({
          events: [
            { seq: 1, role: 'user', content: 'q1', ts: 'x' },
            toolEvent(2, 'ToolStarted', { call_id: 'c1', tool: 'exec', args_preview: 'echo V7' }),
            toolEvent(3, 'ToolFinished', { call_id: 'c1', ok: true, duration_ms: 80 }),
            { seq: 4, role: 'assistant', content: 'V7 done', ts: 'x' },
          ],
        })
      }
      return Promise.resolve({})
    })
    vi.useFakeTimers()
    const w = await mountWithHistory()
    const chat = useChatStore()
    await sendFromUI(w, 'run echo V7')

    await vi.advanceTimersByTimeAsync(45_000)
    await flushPromises()
    feedHistory(lastRequestId(), [
      { role: 'user', content: 'run echo V7' },
      { role: 'assistant', content: 'V7 done' },
    ])
    await flushPromises()

    // 重灌后回放挂卡：最后的 assistant 消息带 toolEvents，卡片渲染。
    const last = chat.messages[chat.messages.length - 1]
    expect(last.role).toBe('assistant')
    expect(last.toolEvents).toHaveLength(1)
    expect(last.toolEvents![0].argsPreview).toContain('echo V7')
    expect(w.findAll('.message .tool-card').length).toBeGreaterThanOrEqual(1)
    w.unmount()
  })
})

/** tool 条目（chat_event_log kind="tool" 形态）。 */
function toolEvent(seq: number, kind: string, data: any) {
  return { seq, kind: 'tool', role: '', content: '', tool: { kind, data } }
}
