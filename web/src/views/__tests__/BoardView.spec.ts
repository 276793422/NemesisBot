// 拆解运行期可观测（2026-10-11「停着」事故）前端契约：
// - board.plan_started SSE → 看板容器层全局 toast（auto_start 建项目后用户
//   停在「项目」页签也能看到「拆解中」信号）；
// - 事件带 issue_number 优先展示，缺失回退 #issue_id。
// 后端真相源 = handlers/board.rs execute_plan_chain（RUNNING_PLANS + 事件）；
// 本 spec 只钉 toast 接线。
import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { useToast } from '../../composables/useToast'

const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
  initWSAPI: vi.fn(),
}))

// useSSE 打桩（连接层不在本测试范围）；plan_started 经注册的 handler 驱动。
// 注意：真实 useSSE 的 on 是「同类型多 handler 数组」（BoardView 的 toast 与
// usePlanning 单例并存）——打桩必须同语义，Map 单槽会让后注册者覆盖先注册
// 者（2026-10-12 横幅接入后实证）。
const sseHandlers = new Map<string, ((data: any) => void)[]>()
vi.mock('../../composables/useSSE', () => ({
  on: (ev: string, fn: (data: any) => void) => {
    const arr = sseHandlers.get(ev) || []
    arr.push(fn)
    sseHandlers.set(ev, arr)
  },
  off: (ev: string, fn?: (data: any) => void) => {
    if (!fn) sseHandlers.delete(ev)
    else sseHandlers.set(ev, (sseHandlers.get(ev) || []).filter((h) => h !== fn))
  },
}))

function dispatchPlanStarted(data: any) {
  for (const h of sseHandlers.get('board.plan_started') || []) h(data)
}

import BoardView from '../BoardView.vue'

beforeEach(() => {
  requestMock.mockReset()
  requestMock.mockResolvedValue({})
  useToast().toasts.splice(0)
})

describe('BoardView 拆解开始 toast（2026-10-11 观测面）', () => {
  it('board.plan_started → info toast 带 issue_number 与预期时长', async () => {
    const w = mount(BoardView)
    await flushPromises()
    expect(sseHandlers.has('board.plan_started')).toBe(true)
    dispatchPlanStarted({ issue_id: 7, issue_number: 'NB-7', plan_id: 'pl-1' })
    await flushPromises()
    const hit = useToast().toasts.some(
      (t) => t.type === 'info' && t.message.includes('NB-7') && t.message.includes('3~8 分钟'),
    )
    expect(hit).toBe(true)
    w.unmount()
  })

  it('issue_number 缺失 → 回退 #issue_id', async () => {
    const w = mount(BoardView)
    await flushPromises()
    dispatchPlanStarted({ issue_id: 9 })
    await flushPromises()
    expect(
      useToast().toasts.some((t) => t.type === 'info' && t.message.includes('#9')),
    ).toBe(true)
    w.unmount()
  })

  it('卸载后注销组件级订阅；usePlanning 单例 handler 常驻（真实语义）', async () => {
    const w = mount(BoardView)
    await flushPromises()
    useToast().toasts.splice(0)
    expect(sseHandlers.has('board.plan_started')).toBe(true)
    w.unmount()
    // 组件级 toast handler 已随卸载注销：再派发不产生新 toast。
    // （usePlanning 单例的 handler 仍留在数组里——模块级常驻是设计语义，
    // 真实 useSSE on() 是数组推送，二者并存。）
    dispatchPlanStarted({ issue_id: 11, issue_number: 'NB-11', plan_id: 'pl-11' })
    await flushPromises()
    expect(
      useToast().toasts.some((t) => t.type === 'info' && t.message.includes('NB-11')),
    ).toBe(false)
  })
})
