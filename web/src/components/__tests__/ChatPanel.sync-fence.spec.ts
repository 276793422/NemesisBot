import { describe, it, expect, vi, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount, flushPromises } from '@vue/test-utils'

// 2026-09-24 会话围栏回归（与 refreshUsage/syncAgentMode 同纪律）：异步
// 响应在飞期间切换会话，旧会话的响应必须整包丢弃——
// - syncMissedChat（断线重连补拉）：旧会话事件不得追加进新会话视图，
//   也不得把旧会话 seq 推进补拉游标；
// - replayToolsFromRing（历史落地后的 primeSeqBaseline 环回放）：旧会话
//   工具卡不得挂进新会话视图。

const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
}))

/** 捕获槽（vi.hoisted——mock 工厂在 import 期执行，普通顶层 let 会 TDZ）。 */
const h = vi.hoisted(() => ({
  wsHandler: null as ((data: any) => void) | null,
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
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
import { sendHistoryRequest } from '../../composables/useWebSocket'
import { useChatStore } from '../../stores/chat'
import { useSessionStore } from '../../stores/session'

beforeEach(() => {
  setActivePinia(createPinia())
  requestMock.mockReset()
  requestMock.mockResolvedValue({})
  h.wsHandler = null
  h.wsStatus.value = 'connected'
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

describe('ChatPanel 在飞响应的会话围栏（2026-09-24）', () => {
  it('sync 补拉在飞中切换会话：旧会话事件整包丢弃，不进新会话视图', async () => {
    prepareSessions()
    const w = mount(ChatPanel)
    await flushPromises()
    // 首拉历史走 sendHistoryRequest（mock 无响应）——手动置齐两态以放行
    // syncMissedChat（同 node-badge.spec 骨架）。
    const chatStore = useChatStore()
    chatStore.historyLoading = false
    chatStore.historyLoaded = true

    // sync 挂起 → 断开→重连触发 syncMissedChat（在飞）。
    let resolveSync!: (v: any) => void
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'sync') {
        return new Promise(resolve => { resolveSync = resolve })
      }
      return Promise.resolve({})
    })
    h.wsStatus.value = 'disconnected'
    await flushPromises()
    h.wsStatus.value = 'connected'
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('chat', 'sync', { session_id: 's1', after_seq: 0 })

    // 在飞中切换会话（切换链已 reset+重拉 s2）→ 旧会话 sync 落地必须被丢弃。
    useSessionStore().currentId = 's2'
    await flushPromises()
    resolveSync({
      events: [
        { seq: 1, role: 'assistant', content: '旧会话补拉行', ts: '2026-09-24T10:00:00+08:00' },
      ],
    })
    await flushPromises()

    expect(chatStore.messages).toHaveLength(0)
    expect(w.text()).not.toContain('旧会话补拉行')
    w.unmount()
  })

  it('环回放在飞中切换会话：旧会话工具卡不挂进新会话', async () => {
    prepareSessions()
    // sync 挂起：历史响应落地后 primeSeqBaseline 会发起环回放并挂住。
    let resolveSync!: (v: any) => void
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'sync') {
        return new Promise(resolve => { resolveSync = resolve })
      }
      return Promise.resolve({})
    })
    const w = mount(ChatPanel)
    await flushPromises()
    feedHistory(lastHistoryRequestId(), [{ role: 'assistant', content: '旧会话历史回复' }])
    await flushPromises()
    // primeSeqBaseline 的环回放已在飞。
    expect(requestMock).toHaveBeenCalledWith('chat', 'sync', { session_id: 's1', after_seq: 0 })

    const chatStore = useChatStore()
    expect(chatStore.messages).toHaveLength(1)

    // 在飞中切换会话（切换链 reset）→ 旧会话环回放落地必须被丢弃。
    useSessionStore().currentId = 's2'
    await flushPromises()
    resolveSync({
      events: [
        {
          seq: 1,
          kind: 'tool',
          role: '',
          content: '',
          tool: { kind: 'ToolStarted', data: { call_id: 'c1', name: 'exec', args: '{}' } },
        },
      ],
    })
    await flushPromises()

    expect(chatStore.messages).toHaveLength(0)
    expect(chatStore.pendingToolEvents).toHaveLength(0)
    w.unmount()
  })
})
