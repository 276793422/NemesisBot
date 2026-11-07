import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { useToast } from '../../../composables/useToast'

// B35「需要人的事」面板（2026-11-07）：四泳道聚合渲染 + 空态 + 影子转正
// + board-changed 静默换新。后端聚合口径（四源分类/终态排除）由
// crates/nemesis-web/src/handlers/board/tests.rs 钉住（后端唯一真相源），
// 这里钉消费面（payload 契约 + 交互）。

const requestMock = vi.fn()
vi.mock('../../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
  initWSAPI: vi.fn(),
}))

// useSSE 打桩（连接层不在本测试范围）：记录订阅，测试内手动触发
// board-changed 验证 200ms 防抖静默换新链路（useBoardChanged 包装它）。
const sseHandlers = new Map<string, (data?: unknown) => void>()
vi.mock('../../../composables/useSSE', () => ({
  on: vi.fn((type: string, handler: (data?: unknown) => void) => {
    sseHandlers.set(type, handler)
  }),
  off: vi.fn((type: string) => {
    sseHandlers.delete(type)
  }),
}))

import BoardNeedsHumanPanel from '../BoardNeedsHumanPanel.vue'

function row(over: Record<string, unknown> = {}) {
  return {
    id: 1,
    number: 'NB-1',
    title: '超预算单',
    status: 'in_review',
    priority: 2,
    reason: '🛑 重派熔断，转人工处置',
    needs_user: null,
    ...over,
  }
}

const emptyData = {
  groups: { escalated: [], fused: [], plan_failed: [], shadow: [] },
  total: 0,
}

beforeEach(() => {
  requestMock.mockReset()
  useToast().toasts.splice(0)
  requestMock.mockImplementation(() => Promise.resolve(emptyData))
})

describe('BoardNeedsHumanPanel（需要人的事）', () => {
  it('空态：total=0 显示无需人工介入，不渲染泳道', async () => {
    const w = mount(BoardNeedsHumanPanel)
    await flushPromises()
    expect(w.find('[data-test="needs-human-empty"]').exists()).toBe(true)
    expect(w.find('[data-test="needs-human-lanes"]').exists()).toBe(false)
    expect(requestMock).toHaveBeenCalledWith('board', 'board.needs_human.list', {})
  })

  it('四泳道渲染：单子行带编号/标题/建议，影子规则带转正按钮', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'board.needs_human.list')
        return Promise.resolve({
          groups: {
            escalated: [
              row({
                id: 11,
                number: 'NB-11',
                title: '预算熔断单',
                needs_user: {
                  stop_kind_label: '预算',
                  suggested_action: '调整预算后继续',
                  hint: 'token 用量超限',
                  stopped_at: null,
                },
              }),
            ],
            fused: [row({ id: 12, number: 'NB-12', title: '冻结单', needs_user: null })],
            plan_failed: [row({ id: 13, number: 'NB-13', title: '拆不动单', needs_user: null })],
            shadow: [{ id: 7, name: '日报规则', title: 'cov {date}', cron: '0 9 * * *', last_run_at: 1 }],
          },
          total: 4,
        })
      return Promise.resolve({})
    })
    const w = mount(BoardNeedsHumanPanel)
    await flushPromises()
    expect(w.find('[data-test="needs-human-empty"]').exists()).toBe(false)
    expect(w.find('[data-test="lane-escalated"]').exists()).toBe(true)
    expect(w.find('[data-test="lane-fused"]').exists()).toBe(true)
    expect(w.find('[data-test="lane-plan_failed"]').exists()).toBe(true)
    expect(w.find('[data-test="lane-shadow"]').exists()).toBe(true)
    const esc = w.find('[data-test="lane-escalated"]').text()
    expect(esc).toContain('NB-11')
    expect(esc).toContain('预算')
    expect(esc).toContain('调整预算后继续')
    // 影子规则：转正按钮 + 规则名。
    const shadowLane = w.find('[data-test="lane-shadow"]')
    expect(shadowLane.text()).toContain('日报规则')
    expect(shadowLane.find('[data-test="promote-btn"]').exists()).toBe(true)
  })

  it('转正：promote-btn → autopilot.update {id, mode: live} → 成功提示 + 重拉', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'board.needs_human.list')
        return Promise.resolve({
          groups: {
            escalated: [],
            fused: [],
            plan_failed: [],
            shadow: [{ id: 7, name: '日报规则', title: 'cov', cron: '0 9 * * *', last_run_at: 1 }],
          },
          total: 1,
        })
      if (cmd === 'autopilot.update') return Promise.resolve({})
      return Promise.resolve({})
    })
    const w = mount(BoardNeedsHumanPanel)
    await flushPromises()
    await w.find('[data-test="promote-btn"]').trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('board', 'autopilot.update', {
      id: 7,
      mode: 'live',
    })
    const toasts = useToast().toasts
    expect(toasts.some((t: any) => t.type === 'success' && t.message.includes('日报规则'))).toBe(
      true,
    )
    // 重拉：needs_human.list 至少调用两次（初载 + promote 后）。
    const calls = requestMock.mock.calls.filter((c: any[]) => c[1] === 'board.needs_human.list')
    expect(calls.length).toBeGreaterThanOrEqual(2)
  })

  it('board-changed 推送：200ms 防抖后静默换新（不闪 loading）', async () => {
    const w = mount(BoardNeedsHumanPanel)
    await flushPromises()
    expect(requestMock).toHaveBeenCalledTimes(1)
    const h = sseHandlers.get('board-changed')
    expect(h).toBeDefined()
    h!()
    // 防抖窗口内未重拉。
    expect(requestMock).toHaveBeenCalledTimes(1)
    await new Promise((r) => setTimeout(r, 300))
    await flushPromises()
    expect(requestMock.mock.calls.length).toBeGreaterThanOrEqual(2)
    // 静默换新不进 loading 分支：空态仍然在（数据未变）。
    expect(w.find('[data-test="needs-human-empty"]').exists()).toBe(true)
  })
})
