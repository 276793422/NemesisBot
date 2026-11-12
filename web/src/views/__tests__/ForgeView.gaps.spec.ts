import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { useToast } from '../../composables/useToast'

const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
  initWSAPI: vi.fn(),
}))

import ForgeView from '../../views/ForgeView.vue'

function defaultImpl(_m: string, cmd: string, _p?: any) {
  switch (cmd) {
    case 'status':
      return Promise.resolve({ enabled: true, running: true, started_at: new Date(Date.now() - 7200_000).toISOString(), reflection_interval_secs: 21600, cleanup_interval_secs: 86400, learning_enabled: false })
    case 'stats':
      return Promise.resolve({ enabled: true, config: {} })
    case 'experiences.stats':
      return Promise.resolve({ total: 200, success: 150, tools: { read_file: { count: 30 }, edit: { count: 10 }, search: { count: 20 } } })
    case 'reflections.list':
      return Promise.resolve({ reports: [{ id: 'r1' }] })
    case 'reflections.latest':
      return Promise.resolve({ found: true, id: 'r0' })
    case 'cycles.list':
      return Promise.resolve({ cycles: [{ id: 'c1', status: 'Running' }] })
    case 'registry.list':
      return Promise.resolve({ artifacts: [{ id: 'a1', status: 'Draft' }], skill_directories: [] })
    case 'reflect':
      return Promise.resolve({ triggered: true, message: '反思已触发' })
    case 'config.get':
      return Promise.resolve({ config: { test: { judge_model: 'glm' } }, defaults: false })
    default:
      return Promise.resolve({})
  }
}

beforeEach(() => {
  requestMock.mockReset()
  requestMock.mockImplementation(defaultImpl)
  useToast().toasts.splice(0)
})

async function mountView() {
  const w = mount(ForgeView)
  await flushPromises()
  return w
}

describe('ForgeView 格式化辅助', () => {
  it('formatRate/formatDuration/formatDate/formatCount 各分支', async () => {
    const w = await mountView()
    const vm = w.vm as any
    expect(vm.formatRate(0.456)).toBe('45.6%')
    expect(vm.formatDuration(500)).toBe('500ms')
    expect(vm.formatDuration(1500)).toBe('1.5s')
    expect(vm.formatDate('')).toBe('--')
    expect(vm.formatCount(undefined)).toBe('0')
    expect(vm.formatCount(500)).toBe('500')
    expect(vm.formatCount(1500)).toBe('1.5k')
    expect(vm.formatCount(25000)).toBe('2.5w')
    expect(vm.statusClass('Active')).toBe('badge-success')
    expect(vm.statusClass('Unknown')).toBe('badge-neutral')
    expect(vm.statusLabel('Observing')).toBe('观察中')
    expect(vm.statusLabel('Whatever')).toBe('Whatever')
    expect(vm.cycleStatusLabel('Running')).toBe('运行中')
    expect(vm.cycleStatusClass('Failed')).toBe('badge-error')
    expect(vm.efficacyColor('positive')).toBe('var(--color-success)')
    expect(vm.efficacyColor('weird')).toBe('var(--text-muted)')
    expect(vm.efficacyLabel('insufficient_data')).toBe('数据不足')
    expect(vm.efficacyLabel('weird')).toBe('weird')
  })

  it('formatCountdown 与倒计时（未运行回 --）', async () => {
    const w = await mountView()
    const vm = w.vm as any
    expect(vm.formatCountdown(0)).toBe('--')
    expect(vm.formatCountdown(45)).toBe('45s')
    expect(vm.formatCountdown(125)).toBe('2m 5s')
    expect(vm.formatCountdown(7325)).toBe('2h 2m 5s')
    // running + started_at → 倒计时已计算（loadStatus 时 updateCountdowns）
    expect(vm.reflectionCountdown).toBeGreaterThan(0)
    expect(vm.cleanupCountdown).toBeGreaterThan(0)
  })
})

describe('ForgeView 动作', () => {
  it('toggleForge 成功与失败', async () => {
    const w = await mountView()
    ;(w.vm as any).enabled = false
    ;(w.vm as any).running = false
    await (w.vm as any).toggleForge()
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('forge', 'config.save', { enabled: true })
    expect(useToast().toasts.some(t => t.message === '已启用并启动')).toBe(true)

    requestMock.mockImplementation((_m, cmd) => cmd === 'config.save' ? Promise.reject(new Error('ro')) : defaultImpl(_m, cmd, {}))
    await (w.vm as any).toggleForge()
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('操作失败'))).toBe(true)
  })

  it('toggleLearning 与 triggerReflect 三态', async () => {
    const w = await mountView()
    const vm = w.vm as any
    requestMock.mockImplementation((_m, cmd) => cmd === 'status' ? Promise.resolve({ enabled: true, running: true, started_at: null, learning_enabled: true }) : defaultImpl(_m, cmd, {}))
    vm.learningEnabled = false
    await vm.toggleLearning()
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('forge', 'learning.toggle', { enabled: true })
    expect(useToast().toasts.some(t => t.message.includes('已启用'))).toBe(true)

    await vm.triggerReflect()
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('forge', 'reflect')
    expect(useToast().toasts.some(t => t.message.includes('反思'))).toBe(true)

    requestMock.mockImplementation((_m, cmd) => cmd === 'reflect' ? Promise.resolve({ triggered: false }) : defaultImpl(_m, cmd, {}))
    await vm.triggerReflect()
    await flushPromises()
    expect(useToast().toasts.some(t => t.message === '无法执行反思')).toBe(true)

    requestMock.mockImplementation((_m, cmd) => cmd === 'reflect' ? Promise.reject(new Error('busy')) : defaultImpl(_m, cmd, {}))
    await vm.triggerReflect()
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('触发失败'))).toBe(true)
  })

  it('updateArtifactStatus 成功刷新；失败提示', async () => {
    const w = await mountView()
    const calls = requestMock.mock.calls.filter(c => c[1] === 'registry.list').length
    await (w.vm as any).updateArtifactStatus('a1', 'Active')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('forge', 'registry.update', { id: 'a1', status: 'Active' })
    expect(useToast().toasts.some(t => t.message === '状态已更新')).toBe(true)
    expect(requestMock.mock.calls.filter(c => c[1] === 'registry.list').length).toBe(calls + 1)

    requestMock.mockImplementation((_m, cmd) => cmd === 'registry.update' ? Promise.reject(new Error('locked')) : defaultImpl(_m, cmd, {}))
    await (w.vm as any).updateArtifactStatus('a1', 'Active')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('更新失败'))).toBe(true)
  })

  it('高级配置保存：8 键整批载荷与失败分支', async () => {
    const w = await mountView()
    await w.findAll('button.tab').find(b => b.text() === '配置')!.trigger('click')
    await flushPromises()
    const vm = w.vm as any
    expect(vm.advConfig.test.judge_model).toBe('glm')
    vm.advConfig.replay_auto_ingest = 'yes'
    vm.advConfig.reflection = { use_llm: true }
    vm.advConfig.optimize = { budget_tokens: 'abc' }
    await vm.saveAdvanced()
    await flushPromises()
    const setCalls = requestMock.mock.calls.filter(c => c[1] === 'config.set')
    expect(setCalls.length).toBe(1)
    const values = setCalls[0][2].values
    expect(values.replay_auto_ingest).toBe(false) // 非严格 true → false
    expect(values['reflection.use_llm']).toBe(true)
    expect(values['optimize.budget_tokens']).toBeNaN() // 后端 ?? 只兜 null，非法串如实传 NaN
    expect(useToast().toasts.some(t => t.message === '高级设置已保存')).toBe(true)

    requestMock.mockImplementation((_m, cmd) => cmd === 'config.set' ? Promise.reject(new Error('bad key')) : defaultImpl(_m, cmd, {}))
    await vm.saveAdvanced()
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('保存失败'))).toBe(true)
    expect(vm.advSaving).toBe(false)
  })
})

describe('ForgeView 页签懒加载与 computed', () => {
  it('switchTab 各分支只加载一次', async () => {
    const w = await mountView()
    const vm = w.vm as any
    await vm.switchTab('experiences')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('forge', 'experiences.stats')
    expect(vm.successRate).toBe('75.0')
    expect(vm.sortedTools.map((t: any) => t.name)).toEqual(['read_file', 'search', 'edit'])

    const calls = requestMock.mock.calls.length
    await vm.switchTab('experiences')
    expect(requestMock.mock.calls.length).toBe(calls)

    await vm.switchTab('reflections')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('forge', 'reflections.list')
    expect(requestMock).toHaveBeenCalledWith('forge', 'reflections.latest')

    await vm.switchTab('cycles')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('forge', 'cycles.list')
    expect(vm.cycles.length).toBe(1)

    await vm.switchTab('artifacts')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('forge', 'registry.list')
    expect(vm.artifacts.length).toBe(1)
  })
})
