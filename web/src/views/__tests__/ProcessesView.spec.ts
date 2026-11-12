import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'

const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
  initWSAPI: vi.fn(),
}))
vi.mock('../../composables/useSSE', () => ({ on: vi.fn(), off: vi.fn() }))
vi.mock('../../composables/useToast', () => {
  const toasts: { type: string; message: string }[] = []
  return {
    useToast: () => ({
      toasts,
      error: (m: string) => toasts.push({ type: 'error', message: m }),
      success: (m: string) => toasts.push({ type: 'success', message: m }),
      warn: (m: string) => toasts.push({ type: 'warn', message: m }),
      info: (m: string) => toasts.push({ type: 'info', message: m }),
    }),
  }
})

const replaceMock = vi.fn()
let queryMock: Record<string, unknown> = {}
vi.mock('vue-router', () => ({
  useRoute: () => ({ query: queryMock, replace: replaceMock }),
  useRouter: () => ({ replace: replaceMock }),
}))

vi.mock('../../components/process/ProcessOutputTail.vue', () => ({
  default: { template: '<div class="stub-tail" />', props: ['jobId', 'mode'] },
}))

import ProcessesView from '../ProcessesView.vue'
import { useToast } from '../../composables/useToast'

const JOB_RUN = { id: 2, command: 'python worker.py  --loop', cwd: 'C:/x', started_at_unix: 1762000000, running: true, steerable: true, total_bytes: 2048, dropped_bytes: 0 }
const JOB_DONE = { id: 1, command: 'git status', cwd: 'C:/x', started_at_unix: 1761990000, running: false, steerable: false, total_bytes: 3000000, dropped_bytes: 10, exit_code: 0, success: true }
const JOB_KILLED = { id: 3, command: 'tail -f big.log', cwd: 'C:/x', started_at_unix: 0, running: false, steerable: false, total_bytes: 512, dropped_bytes: 0, killed: true }

function jobsImpl(jobs: any[]) {
  return (_m: string, cmd: string) => cmd === 'list' ? Promise.resolve({ jobs }) : Promise.resolve({})
}

beforeEach(() => {
  requestMock.mockReset()
  requestMock.mockImplementation(jobsImpl([JOB_DONE, JOB_RUN, JOB_KILLED]))
  queryMock = {}
  replaceMock.mockReset()
  useToast().toasts.splice(0)
  vi.stubGlobal('confirm', vi.fn(() => true))
})

afterEach(() => {
  vi.useRealTimers()
  vi.unstubAllGlobals()
})

describe('ProcessesView 列表', () => {
  it('加载并按 id 倒序渲染；三态徽标与字节格式化', async () => {
    const w = mount(ProcessesView)
    await flushPromises()
    const vm = w.vm as any
    expect(vm.jobs.map((j: any) => j.id)).toEqual([3, 2, 1])
    expect(w.text()).toContain('python worker.py --loop')
    expect(w.text()).toContain('1 个运行中')
    expect(w.text()).toContain('2.0KB')
    expect(w.text()).toContain('2.86MB')
    expect(vm.statusInfo(JOB_RUN).label).toBe('● 运行中')
    expect(vm.statusInfo(JOB_DONE).label).toBe('✓ 退出 0')
    expect(vm.statusInfo(JOB_KILLED).label).toBe('⛔ 已停止')
    expect(vm.statusInfo({ ...JOB_DONE, exit_code: undefined }).label).toBe('已退出')
    expect(vm.statusInfo({ ...JOB_DONE, exit_code: 2 }).label).toBe('✗ 退出 2')
    expect(vm.statusInfo({ stale: true } as any).label).toBe('已过期')
  })

  it('空列表与 WS 失败静默', async () => {
    requestMock.mockImplementation(jobsImpl([]))
    const w = mount(ProcessesView)
    await flushPromises()
    expect(w.text()).toContain('暂无后台任务')

    requestMock.mockImplementation((_m, cmd) => cmd === 'list' ? Promise.reject(new Error('ws down')) : Promise.resolve({}))
    const w2 = mount(ProcessesView)
    await flushPromises()
    expect((w2.vm as any).loading).toBe(false)
  })

  it('轮询：有运行行 2s 拍；全终态 10s 慢拍；卸载后停止', async () => {
    vi.useFakeTimers()
    const count = () => requestMock.mock.calls.filter(c => c[1] === 'list').length
    // A. 有运行行 → 2s 节拍
    let w = mount(ProcessesView)
    await vi.advanceTimersByTimeAsync(1)
    let n = count()
    vi.advanceTimersByTime(2000)
    await vi.advanceTimersByTimeAsync(1)
    expect(count()).toBe(n + 1)
    vi.advanceTimersByTime(2000)
    await vi.advanceTimersByTimeAsync(1)
    expect(count()).toBe(n + 2)
    w.unmount()
    const frozen = count()
    vi.advanceTimersByTime(30000)
    await vi.advanceTimersByTimeAsync(1)
    expect(count()).toBe(frozen)
    // B. 全终态 → 10s 慢拍
    requestMock.mockImplementation(jobsImpl([JOB_DONE]))
    w = mount(ProcessesView)
    await vi.advanceTimersByTimeAsync(1)
    n = count()
    vi.advanceTimersByTime(2000)
    await vi.advanceTimersByTimeAsync(1)
    expect(count()).toBe(n)
    vi.advanceTimersByTime(8000)
    await vi.advanceTimersByTimeAsync(1)
    expect(count()).toBe(n + 1)
    w.unmount()
  })
})

describe('ProcessesView 详情', () => {
  it('点击行打开详情；未知 id 开 stale 占位', async () => {
    const w = mount(ProcessesView)
    await flushPromises()
    const vm = w.vm as any
    vm.openDetail(2)
    expect(vm.detail.id).toBe(2)
    expect(vm.detail.running).toBe(true)
    vm.openDetail('99')
    expect(vm.detail.stale).toBe(true)
    expect(vm.detail.command).toBe('—')
    vm.closeDetail()
    expect(vm.detail).toBe(null)
  })

  it('列表刷新推进详情字段；行被逐出 → 详情关闭并提示', async () => {
    const w = mount(ProcessesView)
    await flushPromises()
    const vm = w.vm as any
    vm.openDetail(2)
    // 下一轮轮询：行字段变化 → 详情同步
    requestMock.mockImplementation(jobsImpl([{ ...JOB_RUN, total_bytes: 4096 }]))
    await vm.loadList()
    expect(vm.detail.total_bytes).toBe(4096)
    // 行被逐出 → 详情诚实关闭
    requestMock.mockImplementation(jobsImpl([]))
    await vm.loadList()
    expect(vm.detail).toBe(null)
    expect(useToast().toasts.some(t => t.message.includes('逐出'))).toBe(true)
  })

  it('stale 占位详情不被列表同步触碰', async () => {
    const w = mount(ProcessesView)
    await flushPromises()
    const vm = w.vm as any
    vm.openDetail('99')
    await vm.loadList()
    expect(vm.detail?.stale).toBe(true)
  })
})

describe('ProcessesView 停止', () => {
  it('confirm 拦截不发请求；成功刷列表并在详情匹配时 300ms 后再刷', async () => {
    vi.useFakeTimers()
    vi.stubGlobal('confirm', vi.fn(() => false))
    const w = mount(ProcessesView)
    await vi.advanceTimersByTimeAsync(1)
    await (w.vm as any).killJob(JOB_RUN)
    expect(requestMock.mock.calls.filter(c => c[1] === 'kill').length).toBe(0)

    vi.stubGlobal('confirm', vi.fn(() => true))
    ;(w.vm as any).openDetail(2)
    await (w.vm as any).killJob(JOB_RUN)
    expect(requestMock).toHaveBeenCalledWith('background', 'kill', { id: 2 })
    expect(useToast().toasts.some(t => t.message === '已发送停止信号')).toBe(true)
    const before = requestMock.mock.calls.filter(c => c[1] === 'list').length
    await vi.advanceTimersByTimeAsync(300)
    expect(requestMock.mock.calls.filter(c => c[1] === 'list').length).toBe(before + 1)
  })

  it('停止失败提示错误', async () => {
    requestMock.mockImplementation((_m, cmd) => cmd === 'kill' ? Promise.reject(new Error('no perms')) : jobsImpl([])(_m, cmd))
    const w = mount(ProcessesView)
    await flushPromises()
    await (w.vm as any).killJob(JOB_RUN)
    expect(useToast().toasts.some(t => t.message.includes('停止失败'))).toBe(true)
  })
})

describe('ProcessesView 深链', () => {
  it('focus query 自动开详情并剥离参数；落空开 stale 占位', async () => {
    queryMock = { focus: '2' }
    const w = mount(ProcessesView)
    await flushPromises()
    expect((w.vm as any).detail?.id).toBe(2)
    expect(replaceMock).toHaveBeenCalledWith({ query: {} })

    queryMock = { focus: '77' }
    const w2 = mount(ProcessesView)
    await flushPromises()
    expect((w2.vm as any).detail?.stale).toBe(true)
    expect(replaceMock).toHaveBeenCalled()
  })

  it('无 focus query 不动作', async () => {
    const w = mount(ProcessesView)
    await flushPromises()
    expect((w.vm as any).detail).toBe(null)
    expect(replaceMock).not.toHaveBeenCalled()
  })
})
