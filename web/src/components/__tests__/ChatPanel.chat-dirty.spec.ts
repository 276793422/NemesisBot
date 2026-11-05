import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount, flushPromises } from '@vue/test-utils'

// BUG②（2026-11-05 死房间补拉 hint）：子代理/后台任务完成回复的目标连接
// 房间已消亡（页面刷新）时投递失败，后端向全部活房间扇出 `chat_dirty`
// push（只带 session_id + reason）。面板钉住三件事：
// 1. 正在看该会话 → 节流（trailing 800ms）触发 chat.sync 增量补拉；
// 2. 异会话/无 session_id 的 hint 忽略（定向会话信号，无 legacy 回退面）；
// 3. 节流窗内重复 hint 只补拉一次。

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

import { onMessage } from '../../composables/useWebSocket'
import ChatPanel from '../ChatPanel.vue'
import { useChatStore } from '../../stores/chat'

function wsHandler(): (frame: any) => void {
  const calls = vi.mocked(onMessage).mock.calls
  expect(calls.length, 'onMessage must have been called on mount').toBeGreaterThan(0)
  return calls[calls.length - 1][0] as (frame: any) => void
}

beforeEach(() => {
  setActivePinia(createPinia())
  requestMock.mockReset()
  requestMock.mockResolvedValue({})
  vi.mocked(onMessage).mockClear()
  vi.useFakeTimers()
})

afterEach(() => {
  vi.useRealTimers()
})

async function mountEmbedded(sid: string) {
  const wrapper = mount(ChatPanel, { props: { sessionId: sid } })
  await flushPromises()
  // 首拉历史走 sendHistoryRequest（mock 无响应，10s 安全超时在假计时器下
  // 不会到期）——手动置齐两态放行 syncMissedChat 入口守卫（sync-fence /
  // node-badge.spec 同款骨架）。
  const chatStore = useChatStore()
  chatStore.historyLoading = false
  chatStore.historyLoaded = true
  return wrapper
}

function syncCalls(): any[] {
  return requestMock.mock.calls.filter((c) => c[0] === 'chat' && c[1] === 'sync')
}

describe('ChatPanel chat_dirty 死房间补拉 hint (BUG②)', () => {
  it('本会话 hint → 节流 800ms 后触发一次 chat.sync', async () => {
    const wrapper = await mountEmbedded('gen-1')
    const h = wsHandler()

    h({ type: 'push', cmd: 'chat_dirty', data: { session_id: 'gen-1', reason: 'delivery_missed' } })
    await flushPromises()
    expect(syncCalls()).toHaveLength(0) // trailing 窗内尚未触发

    vi.advanceTimersByTime(800)
    await flushPromises()
    expect(syncCalls()).toHaveLength(1)
    expect(syncCalls()[0][2]).toMatchObject({ session_id: 'gen-1' })

    wrapper.unmount()
  })

  it('窗内重复 hint 只补拉一次；窗后再来 → 再补一次', async () => {
    const wrapper = await mountEmbedded('gen-1')
    const h = wsHandler()

    h({ type: 'push', cmd: 'chat_dirty', data: { session_id: 'gen-1', reason: 'delivery_missed' } })
    h({ type: 'push', cmd: 'chat_dirty', data: { session_id: 'gen-1', reason: 'delivery_missed' } })
    h({ type: 'push', cmd: 'chat_dirty', data: { session_id: 'gen-1', reason: 'delivery_missed' } })
    await flushPromises()
    vi.advanceTimersByTime(800)
    await flushPromises()
    expect(syncCalls()).toHaveLength(1)

    h({ type: 'push', cmd: 'chat_dirty', data: { session_id: 'gen-1', reason: 'delivery_missed' } })
    vi.advanceTimersByTime(800)
    await flushPromises()
    expect(syncCalls()).toHaveLength(2)

    wrapper.unmount()
  })

  it('异会话 hint 忽略，不触发补拉', async () => {
    const wrapper = await mountEmbedded('gen-1')
    const h = wsHandler()

    h({ type: 'push', cmd: 'chat_dirty', data: { session_id: 'other-sess', reason: 'delivery_missed' } })
    vi.advanceTimersByTime(2000)
    await flushPromises()
    expect(syncCalls()).toHaveLength(0)

    wrapper.unmount()
  })

  it('无 session_id 的 hint 忽略（无锚不猜）', async () => {
    const wrapper = await mountEmbedded('gen-1')
    const h = wsHandler()

    h({ type: 'push', cmd: 'chat_dirty', data: { reason: 'delivery_missed' } })
    vi.advanceTimersByTime(2000)
    await flushPromises()
    expect(syncCalls()).toHaveLength(0)

    wrapper.unmount()
  })
})
