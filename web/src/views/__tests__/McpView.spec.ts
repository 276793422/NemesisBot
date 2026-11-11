import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { useToast } from '../../composables/useToast'

const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
}))
vi.mock('../../composables/useSSE', () => ({
  on: vi.fn(),
  off: vi.fn(),
}))

import McpView from '../McpView.vue'

const SERVER_STDIO = {
  name: 'filesystem',
  transport_type: 'stdio',
  url: 'npx',
  description: '本地文件',
  headers: [],
  args: ['-y', '@mcp/fs'],
  env: ['A=1', 'B=2'],
  timeout: 30,
  provider_name: 'Anthropic',
  provider_url: 'https://anthropic.com',
  tags: ['文件系统', '本地'],
}
const SERVER_HTTP = {
  name: 'remote-mcp',
  transport_type: 'http',
  url: 'https://mcp.example.com/api',
  description: '',
  headers: ['Authorization: Bearer t'],
  args: [],
  env: [],
  timeout: 0,
  provider_name: '',
  provider_url: '',
  tags: [],
}

function defaultImpl(_m: string, cmd: string) {
  switch (cmd) {
    case 'status': return Promise.resolve({ enabled: true })
    case 'servers': return Promise.resolve({ servers: [SERVER_STDIO, SERVER_HTTP] })
    default: return Promise.resolve({})
  }
}

beforeEach(() => {
  requestMock.mockReset()
  requestMock.mockImplementation(defaultImpl)
  useToast().toasts.splice(0)
})

async function mountView() {
  const w = mount(McpView)
  await flushPromises()
  return w
}

describe('McpView 加载与列表', () => {
  it('加载 status 与 servers，渲染卡片与徽章', async () => {
    const w = await mountView()
    expect(requestMock).toHaveBeenCalledWith('mcp', 'status')
    expect(requestMock).toHaveBeenCalledWith('mcp', 'servers')
    expect(w.text()).toContain('已启用')
    expect(w.text()).toContain('filesystem')
    expect(w.text()).toContain('STDIO')
    expect(w.text()).toContain('文件系统')
  })

  it('status enabled=false 显示未启用', async () => {
    requestMock.mockImplementation((_m, cmd) => cmd === 'status' ? Promise.resolve({ enabled: false }) : defaultImpl(_m, cmd))
    const w = await mountView()
    expect(w.text()).toContain('未启用')
  })

  it('servers 为空显示空态', async () => {
    requestMock.mockImplementation((_m, cmd) => cmd === 'servers' ? Promise.resolve({ servers: [] }) : defaultImpl(_m, cmd))
    const w = await mountView()
    expect(w.text()).toContain('暂无 MCP 服务器')
  })

  it('servers 加载失败提示错误并进入空态', async () => {
    requestMock.mockImplementation((_m, cmd) => cmd === 'servers' ? Promise.reject(new Error('boom')) : defaultImpl(_m, cmd))
    const w = await mountView()
    expect(useToast().toasts.some(t => t.message.includes('加载失败'))).toBe(true)
    expect(w.text()).toContain('暂无 MCP 服务器')
  })
})

describe('McpView 详情弹窗', () => {
  it('点击卡片打开详情：args/env/tags/供应商链接', async () => {
    const w = await mountView()
    await w.findAll('.skill-card').find(c => c.text().includes('filesystem'))!.trigger('click')
    expect(w.text()).toContain('超时 30s')
    expect(w.text()).toContain('-y @mcp/fs')
    expect(w.text()).toContain('A=1')
    expect(w.text()).toContain('https://anthropic.com')
  })

  it('http 服务器详情显示请求头', async () => {
    const w = await mountView()
    await w.findAll('.skill-card').find(c => c.text().includes('remote-mcp'))!.trigger('click')
    expect(w.text()).toContain('Authorization: Bearer t')
  })
})

describe('McpView 添加与编辑', () => {
  it('打开添加弹窗：默认 stdio 表单', async () => {
    const w = await mountView()
    await w.findAll('button').find(b => b.text() === '添加服务器')!.trigger('click')
    expect(w.text()).toContain('添加 MCP 服务器')
    expect(w.text()).toContain('命令 *')
    // 切到 HTTP 类型：字段切换
    await w.findAll('.transport-btn').find(b => b.text() === 'HTTP')!.trigger('click')
    expect(w.text()).toContain('URL *')
    expect(w.text()).toContain('请求头')
  })

  it('校验：缺名称/URL 时按钮禁用且保存提示', async () => {
    const w = await mountView()
    await w.findAll('button').find(b => b.text() === '添加服务器')!.trigger('click')
    const vm = w.vm as any
    vm.form.name = ''
    vm.form.url = ''
    await vm.saveServer()
    expect(useToast().toasts.some(t => t.message.includes('请填写名称和 URL/命令'))).toBe(true)
    expect(requestMock.mock.calls.filter(c => c[1] === 'server.add').length).toBe(0)
  })

  it('新增 stdio 服务器：args/env/tags 解析入库', async () => {
    const w = await mountView()
    await w.findAll('button').find(b => b.text() === '添加服务器')!.trigger('click')
    const vm = w.vm as any
    vm.form.name = 'fs2'
    vm.form.url = 'npx'
    vm.form.argsText = '-y  @mcp/fs   /tmp'
    vm.form.envText = ' A=1 \nB=2\n\n'
    vm.form.tagsText = '文件, 本地 ,x'
    await vm.saveServer()
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('mcp', 'server.add', expect.objectContaining({
      name: 'fs2',
      transport_type: 'stdio',
      args: ['-y', '@mcp/fs', '/tmp'],
      env: ['A=1', 'B=2'],
      tags: ['文件', '本地', 'x'],
    }))
    expect(useToast().toasts.some(t => t.message === '已添加')).toBe(true)
  })

  it('新增 http 服务器：headers 分行解析', async () => {
    const w = await mountView()
    await w.findAll('button').find(b => b.text() === '添加服务器')!.trigger('click')
    const vm = w.vm as any
    vm.form.transport_type = 'http'
    vm.form.name = 'r1'
    vm.form.url = 'https://x/api'
    vm.form.headersText = 'Authorization: Bearer t\n\nX-Custom: v\n'
    await vm.saveServer()
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('mcp', 'server.add', expect.objectContaining({
      transport_type: 'http',
      headers: ['Authorization: Bearer t', 'X-Custom: v'],
    }))
  })

  it('编辑回填表单并走 server.update', async () => {
    const w = await mountView()
    await w.findAll('.skill-card').find(c => c.text().includes('filesystem'))!.trigger('click')
    await w.findAll('button').find(b => b.text() === '编辑')!.trigger('click')
    const vm = w.vm as any
    expect(vm.editingServer).toBe('filesystem')
    expect(vm.form.argsText).toBe('-y @mcp/fs')
    expect(vm.form.envText).toBe('A=1\nB=2')
    expect(vm.form.tagsText).toBe('文件系统, 本地')
    vm.form.description = '改过的描述'
    await vm.saveServer()
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('mcp', 'server.update', expect.objectContaining({
      name: 'filesystem',
      description: '改过的描述',
    }))
    expect(useToast().toasts.some(t => t.message === '已更新')).toBe(true)
  })

  it('添加失败提示', async () => {
    requestMock.mockImplementation((_m, cmd) => cmd === 'server.add' ? Promise.reject(new Error('dup')) : defaultImpl(_m, cmd))
    const w = await mountView()
    await w.findAll('button').find(b => b.text() === '添加服务器')!.trigger('click')
    const vm = w.vm as any
    vm.form.name = 'x'
    vm.form.url = 'y'
    await vm.saveServer()
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('添加失败'))).toBe(true)
  })

  it('更新失败提示', async () => {
    requestMock.mockImplementation((_m, cmd) => cmd === 'server.update' ? Promise.reject(new Error('busy')) : defaultImpl(_m, cmd))
    const w = await mountView()
    await w.findAll('.skill-card').find(c => c.text().includes('filesystem'))!.trigger('click')
    await w.findAll('button').find(b => b.text() === '编辑')!.trigger('click')
    await (w.vm as any).saveServer()
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('更新失败'))).toBe(true)
  })
})

describe('McpView 删除', () => {
  async function openDetail(w: ReturnType<typeof mount>) {
    await w.findAll('.skill-card').find(c => c.text().includes('filesystem'))!.trigger('click')
  }

  it('两段式确认：删除 → 确认词 → 确认执行；取消回退', async () => {
    const w = await mountView()
    await openDetail(w)
    await w.findAll('button').find(b => b.text() === '删除')!.trigger('click')
    expect((w.vm as any).confirmDeleteName).toBe('filesystem')
    expect(w.text()).toContain('确定？')
    await w.findAll('button').find(b => b.text() === '取消')!.trigger('click')
    expect((w.vm as any).confirmDeleteName).toBe('')

    await w.findAll('button').find(b => b.text() === '删除')!.trigger('click')
    await w.findAll('button').find(b => b.text() === '确认')!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('mcp', 'server.delete', { name: 'filesystem' })
    expect(useToast().toasts.some(t => t.message === '已删除')).toBe(true)
  })

  it('删除失败提示', async () => {
    requestMock.mockImplementation((_m, cmd) => cmd === 'server.delete' ? Promise.reject(new Error('refused')) : defaultImpl(_m, cmd))
    const w = await mountView()
    await openDetail(w)
    await w.findAll('button').find(b => b.text() === '删除')!.trigger('click')
    await w.findAll('button').find(b => b.text() === '确认')!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('删除失败'))).toBe(true)
  })
})

describe('McpView 杂项', () => {
  it('transportBadge/transportColor 三态映射', () => {
    const vm = McpView as any
    void vm
    return mountView().then((w) => {
      const v = w.vm as any
      expect(v.transportBadge('http')).toBe('badge-success')
      expect(v.transportBadge('sse')).toBe('badge-warning')
      expect(v.transportBadge('stdio')).toBe('badge-info')
      expect(v.transportColor('http')).toBe('var(--success)')
      expect(v.transportColor('sse')).toBe('var(--warning)')
      expect(v.transportColor('x')).toBe('var(--info)')
    })
  })

  it('商城页签显示空态', async () => {
    const w = await mountView()
    await w.findAll('button.tab').find(b => b.text() === '商城')!.trigger('click')
    expect(w.text()).toContain('即将上线')
  })
})
