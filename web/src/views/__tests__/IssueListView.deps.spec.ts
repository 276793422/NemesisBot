import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'

// IssueListView 上游依赖徽标（2026-10-15 需求①）：标题列「🔒 等 NB-x」
// （未完成上游）/「⚠ NB-x 已取消」（死等异常）；done 上游不显示。

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

function titleCell(w: { findAll: (s: string) => any }, idx: number) {
  return w.findAll('tbody tr')[idx].findAll('td')[1]
}

describe('IssueListView 上游依赖徽标（需求①）', () => {
  it('未完成上游 → 「🔒 等 NB-x」；done 上游不显示；已取消上游 → 「⚠ 已取消」示警', async () => {
    const w = await mountWith([
      row({
        id: 9,
        number: 'NB-9',
        depends_on: [
          { id: 1, number: 'NB-1', status: 'done' },
          { id: 2, number: 'NB-2', status: 'in_progress' },
          { id: 3, number: 'NB-3', status: 'cancelled' },
        ],
      }),
    ])
    const cell = titleCell(w, 0)
    expect(cell.text()).toContain('🔒 等 NB-2')
    expect(cell.text()).toContain('⚠ NB-3 已取消')
    expect(cell.text()).not.toContain('NB-1')
    w.unmount()
  })

  it('全部上游 done / 无 depends_on → 无依赖徽标', async () => {
    const w = await mountWith([
      row({ id: 4, number: 'NB-4', depends_on: [{ id: 1, number: 'NB-1', status: 'done' }] }),
      row({ id: 5, number: 'NB-5' }),
    ])
    for (const i of [0, 1]) {
      expect(titleCell(w, i).findAll('.dep-badge').length).toBe(0)
    }
    w.unmount()
  })

  it('徽标 tooltip 说明派发闸语义（上游完成后才可派发）', async () => {
    const w = await mountWith([
      row({ id: 6, number: 'NB-6', depends_on: [{ id: 2, number: 'NB-2', status: 'todo' }] }),
    ])
    const badge = titleCell(w, 0).find('.dep-badge')
    expect(badge.attributes('title')).toContain('完成后本单才可派发')
    w.unmount()
  })
})
