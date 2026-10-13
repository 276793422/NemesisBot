import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'

// ProjectPanel 拆解在途行（§1.2B，2026-10-11 Part 1 配套前端）：
// - issue.list 的 planning 投影按 project_id 聚合 → 项目进度列「🧩 NB-x AI
//   拆解中（已 m:ss）」；attempt>1 带「第 N 次尝试」；
// - plan_ready 后 board-changed 推送触发静默刷新 → 行消失（新列表无 planning）。

const handlers = new Map<string, ((data?: unknown) => void)[]>()

vi.mock('../../../composables/useSSE', () => ({
  on: vi.fn((type: string, handler: (data?: unknown) => void) => {
    const arr = handlers.get(type) || []
    arr.push(handler)
    handlers.set(type, arr)
  }),
  off: vi.fn(),
}))

const requestMock = vi.fn()
vi.mock('../../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
}))

import ProjectPanel from '../ProjectPanel.vue'
import { _resetPlanningForTest } from '../../../composables/usePlanning'

function dispatch(type: string, data?: unknown) {
  for (const h of handlers.get(type) || []) h(data)
}

const PROJECT = {
  id: 1,
  name: 'web 优化',
  description: '',
  status: 'in_progress',
  icon: '🚀',
  created_at: 0,
}

function issueRow(over: Record<string, unknown>) {
  return {
    id: 5,
    number: 'NB-5',
    title: '子任务',
    status: 'backlog',
    priority: 1,
    assignee: null,
    assignee_id: null,
    project_id: 1,
    updated_at: 0,
    ...over,
  }
}

beforeEach(() => {
  handlers.clear()
  _resetPlanningForTest()
  requestMock.mockReset()
  requestMock.mockImplementation((_m: string, cmd: string, _d: any) => {
    if (cmd === 'project.list') return Promise.resolve({ projects: [PROJECT] })
    if (cmd === 'project.progress') return Promise.resolve({ projects: [] })
    if (cmd === 'issue.list') return Promise.resolve({ issues: [], total: 0 })
    return Promise.resolve({})
  })
  vi.useFakeTimers()
  vi.setSystemTime(0)
})

afterEach(() => {
  vi.useRealTimers()
})

async function mountPanel(initialIssues: any[]) {
  requestMock.mockImplementation((_m: string, cmd: string, _d: any) => {
    if (cmd === 'project.list') return Promise.resolve({ projects: [PROJECT] })
    if (cmd === 'project.progress') return Promise.resolve({ projects: [] })
    if (cmd === 'issue.list') return Promise.resolve({ issues: initialIssues, total: initialIssues.length })
    return Promise.resolve({})
  })
  const w = mount(ProjectPanel)
  await flushPromises()
  return w
}

describe('ProjectPanel 拆解在途行（§1.2B）', () => {
  it('planning 投影行按项目聚合 → 进度列「🧩 NB-5 AI 拆解中（已 3:00）」', async () => {
    const w = await mountPanel([
      issueRow({ planning: { elapsed_secs: 180, attempt: 1 } }),
    ])
    const rowsEl = w.findAll('[data-test="project-planning-row"]')
    expect(rowsEl.length).toBe(1)
    expect(rowsEl[0].text()).toContain('NB-5 AI 拆解中')
    expect(rowsEl[0].text()).toContain('3:00')
    expect(rowsEl[0].text()).not.toContain('第')
    expect(rowsEl[0].classes()).not.toContain('abnormal')
    w.unmount()
  })

  it('attempt>1 带「第 N 次尝试」；无 planning 行 → 无在途行', async () => {
    const w = await mountPanel([
      issueRow({ planning: { elapsed_secs: 60, attempt: 2 } }),
      issueRow({ id: 6, number: 'NB-6' }),
    ])
    const rowsEl = w.findAll('[data-test="project-planning-row"]')
    expect(rowsEl.length).toBe(1)
    expect(rowsEl[0].text()).toContain('第 2 次尝试')
    w.unmount()
  })

  it('plan_ready → board-changed 推送静默刷新 → 在途行消失', async () => {
    const w = await mountPanel([
      issueRow({ planning: { elapsed_secs: 60, attempt: 1 } }),
    ])
    expect(w.findAll('[data-test="project-planning-row"]').length).toBe(1)

    // 后端 plan_ready 会撤投影 + 发 board-changed；前端模拟：换响应 + 推送。
    requestMock.mockImplementation((_m: string, cmd: string, _d: any) => {
      if (cmd === 'project.list') return Promise.resolve({ projects: [PROJECT] })
      if (cmd === 'project.progress') return Promise.resolve({ projects: [] })
      if (cmd === 'issue.list') return Promise.resolve({ issues: [issueRow({})], total: 1 })
      return Promise.resolve({})
    })
    dispatch('board-changed')
    await vi.advanceTimersByTimeAsync(250)
    await flushPromises()
    expect(w.findAll('[data-test="project-planning-row"]').length).toBe(0)
    w.unmount()
  })
})
