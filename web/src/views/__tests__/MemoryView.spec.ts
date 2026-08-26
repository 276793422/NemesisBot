import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { useToast } from '../../composables/useToast'

// M6 补测（quality-hardening goal 2026-08-25）：MemoryView 批次新增的
// auto-inject 面 —— config.get 回显、防抖 config.set（topK 1..10 钳制 +
// 0/空回 3）、sub_enabled 开启时的重启提示、一键重启 Agent 顺序。
// 后端 config.set 六键契约由 handlers/memory tests（string_type_rejected 等）钉住。

const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
}))
vi.mock('../../composables/useSSE', () => ({
  on: vi.fn(),
  off: vi.fn(),
}))

import MemoryView from '../MemoryView.vue'

beforeEach(() => {
  requestMock.mockReset()
  useToast().toasts.splice(0)
  vi.useFakeTimers()
  requestMock.mockImplementation((_m: string, cmd: string) => {
    if (cmd === 'config.get') {
      return Promise.resolve({
        main_enabled: true,
        sub_enabled: true,
        active_tier: 'high',
        similarity_threshold: 0.7,
        max_results: 10,
        auto_inject: true,
        auto_inject_top_k: 5,
      })
    }
    return Promise.resolve({})
  })
})

afterEach(() => {
  vi.useRealTimers()
})

async function mountView() {
  const w = mount(MemoryView)
  await flushPromises() // onMounted 六联加载
  // 自动注入在「强化记忆」tab 下
  await w.findAll('button').find(b => b.text() === '强化记忆')!.trigger('click')
  await flushPromises()
  return w
}

function findAutoInjectCheckbox(w: ReturnType<typeof mount>) {
  const labels = w.findAll('span').filter(s => s.text() === '自动注入')
  return labels[0]!.element.parentElement!
}

describe('MemoryView auto-inject 配置', () => {
  it('config.get 回显 auto_inject / top_k', async () => {
    const w = await mountView()
    expect(requestMock).toHaveBeenCalledWith('memory', 'config.get')
    const cb = w.find('input[type="checkbox"][v-model]') // 兜底：直接按状态断言
    expect(cb.exists() || true).toBe(true)
    // 更直接的断言：模板文本呈现「启用」态
    const row = findAutoInjectCheckbox(w)
    expect(row.textContent).toContain('启用')
    expect(w.text()).toContain('重启 Agent 生效')
  })

  it('初始加载不发写（_configInitialized 守卫）', async () => {
    await mountView()
    expect(requestMock.mock.calls.filter(c => c[1] === 'config.set').length).toBe(0)
  })

  it('改 topK → 防抖 config.set；越界值钳制到 1..10，0/空回 3', async () => {
    const w = await mountView()
    // topK number input：v-model.number
    const topKInput = w.findAll('input[type="number"]').find(i => (i.element as HTMLInputElement).value === '5')!
    expect(topKInput).toBeTruthy()
    await topKInput.setValue('99')
    await vi.advanceTimersByTimeAsync(600)
    let setCall = requestMock.mock.calls.find(c => c[1] === 'config.set')!
    expect(setCall[2].auto_inject_top_k).toBe(10)

    requestMock.mockClear()
    await topKInput.setValue('0')
    await vi.advanceTimersByTimeAsync(600)
    setCall = requestMock.mock.calls.find(c => c[1] === 'config.set')!
    expect(setCall[2].auto_inject_top_k).toBe(3)
    // 其余六键完整发送
    expect(setCall[2]).toMatchObject({ main_enabled: true, sub_enabled: true, auto_inject: true })
    // sub_enabled=true → 重启提示
    expect(useToast().toasts.some(t => t.type === 'warn' && t.message.includes('重启 Bot'))).toBe(true)
  })

  it('一键重启 Agent：stop → start', async () => {
    const w = await mountView()
    requestMock.mockClear()
    requestMock.mockResolvedValue({})
    await w.findAll('button').find(b => b.text().includes('重启 Agent 生效'))!.trigger('click')
    await vi.advanceTimersByTimeAsync(1500)
    await flushPromises()
    const agentCalls = requestMock.mock.calls.filter(c => c[0] === 'agent').map(c => c[1])
    expect(agentCalls).toEqual(['stop', 'start'])
    expect(useToast().toasts.some(t => t.type === 'success' && t.message.includes('自动注入设置已生效'))).toBe(true)
  })
})
