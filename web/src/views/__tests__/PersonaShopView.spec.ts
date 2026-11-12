import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { useToast } from '../../composables/useToast'

const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
  initWSAPI: vi.fn(),
}))
vi.mock('../../composables/useSSE', () => ({ on: vi.fn(), off: vi.fn() }))

import PersonaShopView from '../PersonaShopView.vue'

const P1 = { id: 'p1', name: 'Nemesis', emoji: '🤖', division: '开发', description: '主力人格', installed: false }
const P2 = { id: 'p2', name: 'Guardian', emoji: '🛡️', division: '安全', description: '守卫人格', installed: true }

function defaultImpl(_m: string, cmd: string, p?: any) {
  switch (cmd) {
    case 'shop.browse':
      return Promise.resolve({ items: p?.division ? [P2] : [P1, P2] })
    case 'shop.refresh':
      return Promise.resolve({})
    case 'shop.preview':
      return Promise.resolve({ name: 'Nemesis Pro', emoji: '🧠', installed: false, raw: '# Nemesis\n人格定义', converted: { 'PERSONA.json': '{"k":"v"}', 'SOUL.md': '# 灵魂' } })
    case 'shop.download':
      return Promise.resolve({})
    case 'shop.hire':
      return Promise.resolve({ note: '（节点 alpha）' })
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
  const w = mount(PersonaShopView)
  await flushPromises()
  return w
}

describe('PersonaShopView 浏览与搜索', () => {
  it('挂载先 refresh 再 browse 渲染卡片与计数', async () => {
    const w = await mountView()
    expect(requestMock).toHaveBeenCalledWith('persona', 'shop.refresh', {})
    expect(requestMock).toHaveBeenCalledWith('persona', 'shop.browse', { division: undefined })
    expect(w.text()).toContain('共 2 个人格')
    expect(w.text()).toContain('Nemesis')
    expect(w.text()).toContain('开发')
  })

  it('browse 失败提示', async () => {
    requestMock.mockImplementation((_m, cmd) => cmd === 'shop.browse' ? Promise.reject(new Error('net')) : Promise.resolve({}))
    await mountView()
    expect(useToast().toasts.some(t => t.message.includes('加载失败'))).toBe(true)
  })

  it('空关键词搜索回落 browse；有关键词走 shop.search', async () => {
    const w = await mountView()
    ;(w.vm as any).searchQuery = '  '
    await (w.vm as any).search()
    expect(requestMock.mock.calls.filter(c => c[1] === 'shop.search').length).toBe(0)
    expect(requestMock.mock.calls.filter(c => c[1] === 'shop.browse').length).toBe(2)

    ;(w.vm as any).searchQuery = 'guard'
    await (w.vm as any).search()
    expect(requestMock).toHaveBeenCalledWith('persona', 'shop.search', { query: 'guard' })
  })

  it('搜索失败提示；回车触发搜索', async () => {
    requestMock.mockImplementation((_m, cmd) => cmd === 'shop.search' ? Promise.reject(new Error('boom')) : Promise.resolve({}))
    const w = await mountView()
    await w.find('input.form-input').setValue('x')
    await w.find('input.form-input').trigger('keyup.enter')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('搜索失败'))).toBe(true)
  })

  it('部门筛选与来源页签切换都触发 browse', async () => {
    const w = await mountView()
    await w.findAll('.filter-pill').find(p => p.text() === '安全')!.trigger('click')
    expect(requestMock).toHaveBeenCalledWith('persona', 'shop.browse', { division: '安全' })
    await w.findAll('.filter-pill').find(p => p.text() === '全部')!.trigger('click')
    expect(requestMock).toHaveBeenCalledWith('persona', 'shop.browse', { division: undefined })
    await w.findAll('button.tab').find(b => b.text() === 'Agency Agents')!.trigger('click')
    expect((w.vm as any).selectedSource).toBe('Agency Agents')
  })
})

describe('PersonaShopView 预览', () => {
  it('点击卡片打开预览：数据合并、原始/转换双页签', async () => {
    const w = await mountView()
    await w.findAll('.shop-card').find(c => c.text().includes('Nemesis'))!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('persona', 'shop.preview', { id: 'p1' })
    expect(w.text()).toContain('🧠 Nemesis Pro')
    expect(w.find('.markdown-body').exists()).toBe(true)
    expect(w.text()).toContain('人格定义')
    // 转换页签
    await w.findAll('.preview-tab').find(b => b.text() === '转换预览')!.trigger('click')
    expect(w.text()).toContain('PERSONA.json')
    await w.findAll('.converted-file-item').find(f => f.text() === 'SOUL.md')!.trigger('click')
    expect((w.vm as any).selectedConverted).toBe(1)
  })

  it('预览失败提示；关闭清理', async () => {
    requestMock.mockImplementation((_m, cmd) => cmd === 'shop.preview' ? Promise.reject(new Error('nf')) : defaultImpl(_m, cmd, {}))
    const w = await mountView()
    await w.findAll('.shop-card').find(c => c.text().includes('Nemesis'))!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('加载预览失败'))).toBe(true)

    await w.findAll('.shop-card').find(c => c.text().includes('Guardian'))!.trigger('click')
    await flushPromises()
    await w.findAll('.modal-footer button').find(b => b.text() === '关闭')!.trigger('click')
    expect((w.vm as any).previewPersona).toBe(null)
  })

  it('无转换数据显示空态；原始内容缺失显示暂无内容', async () => {
    requestMock.mockImplementation((_m, cmd) => cmd === 'shop.preview' ? Promise.resolve({ name: 'x' }) : defaultImpl(_m, cmd, {}))
    const w = await mountView()
    await w.findAll('.shop-card').find(c => c.text().includes('Nemesis'))!.trigger('click')
    await flushPromises()
    expect(w.text()).toContain('暂无内容')
    await w.findAll('.preview-tab').find(b => b.text() === '转换预览')!.trigger('click')
    expect(w.text()).toContain('暂无转换数据')
  })
})

describe('PersonaShopView 下载与聘用', () => {
  it('下载成功：提示、预览态置 installed、刷新列表', async () => {
    const w = await mountView()
    await w.findAll('.shop-card').find(c => c.text().includes('Nemesis'))!.trigger('click')
    await flushPromises()
    const calls = requestMock.mock.calls.filter(c => c[1] === 'shop.browse').length
    await w.findAll('.modal-footer button').find(b => b.text() === '下载')!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('persona', 'shop.download', { id: 'p1' })
    expect(useToast().toasts.some(t => t.message.includes('下载成功'))).toBe(true)
    expect((w.vm as any).previewPersona.installed).toBe(true)
    expect(requestMock.mock.calls.filter(c => c[1] === 'shop.browse').length).toBe(calls + 1)
  })

  it('下载失败提示；卡片下载按钮', async () => {
    requestMock.mockImplementation((_m, cmd) => cmd === 'shop.download' ? Promise.reject(new Error('deny')) : defaultImpl(_m, cmd, {}))
    const w = await mountView()
    await w.findAll('.shop-card').find(c => c.text().includes('Nemesis'))!.find('.btn-primary').trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('下载失败'))).toBe(true)
    expect((w.vm as any).downloading).toBe('')
  })

  it('集群聘用成功（带 note）关预览；失败提示', async () => {
    // 预览合并后 installed=true，聘用按钮才可用
    requestMock.mockImplementation((_m, cmd) => cmd === 'shop.preview' ? Promise.resolve({ ...defaultImpl(_m, cmd, {}), installed: true }) : defaultImpl(_m, cmd, {}))
    const w = await mountView()
    await w.findAll('.shop-card').find(c => c.text().includes('Guardian'))!.trigger('click')
    await flushPromises()
    await w.findAll('.modal-footer button').find(b => b.text().includes('集群聘用'))!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('persona', 'shop.hire', { id: 'p2' })
    expect(useToast().toasts.some(t => t.message.includes('Guardian'))).toBe(true)
    expect((w.vm as any).previewPersona).toBe(null)

    requestMock.mockImplementation((_m, cmd) => {
      if (cmd === 'shop.hire') return Promise.reject(new Error('full'))
      if (cmd === 'shop.preview') return Promise.resolve({ ...defaultImpl(_m, cmd, {}), installed: true })
      return defaultImpl(_m, cmd, {})
    })
    await w.findAll('.shop-card').find(c => c.text().includes('Guardian'))!.trigger('click')
    await flushPromises()
    await w.findAll('.modal-footer button').find(b => b.text().includes('集群聘用'))!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('集群聘用失败'))).toBe(true)
    expect((w.vm as any).hiring).toBe('')
  })
})
