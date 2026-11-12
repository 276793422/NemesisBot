import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { useToast } from '../../../composables/useToast'

const requestMock = vi.fn()
vi.mock('../../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
  initWSAPI: vi.fn(),
}))
vi.mock('../../../composables/useSSE', () => ({ on: vi.fn(), off: vi.fn() }))
vi.mock('../TaskRow.vue', () => ({
  default: { template: '<div class="stub-taskrow">{{ task?.title }}</div>', props: ['task'] },
}))

import ClusterTasks from '../ClusterTasks.vue'

const TASKS = [
  { id: 't1', status: 'running', title: '任务一' },
  { id: 't2', status: 'queued', title: '任务二' },
  { id: 't3', status: 'failed', title: '任务三' },
]

function defaultImpl(_m: string, cmd: string, p?: any) {
  switch (cmd) {
    case 'tasks.list':
      return Promise.resolve({ tasks: TASKS, stats: { queued: 1, running: 1, completed: 5, failed: 1 } })
    case 'nodes.list':
      return Promise.resolve({ nodes: [{ id: 'n1', name: 'alpha' }] })
    default:
      return Promise.resolve({})
  }
}

beforeEach(() => {
  requestMock.mockReset()
  requestMock.mockImplementation(defaultImpl)
  useToast().toasts.splice(0)
})

afterEach(() => {
  vi.useRealTimers()
})

async function mountView() {
  const w = mount(ClusterTasks)
  await flushPromises()
  return w
}

describe('ClusterTasks 加载与筛选', () => {
  it('挂载拉任务列表与统计', async () => {
    const w = await mountView()
    expect(requestMock).toHaveBeenCalledWith('cluster', 'tasks.list', { status_filter: undefined, offset: 0, limit: 20 })
    expect(w.text()).toContain('任务一')
    expect(w.text()).toContain('排队')
  })

  it('状态筛选走 status_filter 重新拉取', async () => {
    const w = await mountView()
    ;(w.vm as any).filterStatus = 'running'
    await (w.vm as any).loadTasks()
    expect(requestMock).toHaveBeenCalledWith('cluster', 'tasks.list', { status_filter: 'running', offset: 0, limit: 20 })
    // 前端过滤 computed
    expect((w.vm as any).filtered.every((t: any) => t.status === 'running')).toBe(true)
  })

  it('加载更多：append 追加且 offset 推进', async () => {
    const w = await mountView()
    const vm = w.vm as any
    expect(vm.currentOffset).toBe(3)
    await vm.loadMoreTasks()
    expect(requestMock).toHaveBeenCalledWith('cluster', 'tasks.list', { status_filter: undefined, offset: 3, limit: 20 })
    expect(vm.tasks.length).toBe(6)
  })

  it('任务列表失败静默（backend not ready）', async () => {
    requestMock.mockImplementation((_m, cmd) => cmd === 'tasks.list' ? Promise.reject(new Error('x')) : Promise.resolve({}))
    const w = await mountView()
    expect((w.vm as any).loading).toBe(false)
    expect(w.text()).not.toContain('任务一')
  })

  it('5s 轮询自动刷新；卸载清理', async () => {
    vi.useFakeTimers()
    const w = mount(ClusterTasks)
    await vi.advanceTimersByTimeAsync(1)
    const n = requestMock.mock.calls.filter(c => c[1] === 'tasks.list').length
    vi.advanceTimersByTime(5000)
    await vi.advanceTimersByTimeAsync(1)
    expect(requestMock.mock.calls.filter(c => c[1] === 'tasks.list').length).toBe(n + 1)
    w.unmount()
    const frozen = requestMock.mock.calls.filter(c => c[1] === 'tasks.list').length
    vi.advanceTimersByTime(20000)
    await vi.advanceTimersByTimeAsync(1)
    expect(requestMock.mock.calls.filter(c => c[1] === 'tasks.list').length).toBe(frozen)
  })
})

describe('ClusterTasks 取消与提交', () => {
  it('取消任务成功刷新；失败提示', async () => {
    const w = await mountView()
    const calls = requestMock.mock.calls.filter(c => c[1] === 'tasks.list').length
    await (w.vm as any).cancelTask('t1')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('cluster', 'tasks.cancel', { task_id: 't1' })
    expect(useToast().toasts.some(t => t.message === '任务已取消')).toBe(true)
    expect(requestMock.mock.calls.filter(c => c[1] === 'tasks.list').length).toBe(calls + 1)

    requestMock.mockImplementation((_m, cmd) => cmd === 'tasks.cancel' ? Promise.reject('busy') : defaultImpl(_m, cmd, {}))
    await (w.vm as any).cancelTask('t2')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('取消失败'))).toBe(true)
  })

  it('提交弹窗：空内容拦截；成功关窗刷新；失败提示', async () => {
    const w = await mountView()
    const vm = w.vm as any
    await vm.openSubmitModal()
    expect(vm.showSubmitModal).toBe(true)
    expect(requestMock).toHaveBeenCalledWith('cluster', 'nodes.list')
    expect(vm.nodeList).toEqual([{ id: 'n1', name: 'alpha' }])

    vm.submitForm.content = '  '
    await vm.submitTask()
    expect(useToast().toasts.some(t => t.message === '请输入任务内容')).toBe(true)
    expect(requestMock.mock.calls.filter(c => c[1] === 'tasks.submit').length).toBe(0)

    vm.submitForm.content = ' 执行巡检 '
    vm.submitForm.target_node_id = 'n1'
    await vm.submitTask()
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('cluster', 'tasks.submit', { content: '执行巡检', target_node_id: 'n1' })
    expect(useToast().toasts.some(t => t.message === '任务已提交')).toBe(true)
    expect(vm.showSubmitModal).toBe(false)
    expect(vm.submitForm).toEqual({ content: '', target_node_id: '' })

    vm.showSubmitModal = true
    vm.submitForm.content = 'x'
    requestMock.mockImplementation((_m, cmd) => cmd === 'tasks.submit' ? Promise.reject('deny') : defaultImpl(_m, cmd, {}))
    await vm.submitTask()
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('提交失败'))).toBe(true)
    expect(vm.submitSubmitting).toBe(false)
  })
})
