import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { useToast } from '../../../composables/useToast'

const requestMock = vi.fn()
vi.mock('../../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
  initWSAPI: vi.fn(),
}))
vi.mock('../../../composables/useSSE', () => ({ on: vi.fn(), off: vi.fn() }))
vi.mock('../HealthScore.vue', () => ({ default: { template: '<div class="stub-health" />', props: ['score'] } }))
vi.mock('../ActivityFeed.vue', () => ({ default: { template: '<div class="stub-feed" />', props: ['events'] } }))

import ClusterOverview from '../ClusterOverview.vue'

const STATUS = {
  running: true,
  health_score: 92,
  online_nodes: 3,
  total_nodes: 5,
  active_tasks: 2,
  today_completed: 7,
  total_tasks: 100,
  success_rate: 0.93,
  avg_duration: '12s',
  recent_events: [{ id: 'e1', kind: 'node.join' }],
}

function defaultImpl(_m: string, cmd: string) {
  if (cmd === 'runtime.status') return Promise.resolve(STATUS)
  return Promise.resolve({})
}

beforeEach(() => {
  requestMock.mockReset()
  requestMock.mockImplementation(defaultImpl)
  useToast().toasts.splice(0)
})

afterEach(() => {
  vi.useRealTimers()
})

describe('ClusterOverview 加载与渲染', () => {
  it('挂载拉 runtime.status 渲染指标', async () => {
    const w = mount(ClusterOverview)
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('cluster', 'runtime.status')
    expect(w.text()).toContain('3')
    expect(w.text()).toContain('/ 5')
    expect(w.text()).toContain('12s')
    expect(w.find('.stub-health').exists()).toBe(true)
    expect(w.find('.stub-feed').exists()).toBe(true)
    w.unmount()
  })

  it('status 缺字段回退缺省值', async () => {
    requestMock.mockImplementation((_m, cmd) => cmd === 'runtime.status' ? Promise.resolve({}) : Promise.resolve({}))
    const w = mount(ClusterOverview)
    await flushPromises()
    const vm = w.vm as any
    expect(vm.healthScore).toBe(0)
    expect(vm.successRate).toBe(null)
    expect(vm.avgDuration).toBe('--')
    w.unmount()
  })

  it('10s 轮询；卸载清理', async () => {
    vi.useFakeTimers()
    const w = mount(ClusterOverview)
    await vi.advanceTimersByTimeAsync(1)
    const n = requestMock.mock.calls.filter(c => c[1] === 'runtime.status').length
    vi.advanceTimersByTime(10000)
    await vi.advanceTimersByTimeAsync(1)
    expect(requestMock.mock.calls.filter(c => c[1] === 'runtime.status').length).toBe(n + 1)
    w.unmount()
    const frozen = requestMock.mock.calls.filter(c => c[1] === 'runtime.status').length
    vi.advanceTimersByTime(30000)
    await vi.advanceTimersByTimeAsync(1)
    expect(requestMock.mock.calls.filter(c => c[1] === 'runtime.status').length).toBe(frozen)
  })
})

describe('ClusterOverview 启停', () => {
  it('启动成功刷新；失败提示', async () => {
    const w = mount(ClusterOverview)
    await flushPromises()
    const calls = requestMock.mock.calls.filter(c => c[1] === 'runtime.status').length
    await (w.vm as any).startCluster()
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('cluster', 'runtime.start')
    expect(useToast().toasts.some(t => t.message === '集群已启动')).toBe(true)
    expect(requestMock.mock.calls.filter(c => c[1] === 'runtime.status').length).toBe(calls + 1)
    expect((w.vm as any).actionRunning).toBe(false)

    requestMock.mockImplementation((_m, cmd) => cmd === 'runtime.start' ? Promise.reject('busy') : defaultImpl(_m, cmd))
    await (w.vm as any).startCluster()
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('启动失败'))).toBe(true)
    w.unmount()
  })

  it('停止成功与失败', async () => {
    const w = mount(ClusterOverview)
    await flushPromises()
    await (w.vm as any).stopCluster()
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('cluster', 'runtime.stop')
    expect(useToast().toasts.some(t => t.message === '集群已停止')).toBe(true)

    requestMock.mockImplementation((_m, cmd) => cmd === 'runtime.stop' ? Promise.reject('x') : defaultImpl(_m, cmd))
    await (w.vm as any).stopCluster()
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('停止失败'))).toBe(true)
    expect((w.vm as any).actionRunning).toBe(false)
    w.unmount()
  })
})
