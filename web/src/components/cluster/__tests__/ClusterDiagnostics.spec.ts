// ClusterDiagnostics：诊断面板契约（节点侧栏/单项与全部诊断/结果分区渲染/
// 超时与错误态/切换节点丢弃在途结果/远程命令提交-轮询-完成全链路）。
// 后端行为由 handlers/cluster.rs（diagnostics.run/nodes.ping/tasks.submit/
// tasks.detail）钉住；本 spec 只钉前端编排。30s 超时与 2s 轮询走 fake timers。
import { mount, flushPromises } from '@vue/test-utils'
import type { VueWrapper } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { useToast } from '../../../composables/useToast'

const requestMock = vi.fn()
vi.mock('../../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
}))

import ClusterDiagnostics from '../ClusterDiagnostics.vue'

// 无名节点 id 取前 8 位作简称：'betanode-9999-long'.slice(0, 8) === 'betanode'
const NODES = [
  { id: 'node-alpha-1', name: 'Alpha', online: true, isLocal: true },
  { id: 'betanode-9999-long', online: false },
]

// system 按调用次序吐不同 payload，钉住 formatBytes/formatUptime 各分支。
let systemPayloads: any[] = []
// tasks.detail 轮询队列：每次 poll 取一个工厂函数，空则默认 completed。
let detailQueue: Array<() => Promise<any>> = []

function defaultImpl(_m: string, cmd: string, p?: any) {
  switch (cmd) {
    case 'nodes.list': return Promise.resolve({ nodes: NODES })
    case 'nodes.ping': return Promise.resolve({ latency: 42 })
    case 'diagnostics.run':
      switch (p?.action) {
        case 'get_info':
          return Promise.resolve({
            name: 'Alpha', role: 'worker', category: 'development', status: 'online',
            addresses: ['10.0.0.1', '10.0.0.2'], capabilities: ['ssh', 'build'],
          })
        case 'diagnostics.system': {
          const s = systemPayloads.shift() ?? {
            os_version: 'Linux 6.1', arch: 'x86_64', hostname: 'alpha-host',
            memory_used_bytes: 1024, memory_total_bytes: 2048, uptime_secs: 90061,
          }
          return Promise.resolve(s)
        }
        case 'diagnostics.network':
          return Promise.resolve({
            interfaces: [{ ip: '10.0.0.1', mask: '255.255.255.0', network_ip: '10.0.0.0' }],
            all_ips: ['10.0.0.1', '10.0.0.2'],
          })
        case 'diagnostics.cluster_state':
          return Promise.resolve({
            online_count: 1, node_count: 2,
            nodes: [{ id: 'node-alpha-1', name: 'Alpha', address: '10.0.0.1:21950', role: 'worker', last_seen: '3s ago' }],
          })
        default:
          return Promise.resolve({})
      }
    case 'tasks.submit': return Promise.resolve({ task_id: 'task-1' })
    case 'tasks.detail': {
      const f = detailQueue.shift()
      return f ? f() : Promise.resolve({ status: 'completed', result: { response: 'hello world' } })
    }
    default: return Promise.resolve({})
  }
}

beforeEach(() => {
  requestMock.mockReset()
  systemPayloads = []
  detailQueue = []
  requestMock.mockImplementation(defaultImpl)
  useToast().toasts.splice(0)
})

afterEach(() => {
  vi.unstubAllGlobals()
  vi.useRealTimers()
})

async function mountDiag(impl?: (_m: string, cmd: string, p?: any) => Promise<any>) {
  if (impl) requestMock.mockImplementation(impl)
  const w = mount(ClusterDiagnostics)
  await flushPromises()
  return w as VueWrapper<any>
}

async function selectNode(w: VueWrapper<any>, name: string) {
  await w.findAll('.node-item').find(b => b.text().includes(name))!.trigger('click')
  await flushPromises()
}

function diagBtn(w: VueWrapper<any>, text: string) {
  return w.findAll('button').find(b => b.text() === text)!
}

function sectionByTitle(w: VueWrapper<any>, title: string) {
  return w.findAll('.result-section').find(s => s.find('.result-title').text() === title)!
}

function hasToast(msg: string) {
  return useToast().toasts.some(t => t.message.includes(msg))
}

describe('ClusterDiagnostics 节点侧栏与选择', () => {
  it('渲染节点列表：简称/本机/离线样式；未选节点时按钮禁用', async () => {
    const w = await mountDiag()
    expect(w.findAll('.node-item').length).toBe(2)
    expect(w.text()).toContain('Alpha')
    // 无名节点 → id 前 8 位作简称
    expect(w.text()).toContain('betanode')
    expect(w.text()).toContain('本机')
    expect(w.findAll('.node-item').find(b => b.text().includes('betanode'))!.classes()).toContain('offline')
    // 未选择：全部诊断按钮禁用；选中后可用
    expect(diagBtn(w, '全部诊断').attributes('disabled')).toBeDefined()
    await selectNode(w, 'Alpha')
    expect(diagBtn(w, '全部诊断').attributes('disabled')).toBeUndefined()
    expect(diagBtn(w, 'Ping').attributes('disabled')).toBeUndefined()
    // 面板副标题
    expect(w.text()).toContain('目标: Alpha')
    expect(w.text()).toContain('→ Alpha')
    w.unmount()
  })

  it('空节点列表显示空态', async () => {
    const w = await mountDiag((_m, cmd) =>
      cmd === 'nodes.list' ? Promise.resolve({ nodes: [] }) : Promise.resolve({}))
    expect(w.text()).toContain('暂无节点')
    w.unmount()
  })

  it('nodes.list 失败静默', async () => {
    const w = await mountDiag((_m, cmd) =>
      cmd === 'nodes.list' ? Promise.reject('down') : Promise.resolve({}))
    expect(w.text()).toContain('暂无节点')
    w.unmount()
  })

  it('切换节点清空旧结果（watch 重置）', async () => {
    vi.useFakeTimers()
    const w = await mountDiag()
    await selectNode(w, 'Alpha')
    await diagBtn(w, 'Ping').trigger('click')
    await flushPromises()
    expect(w.find('.result-section').exists()).toBe(true)
    await selectNode(w, 'betanode')
    expect(w.find('.result-section').exists()).toBe(false)
    expect(w.text()).toContain('点击上方按钮开始诊断')
    w.unmount()
  })
})

describe('ClusterDiagnostics 诊断成功渲染', () => {
  it('基本信息：kv 行 + addresses 拼接 + capabilities 拼接', async () => {
    vi.useFakeTimers()
    const w = await mountDiag()
    await selectNode(w, 'Alpha')
    await diagBtn(w, '基本信息').trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('cluster', 'diagnostics.run', { node_id: 'node-alpha-1', action: 'get_info' })
    const sec = sectionByTitle(w, '基本信息')
    expect(sec.text()).toContain('name')
    expect(sec.text()).toContain('Alpha')
    expect(sec.text()).toContain('10.0.0.1, 10.0.0.2')
    expect(sec.text()).toContain('ssh, build')
    expect(diagBtn(w, '基本信息').classes()).toContain('state-success')
    w.unmount()
  })

  it('系统信息：formatBytes/formatUptime 各分支', async () => {
    vi.useFakeTimers()
    systemPayloads = [
      { os_version: 'Linux 6.1', arch: 'x86_64', hostname: 'alpha-host', memory_used_bytes: 1024, memory_total_bytes: 2048, uptime_secs: 90061 },
      { os_version: 'L', arch: 'a', hostname: 'h', memory_used_bytes: 0, memory_total_bytes: 0, uptime_secs: 3600 },
      { os_version: 'L', arch: 'a', hostname: 'h', memory_used_bytes: 1048576 * 5, memory_total_bytes: 1048576 * 10, uptime_secs: 60 },
    ]
    const w = await mountDiag()
    await selectNode(w, 'Alpha')
    await diagBtn(w, '系统信息').trigger('click')
    await flushPromises()
    expect(w.text()).toContain('Linux 6.1')
    expect(w.text()).toContain('x86_64')
    expect(w.text()).toContain('alpha-host')
    expect(w.text()).toContain('1.0 KB')
    expect(w.text()).toContain('2.0 KB')
    expect(w.text()).toContain('1d 1h 1m')

    await diagBtn(w, '系统信息').trigger('click')
    await flushPromises()
    expect(w.text()).toContain('0 B')
    expect(w.text()).toContain('1h 0m')

    await diagBtn(w, '系统信息').trigger('click')
    await flushPromises()
    expect(w.text()).toContain('5.0 MB')
    expect(w.text()).toContain('10.0 MB')
    expect(w.text()).toContain('1m')
    w.unmount()
  })

  it('网络接口：接口表 + 所有 IP', async () => {
    vi.useFakeTimers()
    const w = await mountDiag()
    await selectNode(w, 'Alpha')
    await diagBtn(w, '网络接口').trigger('click')
    await flushPromises()
    const sec = sectionByTitle(w, '网络接口')
    expect(sec.find('table').exists()).toBe(true)
    expect(sec.text()).toContain('10.0.0.1')
    expect(sec.text()).toContain('255.255.255.0')
    expect(sec.text()).toContain('10.0.0.0')
    expect(sec.text()).toContain('所有 IP')
    expect(sec.text()).toContain('10.0.0.1, 10.0.0.2')
    w.unmount()
  })

  it('集群视角：在线徽标 + 节点表', async () => {
    vi.useFakeTimers()
    const w = await mountDiag()
    await selectNode(w, 'Alpha')
    await diagBtn(w, '集群视角').trigger('click')
    await flushPromises()
    const sec = sectionByTitle(w, '集群视角')
    expect(sec.text()).toContain('1/2')
    expect(sec.text()).toContain('Alpha')
    expect(sec.text()).toContain('10.0.0.1:21950')
    expect(sec.text()).toContain('worker')
    expect(sec.text()).toContain('3s ago')
    w.unmount()
  })

  it('Ping：latency 徽标 + kv', async () => {
    vi.useFakeTimers()
    const w = await mountDiag()
    await selectNode(w, 'Alpha')
    await diagBtn(w, 'Ping').trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('cluster', 'nodes.ping', { node_id: 'node-alpha-1' })
    const sec = sectionByTitle(w, 'Ping')
    expect(sec.text()).toContain('42ms')
    expect(sec.find('.badge-success').exists()).toBe(true)
    expect(diagBtn(w, 'Ping').classes()).toContain('state-success')
    w.unmount()
  })

  it('全部诊断：依次跑 4 项 + Ping，完成后 toast', async () => {
    vi.useFakeTimers()
    const w = await mountDiag()
    await selectNode(w, 'Alpha')
    await diagBtn(w, '全部诊断').trigger('click')
    await flushPromises()
    expect(requestMock.mock.calls.filter(c => c[1] === 'diagnostics.run').length).toBe(4)
    expect(requestMock.mock.calls.filter(c => c[1] === 'nodes.ping').length).toBe(1)
    expect(useToast().toasts.some(t => t.message === '全部诊断完成')).toBe(true)
    expect(w.findAll('.result-section').length).toBe(5)
    w.unmount()
  })

  it('空数据兜底：addresses/capabilities 缺省、无接口表、无在线节点', async () => {
    vi.useFakeTimers()
    const w = await mountDiag((_m, cmd, p) => {
      if (cmd === 'diagnostics.run' && p?.action === 'get_info') return Promise.resolve({ name: 'Alpha' })
      if (cmd === 'diagnostics.run' && p?.action === 'diagnostics.network') return Promise.resolve({ interfaces: [], all_ips: [] })
      if (cmd === 'diagnostics.run' && p?.action === 'diagnostics.cluster_state') return Promise.resolve({ online_count: 0, node_count: 2, nodes: [] })
      return defaultImpl(_m, cmd, p)
    })
    await selectNode(w, 'Alpha')
    await diagBtn(w, '基本信息').trigger('click')
    await flushPromises()
    expect(sectionByTitle(w, '基本信息').text()).toContain('(none)')

    await diagBtn(w, '网络接口').trigger('click')
    await flushPromises()
    expect(sectionByTitle(w, '网络接口').find('table').exists()).toBe(false)

    await diagBtn(w, '集群视角').trigger('click')
    await flushPromises()
    expect(sectionByTitle(w, '集群视角').text()).toContain('无在线节点')
    w.unmount()
  })

  it('分区头点击收起/展开（toggleSection）', async () => {
    vi.useFakeTimers()
    const w = await mountDiag()
    await selectNode(w, 'Alpha')
    await diagBtn(w, 'Ping').trigger('click')
    await flushPromises()
    const header = sectionByTitle(w, 'Ping').find('.result-header')
    await header.trigger('click')
    expect(sectionByTitle(w, 'Ping').find('.kv-grid').exists()).toBe(false)
    await header.trigger('click')
    expect(sectionByTitle(w, 'Ping').find('.kv-grid').exists()).toBe(true)
    w.unmount()
  })
})

describe('ClusterDiagnostics 失败/超时/切换丢弃', () => {
  it('单项诊断失败：错误文案 + state-error + 失败徽标', async () => {
    vi.useFakeTimers()
    const w = await mountDiag((_m, cmd, p) =>
      (cmd === 'diagnostics.run' && p?.action === 'get_info') ? Promise.reject('boom') : defaultImpl(_m, cmd, p))
    await selectNode(w, 'Alpha')
    await diagBtn(w, '基本信息').trigger('click')
    await flushPromises()
    expect(sectionByTitle(w, '基本信息').text()).toContain('boom')
    expect(diagBtn(w, '基本信息').classes()).toContain('state-error')
    expect(w.find('.badge-error').exists()).toBe(true)
    w.unmount()
  })

  it('Ping 失败：错误文案 + state-error', async () => {
    vi.useFakeTimers()
    const w = await mountDiag((_m, cmd) =>
      cmd === 'nodes.ping' ? Promise.reject('unreachable') : defaultImpl(_m, cmd))
    await selectNode(w, 'Alpha')
    await diagBtn(w, 'Ping').trigger('click')
    await flushPromises()
    expect(sectionByTitle(w, 'Ping').text()).toContain('unreachable')
    expect(diagBtn(w, 'Ping').classes()).toContain('state-error')
    w.unmount()
  })

  it('网络接口/集群视角失败：错误文案渲染', async () => {
    vi.useFakeTimers()
    const w = await mountDiag((_m, cmd, p) => {
      if (cmd === 'diagnostics.run' && (p?.action === 'diagnostics.network' || p?.action === 'diagnostics.cluster_state')) {
        return Promise.reject('probe-fail')
      }
      return defaultImpl(_m, cmd, p)
    })
    await selectNode(w, 'Alpha')
    await diagBtn(w, '网络接口').trigger('click')
    await flushPromises()
    const netSec = sectionByTitle(w, '网络接口')
    expect(netSec.text()).toContain('probe-fail')
    expect(netSec.find('.badge-error').exists()).toBe(true)

    await diagBtn(w, '集群视角').trigger('click')
    await flushPromises()
    const csSec = sectionByTitle(w, '集群视角')
    expect(csSec.text()).toContain('probe-fail')
    expect(csSec.find('.badge-error').exists()).toBe(true)
    w.unmount()
  })

  it('Ping 30s 超时：state-timeout + __TIMEOUT__ 文案，随后按钮恢复', async () => {
    vi.useFakeTimers()
    const w = await mountDiag((_m, cmd) =>
      cmd === 'nodes.ping' ? new Promise(() => {}) : defaultImpl(_m, cmd))
    await selectNode(w, 'Alpha')
    await diagBtn(w, 'Ping').trigger('click')
    await flushPromises()
    expect(diagBtn(w, 'Ping').classes()).toContain('state-running')
    expect(diagBtn(w, 'Ping').attributes('disabled')).toBeDefined()

    await vi.advanceTimersByTimeAsync(30000)
    await flushPromises()
    expect(diagBtn(w, 'Ping').classes()).toContain('state-timeout')
    expect(w.text()).toContain('__TIMEOUT__')
    // runningDiag 已清空 → 按钮恢复可用
    expect(diagBtn(w, 'Ping').attributes('disabled')).toBeUndefined()
    w.unmount()
  })

  it('在途结果丢弃（成功路径）：切换节点后 resolve 不写入新节点视图', async () => {
    vi.useFakeTimers()
    let resolvePing: (v: any) => void = () => {}
    const w = await mountDiag((_m, cmd) =>
      cmd === 'nodes.ping' ? new Promise(res => { resolvePing = res }) : defaultImpl(_m, cmd))
    await selectNode(w, 'Alpha')
    await diagBtn(w, 'Ping').trigger('click')
    await flushPromises()
    await selectNode(w, 'betanode')
    resolvePing({ latency: 5 })
    await flushPromises()
    expect(w.find('.result-section').exists()).toBe(false)
    // watch 已清 runningDiag → 按钮可用
    expect(diagBtn(w, 'Ping').attributes('disabled')).toBeUndefined()
    w.unmount()
  })

  it('在途结果丢弃（失败路径）：切换节点后 reject 同样不写入', async () => {
    vi.useFakeTimers()
    let rejectPing: (e: any) => void = () => {}
    const w = await mountDiag((_m, cmd) =>
      cmd === 'nodes.ping' ? new Promise((_res, rej) => { rejectPing = rej }) : defaultImpl(_m, cmd))
    await selectNode(w, 'Alpha')
    await diagBtn(w, 'Ping').trigger('click')
    await flushPromises()
    await selectNode(w, 'betanode')
    rejectPing(new Error('late-fail'))
    await flushPromises()
    expect(w.find('.result-section').exists()).toBe(false)
    w.unmount()
  })

  it('未选节点调用各诊断函数直接返回（守卫分支）', async () => {
    const w = await mountDiag()
    const vm = w.vm as any
    await vm.runDiagnostic('get_info')
    await vm.runPing()
    await vm.runAllDiagnostics()
    await vm.sendCommand()
    expect(requestMock.mock.calls.filter(c =>
      c[1] === 'diagnostics.run' || c[1] === 'nodes.ping' || c[1] === 'tasks.submit').length).toBe(0)
    w.unmount()
  })

  it('nodeShortName 辅助分支：null → 空、短 id 原样', async () => {
    const w = await mountDiag()
    const vm = w.vm as any
    expect(vm.nodeShortName(null)).toBe('')
    expect(vm.nodeShortName({ id: 'shortid7' })).toBe('shortid7')
    w.unmount()
  })
})

describe('ClusterDiagnostics 远程命令', () => {
  it('提交 → 轮询 running → 完成：最终输出 + 状态恢复', async () => {
    vi.useFakeTimers()
    detailQueue = [() => Promise.resolve({ status: 'running' })]
    const w = await mountDiag()
    await selectNode(w, 'Alpha')
    await w.find('input.form-input').setValue('ls /')
    await diagBtn(w, '发送').trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('cluster', 'tasks.submit', { target_node_id: 'node-alpha-1', content: 'ls /' })
    expect(w.text()).toContain('任务已提交 (task-1)')
    // 轮询中输入禁用
    expect(w.find('input.form-input').attributes('disabled')).toBeDefined()

    await vi.advanceTimersByTimeAsync(2000)
    await flushPromises()
    expect(w.find('.chat-output pre').text()).toContain('等待中... (2s)')

    await vi.advanceTimersByTimeAsync(2000)
    await flushPromises()
    expect(w.find('.chat-output pre').text()).toContain('Alpha > hello world')
    // 完成后输入恢复
    expect(w.find('input.form-input').attributes('disabled')).toBeUndefined()
    expect(diagBtn(w, '发送')).toBeDefined()
    w.unmount()
  })

  it('轮询 failed：字符串与对象 result 两种文案；completed 纯字符串 result', async () => {
    vi.useFakeTimers()
    const w = await mountDiag()
    await selectNode(w, 'Alpha')

    detailQueue = [() => Promise.resolve({ status: 'failed', result: 'boom-string' })]
    await w.find('input.form-input').setValue('cmd1')
    await diagBtn(w, '发送').trigger('click')
    await vi.advanceTimersByTimeAsync(2000)
    await flushPromises()
    expect(w.find('.chat-output pre').text()).toContain('Alpha > [失败] boom-string')

    detailQueue = [() => Promise.resolve({ status: 'failed', result: { error: 'obj-err' } })]
    await w.find('input.form-input').setValue('cmd2')
    await diagBtn(w, '发送').trigger('click')
    await vi.advanceTimersByTimeAsync(2000)
    await flushPromises()
    expect(w.find('.chat-output pre').text()).toContain('Alpha > [失败] obj-err')

    detailQueue = [() => Promise.resolve({ status: 'completed', result: 'plain-out' })]
    await w.find('input.form-input').setValue('cmd3')
    await diagBtn(w, '发送').trigger('click')
    await vi.advanceTimersByTimeAsync(2000)
    await flushPromises()
    expect(w.find('.chat-output pre').text()).toContain('Alpha > plain-out')
    w.unmount()
  })

  it('120s 全轮询未完成 → 超时提示', async () => {
    vi.useFakeTimers()
    const w = await mountDiag((_m, cmd) =>
      cmd === 'tasks.detail' ? Promise.resolve({ status: 'running', result: null }) : defaultImpl(_m, cmd))
    await selectNode(w, 'Alpha')
    await w.find('input.form-input').setValue('long task')
    await diagBtn(w, '发送').trigger('click')
    await flushPromises()
    await vi.advanceTimersByTimeAsync(120000)
    await flushPromises()
    expect(w.find('.chat-output pre').text()).toContain('任务超时（120s）')
    expect(w.find('input.form-input').attributes('disabled')).toBeUndefined()
    w.unmount()
  })

  it('轮询单次失败被吞掉，下一轮继续直至完成', async () => {
    vi.useFakeTimers()
    detailQueue = [
      () => Promise.reject('poll-net'),
      () => Promise.resolve({ status: 'completed', result: { response: 'done-after-err' } }),
    ]
    const w = await mountDiag()
    await selectNode(w, 'Alpha')
    await w.find('input.form-input').setValue('cmd')
    await diagBtn(w, '发送').trigger('click')
    await flushPromises()
    await vi.advanceTimersByTimeAsync(2000)
    await flushPromises()
    await vi.advanceTimersByTimeAsync(2000)
    await flushPromises()
    expect(w.find('.chat-output pre').text()).toContain('Alpha > done-after-err')
    w.unmount()
  })

  it('提交失败：错误文案 + 状态解除', async () => {
    vi.useFakeTimers()
    const w = await mountDiag((_m, cmd) =>
      cmd === 'tasks.submit' ? Promise.reject('refused') : defaultImpl(_m, cmd))
    await selectNode(w, 'Alpha')
    await w.find('input.form-input').setValue('cmd')
    await diagBtn(w, '发送').trigger('click')
    await flushPromises()
    expect(w.find('.chat-output pre').text()).toContain('错误: refused')
    expect(w.find('input.form-input').attributes('disabled')).toBeUndefined()
    w.unmount()
  })

  it('提交在途切换节点：submit 结果被丢弃', async () => {
    let resolveSubmit: (v: any) => void = () => {}
    const w = await mountDiag((_m, cmd) =>
      cmd === 'tasks.submit' ? new Promise(res => { resolveSubmit = res }) : defaultImpl(_m, cmd))
    await selectNode(w, 'Alpha')
    await w.find('input.form-input').setValue('cmd')
    await diagBtn(w, '发送').trigger('click')
    await flushPromises()
    await selectNode(w, 'betanode')
    resolveSubmit({ task_id: 't-9' })
    await flushPromises()
    expect(w.text()).not.toContain('任务已提交')
    expect(w.find('.chat-output').exists()).toBe(false)
    w.unmount()
  })

  it('轮询在途切换节点：poll 结果被丢弃（line 180 守卫）', async () => {
    vi.useFakeTimers()
    let resolveDetail: (v: any) => void = () => {}
    const w = await mountDiag((_m, cmd) =>
      cmd === 'tasks.detail' ? new Promise(res => { resolveDetail = res }) : defaultImpl(_m, cmd))
    await selectNode(w, 'Alpha')
    await w.find('input.form-input').setValue('cmd')
    await diagBtn(w, '发送').trigger('click')
    await flushPromises()
    await vi.advanceTimersByTimeAsync(2000)
    await flushPromises()
    // 首轮 poll 已发出（pending），切换节点后 resolve
    await selectNode(w, 'betanode')
    resolveDetail({ status: 'completed', result: { response: 'late' } })
    await flushPromises()
    expect(w.find('.chat-output').exists()).toBe(false)
    expect(w.text()).not.toContain('late')
    w.unmount()
  })

  it('输入框 Enter 键触发发送', async () => {
    vi.useFakeTimers()
    const w = await mountDiag()
    await selectNode(w, 'Alpha')
    await w.find('input.form-input').setValue('echo hi')
    await w.find('input.form-input').trigger('keydown.enter')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('cluster', 'tasks.submit', { target_node_id: 'node-alpha-1', content: 'echo hi' })
    // 默认轮询（completed）收尾
    await vi.advanceTimersByTimeAsync(2000)
    await flushPromises()
    expect(w.find('.chat-output pre').text()).toContain('Alpha > hello world')
    w.unmount()
  })

  it('空输入 + 已选节点：sendCommand 守卫不发请求', async () => {
    const w = await mountDiag()
    await selectNode(w, 'Alpha')
    await (w.vm as any).sendCommand()
    expect(requestMock.mock.calls.filter(c => c[1] === 'tasks.submit').length).toBe(0)
    w.unmount()
  })
})

describe('ClusterDiagnostics 面板折叠', () => {
  it('诊断/远程命令面板头点击折叠与展开', async () => {
    const w = await mountDiag()
    const panels = w.findAll('section.panel')
    expect(panels.length).toBe(2)
    await panels[0].find('.panel-header').trigger('click')
    expect(panels[0].classes()).toContain('collapsed')
    await panels[0].find('.panel-header').trigger('click')
    expect(panels[0].classes()).not.toContain('collapsed')
    await panels[1].find('.panel-header').trigger('click')
    expect(panels[1].classes()).toContain('collapsed')
    // 折叠后面板体 v-show 隐藏
    expect(panels[1].find('.panel-body').isVisible()).toBe(false)
    w.unmount()
  })
})
