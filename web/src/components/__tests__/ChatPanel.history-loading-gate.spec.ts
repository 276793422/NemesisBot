import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount, flushPromises } from '@vue/test-utils'

// W4（2026-10-14 审计）回归钉：
//
// 1) handleHistoryResponse 只在 inflight.kind === 'page' 时清 historyLoading
//    ——watchdog/resync 响应不得提前抬起翻页闸（旧链路：翻页在飞 + watchdog
//    响应先到清闸 → 滚动处理器对同一 oldestIndex 再发一页 → 两响应各过各的
//    栅栏 → 同批行前置两遍，重复消息）；
// 2) onUnmounted 清 watchdog 定时器与 @ 补全 debounce——残留 timer 在组件
//    实例外开火（onWatchdog 读实例 ref 态误触 reloadLatest 重灌；at 补全
//    回包写已卸载实例的 atItems）。

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

import { sendHistoryRequest, onMessage, send } from '../../composables/useWebSocket'
import ChatPanel from '../ChatPanel.vue'
import { useChatStore } from '../../stores/chat'
import { useSessionStore } from '../../stores/session'

async function mountAndCapture() {
  const sessionStore = useSessionStore()
  sessionStore.sessions = [
    { id: 's1', channel: 'web', startTime: '', lastTime: '', messageCount: 0, firstMessage: 'A', model: '' },
  ] as any
  sessionStore.currentId = 's1'

  const wrapper = mount(ChatPanel)
  await vi.advanceTimersByTimeAsync(0)
  await flushPromises()
  const handler = vi.mocked(onMessage).mock.calls.at(-1)![0] as (frame: any) => void
  return { wrapper, sessionStore, handler }
}

function historyFrame(rid: string, extra: Record<string, unknown> = {}) {
  return {
    type: 'message',
    module: 'chat',
    cmd: 'history',
    data: {
      request_id: rid,
      session_id: 's1',
      has_more: false,
      oldest_index: 0,
      total_count: 1,
      messages: [{ role: 'user', content: `msg-${rid}`, timestamp: '2026-10-14T10:00:00Z' }],
      ...extra,
    },
  }
}

/** Ctrl+Enter 发送一条消息（send 流会 setBusy + startWatchdog 臂 45s 定时器）。 */
async function sendViaEnter(wrapper: any) {
  const chat = useChatStore()
  chat.input = 'wd'
  await wrapper.find('textarea').trigger('keydown', { key: 'Enter', ctrlKey: true })
  await flushPromises()
}

function historyCalls(): number {
  return vi.mocked(sendHistoryRequest).mock.calls.length
}

beforeEach(() => {
  setActivePinia(createPinia())
  requestMock.mockReset()
  requestMock.mockResolvedValue({})
  vi.mocked(sendHistoryRequest).mockClear()
  vi.mocked(onMessage).mockClear()
  vi.useFakeTimers()
})

afterEach(() => {
  vi.useRealTimers()
})

describe('W4：historyLoading 只由 page 响应清（watchdog 响应不越权）', () => {
  it('翻页在飞时 watchdog 响应先到：不清闸、滚动不重发同页', async () => {
    const { wrapper, handler } = (await mountAndCapture()) as any
    const chat = useChatStore()
    const rid1 = vi.mocked(sendHistoryRequest).mock.calls[0][0] as string

    // 首拉响应到场（has_more + oldest_index=20 → 可翻页），闸抬起。
    handler(historyFrame(rid1, { has_more: true, oldest_index: 20, last_seq: 50 }))
    await flushPromises()
    expect(chat.historyLoading).toBe(false)

    // Ctrl+Enter 发送 → busy(streaming) + watchdog 臂上（45s）。此刻无翻页
    // 在飞——先推进 45s 只触发 watchdog，不会误触翻页请求的 10s 安全超时。
    await sendViaEnter(wrapper)
    expect(vi.mocked(send), 'Ctrl+Enter 应触发 send 流').toHaveBeenCalled()
    expect(chat.isBusy('s1'), 'send 流应置 busy').toBe(true)

    // watchdog 到点 → reloadLatest 发出 watchdog 请求（kind: 'watchdog'）。
    await vi.advanceTimersByTimeAsync(45_000)
    expect(historyCalls()).toBe(2)
    const ridW = vi.mocked(sendHistoryRequest).mock.calls[1][0] as string
    expect(ridW).toContain('watchdog_')

    // 滚到顶 → 翻页请求 #3 在飞，闸落下（其 10s 安全超时要到 10s 后——
    // 此后不再推进时钟，让 watchdog 响应插队先到）。
    const el = wrapper.find('.chat-messages').element as HTMLElement
    el.scrollTop = 0
    await wrapper.find('.chat-messages').trigger('scroll')
    await flushPromises()
    expect(historyCalls()).toBe(3)
    const rid2 = vi.mocked(sendHistoryRequest).mock.calls[2][0] as string
    expect(chat.historyLoading, '翻页在飞=闸落下').toBe(true)

    // ★ W4 修复点：watchdog 响应先到，不清翻页闸。
    handler(historyFrame(ridW, { last_seq: 50 }))
    await flushPromises()
    expect(chat.historyLoading, 'watchdog 响应不得清 historyLoading').toBe(true)

    // 闸仍在 → 再滚不重发同页（旧 bug：同 oldestIndex 第二页 → 行重复）。
    el.scrollTop = 0
    await wrapper.find('.chat-messages').trigger('scroll')
    await flushPromises()
    expect(historyCalls(), '翻页闸未抬 → 不得重发').toBe(3)

    // page 响应到场 → 闸由自己正常抬起。
    handler(historyFrame(rid2, { has_more: true, last_seq: 50 }))
    await flushPromises()
    expect(chat.historyLoading).toBe(false)
    chat.setBusy('s1', false)
    wrapper.unmount()
  })
})

describe('W4：卸载清残留定时器', () => {
  it('卸载后 @ 补全 debounce 不再开火（不发 fs.complete_path）', async () => {
    const { wrapper } = (await mountAndCapture()) as any
    const chat = useChatStore()

    // 挂载生命周期自身的轮询调用先记账。
    await vi.advanceTimersByTimeAsync(0)
    await flushPromises()
    const callsAtUnmount = requestMock.mock.calls.length

    // @片段 调度 atDebounce——先让 pre-flush watcher 跑掉（debounce 排上，
    // 但不推进到触发点），再卸载、再推进。若同 tick 内直接 unmount，
    // watcher 随组件停摆、debounce 从未排上，用例假绿。
    chat.input = '@src'
    await vi.advanceTimersByTimeAsync(0)
    wrapper.unmount()
    await vi.advanceTimersByTimeAsync(1_000)

    expect(requestMock.mock.calls.length, '卸载后 at debounce 不得再发请求').toBe(callsAtUnmount)
  })

  it('卸载后 watchdog 定时器不再开火（不误触 reloadLatest 重灌）', async () => {
    const { wrapper, handler } = (await mountAndCapture()) as any
    const chat = useChatStore()
    const rid1 = vi.mocked(sendHistoryRequest).mock.calls[0][0] as string
    handler(historyFrame(rid1))
    await flushPromises()

    // Enter 发送 → watchdog 臂上（45s）。
    await sendViaEnter(wrapper)
    const callsAtUnmount = historyCalls()

    // 不响应发送帧，直接卸载——残留 timer（旧 bug）45s 后 onWatchdog：
    // streaming 仍 true（busy 条目未清）→ reloadLatest 再发请求。
    wrapper.unmount()
    await vi.advanceTimersByTimeAsync(60_000)

    expect(historyCalls(), '卸载后 watchdog 不得再发 history 请求').toBe(callsAtUnmount)
  })
})
