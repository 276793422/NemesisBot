import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { useToast } from '../../../composables/useToast'

// 工作流设置（批次 2 配置页补全）：workflow.wall_clock_budget_secs 墙钟
// 保险丝——缺省 3600、改动 set_field、非法回退不写盘。后端执行侧语义由
// workflow runner 测试钉住；本 spec 钉前端渲染分支与请求形态。

const requestMock = vi.fn()
vi.mock('../../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
  initWSAPI: vi.fn(),
}))

import WorkflowSettings from '../WorkflowSettings.vue'

beforeEach(() => {
  requestMock.mockReset()
  useToast().toasts.splice(0)
})

function impl(cfg: any) {
  requestMock.mockImplementation((_m: string, cmd: string) => {
    if (cmd === 'get') return Promise.resolve(cfg)
    if (cmd === 'set_field') return Promise.resolve({ ok: true })
    return Promise.resolve({})
  })
}

describe('WorkflowSettings 墙钟保险丝（批次 2）', () => {
  it('config 缺 workflow 段 → 渲染缺省 3600；改动 → set_field {path, value:600}', async () => {
    impl({})
    const w = mount(WorkflowSettings)
    await flushPromises()
    const input = w.find('input[type="number"]')
    expect((input.element as HTMLInputElement).value).toBe('3600')
    await input.setValue('600')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('config', 'set_field', { path: 'workflow.wall_clock_budget_secs', value: 600 })
    expect(useToast().toasts.some(t => t.type === 'success' && t.message.includes('workflow.wall_clock_budget_secs'))).toBe(true)
    w.unmount()
  })

  it('已配置 1800 → 渲染 1800（不猜缺省）', async () => {
    impl({ workflow: { wall_clock_budget_secs: 1800 } })
    const w = mount(WorkflowSettings)
    await flushPromises()
    expect((w.find('input[type="number"]').element as HTMLInputElement).value).toBe('1800')
    w.unmount()
  })

  it('非法（-1）→ 本地拦截不写盘 + error toast；config.get 失败 → 兜底 3600', async () => {
    impl({})
    const w = mount(WorkflowSettings)
    await flushPromises()
    await w.find('input[type="number"]').setValue('-1')
    await flushPromises()
    expect(requestMock).not.toHaveBeenCalledWith('config', 'set_field', expect.anything())
    expect(useToast().toasts.some(t => t.type === 'error')).toBe(true)
    w.unmount()

    requestMock.mockReset()
    requestMock.mockImplementation(() => Promise.reject(new Error('cfg down')))
    const w2 = mount(WorkflowSettings)
    await flushPromises()
    expect((w2.find('input[type="number"]').element as HTMLInputElement).value).toBe('3600')
    w2.unmount()
  })
})
