import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount, flushPromises } from '@vue/test-utils'

// B 批切换恢复整改回归钉（2026-11-19）：
// - B1（store 级）：ensurePendingWork 惰性建槽锚定本会话**最后一条消息
//   时间戳**——切回恢复期早到事件先于 work_current 回包建槽时，时间线
//   起点不用 Date.now() 从零走表（执行用时撒谎）。
// - B2（组件级）：detectPendingTurn 起轮置 workRestoring → 占位行渲染
//   「恢复中」骨架（work_current 快照落地前不裸等）；快照采纳后骨架让位
//   给真时间线；null 盲区不解骨架（等轮询下一拍）；死轮停轮骨架收口。
// - A4：live 过程占位行无头像（头像只属于答案）。

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

import { onMessage, sendHistoryRequest } from '../../composables/useWebSocket'
import ChatPanel from '../ChatPanel.vue'
import { useChatStore } from '../../stores/chat'
import { useSessionStore } from '../../stores/session'

beforeEach(() => {
  setActivePinia(createPinia())
  requestMock.mockReset()
  // 默认：inbox busy（轮询持续）、work_current 盲区（res.work == null）。
  requestMock.mockImplementation((_mod: string, cmd: string) => {
    if (cmd === 'inbox_status') return Promise.resolve({ available: true, busy: true })
    return Promise.resolve({})
  })
  vi.mocked(sendHistoryRequest).mockClear()
  vi.mocked(onMessage).mockClear()
})

afterEach(() => {
  vi.useRealTimers()
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

describe('B1：pendingWork 建槽锚定消息尾部时间戳', () => {
  it('惰性建槽 started_at_ms = 尾部消息 timestamp（非 Date.now()）', () => {
    const chat = useChatStore()
    const anchorIso = '2026-11-19T10:00:00.000Z'
    chat.messages.push(
      { role: 'user', content: '跑个长任务', timestamp: '2026-11-19T09:59:00.000Z' } as any,
      { role: 'user', content: '（切回，悬空）', timestamp: anchorIso } as any,
    )
    // 首个中间事件建槽——锚必须是尾部消息时间戳。
    chat.ingestWorkRoundText('开始执行…')
    expect(chat.pendingWork).not.toBeNull()
    expect(chat.pendingWork!.started_at_ms).toBe(Date.parse(anchorIso))
  })

  it('空视图（无锚）回落 Date.now()（新 turn 首个事件本无更早锚）', () => {
    const chat = useChatStore()
    const before = Date.now()
    chat.ingestWorkRoundText('首轮叙述')
    expect(chat.pendingWork).not.toBeNull()
    expect(chat.pendingWork!.started_at_ms).toBeGreaterThanOrEqual(before)
  })
})

describe('B2：恢复中骨架生命周期（占位行不裸等 work_current）', () => {
  it('悬空 user 起轮 + work_current 盲区 → 「恢复中」骨架可见', async () => {
    prepareSession()
    vi.useFakeTimers()
    const w = mount(ChatPanel)
    await flushPromises()

    feedHistory(lastRequestId(), [{ role: 'user', content: '切回时还在跑' }])
    await flushPromises()

    // 骨架出现（无快照可采纳，占位行立即可见——不裸等）。
    expect(w.find('.work-restoring').exists()).toBe(true)
    expect(w.text()).toContain('正在恢复过程视图')
    // 无头像（A4：过程行无头像）。
    expect(w.find('.process-live .message-avatar').exists()).toBe(false)
    // 盲区不解骨架：轮询一拍后仍显示（等下一拍）。
    await vi.advanceTimersByTimeAsync(4000)
    await flushPromises()
    expect(w.find('.work-restoring').exists()).toBe(true)
    w.unmount()
  })

  it('work_current 快照采纳 → 骨架让位给真时间线', async () => {
    prepareSession()
    requestMock.mockImplementation((_mod: string, cmd: string) => {
      if (cmd === 'inbox_status') return Promise.resolve({ available: true, busy: true })
      if (cmd === 'work_current') {
        return Promise.resolve({
          work: {
            started_at_ms: 1000,
            ended_at_ms: 2000,
            rounds: [{ text: '第一步：分析现场', tools: [] }],
          },
        })
      }
      return Promise.resolve({})
    })
    vi.useFakeTimers()
    const w = mount(ChatPanel)
    await flushPromises()

    feedHistory(lastRequestId(), [{ role: 'user', content: '切回时还在跑' }])
    await flushPromises()

    // 快照采纳：真时间线 live 块在场，骨架消失。
    expect(w.find('.wt-block.live').exists()).toBe(true)
    expect(w.find('.work-restoring').exists()).toBe(false)
    w.unmount()
  })

  it('死轮停轮 → 骨架一并收口', async () => {
    prepareSession()
    requestMock.mockImplementation((_mod: string, cmd: string) => {
      if (cmd === 'inbox_status') return Promise.resolve({ available: true, busy: false })
      return Promise.resolve({})
    })
    vi.useFakeTimers()
    const w = mount(ChatPanel)
    await flushPromises()

    feedHistory(lastRequestId(), [{ role: 'user', content: 'lost turn' }])
    await flushPromises()
    expect(w.find('.work-restoring').exists()).toBe(true)

    // 两次死轮达上限 → 停轮，骨架随 stopPendingTurnPolling 收口。
    await vi.advanceTimersByTimeAsync(4000)
    await flushPromises()
    expect(w.find('.work-restoring').exists()).toBe(true)
    await vi.advanceTimersByTimeAsync(4000)
    await flushPromises()
    expect(w.find('.work-restoring').exists()).toBe(false)
    w.unmount()
  })

  it('切会话 reset 不让旧会话骨架泄漏', async () => {
    prepareSession()
    vi.useFakeTimers()
    const w = mount(ChatPanel)
    await flushPromises()

    feedHistory(lastRequestId(), [{ role: 'user', content: 'lost turn' }])
    await flushPromises()
    expect(w.find('.work-restoring').exists()).toBe(true)

    // 切走再切回（reset 链）——骨架不得跨会话残留。
    const chat = useChatStore()
    chat.reset()
    await flushPromises()
    expect(w.find('.work-restoring').exists()).toBe(false)
    w.unmount()
  })
})
