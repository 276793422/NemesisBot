import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'

// BoardView 拆解在途横幅（§1.2C，2026-10-11 Part 1 配套前端）：
// - plan_started 后横幅出现（编号 + 自计时 + attempt 文案）；
// - 超过 ABNORMAL_SECS 转「拆解异常」黄态；
// - plan_ready 后横幅撤下；
// - 无在途时横幅不渲染。

const handlers = new Map<string, ((data?: unknown) => void)[]>()

vi.mock('../../composables/useSSE', () => ({
  on: vi.fn((type: string, handler: (data?: unknown) => void) => {
    const arr = handlers.get(type) || []
    arr.push(handler)
    handlers.set(type, arr)
  }),
  off: vi.fn(),
}))

const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
}))

import BoardView from '../BoardView.vue'
import { ABNORMAL_SECS, _resetPlanningForTest } from '../../composables/usePlanning'

function dispatch(type: string, data?: unknown) {
  for (const h of handlers.get(type) || []) h(data)
}

beforeEach(() => {
  handlers.clear()
  // 单例的 SSE 注册态同步复位（打桩表被清空后必须重注册，见 composable 注释）。
  _resetPlanningForTest()
  requestMock.mockReset()
  requestMock.mockResolvedValue({})
  vi.useFakeTimers()
  vi.setSystemTime(0)
})

afterEach(() => {
  vi.useRealTimers()
})

async function mountView() {
  const w = mount(BoardView)
  await flushPromises()
  return w
}

describe('BoardView 拆解在途横幅（§1.2C）', () => {
  it('无在途 → 横幅不渲染', async () => {
    const w = await mountView()
    expect(w.find('[data-test="planning-banner"]').exists()).toBe(false)
    w.unmount()
  })

  it('plan_started → 横幅出现：编号 + 自计时；attempt>1 带「第 N 次尝试」', async () => {
    const w = await mountView()
    dispatch('board.plan_started', { plan_id: 'p1', issue_id: 5, issue_number: 'NB-5', attempt: 2 })
    await flushPromises()
    const banner = w.find('[data-test="planning-banner"]')
    expect(banner.exists()).toBe(true)
    const line = banner.find('[data-test="planning-line-NB-5"]')
    expect(line.exists()).toBe(true)
    expect(line.text()).toContain('正在 AI 拆解 NB-5')
    expect(line.text()).toContain('第 2 次尝试')
    expect(line.text()).toContain('0:00')
    expect(line.text()).not.toContain('拆解异常')
    w.unmount()
  })

  it('超 ABNORMAL_SECS → 黄色异常态文案', async () => {
    const w = await mountView()
    dispatch('board.plan_started', { plan_id: 'p1', issue_id: 5, issue_number: 'NB-5', attempt: 1 })
    await flushPromises()
    vi.advanceTimersByTime(ABNORMAL_SECS * 1000 + 2000)
    await flushPromises()
    const line = w.find('[data-test="planning-line-NB-5"]')
    expect(line.classes()).toContain('abnormal')
    expect(line.text()).toContain('拆解异常')
    expect(line.text()).toContain('20:02')
    w.unmount()
  })

  it('plan_ready → 横幅撤下', async () => {
    const w = await mountView()
    dispatch('board.plan_started', { plan_id: 'p1', issue_id: 5, issue_number: 'NB-5', attempt: 1 })
    await flushPromises()
    expect(w.find('[data-test="planning-banner"]').exists()).toBe(true)
    dispatch('board.plan_ready', { plan_id: 'p1', issue_id: 5, subs: [] })
    await flushPromises()
    expect(w.find('[data-test="planning-banner"]').exists()).toBe(false)
    w.unmount()
  })

  it('plan_ready 带 auto_confirm → 「发车派发中」横幅；超时不转异常（dispatching 非拆解）', async () => {
    const w = await mountView()
    dispatch('board.plan_started', { plan_id: 'p1', issue_id: 5, issue_number: 'NB-5', attempt: 1 })
    await flushPromises()
    dispatch('board.plan_ready', { plan_id: 'p1', issue_id: 5, subs: [], auto_confirm: true })
    await flushPromises()
    const line = w.find('[data-test="planning-line-NB-5"]')
    expect(line.exists()).toBe(true)
    expect(line.text()).toContain('自动发车派发中')
    expect(line.text()).not.toContain('拆解异常')
    // dispatching 超 ABNORMAL_SECS 不得转异常黄态。
    vi.advanceTimersByTime(ABNORMAL_SECS * 1000 + 2000)
    await flushPromises()
    expect(line.classes()).not.toContain('abnormal')
    expect(line.text()).toContain('自动发车派发中')
    w.unmount()
  })
})
