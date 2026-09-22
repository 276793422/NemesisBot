import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount, flushPromises } from '@vue/test-utils'

// BUG 2026-09-21 ①：限流重试过程切走切回后占位区可见重试进度。
//
// 根因：重试进度只走实时帧（不落盘，既有裁决），切走再切回后占位区只剩
// 哑转圈——用户无法区分「在重试」与「死了」。
//
// 修复语义（本 spec 钉死）：
// - 悬空轮次占位期间轮询 agent.retry_status → retrying 时占位区渲染
//   「第 N/M 次重试」进度文案（含模型名与等待秒数）；
// - retrying=false（重试已终局/无重试）→ 文案消失，占位退化为转圈；
// - retry_status 查询失败 → 静默退化（不炸轮询、不影响停轮判定）；
// - assistant 行到达 → 占位与文案一并消失。

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
  // 默认：inbox busy（占位轮询持续）、sync 无新事件、retry_status 无重试。
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

async function mountWithDanglingTurn(retryStatus: any, retryThrows = false) {
  requestMock.mockImplementation((_mod: string, cmd: string) => {
    if (cmd === 'inbox_status') return Promise.resolve({ available: true, busy: true })
    if (cmd === 'retry_status') {
      if (retryThrows) return Promise.reject(new Error('backend gone'))
      return Promise.resolve(retryStatus)
    }
    return Promise.resolve({})
  })
  vi.useFakeTimers()
  const w = mount(ChatPanel)
  await flushPromises()
  feedHistory(lastRequestId(), [{ role: 'user', content: 'dangling turn' }])
  await flushPromises()
  return w
}

describe('限流重试进度文案（agent.retry_status 轮询，BUG ①）', () => {
  it('retrying=true → 占位区渲染「第 N/M 次重试」进度文案', async () => {
    prepareSession()
    const w = await mountWithDanglingTurn({
      available: true,
      retrying: true,
      retry: 3,
      max_retries: 10,
      wait_secs: 40,
      model: 'gpt-5.6-sol',
    })
    expect(w.find('.typing-indicator').exists()).toBe(true)

    // 首个轮询拍（4s）：retry_status 快照落地 → 文案渲染。
    await vi.advanceTimersByTimeAsync(4000)
    await flushPromises()
    const text = w.find('.retry-status-text')
    expect(text.exists()).toBe(true)
    expect(text.text()).toContain('第 3/10 次重试')
    expect(text.text()).toContain('gpt-5.6-sol')
    expect(text.text()).toContain('40')
    w.unmount()
  })

  it('retrying=false（重试终局）→ 文案消失，占位退化为转圈', async () => {
    prepareSession()
    const w = await mountWithDanglingTurn({ available: true, retrying: false })
    await vi.advanceTimersByTimeAsync(4000)
    await flushPromises()
    expect(w.find('.retry-status-text').exists()).toBe(false)
    // 占位本身仍在（轮次依旧悬空）。
    expect(w.find('.typing-indicator').exists()).toBe(true)
    w.unmount()
  })

  it('retry_status 查询失败 → 静默退化，轮询不炸', async () => {
    prepareSession()
    const w = await mountWithDanglingTurn(null, true)
    await vi.advanceTimersByTimeAsync(4000)
    await flushPromises()
    expect(w.find('.retry-status-text').exists()).toBe(false)
    expect(w.find('.typing-indicator').exists()).toBe(true)
    // 轮询存活：下一拍照常。
    await vi.advanceTimersByTimeAsync(4000)
    await flushPromises()
    expect(w.find('.typing-indicator').exists()).toBe(true)
    w.unmount()
  })

  it('assistant 行到达（sync 增量）→ 占位与文案一并消失', async () => {
    prepareSession()
    let syncN = 0
    requestMock.mockImplementation((_mod: string, cmd: string) => {
      if (cmd === 'inbox_status') return Promise.resolve({ available: true, busy: true })
      if (cmd === 'retry_status')
        return Promise.resolve({ available: true, retrying: true, retry: 5, max_retries: 10, wait_secs: 60, model: 'm' })
      if (cmd === 'sync') {
        syncN += 1
        // 第 1 次是 primeSeqBaseline 基线、第 2 次是拍 1 轮询——都空；
        // 第 3 次起返回 assistant 事件（AI 完成）。
        if (syncN <= 2) return Promise.resolve({})
        return Promise.resolve({
          events: [{ role: 'assistant', content: 'finally done', seq: 9, ts: '2026-09-21T00:00:00Z' }],
        })
      }
      return Promise.resolve({})
    })
    vi.useFakeTimers()
    const w = mount(ChatPanel)
    await flushPromises()
    feedHistory(lastRequestId(), [{ role: 'user', content: 'dangling turn' }])
    await flushPromises()

    // 先看到重试文案。
    await vi.advanceTimersByTimeAsync(4000)
    await flushPromises()
    expect(w.find('.retry-status-text').exists()).toBe(true)

    // assistant 行落盘 → 停轮清占位，文案一并消失。
    await vi.advanceTimersByTimeAsync(4000)
    await flushPromises()
    expect(w.find('.retry-status-text').exists()).toBe(false)
    expect(w.find('.typing-indicator').exists()).toBe(false)
    expect(w.text()).toContain('finally done')
    w.unmount()
  })
})
