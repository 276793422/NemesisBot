import { describe, it, expect, vi, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount, flushPromises } from '@vue/test-utils'

// M5（2026-09-05）：ChatPanel 会话级 context/cost 常驻条——
// chat.context_status + logs.session_usage 两路回包驱动渲染；
// hot 阈值 80%；0/缺省 cost 不显示；会话切换后迟到回包不串话。

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
import { useSessionStore } from '../../stores/session'

// 可切换的回包内容 + 可持有的 pending promise（迟到回包用例）。
let ctxResponse: any = {}
let usageResponse: any = {}
let holdResponses = false
let pending: Array<{ cmd: string; res: (v: any) => void }> = []

function installRequestMock() {
  requestMock.mockImplementation((_module: string, cmd: string) => {
    if (holdResponses && (cmd === 'context_status' || cmd === 'session_usage')) {
      return new Promise((res) => pending.push({ cmd, res }))
    }
    if (cmd === 'context_status') return Promise.resolve(ctxResponse)
    if (cmd === 'session_usage') return Promise.resolve(usageResponse)
    return Promise.resolve({})
  })
}

beforeEach(() => {
  setActivePinia(createPinia())
  ctxResponse = {}
  usageResponse = {}
  holdResponses = false
  pending = []
  requestMock.mockReset()
  requestMock.mockResolvedValue({})
  vi.mocked(onMessage).mockClear()
})

async function mountPanelWithSession(sid: string) {
  useSessionStore().currentId = sid
  const wrapper = mount(ChatPanel)
  await flushPromises()
  return wrapper
}

describe('ChatPanel M5 用量常驻条', () => {
  it('context_status + session_usage 回包驱动渲染；≥80% 带 hot', async () => {
    ctxResponse = { context: { pct: 85, used_tokens: 108800, window: 128000 } }
    usageResponse = { total_cost_usd: 0.5 }
    installRequestMock()
    const w = await mountPanelWithSession('s1')

    const strip = w.find('.usage-strip')
    expect(strip.exists()).toBe(true)
    const ctx = w.find('.usage-ctx')
    expect(ctx.text()).toBe('85% context')
    expect(ctx.classes()).toContain('hot')
    expect(w.find('.usage-cost').text()).toBe('$0.50')
    w.unmount()
  })

  it('<80% 不带 hot；cost 为 0 时不渲染 cost 段', async () => {
    ctxResponse = { context: { pct: 40 } }
    usageResponse = { total_cost_usd: 0 }
    installRequestMock()
    const w = await mountPanelWithSession('s1')

    expect(w.find('.usage-strip').exists()).toBe(true)
    expect(w.find('.usage-ctx').classes()).not.toContain('hot')
    expect(w.find('.usage-cost').exists()).toBe(false)
    w.unmount()
  })

  it('默认会话（无 currentId）不发用量请求、不渲染常驻条', async () => {
    installRequestMock()
    const w = mount(ChatPanel)
    await flushPromises()

    const usageCalls = requestMock.mock.calls.filter(([, cmd]) => cmd === 'context_status' || cmd === 'session_usage')
    expect(usageCalls.length).toBe(0)
    expect(w.find('.usage-strip').exists()).toBe(false)
    w.unmount()
  })

  it('会话切换后迟到的旧回包被丢弃（不串话），新回包正常渲染', async () => {
    holdResponses = true
    installRequestMock()
    useSessionStore().currentId = 's1'
    const w = mount(ChatPanel)
    await flushPromises()
    // s1 的两路请求已被持有。
    const s1Pending = pending.splice(0)

    // 切到 s2：watch 清空 refs 并为 s2 发起新请求。
    useSessionStore().switchTo('s2')
    await flushPromises()
    const s2Pending = pending.splice(0)
    expect(s1Pending.length).toBe(2)
    expect(s2Pending.length).toBe(2)

    // 旧回包先到——guard（currentId !== 's1'）丢弃，不渲染。
    s1Pending.forEach((p) => p.res({ context: { pct: 99 }, total_cost_usd: 9.99 }))
    await flushPromises()
    expect(w.find('.usage-strip').exists()).toBe(false)

    // 新回包到达 → 正常渲染。
    s2Pending.forEach((p) => p.res({ context: { pct: 10 }, total_cost_usd: 0.25 }))
    await flushPromises()
    expect(w.find('.usage-ctx').text()).toBe('10% context')
    expect(w.find('.usage-cost').text()).toBe('$0.25')
    w.unmount()
  })

  it('assistant 回复落地（turn 完成）触发再次刷新', async () => {
    ctxResponse = { context: { pct: 55 } }
    usageResponse = { total_cost_usd: 0.1 }
    installRequestMock()
    const w = await mountPanelWithSession('s1')
    const callsAfterMount = requestMock.mock.calls.filter(([, cmd]) => cmd === 'context_status').length
    expect(callsAfterMount).toBe(1)

    const calls = vi.mocked(onMessage).mock.calls
    expect(calls.length, 'onMessage must have been called on mount').toBeGreaterThan(0)
    const h = calls[calls.length - 1][0] as (frame: any) => void
    h({
      type: 'message',
      module: 'chat',
      cmd: 'receive',
      data: { role: 'assistant', content: '本轮完成' },
      timestamp: 't1',
    })
    await flushPromises()

    const callsAfterTurn = requestMock.mock.calls.filter(([, cmd]) => cmd === 'context_status').length
    // turn 完成 → refreshUsage 再次拉取
    expect(callsAfterTurn).toBe(2)
    w.unmount()
  })
})
