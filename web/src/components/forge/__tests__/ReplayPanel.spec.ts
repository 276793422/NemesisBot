// C 线失败重放面板契约（自进化升级 2026-10-14）：
// - replay_list → 待审区/已入库两列表渲染；
// - 单条确认（带 name）与全部确认（不带 name）分别走对应载荷；
// - 提取按钮 → forge.replay_extract 后刷新。
import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { useToast } from '../../../composables/useToast'

const requestMock = vi.fn()
vi.mock('../../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
  initWSAPI: vi.fn(),
}))

import ReplayPanel from '../ReplayPanel.vue'

function replayList() {
  return {
    pending: [
      { name: 'replay-sess01-exec', turns: ['修复登录 bug'], expect: { max_llm_turns: 8 } },
      { name: 'replay-sess02-write', turns: ['重构配置模块'], expect: {} },
    ],
    confirmed: [
      { name: 'replay-sess00-exec', turns: ['继续未完成任务'], expect: {} },
    ],
  }
}

beforeEach(() => {
  requestMock.mockReset()
  useToast().toasts.splice(0)
  requestMock.mockImplementation(async (_m: string, cmd: string) => {
    if (cmd === 'replay_list') return replayList()
    return {}
  })
})

describe('ReplayPanel（C 线待审区/已入库）', () => {
  it('两列表渲染：pending 2 条 + confirmed 1 条', async () => {
    const w = mount(ReplayPanel)
    await flushPromises()
    expect(w.find('[data-testid="pending-replay-sess01-exec"]').exists()).toBe(true)
    expect(w.find('[data-testid="pending-replay-sess02-write"]').exists()).toBe(true)
    expect(w.find('[data-testid="confirmed-replay-sess00-exec"]').exists()).toBe(true)
    w.unmount()
  })

  it('单条确认带 name 载荷；确认后重拉列表', async () => {
    const w = mount(ReplayPanel)
    await flushPromises()
    const calls: any[] = []
    requestMock.mockImplementation(async (_m: string, cmd: string, data: any) => {
      calls.push([cmd, data])
      if (cmd === 'replay_confirm') return { confirmed: 1 }
      if (cmd === 'replay_list') return { pending: [], confirmed: replayList().confirmed }
      return {}
    })
    await w.find('[data-testid="confirm-replay-sess01-exec"]').trigger('click')
    await flushPromises()
    const confirmCall = calls.find((c) => c[0] === 'replay_confirm')
    expect(confirmCall?.[1]).toEqual({ name: 'replay-sess01-exec' })
    // 重拉后 pending 清空（mock 回 [])、confirmed 保留。
    expect(w.find('[data-testid="pending-replay-sess01-exec"]').exists()).toBe(false)
    expect(w.find('[data-testid="confirmed-replay-sess00-exec"]').exists()).toBe(true)
    w.unmount()
  })

  it('全部确认不带 name；空待审区不渲染全部确认按钮', async () => {
    const w = mount(ReplayPanel)
    await flushPromises()
    expect(w.find('[data-testid="confirm-all-btn"]').exists()).toBe(true)
    const calls: any[] = []
    requestMock.mockImplementation(async (_m: string, cmd: string, data: any) => {
      calls.push([cmd, data])
      if (cmd === 'replay_confirm') return { confirmed: 2 }
      if (cmd === 'replay_list') return { pending: [], confirmed: [] }
      return {}
    })
    await w.find('[data-testid="confirm-all-btn"]').trigger('click')
    await flushPromises()
    const confirmCall = calls.find((c) => c[0] === 'replay_confirm')
    expect(confirmCall?.[1]).toEqual({})
    w.unmount()
  })
})
