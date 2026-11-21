import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'

// IssueListView 列表加载陈旧回包围栏（BUG-20261120-032）：过滤输入框
// Enter 直调 loadIssues，与 board-changed 推送驱动的 silentRefresh 并发
// 时无任何闸——慢的旧回包后到会把新列表覆盖成旧过滤条件的结果。

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

beforeEach(() => {
  handlers.clear()
  _resetPlanningForTest()
  requestMock.mockReset()
  requestMock.mockImplementation(() => Promise.resolve({ issues: [], total: 0 }))
})

describe('IssueListView 列表加载陈旧回包围栏（BUG-20261120-032）', () => {
  it('并发 loadIssues：慢的旧回包不覆盖新列表', async () => {
    const w = mount(IssueListView)
    await flushPromises()
    const vm = w.vm as any
    const resolvers: ((v: any) => void)[] = []
    requestMock.mockImplementation((_m: string, cmd: string) => cmd === 'issue.list'
      ? new Promise(resolve => { resolvers.push(resolve) })
      : Promise.resolve({}))
    // 第一发（旧过滤条件）在飞
    vm.filterQuery = 'old'
    const p1 = vm.loadIssues()
    await flushPromises()
    // 第二发（新过滤条件）并发触发（Enter / board-changed 推送均可达）
    vm.filterQuery = 'new'
    const p2 = vm.loadIssues()
    await flushPromises()
    expect(resolvers.length).toBe(2)
    // 乱序回包：新（第 2 发）先回、旧（第 1 发）后回
    resolvers[1]({ issues: [{ id: 2, number: 'NB-2', title: '新结果' }], total: 1 })
    await flushPromises()
    resolvers[0]({ issues: [{ id: 1, number: 'NB-1', title: '旧结果' }], total: 1 })
    await flushPromises()
    await Promise.all([p1, p2])
    expect(vm.issues.map((i: any) => i.number)).toEqual(['NB-2'])
    expect(vm.total).toBe(1)
    w.unmount()
  })

  it('旧请求失败不弹错误提示（陈旧 toast 抑制）', async () => {
    const w = mount(IssueListView)
    await flushPromises()
    const vm = w.vm as any
    const gates: ((ok: boolean, v?: any) => void)[] = []
    requestMock.mockImplementation((_m: string, cmd: string) => cmd === 'issue.list'
      ? new Promise((resolve, reject) => gates.push((ok, v) => (ok ? resolve(v) : reject(v))))
      : Promise.resolve({}))
    vm.filterQuery = 'old'
    const p1 = vm.loadIssues()
    await flushPromises()
    vm.filterQuery = 'new'
    const p2 = vm.loadIssues()
    await flushPromises()
    gates[1](true, { issues: [{ id: 2, number: 'NB-2' }], total: 1 })
    await flushPromises()
    gates[0](false, new Error('stale net'))
    await flushPromises()
    await Promise.all([p1, p2])
    expect(vm.issues.map((i: any) => i.number)).toEqual(['NB-2'])
    w.unmount()
  })
})
