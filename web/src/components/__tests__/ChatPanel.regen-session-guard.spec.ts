import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount, flushPromises } from '@vue/test-utils'

// 回退链跨会话守卫回归（resync 10s 安全超时 + 切换围栏的组合缝隙）：
// - BUG：rewindToTurnStart await resyncFromLog() 窗口内切换会话 → resync
//   响应被切换围栏丢弃（inFlightHistory.clear()）→ Promise 由 10s 安全超时
//   放行 → doRegenerate 续体照走 sendValidated(旧会话提问原文)——而
//   sendValidated 全程用活的 effectiveSid，旧会话的提问原文被发进新会话
//   并触发真实 agent 执行。
// - 修复 a：resync 放行后校验 effectiveSid 仍是回退目标会话，不匹配则
//   中止续作（回退本身按显式 sid 已落在旧会话，正确，只裁续作）。
// - 修复 b：会话切换 watcher 与 inFlightHistory.clear() 同点清
//   pendingResync/pendingWatchdogReload——旗标是组件实例级残留，不清则
//   新会话首个历史响应被 resync 分支冒领（setBusy 误清新会话 busy、
//   historyLoaded 不落，后续翻页/重连闸全部错位）。

const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
}))

/** 捕获槽（vi.hoisted——mock 工厂在 import 期执行，普通顶层 let 会 TDZ）。 */
const h = vi.hoisted(() => ({
  wsHandler: null as ((data: any) => void) | null,
  wsStatus: null as any,
}))

vi.mock('../../composables/useWebSocket', async () => {
  const { ref } = await import('vue')
  h.wsStatus = ref<'connected' | 'disconnected'>('connected')
  return {
    connect: vi.fn(),
    send: vi.fn(),
    sendHistoryRequest: vi.fn(),
    onMessage: vi.fn((fn: (data: any) => void) => { h.wsHandler = fn }),
    addMessageHandler: vi.fn(),
    removeMessageHandler: vi.fn(),
    wsStatus: h.wsStatus,
  }
})

import ChatPanel from '../ChatPanel.vue'
import { sendHistoryRequest, send as wsSend } from '../../composables/useWebSocket'
import { useChatStore } from '../../stores/chat'
import { useSessionStore } from '../../stores/session'
import { useToast } from '../../composables/useToast'

beforeEach(() => {
  setActivePinia(createPinia())
  requestMock.mockReset()
  requestMock.mockResolvedValue({})
  h.wsHandler = null
  h.wsStatus.value = 'connected'
})

afterEach(() => {
  vi.useRealTimers()
})

function prepareSessions() {
  const s = useSessionStore()
  s.sessions = [
    { id: 's1', channel: 'web', startTime: '', lastTime: '', messageCount: 0, firstMessage: 'A', model: '' },
    { id: 's2', channel: 'web', startTime: '', lastTime: '', messageCount: 0, firstMessage: 'B', model: '' },
  ] as any
  s.currentId = 's1'
}

function feedHistory(requestId: string, messages: any[]) {
  h.wsHandler!({
    type: 'message',
    module: 'chat',
    cmd: 'history_response',
    data: { request_id: requestId, messages, has_more: false, oldest_index: 0 },
  })
}

function lastHistoryRequestId(): string {
  const calls = vi.mocked(sendHistoryRequest).mock.calls
  return calls[calls.length - 1][0] as string
}

async function mountWithS1History() {
  prepareSessions()
  const w = mount(ChatPanel)
  await flushPromises()
  feedHistory(lastHistoryRequestId(), [
    { role: 'user', content: 'q1' },
    { role: 'assistant', content: 'a1' },
  ])
  await flushPromises()
  return w
}

describe('ChatPanel 回退链跨会话守卫（resync 安全超时缝隙）', () => {
  it('重新生成 await 窗口内切换会话：10s 超时放行后中止重发，不把旧会话提问发进新会话', async () => {
    vi.useFakeTimers()
    const w = await mountWithS1History()
    const chat = useChatStore()
    expect(chat.messages).toHaveLength(2)

    // assistant 行（a1）的「↻ 重新生成」→ doRegenerate → rewind 请求落地 →
    // resyncFromLog 挂起（其响应不喂）。
    const regenBtn = w.findAll('.msg-action-btn').find(b => b.text().includes('重新生成'))
    expect(regenBtn, 'assistant 行应有重新生成入口').toBeTruthy()
    await regenBtn!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('sessions', 'rewind_to_message', {
      session_id: 's1',
      message_index: 0,
      mode: 'before',
    })

    // await 窗口内切换到 s2：切换围栏清掉在飞 resync 登记（响应永不到达）。
    useSessionStore().currentId = 's2'
    await flushPromises()

    // 10s 安全超时放行 resyncFromLog——守卫必须在此中止重发。
    await vi.advanceTimersByTimeAsync(10_000)
    await flushPromises()

    // 主断言对准真实发送通道（sendValidated → useWebSocket send）；副断言
    // 兜 chat.send WSAPI 请求面。
    const wsSendCalls = vi.mocked(wsSend).mock.calls.filter(
      (c: any[]) => c[0] === 'q1',
    )
    expect(wsSendCalls, '旧会话提问原文不得发进新会话').toHaveLength(0)
    const sendCalls = requestMock.mock.calls.filter(
      (c: any[]) => c[0] === 'chat' && c[1] === 'send',
    )
    expect(sendCalls).toHaveLength(0)
    const toasts = useToast().toasts
    expect(
      toasts.some(t => t.message.includes('会话已切换')),
      '应 toast 告知续作已中止',
    ).toBe(true)
    w.unmount()
  })

  it('切换会话清分支旗标：新会话首个页响应走 page 分支（historyLoaded 落位），不被 resync 分支冒领', async () => {
    const w = await mountWithS1History()

    // user 行（q1）的「⏪ 撤销此问」→ rewind → resyncFromLog 挂起（不喂响应）
    // → pendingResync = true 残留。
    const undoBtn = w.findAll('.msg-action-btn').find(b => b.text().includes('撤销此问'))
    expect(undoBtn, 'user 行应有撤销此问入口').toBeTruthy()
    await undoBtn!.trigger('click')
    await flushPromises()

    // 切到 s2（watcher 清旗标）→ 新会话 loadHistory 在飞 → 喂 s2 页响应。
    useSessionStore().currentId = 's2'
    await flushPromises()
    const chat = useChatStore()
    feedHistory(lastHistoryRequestId(), [{ role: 'user', content: 's2q' }])
    await flushPromises()

    expect(chat.historyLoaded, 's2 页响应必须走 page 分支落 historyLoaded').toBe(true)
    expect(chat.messages.map(m => m.content)).toEqual(['s2q'])
    w.unmount()
  })
})
