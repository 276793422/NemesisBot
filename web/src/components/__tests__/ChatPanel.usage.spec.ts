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

  it('pct_trigger（压缩点口径）优先于 pct；旧后端缺字段回落 pct', async () => {
    // 2026-10-20 压缩点一等公民化：徽标语义 =「距压缩还有多远」。
    ctxResponse = { context: { pct: 85, pct_trigger: 95 } }
    usageResponse = { total_cost_usd: 0 }
    installRequestMock()
    const w = await mountPanelWithSession('s1')
    expect(w.find('.usage-ctx').text()).toBe('95% context')
    expect(w.find('.usage-ctx').classes()).toContain('hot')
    w.unmount()

    ctxResponse = { context: { pct: 85 } }
    const w2 = await mountPanelWithSession('s1')
    expect(w2.find('.usage-ctx').text()).toBe('85% context')
    w2.unmount()
  })

  // B4.5（2026-11-09 设计定稿 §7 构成分解 + 比例投影 + 7 段化拍板）：
  // breakdown 在场 = 构成条形态——左文案框 + 右全宽条（100% = 窗口）、
  // 七段含空闲、压缩竖线、hover 弹层。
  it('B4.5 构成条：七段窗口刻度宽度 + 压缩竖线 75% + ◐锚标 + 空闲段；文案 = used/window 口径', async () => {
    ctxResponse = {
      context: {
        pct: 50,
        pct_trigger: 66,
        window: 1000,
        trigger: 750,
        anchor_tokens: 400,
        delta_tokens: 100,
        anchor_state: 'stale',
        breakdown: { system: 40, tools: 160, mcp: 60, skill: 30, memory: 10, summary: 100, history: 100, free: 500 },
      },
    }
    usageResponse = { total_cost_usd: 0 }
    installRequestMock()
    const w = await mountPanelWithSession('s1')

    const bar = w.find('.usage-bar')
    expect(bar.exists()).toBe(true)
    // 八段：used 七段 + 空闲段（设计定稿 §7.1 第五段语义保留）。
    const segs = w.findAll('.usage-seg')
    expect(segs.length).toBe(8)
    // 段宽 = 段值/窗口 × 100（窗口刻度直读，非配比）：system 4% / tools 16% /
    // mcp 6% / skill 3% / memory 1% / summary 10% / history 10% / free 50%，
    // 合计 = 100%。
    const widths = segs.map((s) => {
      const m = /width:\s*([\d.]+)%/.exec(s.attributes('style') ?? '')
      return m ? parseFloat(m[1]) : 0
    })
    expect(widths[0]).toBeCloseTo(4, 5)
    expect(widths[1]).toBeCloseTo(16, 5)
    expect(widths[2]).toBeCloseTo(6, 5)
    expect(widths[3]).toBeCloseTo(3, 5)
    expect(widths[4]).toBeCloseTo(1, 5)
    expect(widths[5]).toBeCloseTo(10, 5)
    expect(widths[6]).toBeCloseTo(10, 5)
    expect(widths[7]).toBeCloseTo(50, 5)
    expect(segs[7].classes()).toContain('usage-seg-free')
    // 压缩竖线位置 = trigger/window = 75%。
    expect(w.find('.usage-trigger-line').attributes('style')).toContain('left: 75%')
    // 锚标 ◐（stale 增量在途）；% 标签 = used/window（50），不取 pct_trigger。
    expect(w.find('.usage-anchor-mark').text()).toBe('◐')
    expect(w.find('.usage-text').text()).toContain('50% context')
    expect(w.find('.usage-text').classes()).not.toContain('hot')
    // hover 文案框弹 §7.2 文字明细：七段（估算 ~，含 MCP/SKILL/MEMORY）+
    // 已用/触发点 + 窗口行。
    await w.find('.usage-text').trigger('mouseenter')
    const tipText = w.find('.usage-tip').text()
    expect(tipText).toContain('系统提示')
    expect(tipText).toContain('~160 tok')
    expect(tipText).toContain('MCP')
    expect(tipText).toContain('~60 tok')
    expect(tipText).toContain('SKILL')
    expect(tipText).toContain('~30 tok')
    expect(tipText).toContain('MEMORY')
    expect(tipText).toContain('~10 tok')
    expect(tipText).toContain('~100 tok')
    expect(tipText).toContain('已用(实报 400 + 增量 ~100) / 触发点 750')
    expect(tipText).toContain('窗口 1,000（触发点 = 窗口的 75%）')
    await w.find('.usage-text').trigger('mouseleave')
    expect(w.find('.usage-tip').exists()).toBe(false)
    w.unmount()
  })

  // 设计定稿 §7.1 文案警示三态：逼近竖线（距触发点 ≤ 窗口 5%）= 警示色；
  // 越过竖线 = 压缩 stuck 可见（红）；超窗口 = 钳满 + 红「超限」。
  it('文案警示三态：逼近竖线 hot / 越线 over / 超窗口「超限」徽标', async () => {
    usageResponse = { total_cost_usd: 0 }
    installRequestMock()
    const bd = { system: 40, tools: 160, summary: 0, history: 520, free: 280 }

    // 逼近：pct 72 ≥ triggerPct 75 − 5 → hot（原 ≥80% 阈值不会触发 72）。
    ctxResponse = {
      context: { pct: 72, window: 1000, trigger: 750, anchor_state: 'stale', anchor_tokens: 700, delta_tokens: 20, breakdown: bd },
    }
    const w = await mountPanelWithSession('s1')
    expect(w.find('.usage-text').classes()).toContain('hot')
    expect(w.find('.usage-text').classes()).not.toContain('over')
    w.unmount()

    // 越线：pct 80 > triggerPct 75 → over（红）。
    ctxResponse = {
      context: { pct: 80, window: 1000, trigger: 750, anchor_state: 'stale', anchor_tokens: 780, delta_tokens: 20, breakdown: bd },
    }
    const w2 = await mountPanelWithSession('s1')
    expect(w2.find('.usage-text').classes()).toContain('over')
    await w2.find('.usage-text').trigger('mouseenter')
    expect(w2.find('.usage-tip').text()).toContain('⚠ 已越过压缩线')
    w2.unmount()

    // 超窗口：pct 100（钳满）→「超限」徽标。
    ctxResponse = {
      context: { pct: 100, window: 1000, trigger: 750, anchor_state: 'stale', anchor_tokens: 990, delta_tokens: 10, breakdown: { system: 40, tools: 160, summary: 0, history: 800, free: 0 } },
    }
    const w3 = await mountPanelWithSession('s1')
    expect(w3.find('.usage-over-limit').exists()).toBe(true)
    expect(w3.find('.usage-over-limit').text()).toBe('超限')
    w3.unmount()
  })

  it('B4 锚标三态：fresh ● / absent ○；旧形态（无 breakdown）不渲染条', async () => {
    usageResponse = { total_cost_usd: 0 }
    installRequestMock()

    ctxResponse = {
      context: {
        pct: 3,
        window: 1000,
        trigger: 750,
        anchor_tokens: 100,
        delta_tokens: 0,
        anchor_state: 'fresh',
        breakdown: { system: 25, tools: 50, summary: 0, history: 25, free: 900 },
      },
    }
    const w = await mountPanelWithSession('s1')
    expect(w.find('.usage-anchor-mark').text()).toBe('●')
    expect(w.find('.usage-bar').exists()).toBe(true)
    w.unmount()

    ctxResponse = {
      context: {
        pct: 3,
        window: 1000,
        trigger: 750,
        anchor_tokens: null,
        delta_tokens: 0,
        anchor_state: 'absent',
        breakdown: { system: 25, tools: 50, summary: 0, history: 0, free: 925 },
      },
    }
    const w2 = await mountPanelWithSession('s1')
    expect(w2.find('.usage-anchor-mark').text()).toBe('○')
    w2.unmount()

    // 旧后端形态（无 breakdown）：无条、无锚标，M5 徽标原样。
    ctxResponse = { context: { pct: 85, pct_trigger: 95 } }
    const w3 = await mountPanelWithSession('s1')
    expect(w3.find('.usage-bar').exists()).toBe(false)
    expect(w3.find('.usage-anchor-mark').exists()).toBe(false)
    expect(w3.find('.usage-ctx').text()).toBe('95% context')
    w3.unmount()
  })
})
