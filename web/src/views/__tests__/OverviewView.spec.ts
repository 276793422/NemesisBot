import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { useToast } from '../../composables/useToast'

const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
  initWSAPI: vi.fn(),
}))

const httpGetMock = vi.fn()
vi.mock('../../composables/useWebSocket', () => ({
  httpGet: (...args: any[]) => httpGetMock(...args),
}))
vi.mock('../../lib/appBase', () => ({
  apiUrl: (p: string) => 'http://test' + p,
}))

const sseHandlers: Record<string, ((d: any) => void)[]> = {}
const sseOnMock = vi.fn((ev: string, fn: (d: any) => void) => {
  (sseHandlers[ev] ||= []).push(fn)
})
const sseOffMock = vi.fn()
vi.mock('../../composables/useSSE', () => ({
  on: (...a: any[]) => sseOnMock(...(a as [string, (d: any) => void])),
  off: (...a: any[]) => sseOffMock(...(a as [string, (d: any) => void])),
}))

import OverviewView from '../OverviewView.vue'

const STATUS = {
  version: '1.2.3',
  uptime_seconds: 90061, // 1天1小时1分钟1秒
  ws_connected: true,
  session_count: 7,
  model: 'glm-5',
  model_base: 'https://api.example.com',
  model_has_key: true,
}

beforeEach(() => {
  vi.useFakeTimers()
  requestMock.mockReset()
  requestMock.mockImplementation(() => Promise.resolve({}))
  httpGetMock.mockReset()
  httpGetMock.mockResolvedValue(STATUS)
  for (const k of Object.keys(sseHandlers)) delete sseHandlers[k]
  sseOnMock.mockClear()
  sseOffMock.mockClear()
  useToast().toasts.splice(0)
  vi.stubGlobal('confirm', vi.fn(() => true))
})

afterEach(() => {
  vi.useRealTimers()
  vi.unstubAllGlobals()
})

describe('OverviewView 概览数据', () => {
  it('加载 status 渲染四卡片；uptime 每秒自增', async () => {
    vi.useFakeTimers()
    const w = mount(OverviewView)
    await vi.advanceTimersByTimeAsync(1)
    expect(httpGetMock).toHaveBeenCalledWith('http://test/api/status')
    expect(w.text()).toContain('1.2.3')
    expect(w.text()).toContain('已连接')
    expect(w.text()).toContain('7')
    expect(w.text()).toContain('1天 1小时 1分钟')
    const vm = w.vm as any
    const base = vm.displayUptime
    vi.advanceTimersByTime(3000)
    await vi.advanceTimersByTimeAsync(1)
    expect(vm.displayUptime).toBe(base + 3)
    w.unmount()
  })

  it('status 拉取失败静默进入非加载态', async () => {
    const errSpy = vi.spyOn(console, 'error').mockImplementation(() => {})
    httpGetMock.mockRejectedValue(new Error('down'))
    const w = mount(OverviewView)
    await vi.advanceTimersByTimeAsync(1)
    expect(errSpy).toHaveBeenCalled()
    expect((w.vm as any).loading).toBe(false)
    expect(w.text()).toContain('--')
    errSpy.mockRestore()
    w.unmount()
  })

  it('SSE status 事件合并数据并重启 uptime 计时', async () => {
    vi.useFakeTimers()
    const w = mount(OverviewView)
    await vi.advanceTimersByTimeAsync(1)
    expect(sseOnMock).toHaveBeenCalledWith('status', expect.any(Function))
    sseHandlers['status'][0]({ version: '2.0.0', uptime_seconds: 10, session_count: 3 })
    await flushPromises()
    expect(w.text()).toContain('2.0.0')
    expect((w.vm as any).displayUptime).toBe(10)
    vi.advanceTimersByTime(1000)
    await vi.advanceTimersByTimeAsync(1)
    expect((w.vm as any).displayUptime).toBe(11)
    w.unmount()
  })

  it('卸载时注销 SSE 与计时器', async () => {
    vi.useFakeTimers()
    const w = mount(OverviewView)
    await vi.advanceTimersByTimeAsync(1)
    w.unmount()
    expect(sseOffMock).toHaveBeenCalledWith('status', expect.any(Function))
    const v = (w.vm as any).displayUptime
    vi.advanceTimersByTime(5000)
    expect((w.vm as any).displayUptime).toBe(v)
  })
})

describe('OverviewView formatUptime', () => {
  it('各分支', async () => {
    const w = mount(OverviewView)
    await vi.advanceTimersByTimeAsync(1)
    const vm = w.vm as any
    expect(vm.formatUptime(-1)).toBe('--')
    expect(vm.formatUptime(0)).toBe('0秒')
    expect(vm.formatUptime(59)).toBe('59秒')
    expect(vm.formatUptime(60)).toBe('1分钟')
    expect(vm.formatUptime(3660)).toBe('1小时 1分钟')
    expect(vm.formatUptime(90061)).toBe('1天 1小时 1分钟')
    expect(vm.formatUptime(86400)).toBe('1天')
    w.unmount()
  })
})

describe('OverviewView Agent 开关', () => {
  it('agent.status 投影 running；toggle 启动成功', async () => {
    let statusCalls = 0
    requestMock.mockImplementation((_m, cmd) => {
      if (cmd === 'status') { statusCalls++; return Promise.resolve({ running: statusCalls > 1 }) }
      if (cmd === 'start') return Promise.resolve({ started: true })
      return Promise.resolve({})
    })
    const w = mount(OverviewView)
    await vi.advanceTimersByTimeAsync(1)
    expect(w.text()).toContain('停止中')
    await w.find('.toggle').trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('agent', 'start')
    expect(useToast().toasts.some(t => t.message.includes('已启动'))).toBe(true)
    expect((w.vm as any).agentRunning).toBe(true)
    w.unmount()
  })

  it('启动未 started 响应与失败分支', async () => {
    requestMock.mockImplementation((_m, cmd) => cmd === 'start' ? Promise.resolve({}) : Promise.resolve({}))
    const w = mount(OverviewView)
    await vi.advanceTimersByTimeAsync(1)
    ;(w.vm as any).agentRunning = false
    await w.find('.toggle').trigger('click')
    await flushPromises()
    expect((w.vm as any).agentRunning).toBe(false)

    requestMock.mockImplementation((_m, cmd) => cmd === 'start' ? Promise.reject(new Error('busy')) : Promise.resolve({}))
    await w.find('.toggle').trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('启动失败'))).toBe(true)
    expect((w.vm as any).agentLoading).toBe(false)
    w.unmount()
  })

  it('停止：confirm 拦截、成功、失败', async () => {
    const w = mount(OverviewView)
    await vi.advanceTimersByTimeAsync(1)
    vi.stubGlobal('confirm', vi.fn(() => false))
    await w.find('.toggle').trigger('click')
    await flushPromises()
    expect(requestMock.mock.calls.filter(c => c[1] === 'stop').length).toBe(0)

    requestMock.mockImplementation((_m, cmd) => cmd === 'stop' ? Promise.resolve({ stopped: true }) : Promise.resolve({}))
    vi.stubGlobal('confirm', vi.fn(() => true))
    await w.find('.toggle').trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('已停止'))).toBe(true)
    expect((w.vm as any).agentRunning).toBe(false)

    requestMock.mockImplementation((_m, cmd) => cmd === 'stop' ? Promise.reject(new Error('x')) : Promise.resolve({}))
    ;(w.vm as any).agentRunning = true
    await w.find('.toggle').trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('停止失败'))).toBe(true)
    w.unmount()
  })
})
