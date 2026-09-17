import { describe, it, expect, vi, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount, flushPromises } from '@vue/test-utils'

// HD（2026-09-17）：历史请求 request_id 路由围栏。
//
// 根因：handleHistoryResponse 从不比对 request_id——会话切换 reset() 清掉
// historyLoading 守卫后，两个在飞请求的响应先后到达各自 prependHistory →
// 消息成对重复（U,NB,U,NB）；迟到的 hist_ 响应还会冒领 pendingResync /
// pendingWatchdogReload 标志拿错误 payload 整段替换。
//
// 修复语义（本 spec 钉死）：
// - 响应按 request_id 匹配在飞登记，不匹配即丢（不前插、不清加载态、
//   不冒领标志）；
// - 一次匹配即焚（同 rid 二次响应丢弃）；
// - 响应带 session_id 且与当前会话不符 → 丢弃（快速切换防串台）；
// - 会话切换 → 全部在飞登记作废。

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

import { sendHistoryRequest, onMessage } from '../../composables/useWebSocket'
import ChatPanel from '../ChatPanel.vue'
import { useChatStore } from '../../stores/chat'
import { useSessionStore } from '../../stores/session'

beforeEach(() => {
  setActivePinia(createPinia())
  requestMock.mockReset()
  requestMock.mockResolvedValue({})
  vi.mocked(sendHistoryRequest).mockClear()
  vi.mocked(onMessage).mockClear()
})

/** mount 并取回组件注册的 WS 下行帧 handler。 */
async function mountAndCapture() {
  const sessionStore = useSessionStore()
  sessionStore.sessions = [
    { id: 's1', channel: 'web', startTime: '', lastTime: '', messageCount: 0, firstMessage: 'A', model: '' },
    { id: 's2', channel: 'web', startTime: '', lastTime: '', messageCount: 0, firstMessage: 'B', model: '' },
  ] as any
  sessionStore.currentId = 's1'

  const wrapper = mount(ChatPanel)
  await flushPromises()
  const handler = vi.mocked(onMessage).mock.calls.at(-1)![0] as (frame: any) => void
  return { wrapper, sessionStore, handler }
}

function historyFrame(rid: string, sessionId: string | null, count = 1) {
  return {
    type: 'message',
    module: 'chat',
    cmd: 'history',
    data: {
      request_id: rid,
      session_id: sessionId,
      has_more: false,
      oldest_index: 0,
      total_count: count,
      messages: Array.from({ length: count }, (_, i) => ({
        role: 'user',
        content: `msg-${rid}-${i}`,
        timestamp: '2026-09-17T10:00:00Z',
      })),
    },
  }
}

describe('历史响应 request_id 围栏（HD）', () => {
  it('匹配 request_id 的响应正常前插', async () => {
    const { wrapper, handler } = (await mountAndCapture()) as any
    const chat = useChatStore()
    const rid = vi.mocked(sendHistoryRequest).mock.calls[0][0] as string

    handler(historyFrame(rid, 's1', 2))
    await flushPromises()

    expect(chat.messages).toHaveLength(2)
    wrapper.unmount()
  })

  it('未登记 request_id 的响应被丢弃：不前插、不清加载态', async () => {
    const { wrapper, handler } = (await mountAndCapture()) as any
    const chat = useChatStore()
    const chatStore = chat as ReturnType<typeof useChatStore>
    expect(chatStore.historyLoading).toBe(true)

    // 迟到/并行/伪造的响应（rid 未登记）。
    handler(historyFrame('hist_stale', 's1', 3))
    await flushPromises()

    expect(chatStore.messages).toHaveLength(0)
    expect(chatStore.historyLoading).toBe(true)
    wrapper.unmount()
  })

  it('一次匹配即焚：同 request_id 二次响应丢弃（成对重复根因）', async () => {
    const { wrapper, handler } = (await mountAndCapture()) as any
    const chat = useChatStore()
    const rid = vi.mocked(sendHistoryRequest).mock.calls[0][0] as string

    handler(historyFrame(rid, 's1', 2))
    handler(historyFrame(rid, 's1', 2)) // 竞态双响应
    await flushPromises()

    expect(chat.messages).toHaveLength(2)
    wrapper.unmount()
  })

  it('会话切换作废在飞：旧会话响应被丢弃，新会话响应正常前插', async () => {
    const { wrapper, sessionStore, handler } = await mountAndCapture() as any
    const oldRid = vi.mocked(sendHistoryRequest).mock.calls[0][0] as string

    // 切换会话 → watch 作废在飞 + reset + 发起新请求。
    sessionStore.currentId = 's2'
    await flushPromises()
    expect(vi.mocked(sendHistoryRequest).mock.calls.length).toBeGreaterThanOrEqual(2)
    const newRid = vi.mocked(sendHistoryRequest).mock.calls.at(-1)![0] as string

    // 旧请求的响应迟到 → 围栏丢弃（不串台、不双前插）。
    handler(historyFrame(oldRid, 's1', 2))
    await flushPromises()
    const chatStore = useChatStore()
    expect(chatStore.messages).toHaveLength(0)

    // 新请求的响应正常落地。
    handler(historyFrame(newRid, 's2', 1))
    await flushPromises()
    expect(chatStore.messages).toHaveLength(1)
    expect(chatStore.messages[0].content).toContain(newRid)
    wrapper.unmount()
  })

  it('session_id 与当前会话不符的响应被丢弃（防串台）', async () => {
    const { wrapper, handler } = await mountAndCapture() as any
    const rid = vi.mocked(sendHistoryRequest).mock.calls[0][0] as string

    handler(historyFrame(rid, 's2', 2)) // 响应归属 s2，当前选中 s1
    await flushPromises()

    expect(useChatStore().messages).toHaveLength(0)
    wrapper.unmount()
  })
})
