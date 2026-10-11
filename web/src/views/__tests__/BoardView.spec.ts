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
const sseHandlers = new Map<string, (data: any) => void>()
vi.mock('../../composables/useSSE', () => ({
  on: (ev: string, fn: (data: any) => void) => sseHandlers.set(ev, fn),
  off: (ev: string) => sseHandlers.delete(ev),
  useSSE: () => ({ on: sseHandlers.get, off: sseHandlers.delete }),
}))

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
    const h = sseHandlers.get('board.plan_started')
    expect(h).toBeTruthy()
    h!({ issue_id: 7, issue_number: 'NB-7', plan_id: 'pl-1' })
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
    sseHandlers.get('board.plan_started')!({ issue_id: 9 })
    await flushPromises()
    expect(
      useToast().toasts.some((t) => t.type === 'info' && t.message.includes('#9')),
    ).toBe(true)
    w.unmount()
  })

  it('卸载后注销订阅（off 被调用）', async () => {
    const w = mount(BoardView)
    await flushPromises()
    expect(sseHandlers.has('board.plan_started')).toBe(true)
    w.unmount()
    expect(sseHandlers.has('board.plan_started')).toBe(false)
  })
})
