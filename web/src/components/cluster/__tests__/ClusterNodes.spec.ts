// ClusterNodes：节点列表契约（过滤/展开/NodeCard 操作/添加弹窗/定时刷新）。
// 后端行为由 handlers/cluster.rs（nodes.list/add/remove/ping/refresh）钉住；
// 本 spec 只钉前端编排与 UI 反馈。NodeCard 为纯展示组件，保留真实实现以
// 钉住 emit 接线（ping/remove/refresh）。
import { mount, flushPromises } from '@vue/test-utils'
import type { VueWrapper } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { useToast } from '../../../composables/useToast'

const requestMock = vi.fn()
vi.mock('../../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
}))

import ClusterNodes from '../ClusterNodes.vue'

const NODES = [
  { id: 'node-alpha', name: 'Alpha', role: 'manager', online: true, isLocal: true, address: 'local', capabilities: ['ssh', 'build', 'dev', 'ops'], lastSeen: '刚刚', build: 'abc1234', roamingPath: 'direct' },
  { id: 'node-beta', name: 'Beta', role: 'worker', online: true, address: '10.0.0.2:7900', capabilities: ['ssh'], roamingPath: 'relay' },
  { id: 'node-gamma', name: 'Gamma', role: 'worker', online: false, address: '10.0.0.3:7900', capabilities: [], roamingPath: 'unknown' },
  { id: 'placeholder-1', name: 'Delta', role: 'worker', online: false, address: '10.0.0.4:7900' },
]

function defaultImpl(_m: string, cmd: string, _p?: any) {
  switch (cmd) {
    case 'nodes.list': return Promise.resolve({ nodes: NODES })
    case 'nodes.ping': return Promise.resolve({ latency: 12 })
    case 'nodes.refresh': return Promise.resolve({ upgraded_from_placeholder: true, canonical_id: 'node-real-1' })
    case 'nodes.remove': return Promise.resolve({})
    case 'nodes.add': return Promise.resolve({})
    default: return Promise.resolve({})
  }
}

beforeEach(() => {
  requestMock.mockReset()
  requestMock.mockImplementation(defaultImpl)
  useToast().toasts.splice(0)
})

afterEach(() => {
  vi.unstubAllGlobals()
  vi.useRealTimers()
})

async function mountNodes(impl: (_m: string, cmd: string, p?: any) => Promise<any> = defaultImpl) {
  requestMock.mockImplementation(impl)
  const w = mount(ClusterNodes)
  await flushPromises()
  return w as VueWrapper<any>
}

function rowByText(w: VueWrapper<any>, text: string) {
  return w.findAll('.node-row').find(r => r.text().includes(text))!
}

// NodeCard（展开详情）内的操作按钮——不能误中工具栏同名按钮。
function cardBtn(w: VueWrapper<any>, text: string) {
  return w.findAll('.node-detail button').find(b => b.text() === text)!
}

function hasToast(msg: string) {
  return useToast().toasts.some(t => t.message.includes(msg))
}

describe('ClusterNodes 列表渲染与过滤', () => {
  it('渲染节点行：本节点/占位 ID/路径徽标/能力截断/lastSeen 兜底', async () => {
    const w = await mountNodes()
    expect(w.findAll('.node-row').length).toBe(4)
    expect(w.text()).toContain('Alpha')
    expect(rowByText(w, 'Alpha').text()).toContain('本节点')
    expect(rowByText(w, 'Delta').text()).toContain('占位 ID')
    expect(rowByText(w, 'Alpha').text()).toContain('打洞直连')
    expect(rowByText(w, 'Beta').text()).toContain('经中继转发')
    expect(rowByText(w, 'Gamma').text()).toContain('路径协商中')
    // capabilities 只展示前 3 个
    expect(rowByText(w, 'Alpha').text()).toContain('ssh, build, dev')
    // lastSeen 缺省 → '--'
    expect(rowByText(w, 'Delta').text()).toContain('--')
    // build 徽标只在有 build 时出现
    expect(rowByText(w, 'Alpha').text()).toContain('abc1234')
    expect(rowByText(w, 'Beta').text()).not.toContain('abc1234')
    w.unmount()
  })

  it('角色/能力/状态三重过滤', async () => {
    const w = await mountNodes()
    const selects = w.findAll('select')
    expect(selects.length).toBe(3)
    await selects[0].setValue('manager')
    expect(w.findAll('.node-row').length).toBe(1)
    // 角色仍为 manager，叠加能力 build → Alpha
    await selects[1].setValue('build')
    expect(w.findAll('.node-row').length).toBe(1)
    // 清角色，能力改回 ssh → Alpha + Beta
    await selects[0].setValue('')
    await selects[1].setValue('ssh')
    expect(w.findAll('.node-row').length).toBe(2)
    // 状态 offline + ssh → 0 行，空态
    await selects[2].setValue('offline')
    expect(w.findAll('.node-row').length).toBe(0)
    expect(w.text()).toContain('暂无节点')
    w.unmount()
  })

  it('空节点列表显示空态', async () => {
    const w = await mountNodes((_m, cmd) =>
      cmd === 'nodes.list' ? Promise.resolve({ nodes: [] }) : Promise.resolve({}))
    expect(w.text()).toContain('暂无节点')
    expect(w.text()).toContain('启动集群节点后')
    w.unmount()
  })

  it('nodes.list 失败静默（loading 解除，不阻塞页面）', async () => {
    const w = await mountNodes((_m, cmd) =>
      cmd === 'nodes.list' ? Promise.reject('down') : Promise.resolve({}))
    expect(w.text()).toContain('暂无节点')
    w.unmount()
  })
})

describe('ClusterNodes 展开与 NodeCard 操作', () => {
  it('点击行展开 NodeCard 详情，再点收起', async () => {
    const w = await mountNodes()
    await rowByText(w, 'Beta').trigger('click')
    const card = w.find('.node-detail')
    expect(card.exists()).toBe(true)
    expect(card.text()).toContain('10.0.0.2:7900')
    expect(card.text()).toContain('工作节点')
    await rowByText(w, 'Beta').trigger('click')
    expect(w.find('.node-detail').exists()).toBe(false)
    w.unmount()
  })

  it('Ping：latency / OK / 失败三种反馈', async () => {
    const w = await mountNodes()
    await rowByText(w, 'Beta').trigger('click')
    await cardBtn(w, 'Ping').trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('cluster', 'nodes.ping', { node_id: 'node-beta' })
    expect(useToast().toasts.some(t => t.message === 'Ping node-beta: 12ms')).toBe(true)

    // 无 latency 字段 → OK 文案
    requestMock.mockImplementation((_m, cmd) =>
      cmd === 'nodes.ping' ? Promise.resolve({}) : defaultImpl(_m, cmd))
    await cardBtn(w, 'Ping').trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message === 'Ping node-beta: OK')).toBe(true)

    // 失败提示
    requestMock.mockImplementation((_m, cmd) =>
      cmd === 'nodes.ping' ? Promise.reject('down') : defaultImpl(_m, cmd))
    await cardBtn(w, 'Ping').trigger('click')
    await flushPromises()
    expect(hasToast('Ping 失败')).toBe(true)
    w.unmount()
  })

  it('刷新 ID：升级占位 / canonical / 均无 / 失败 四分支，成功后重拉列表', async () => {
    const w = await mountNodes()
    await rowByText(w, 'Beta').trigger('click')

    await cardBtn(w, '刷新').trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('cluster', 'nodes.refresh', { node_id: 'node-beta' })
    expect(useToast().toasts.some(t => t.message === 'ID 已升级为真实 ID: node-real-1')).toBe(true)

    requestMock.mockImplementation((_m, cmd) =>
      cmd === 'nodes.refresh' ? Promise.resolve({ canonical_id: 'node-canonical' }) : defaultImpl(_m, cmd))
    await cardBtn(w, '刷新').trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message === '已刷新: node-canonical')).toBe(true)

    requestMock.mockImplementation((_m, cmd) =>
      cmd === 'nodes.refresh' ? Promise.resolve({}) : defaultImpl(_m, cmd))
    await cardBtn(w, '刷新').trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message === '已刷新')).toBe(true)

    requestMock.mockImplementation((_m, cmd) =>
      cmd === 'nodes.refresh' ? Promise.reject('gone') : defaultImpl(_m, cmd))
    const listCalls = requestMock.mock.calls.filter(c => c[1] === 'nodes.list').length
    await cardBtn(w, '刷新').trigger('click')
    await flushPromises()
    expect(hasToast('刷新失败')).toBe(true)
    // 失败不重拉列表
    expect(requestMock.mock.calls.filter(c => c[1] === 'nodes.list').length).toBe(listCalls)
    w.unmount()
  })

  it('移除节点：成功提示并重拉列表；失败提示', async () => {
    const w = await mountNodes()
    await rowByText(w, 'Beta').trigger('click')
    await cardBtn(w, '移除').trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('cluster', 'nodes.remove', { node_id: 'node-beta' })
    expect(useToast().toasts.some(t => t.message === '节点已移除')).toBe(true)
    const listCalls = requestMock.mock.calls.filter(c => c[1] === 'nodes.list').length
    expect(listCalls).toBeGreaterThan(1)

    requestMock.mockImplementation((_m, cmd) =>
      cmd === 'nodes.remove' ? Promise.reject('busy') : defaultImpl(_m, cmd))
    await cardBtn(w, '移除').trigger('click')
    await flushPromises()
    expect(hasToast('移除失败')).toBe(true)
    w.unmount()
  })
})

describe('ClusterNodes 添加节点弹窗', () => {
  async function openModal(w: VueWrapper<any>) {
    await w.findAll('button').find(b => b.text() === '添加节点')!.trigger('click')
    expect(w.find('.modal').exists()).toBe(true)
  }

  it('空地址拦截：warn 且不发请求', async () => {
    const w = await mountNodes()
    await openModal(w)
    await w.findAll('button').find(b => b.text() === '确认添加')!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message === '请输入节点地址')).toBe(true)
    expect(requestMock.mock.calls.filter(c => c[1] === 'nodes.add').length).toBe(0)
    w.unmount()
  })

  it('完整提交：trim + payload + 提交中状态 + 成功后关弹窗并重置表单', async () => {
    const w = await mountNodes()
    await openModal(w)
    // 延迟 resolve：钉住「提交中...」瞬态
    let resolveAdd: (v: any) => void = () => {}
    requestMock.mockImplementation((_m, cmd) =>
      cmd === 'nodes.add' ? new Promise(res => { resolveAdd = res }) : defaultImpl(_m, cmd))
    const inputs = w.findAll('.modal input.form-input')
    await inputs[0].setValue(' 10.0.0.9:7900 ')
    await inputs[1].setValue(' Eta ')
    await inputs[2].setValue(' node-eta ')
    await inputs[3].setValue(' dev ')
    await w.find('.modal select').setValue('manager')
    await w.findAll('button').find(b => b.text() === '确认添加')!.trigger('click')
    // 提交中状态
    expect(w.findAll('button').find(b => b.text() === '提交中...')).toBeDefined()
    resolveAdd({})
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('cluster', 'nodes.add', {
      address: '10.0.0.9:7900',
      id: 'node-eta',
      name: 'Eta',
      role: 'manager',
      category: 'dev',
    })
    expect(useToast().toasts.some(t => t.message === '节点添加成功')).toBe(true)
    expect(w.find('.modal').exists()).toBe(false)
    // 表单已重置
    await openModal(w)
    const addr = w.findAll('.modal input.form-input')[0].element as HTMLInputElement
    expect(addr.value).toBe('')
    w.unmount()
  })

  it('留空可选字段 → payload 对应键为 undefined', async () => {
    const w = await mountNodes()
    await openModal(w)
    await w.findAll('.modal input.form-input')[0].setValue('10.0.0.10:7900')
    await w.findAll('button').find(b => b.text() === '确认添加')!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('cluster', 'nodes.add', {
      address: '10.0.0.10:7900',
      id: undefined,
      name: undefined,
      role: 'worker',
      category: undefined,
    })
    w.unmount()
  })

  it('提交失败：提示错误、弹窗保留、提交态解除', async () => {
    const w = await mountNodes()
    await openModal(w)
    requestMock.mockImplementation((_m, cmd) =>
      cmd === 'nodes.add' ? Promise.reject('taken') : defaultImpl(_m, cmd))
    await w.findAll('.modal input.form-input')[0].setValue('10.0.0.11:7900')
    await w.findAll('button').find(b => b.text() === '确认添加')!.trigger('click')
    await flushPromises()
    expect(hasToast('添加失败')).toBe(true)
    expect(w.find('.modal').exists()).toBe(true)
    expect(w.findAll('button').find(b => b.text() === '确认添加')).toBeDefined()
    w.unmount()
  })

  it('取消按钮与遮罩点击关闭弹窗', async () => {
    const w = await mountNodes()
    await openModal(w)
    await w.findAll('button').find(b => b.text() === '取消')!.trigger('click')
    expect(w.find('.modal').exists()).toBe(false)
    await openModal(w)
    await w.find('.modal-overlay').trigger('click')
    expect(w.find('.modal').exists()).toBe(false)
    w.unmount()
  })
})

describe('ClusterNodes 刷新与定时轮询', () => {
  it('工具栏刷新按钮重拉 nodes.list；10s 定时器轮询；卸载后停止', async () => {
    vi.useFakeTimers()
    const w = await mountNodes()
    const listCalls = () => requestMock.mock.calls.filter(c => c[1] === 'nodes.list').length
    const base = listCalls()

    await w.findAll('button').find(b => b.text() === '刷新')!.trigger('click')
    await flushPromises()
    expect(listCalls()).toBe(base + 1)

    vi.advanceTimersByTime(10000)
    await flushPromises()
    expect(listCalls()).toBe(base + 2)

    w.unmount()
    vi.advanceTimersByTime(30000)
    await flushPromises()
    expect(listCalls()).toBe(base + 2)
  })
})
