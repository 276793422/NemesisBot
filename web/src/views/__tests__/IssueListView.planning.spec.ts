import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'

// IssueListView 拆解在途徽标（§1.2A，2026-10-11 Part 1 配套前端）：
// - issue.list 带 planning 投影 → 状态列「🧩 拆解中 m:ss」替换原状态徽标
//   （reconcile 回填成 live 条目，自计时）；
// - SSE plan_started（attempt>1）→ 徽标带「重试 N」；
// - plan_ready → 恢复原状态徽标；
// - 终态行（done）即使带 planning 残影也显示原状态徽章。

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

import IssueListView from '../IssueListView.vue'
import { _resetPlanningForTest } from '../../composables/usePlanning'

function dispatch(type: string, data?: unknown) {
  for (const h of handlers.get(type) || []) h(data)
}

function row(over: Record<string, unknown>) {
  return {
    id: 1,
    number: 'NB-1',
    title: '任务',
    status: 'backlog',
    priority: 1,
    assignee: null,
    assignee_id: null,
    project_id: null,
    updated_at: 0,
    ...over,
  }
}

beforeEach(() => {
  handlers.clear()
  _resetPlanningForTest()
  requestMock.mockReset()
  requestMock.mockImplementation((_m: string, cmd: string, _d: any) => {
    if (cmd === 'issue.list') return Promise.resolve({ issues: [], total: 0 })
    return Promise.resolve({})
  })
  vi.useFakeTimers()
  vi.setSystemTime(0)
})

afterEach(() => {
  vi.useRealTimers()
})

async function mountWith(issues: any[]) {
  requestMock.mockImplementation((_m: string, cmd: string, _d: any) => {
    if (cmd === 'issue.list') return Promise.resolve({ issues, total: issues.length })
    return Promise.resolve({})
  })
  const w = mount(IssueListView)
  await flushPromises()
  return w
}

function statusCell(w: { findAll: (s: string) => any }, idx: number) {
  return w.findAll('tbody tr')[idx].findAll('td')[2]
}

describe('IssueListView 拆解在途徽标（§1.2A）', () => {
  it('planning 投影行 → 「🧩 拆解中 m:ss」徽标（reconcile 回填自计时）；无投影行 → 原状态徽标', async () => {
    const w = await mountWith([
      row({ id: 5, number: 'NB-5', planning: { elapsed_secs: 90, attempt: 1 } }),
      row({ id: 6, number: 'NB-6' }),
    ])
    const c5 = statusCell(w, 0)
    expect(c5.text()).toContain('🧩 拆解中')
    expect(c5.text()).toContain('1:30')
    expect(c5.text()).not.toContain('重试')
    expect(c5.text()).not.toContain('Backlog')

    const c6 = statusCell(w, 1)
    expect(c6.text()).toContain('Backlog')
    expect(c6.text()).not.toContain('拆解中')
    w.unmount()
  })

  it('SSE plan_started attempt>1 → 徽标带「重试 N」；plan_ready → 恢复原状态徽标', async () => {
    const w = await mountWith([row({ id: 6, number: 'NB-6' })])
    expect(statusCell(w, 0).text()).toContain('Backlog')

    dispatch('board.plan_started', { plan_id: 'p1', issue_id: 6, issue_number: 'NB-6', attempt: 3 })
    await flushPromises()
    const cell = statusCell(w, 0)
    expect(cell.text()).toContain('🧩 拆解中')
    expect(cell.text()).toContain('重试 3')

    dispatch('board.plan_ready', { plan_id: 'p1', issue_id: 6, subs: [] })
    await flushPromises()
    expect(statusCell(w, 0).text()).toContain('Backlog')
    expect(statusCell(w, 0).text()).not.toContain('拆解中')
    w.unmount()
  })

  it('终态行（done）即使带 planning 残影也显示原状态徽章', async () => {
    const w = await mountWith([
      row({ id: 7, number: 'NB-7', status: 'done', planning: { elapsed_secs: 30, attempt: 1 } }),
    ])
    const cell = statusCell(w, 0)
    expect(cell.text()).toContain('已完成')
    expect(cell.text()).not.toContain('拆解中')
    w.unmount()
  })
})
