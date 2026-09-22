import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount, flushPromises } from '@vue/test-utils'

// BUG 2026-09-21 ③：历史加载失败静默 → 可见失败态 + 有限自动重试。
//
// 根因：10s safety timeout 只清 historyLoading——请求蒸发（重启断连窗口）
// 后无任何提示、无重试、无再触发源，空视图与「会话本来就空」不可区分。
//
// 修复语义（本 spec 钉死）：
// - 超时 → 失败态可见（「历史消息加载失败」+「重新加载历史」按钮）；
// - 自动重试 ≤2 次（2s/4s 退避），成功即清失败态；
// - 超次数后停止自动重试，手动按钮仍可恢复；
// - WS 重连认失败态补偿重拉；
// - 断连中的重试拍不消耗次数（交给重连补偿）。

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

import { onMessage, sendHistoryRequest, wsStatus } from '../../composables/useWebSocket'
import ChatPanel from '../ChatPanel.vue'
import { useChatStore } from '../../stores/chat'
import { useSessionStore } from '../../stores/session'

beforeEach(() => {
  setActivePinia(createPinia())
  requestMock.mockReset()
  requestMock.mockResolvedValue({})
  vi.mocked(sendHistoryRequest).mockClear()
  vi.mocked(onMessage).mockClear()
  ;(wsStatus as any).value = 'connected'
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

/** 空会话（历史为空但加载成功）与失败态共用断言基线：失败态下
 *  「加载失败」文案 + 手动按钮可见；成功态下两者消失。 */
function failureVisible(w: any) {
  expect(w.text()).toContain('历史消息加载失败')
  const btn = w.find('button')
  expect(btn.exists()).toBe(true)
  expect(btn.text()).toContain('重新加载历史')
}

describe('历史加载失败：可见失败态与有限自动重试（BUG ③）', () => {
  it('请求蒸发（10s 无响应）→ 失败态可见 + 2s 后自动重发', async () => {
    prepareSession()
    vi.useFakeTimers()
    const w = mount(ChatPanel)
    await flushPromises()
    expect(sendHistoryRequest).toHaveBeenCalledTimes(1)

    // 响应蒸发：推进过 safety timeout。
    await vi.advanceTimersByTimeAsync(10000)
    await flushPromises()
    failureVisible(w)

    // 自动重试第 1 拍（2s 退避）。
    await vi.advanceTimersByTimeAsync(2000)
    await flushPromises()
    expect(sendHistoryRequest).toHaveBeenCalledTimes(2)
    w.unmount()
  })

  it('自动重试成功 → 失败态清除，消息正常显示', async () => {
    prepareSession()
    vi.useFakeTimers()
    const w = mount(ChatPanel)
    await flushPromises()

    await vi.advanceTimersByTimeAsync(10000)
    await flushPromises()
    failureVisible(w)

    await vi.advanceTimersByTimeAsync(2000)
    await flushPromises()
    expect(sendHistoryRequest).toHaveBeenCalledTimes(2)

    // 第 2 次请求的响应落地 → 失败态清、消息渲染。
    feedHistory(lastRequestId(), [{ role: 'assistant', content: 'recovered' }])
    await flushPromises()
    expect(w.text()).not.toContain('历史消息加载失败')
    expect(w.text()).toContain('recovered')
    w.unmount()
  })

  it('自动重试耗尽（2 次）→ 停止自动重发，手动按钮恢复', async () => {
    prepareSession()
    vi.useFakeTimers()
    const w = mount(ChatPanel)
    await flushPromises()

    // 首拉超时（10s）→ retry#1（12s）→ 超时（22s）→ retry#2（26s）。
    await vi.advanceTimersByTimeAsync(10000)
    await vi.advanceTimersByTimeAsync(2000)
    expect(sendHistoryRequest).toHaveBeenCalledTimes(2)
    await vi.advanceTimersByTimeAsync(10000)
    await vi.advanceTimersByTimeAsync(4000)
    expect(sendHistoryRequest).toHaveBeenCalledTimes(3)
    await vi.advanceTimersByTimeAsync(10000)
    await flushPromises()
    failureVisible(w)

    // 已达上限：再推进多个退避周期不再自动重发。
    await vi.advanceTimersByTimeAsync(20000)
    await flushPromises()
    expect(sendHistoryRequest).toHaveBeenCalledTimes(3)

    // 手动按钮 → 立即重拉，响应成功 → 恢复。
    await w.find('button').trigger('click')
    await flushPromises()
    expect(sendHistoryRequest).toHaveBeenCalledTimes(4)
    feedHistory(lastRequestId(), [{ role: 'user', content: 'old msg' }])
    await flushPromises()
    expect(w.text()).not.toContain('历史消息加载失败')
    expect(w.text()).toContain('old msg')
    w.unmount()
  })

  it('WS 重连认失败态：补偿重拉成功后失败态清除', async () => {
    prepareSession()
    ;(wsStatus as any).value = 'disconnected'
    vi.useFakeTimers()
    const w = mount(ChatPanel)
    await flushPromises()
    // 断连中挂载不发历史（既有守卫）。
    expect(sendHistoryRequest).toHaveBeenCalledTimes(0)

    // 建连 → 首拉；响应蒸发 → 失败态。
    ;(wsStatus as any).value = 'connected'
    await flushPromises()
    expect(sendHistoryRequest).toHaveBeenCalledTimes(1)
    await vi.advanceTimersByTimeAsync(10000)
    await flushPromises()
    failureVisible(w)

    // 又断连：重试拍不消耗次数（退避后不重发）。
    ;(wsStatus as any).value = 'disconnected'
    await vi.advanceTimersByTimeAsync(2000)
    await flushPromises()
    expect(sendHistoryRequest).toHaveBeenCalledTimes(1)

    // 重连 → 失败态分支补偿重拉 → 成功 → 失败态清除。
    ;(wsStatus as any).value = 'connected'
    await flushPromises()
    expect(sendHistoryRequest).toHaveBeenCalledTimes(2)
    feedHistory(lastRequestId(), [{ role: 'user', content: 'restored' }])
    await flushPromises()
    expect(w.text()).not.toContain('历史消息加载失败')
    expect(w.text()).toContain('restored')
    w.unmount()
  })

  it('加载成功不置失败态；换会话失败态不带到新会话', async () => {
    prepareSession()
    const sessionStore = useSessionStore()
    sessionStore.sessions.push(
      { id: 's2', channel: 'web', startTime: '', lastTime: '', messageCount: 0, firstMessage: 'B', model: '' } as any,
    )
    vi.useFakeTimers()
    const w = mount(ChatPanel)
    await flushPromises()

    // s1 正常落地 → 无失败态。
    feedHistory(lastRequestId(), [{ role: 'user', content: 's1 msg' }])
    await flushPromises()
    expect(w.text()).not.toContain('历史消息加载失败')

    // 切 s2 且其历史请求蒸发 → s2 失败态。
    sessionStore.currentId = 's2'
    await flushPromises()
    await vi.advanceTimersByTimeAsync(10000)
    await flushPromises()
    failureVisible(w)

    // 切回 s1：失败态已复位（不显示 s2 的失败），s1 重拉落地后正常显示。
    sessionStore.currentId = 's1'
    await flushPromises()
    expect(w.text()).not.toContain('历史消息加载失败')
    expect(sendHistoryRequest).toHaveBeenCalledTimes(3)
    feedHistory(lastRequestId(), [{ role: 'user', content: 's1 again' }])
    await flushPromises()
    expect(w.text()).not.toContain('历史消息加载失败')
    expect(w.text()).toContain('s1 again')
    w.unmount()
  })

  it('更早请求的迟到 timeout 不误杀当前在飞请求（per-request 围栏）', async () => {
    // 真机挂起场景实测：10s safety timer 无法逐请求取消，若回调只看全局
    // historyLoading，更早请求（已响应/已作废）的迟到 timer 会清掉当前
    // 在飞请求的 loading 并提前判死——失败态相位错乱、重试拍被误杀。
    prepareSession()
    const sessionStore = useSessionStore()
    sessionStore.sessions.push(
      { id: 's2', channel: 'web', startTime: '', lastTime: '', messageCount: 0, firstMessage: 'B', model: '' } as any,
    )
    vi.useFakeTimers()
    const w = mount(ChatPanel)
    await flushPromises()

    // #1（s1）立即响应：登记删除，但其 timer 未取消（t=10s 到期）。
    feedHistory(lastRequestId(), [{ role: 'user', content: 's1 msg' }])
    await flushPromises()

    // t=4s 切 s2 → #2 在飞（timer 到期 t=14s）。
    sessionStore.currentId = 's2'
    await flushPromises()
    expect(sendHistoryRequest).toHaveBeenCalledTimes(2)

    // t=10s：#1 的迟到 timer 到期——不得动 #2 的在飞状态。
    await vi.advanceTimersByTimeAsync(6000)
    await flushPromises()
    expect(sendHistoryRequest).toHaveBeenCalledTimes(2)
    const { useChatStore } = await import('../../stores/chat')
    expect(useChatStore().historyLoading).toBe(true)
    expect(w.text()).not.toContain('历史消息加载失败')

    // t=14s：#2 自己的 timer 到期 → 失败态照常置位。
    await vi.advanceTimersByTimeAsync(4000)
    await flushPromises()
    failureVisible(w)
    w.unmount()
  })
})
