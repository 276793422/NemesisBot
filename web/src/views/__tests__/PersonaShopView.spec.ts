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

  it('挂载刷新窗口全程转圈：refresh 未返回前 spinner 在场、不渲染空态文案', async () => {
    // 回归钉（2026-11-14）：shop.refresh 在线拉 GitHub 可达 30s 超时窗，
    // 此前该窗口 loading=false + items=[]，空态「没有找到匹配的人格」
    // 先行渲染，用户以为界面卡死。
    let resolveRefresh!: (v: any) => void
    requestMock.mockImplementation((_m, cmd) => cmd === 'shop.refresh'
      ? new Promise(r => { resolveRefresh = r })
      : Promise.resolve({ items: [P1, P2] }))
    const w = mount(PersonaShopView)
    await w.vm.$nextTick()
    expect(w.find('.spinner-lg').exists()).toBe(true)
    expect(w.text()).toContain('正在加载人格超市')
    expect(w.text()).not.toContain('没有找到匹配的人格')
    resolveRefresh({})
    await flushPromises()
    expect(w.find('.spinner-lg').exists()).toBe(false)
    expect(w.text()).toContain('共 2 个人格')
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
    // live 来源页签带「· 在线」后缀（来源标注拍板 2026-11-12）
    await w.findAll('button.tab').find(b => b.text() === 'Agency Agents · 在线')!.trigger('click')
    expect((w.vm as any).selectedSource).toBe('Agency Agents')
  })

  it('快照来源响应：页签标注内置快照（label）+ 警示色类', async () => {
    requestMock.mockImplementation((_m, cmd) => cmd === 'shop.browse'
      ? Promise.resolve({ items: [P1], source: 'snapshot', snapshot_info: { commit: 'abc1234', fetched_at: '2026-11-11T00:00:00Z', file_count: 274, label: 'abc1234 @ 2026-11-11' } })
      : Promise.resolve({}))
    const w = await mountView()
    const tab = w.findAll('button.tab').find(b => b.text().includes('Agency Agents'))!
    expect(tab.text()).toBe('Agency Agents · 内置快照（abc1234 @ 2026-11-11）')
    expect(tab.classes()).toContain('tab-snapshot')
    expect((w.vm as any).dataSource).toBe('snapshot')
    // live 响应（如刷新失败后切标签页回来）恢复「· 在线」且无警示色
    requestMock.mockImplementation(defaultImpl)
    await (w.vm as any).browse()
    await flushPromises()
    const liveTab = w.findAll('button.tab').find(b => b.text().includes('Agency Agents'))!
    expect(liveTab.text()).toBe('Agency Agents · 在线')
    expect(liveTab.classes()).not.toContain('tab-snapshot')
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

describe('PersonaShopView 雇佣专家', () => {
  it('雇佣专家成功：请求 shop.hire_expert、提示、清 busy；失败提示', async () => {
    const w = await mountView()
    // P2 installed=true → 按钮可用；P1 未安装禁用
    const guardCard = w.findAll('.shop-card').find(c => c.text().includes('Guardian'))!
    await guardCard.findAll('button').find(b => b.text().includes('雇佣专家'))!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('persona', 'shop.hire_expert', { id: 'p2' })
    expect(useToast().toasts.some(t => t.message.includes('已雇佣为专家'))).toBe(true)
    expect((w.vm as any).hiringExpert).toBe('')

    requestMock.mockImplementation((_m, cmd) =>
      cmd === 'shop.hire_expert' ? Promise.reject(new Error('SOUL.md 为空')) : defaultImpl(_m, cmd, {}))
    await guardCard.findAll('button').find(b => b.text().includes('雇佣专家'))!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('雇佣专家失败'))).toBe(true)
    expect((w.vm as any).hiringExpert).toBe('')
  })

  it('未安装人格的雇佣按钮禁用；无 cluster 也恒渲染（不随集群裁剪）', async () => {
    const w = await mountView()
    const nemesisBtn = w.findAll('.shop-card').find(c => c.text().includes('Nemesis'))!
      .findAll('button').find(b => b.text().includes('雇佣专家'))!
    expect(nemesisBtn.attributes('disabled')).toBeDefined()
    const guardBtn = w.findAll('.shop-card').find(c => c.text().includes('Guardian'))!
      .findAll('button').find(b => b.text().includes('雇佣专家'))!
    expect(guardBtn.attributes('disabled')).toBeUndefined()
  })

  it('预览弹窗内雇佣专家成功关预览', async () => {
    // 预览合并 installed 才可用（默认 preview 响应 installed=false，需覆盖）
    requestMock.mockImplementation((_m, cmd) =>
      cmd === 'shop.preview' ? Promise.resolve({ ...defaultImpl(_m, cmd, {}), installed: true }) : defaultImpl(_m, cmd, {}))
    const w = await mountView()
    await w.findAll('.shop-card').find(c => c.text().includes('Guardian'))!.trigger('click')
    await flushPromises()
    await w.findAll('.modal-footer button').find(b => b.text().includes('雇佣专家'))!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('persona', 'shop.hire_expert', { id: 'p2' })
    expect((w.vm as any).previewPersona).toBe(null)
    expect((w.vm as any).hiringExpert).toBe('')
  })
})
