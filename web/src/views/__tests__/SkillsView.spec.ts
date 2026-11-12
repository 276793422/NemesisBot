import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { useToast } from '../../composables/useToast'

const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
}))
vi.mock('../../composables/useSSE', () => ({
  on: vi.fn(),
  off: vi.fn(),
}))

import SkillsView from '../SkillsView.vue'

const BROWSE_ITEMS = [
  { name: 'code-review', slug: 'code-review', description: '审查代码', source: 'clawhub', version: '1.0.0', installed: false },
  { name: 'git-helper', slug: 'git-helper', description: 'git 助手', source: 'clawhub', version: 'latest' },
]

function defaultImpl(_m: string, cmd: string, _p?: any) {
  switch (cmd) {
    case 'installed':
      return Promise.resolve({ skills: [{ name: 'code-review', has_skill_md: true, description: '审查代码' }] })
    case 'browse':
      return Promise.resolve({ items: BROWSE_ITEMS, next_cursor: null })
    case 'search':
      return Promise.resolve({ results: BROWSE_ITEMS })
    case 'detail':
      return Promise.resolve({ content: '# code-review\n说明' })
    case 'config.get':
      return Promise.resolve({ enabled: true })
    case 'config.save':
      return Promise.resolve({})
    case 'source.list':
      return Promise.resolve({ sources: [{ type: 'github', name: 'acme', repo: 'acme/skills', enabled: true, deletable: true }] })
    case 'shop_detail':
      return Promise.resolve({ name: 'code-review', description: '详情' })
    case 'shop_code':
      return Promise.resolve({ code: '---\nname: x\n---', filename: 'SKILL.md' })
    case 'source.add':
      return Promise.resolve({ success: true, source: { name: 'acme' } })
    case 'source.add.manual':
      return Promise.resolve({ success: true, source: { name: 'manual-src' } })
    case 'learn':
      return Promise.resolve({ message: '已开始' })
    default:
      return Promise.resolve({})
  }
}

beforeEach(() => {
  requestMock.mockReset()
  requestMock.mockImplementation(defaultImpl)
  useToast().toasts.splice(0)
  vi.stubGlobal('confirm', vi.fn(() => true))
})

afterEach(() => {
  vi.unstubAllGlobals()
  vi.useRealTimers()
})

async function mountView() {
  const w = mount(SkillsView)
  await flushPromises()
  return w
}

async function gotoShop(w: ReturnType<typeof mount>) {
  await w.findAll('button.tab').find(b => b.text() === '商店')!.trigger('click')
  await flushPromises()
}

async function gotoConfig(w: ReturnType<typeof mount>) {
  await w.findAll('button.tab').find(b => b.text() === '配置')!.trigger('click')
  await flushPromises()
}

describe('SkillsView 已安装页签', () => {
  it('加载已安装技能并渲染卡片', async () => {
    const w = await mountView()
    expect(requestMock).toHaveBeenCalledWith('skills', 'installed')
    expect(w.text()).toContain('code-review')
    expect(w.text()).toContain('有效')
    expect(w.text()).toContain('审查代码')
  })

  it('空列表显示空态', async () => {
    requestMock.mockImplementation((_m, cmd) => cmd === 'installed' ? Promise.resolve({ skills: [] }) : Promise.resolve({}))
    const w = await mountView()
    expect(w.text()).toContain('暂无技能')
  })

  it('加载失败提示错误', async () => {
    requestMock.mockImplementation((_m, cmd) => cmd === 'installed' ? Promise.reject(new Error('boom')) : Promise.resolve({}))
    await mountView()
    expect(useToast().toasts.some(t => t.message.includes('加载失败'))).toBe(true)
  })

  it('查看 → detail 弹层显示内容；读取失败提示', async () => {
    const w = await mountView()
    await w.findAll('button').find(b => b.text() === '查看')!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('skills', 'detail', { name: 'code-review' })
    expect(w.text()).toContain('# code-review')

    requestMock.mockImplementation((_m, cmd) => cmd === 'detail' ? Promise.reject(new Error('nope')) : Promise.resolve({}))
    await w.findAll('button').find(b => b.text() === '查看')!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('读取失败'))).toBe(true)
  })

  it('目录按钮调用 open_dir；失败提示', async () => {
    const w = await mountView()
    await w.findAll('button').find(b => b.text() === '目录')!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('skills', 'open_dir', { name: 'code-review' })

    requestMock.mockImplementation((_m, cmd) => cmd === 'open_dir' ? Promise.reject(new Error('x')) : Promise.resolve({}))
    await w.findAll('button').find(b => b.text() === '目录')!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('打开目录失败'))).toBe(true)
  })

  it('卸载：confirm 拦截不发请求；成功后刷新并清理详情', async () => {
    const confirmSpy = vi.fn(() => false)
    vi.stubGlobal('confirm', confirmSpy)
    const w = await mountView()
    await w.findAll('button').find(b => b.text() === '卸载')!.trigger('click')
    expect(confirmSpy).toHaveBeenCalled()
    expect(requestMock.mock.calls.filter(c => c[1] === 'uninstall').length).toBe(0)

    vi.stubGlobal('confirm', vi.fn(() => true))
    await w.findAll('button').find(b => b.text() === '查看')!.trigger('click')
    await flushPromises()
    expect(w.text()).toContain('# code-review')
    await w.findAll('button').find(b => b.text() === '卸载')!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('skills', 'uninstall', { name: 'code-review' })
    expect(useToast().toasts.some(t => t.message === '已卸载')).toBe(true)
  })

  it('卸载失败提示错误', async () => {
    requestMock.mockImplementation((_m, cmd) => cmd === 'uninstall' ? Promise.reject(new Error('busy')) : defaultImpl(_m, cmd))
    const w = await mountView()
    await w.findAll('button').find(b => b.text() === '卸载')!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('卸载失败'))).toBe(true)
  })
})

describe('SkillsView 商店页签：浏览', () => {
  it('进入商店触发 browse 并渲染结果', async () => {
    const w = await mountView()
    await gotoShop(w)
    expect(requestMock).toHaveBeenCalledWith('skills', 'browse', { registry: 'clawhub', sort: 'trending', limit: 100 })
    expect(w.text()).toContain('code-review')
    expect(w.text()).toContain('安装')
  })

  it('60 秒内缓存命中不重复请求', async () => {
    vi.useFakeTimers()
    const w = await mountView()
    await gotoShop(w)
    const calls = requestMock.mock.calls.filter(c => c[1] === 'browse').length
    // 同一 key 第二次 browse：走缓存
    await (w.vm as any).browseSkills()
    expect(requestMock.mock.calls.filter(c => c[1] === 'browse').length).toBe(calls)
    // 超时后重新请求
    vi.advanceTimersByTime(61000)
    await (w.vm as any).browseSkills()
    expect(requestMock.mock.calls.filter(c => c[1] === 'browse').length).toBe(calls + 1)
  })

  it('切换排序与数据源重新 browse', async () => {
    const w = await mountView()
    await gotoShop(w)
    await w.findAll('.filter-pill').find(p => p.text() === '下载量')!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('skills', 'browse', { registry: 'clawhub', sort: 'downloads', limit: 100 })
    await w.findAll('.filter-pill').find(p => p.text() === 'ModelScope')!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('skills', 'browse', { registry: 'modelscope', sort: 'downloads', limit: 100 })
  })

  it('browse 失败提示错误', async () => {
    requestMock.mockImplementation((_m, cmd) => cmd === 'browse' ? Promise.reject(new Error('net')) : Promise.resolve({}))
    const w = await mountView()
    await gotoShop(w)
    expect(useToast().toasts.some(t => t.message.includes('浏览失败'))).toBe(true)
    expect(w.text()).toContain('暂无技能，请检查')
  })

  it('loadMore：客户端分页优先，耗尽后走服务端 cursor', async () => {
    const many = Array.from({ length: 25 }, (_, i) => ({ name: `s${i}`, slug: `s${i}`, source: 'clawhub' }))
    let browseCalls = 0
    requestMock.mockImplementation((_m, cmd) => {
      if (cmd === 'browse') {
        browseCalls++
        return Promise.resolve({ items: many, next_cursor: browseCalls === 1 ? 'c1' : null })
      }
      return Promise.resolve({})
    })
    const w = await mountView()
    await gotoShop(w)
    const vm = w.vm as any
    // 25 条 > displayCount 20：客户端分页，不发请求
    await vm.loadMore()
    expect(vm.browseDisplayCount).toBe(40)
    expect(requestMock.mock.calls.filter(c => c[1] === 'browse').length).toBe(1)
    // displayCount 40 >= 25：服务端 cursor 追加
    await vm.loadMore()
    expect(requestMock).toHaveBeenCalledWith('skills', 'browse', { registry: 'clawhub', sort: 'trending', limit: 100, cursor: 'c1' })
    expect(vm.browseResults.length).toBe(50)
    // cursor 已清空：不再请求
    await vm.loadMore()
    expect(requestMock.mock.calls.filter(c => c[1] === 'browse').length).toBe(2)
  })

  it('loadMore：条数少于 displayCount 时直接走服务端追加', async () => {
    requestMock.mockImplementation((_m, cmd) => cmd === 'browse'
      ? Promise.resolve({ items: BROWSE_ITEMS.slice(0, 2), next_cursor: 'c1' })
      : Promise.resolve({}))
    const w = await mountView()
    await gotoShop(w)
    const vm = w.vm as any
    await vm.loadMore()
    expect(requestMock).toHaveBeenCalledWith('skills', 'browse', { registry: 'clawhub', sort: 'trending', limit: 100, cursor: 'c1' })
    expect(vm.browseResults.length).toBe(4)
    expect(vm.browseDisplayCount).toBe(4)
  })

  it('loadMore 服务端失败提示', async () => {
    requestMock.mockImplementation((_m, cmd) => {
      if (cmd === 'browse') {
        const calls = requestMock.mock.calls.filter(c => c[1] === 'browse').length
        return calls === 1 ? Promise.resolve({ items: BROWSE_ITEMS.slice(0, 15), next_cursor: 'c1' }) : Promise.reject(new Error('timeout'))
      }
      return Promise.resolve({})
    })
    const w = await mountView()
    await gotoShop(w)
    await (w.vm as any).loadMore()
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('加载更多失败'))).toBe(true)
  })
})

describe('SkillsView 商店页签：搜索与安装', () => {
  it('空关键词不触发搜索', async () => {
    const w = await mountView()
    await gotoShop(w)
    const calls = requestMock.mock.calls.filter(c => c[1] === 'search').length
    await (w.vm as any).searchSkills()
    expect(requestMock.mock.calls.filter(c => c[1] === 'search').length).toBe(calls)
  })

  it('搜索成功渲染结果计数；失败提示', async () => {
    const w = await mountView()
    await gotoShop(w)
    await w.find('input.form-input').setValue('code')
    await w.findAll('button').find(b => b.text() === '搜索')!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('skills', 'search', { query: 'code' })
    expect(w.text()).toContain('2 个结果')
    expect(w.text()).toContain('返回浏览')

    requestMock.mockImplementation((_m, cmd) => cmd === 'search' ? Promise.reject(new Error('net')) : Promise.resolve({}))
    await w.findAll('button').find(b => b.text() === '搜索')!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('搜索失败'))).toBe(true)
  })

  it('搜索 message 字段触发 info toast；返回浏览清空结果', async () => {
    requestMock.mockImplementation((_m, cmd) => cmd === 'search' ? Promise.resolve({ results: [], message: '无结果' }) : Promise.resolve({}))
    const w = await mountView()
    await gotoShop(w)
    await w.find('input.form-input').setValue('zzz')
    await w.findAll('button').find(b => b.text() === '搜索')!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message === '无结果')).toBe(true)
    expect(w.text()).toContain('未找到匹配的技能')

    await w.findAll('button').find(b => b.text() === '返回浏览')!.trigger('click')
    await flushPromises()
    expect((w.vm as any).searchQuery).toBe('')
    expect((w.vm as any).isBrowseMode).toBe(true)
  })

  it('分类搜索：填充关键词并搜索', async () => {
    const w = await mountView()
    await gotoShop(w)
    await w.findAll('button').find(b => b.text() === '分类搜索')!.trigger('click')
    await w.findAll('.filter-pill').find(p => p.text() === '编程开发')!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('skills', 'search', { query: 'coding' })
  })

  it('安装成功刷新列表；already_installed 提示更新', async () => {
    const w = await mountView()
    await gotoShop(w)
    await w.findAll('button').find(b => b.text() === '安装')!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('skills', 'install', { registry: 'clawhub', slug: 'code-review', force: false })
    expect(useToast().toasts.some(t => t.message === '已安装: code-review')).toBe(true)

    requestMock.mockImplementation((_m, cmd) => cmd === 'install' ? Promise.resolve({ already_installed: true }) : defaultImpl(_m, cmd))
    await w.findAll('button.btn-primary').find(b => b.text() === '安装')!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('点击"更新"可覆盖安装'))).toBe(true)
    expect((w.vm as any).alreadyInstalledSlugs.has('code-review')).toBe(true)
  })

  it('已标记安装的条目点击走 force=true 更新', async () => {
    const w = await mountView()
    await gotoShop(w)
    const vm = w.vm as any
    vm.alreadyInstalledSlugs.add('code-review')
    await w.findAll('button').find(b => b.text() === '安装')!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('skills', 'install', { registry: 'clawhub', slug: 'code-review', force: true })
    expect(useToast().toasts.some(t => t.message === '已更新: code-review')).toBe(true)
  })

  it('安装失败提示错误', async () => {
    requestMock.mockImplementation((_m, cmd) => cmd === 'install' ? Promise.reject(new Error('denied')) : defaultImpl(_m, cmd))
    const w = await mountView()
    await gotoShop(w)
    await w.findAll('button').find(b => b.text() === '安装')!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('安装失败'))).toBe(true)
    expect((w.vm as any).installingSlug).toBe('')
  })

  it('商店详情：打开、获取源码、再点收起、关闭清理', async () => {
    const w = await mountView()
    await gotoShop(w)
    await w.find('.skill-card').trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('skills', 'shop_detail', { registry: 'clawhub', slug: 'code-review' })
    expect(w.text()).toContain('详情')

    const vm = w.vm as any
    await vm.viewShopCode('clawhub', 'code-review')
    expect(vm.showShopCode).toBe(true)
    expect(vm.shopCode).toContain('name: x')
    // 再点：收起
    await vm.viewShopCode('clawhub', 'code-review')
    expect(vm.showShopCode).toBe(false)
    // 重新打开后 closeShopDetail 清空
    await vm.viewShopCode('clawhub', 'code-review')
    vm.closeShopDetail()
    expect(vm.shopDetail).toBe(null)
    expect(vm.showShopCode).toBe(false)
    expect(vm.shopCodeFilename).toBe('SKILL.md')
  })

  it('商店详情与源码失败提示', async () => {
    requestMock.mockImplementation((_m, cmd) => (cmd === 'shop_detail' || cmd === 'shop_code') ? Promise.reject(new Error('net')) : defaultImpl(_m, cmd))
    const w = await mountView()
    await gotoShop(w)
    await w.find('.skill-card').trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('获取详情失败'))).toBe(true)

    await (w.vm as any).viewShopCode('clawhub', 'code-review')
    expect(useToast().toasts.some(t => t.message.includes('获取源码失败'))).toBe(true)
  })
})

describe('SkillsView 配置页签', () => {
  it('切到配置页加载 config 与 sources', async () => {
    const w = await mountView()
    await gotoConfig(w)
    expect(requestMock).toHaveBeenCalledWith('skills', 'config.get')
    expect(requestMock).toHaveBeenCalledWith('skills', 'source.list')
    expect(w.text()).toContain('acme')
  })

  it('开关总开关与源开关，失败提示', async () => {
    const w = await mountView()
    await gotoConfig(w)
    await (w.vm as any).toggleSkillsEnabled(false)
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('skills', 'config.update', { enabled: false })

    requestMock.mockImplementation((_m, cmd) => cmd === 'source.toggle' ? Promise.reject(new Error('x')) : defaultImpl(_m, cmd))
    await (w.vm as any).toggleSource('acme', false)
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('操作失败'))).toBe(true)
  })

  it('JSON 编辑器保存成功；非法 JSON 提示', async () => {
    const w = await mountView()
    await gotoConfig(w)
    const vm = w.vm as any
    await vm.openJsonEditor()
    expect(vm.configEditing).toBe(true)
    vm.configEditContent = '{"enabled": false}'
    await vm.saveConfig()
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('skills', 'config.save', { enabled: false })
    expect(useToast().toasts.some(t => t.message === '配置已保存')).toBe(true)
    expect(vm.configEditing).toBe(false)

    vm.configEditing = true
    vm.configEditContent = '{broken'
    await vm.saveConfig()
    expect(useToast().toasts.some(t => t.message.includes('保存失败'))).toBe(true)
  })
})

describe('SkillsView 源管理', () => {
  it('添加源：空 URL 拦截；成功清理并刷新', async () => {
    const w = await mountView()
    await gotoConfig(w)
    const vm = w.vm as any
    vm.addUrl = '  '
    await vm.addSource()
    expect(requestMock.mock.calls.filter(c => c[1] === 'source.add').length).toBe(0)

    vm.addUrl = ' https://github.com/acme/skills '
    await vm.addSource()
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('skills', 'source.add', { url: 'https://github.com/acme/skills' }, 0)
    expect(useToast().toasts.some(t => t.message.includes('已添加'))).toBe(true)
    expect(vm.showAddDialog).toBe(false)
    expect(vm.addUrl).toBe('')
  })

  it('添加源：partial 分支转手工弹窗并回填', async () => {
    requestMock.mockImplementation((_m, cmd) => cmd === 'source.add'
      ? Promise.resolve({ partial: { name: 'p1', repo: 'a/b' }, error: '探测失败' })
      : defaultImpl(_m, cmd))
    const w = await mountView()
    await gotoConfig(w)
    const vm = w.vm as any
    vm.addUrl = 'https://x'
    await vm.addSource()
    await flushPromises()
    expect(vm.showManualDialog).toBe(true)
    expect(vm.detectError).toBe('探测失败')
    expect(vm.manualForm.name).toBe('p1')
    expect(vm.manualForm.repo).toBe('a/b')
  })

  it('添加源失败与手工添加成功', async () => {
    const w = await mountView()
    await gotoConfig(w)
    const vm = w.vm as any
    requestMock.mockImplementation((_m, cmd) => cmd === 'source.add' ? Promise.reject(new Error('net')) : defaultImpl(_m, cmd))
    vm.addUrl = 'https://x'
    await vm.addSource()
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('添加失败'))).toBe(true)
    expect(vm.adding).toBe(false)

    await vm.addSourceManual()
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('skills', 'source.add.manual', vm.manualForm)
    expect(vm.showManualDialog).toBe(false)
    expect(vm.detectError).toBe('')
  })

  it('手工添加失败提示', async () => {
    requestMock.mockImplementation((_m, cmd) => cmd === 'source.add.manual' ? Promise.reject(new Error('net')) : defaultImpl(_m, cmd))
    const w = await mountView()
    await gotoConfig(w)
    await (w.vm as any).addSourceManual()
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('添加失败'))).toBe(true)
  })

  it('删除源：确认词不匹配拦截；匹配成功删除', async () => {
    const w = await mountView()
    await gotoConfig(w)
    const vm = w.vm as any
    await vm.removeSource('acme')
    expect(requestMock.mock.calls.filter(c => c[1] === 'source.remove').length).toBe(0)

    vm.confirmDelete = 'acme'
    await vm.removeSource('acme')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('skills', 'source.remove', { name: 'acme' })
    expect(useToast().toasts.some(t => t.message.includes('已删除'))).toBe(true)
    expect(vm.confirmDelete).toBe('')
  })

  it('删除源失败提示', async () => {
    requestMock.mockImplementation((_m, cmd) => cmd === 'source.remove' ? Promise.reject(new Error('locked')) : defaultImpl(_m, cmd))
    const w = await mountView()
    await gotoConfig(w)
    const vm = w.vm as any
    vm.confirmDelete = 'acme'
    await vm.removeSource('acme')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('删除失败'))).toBe(true)
  })
})

describe('SkillsView 学习技能', () => {
  it('空来源拦截；成功后 8 秒刷新列表', async () => {
    vi.useFakeTimers()
    const w = await mountView()
    const vm = w.vm as any
    vm.learnSource = '  '
    await vm.learnSkill()
    expect(requestMock.mock.calls.filter(c => c[1] === 'learn').length).toBe(0)

    vm.learnSource = ' acme/skills '
    vm.learnName = ' my-skill '
    await vm.learnSkill()
    expect(requestMock).toHaveBeenCalledWith('skills', 'learn', { source: 'acme/skills', name: 'my-skill' })
    expect(vm.showLearnDialog).toBe(false)
    expect(vm.learning).toBe(false)
    const installedCalls = requestMock.mock.calls.filter(c => c[1] === 'installed').length
    vi.advanceTimersByTime(8000)
    expect(requestMock.mock.calls.filter(c => c[1] === 'installed').length).toBe(installedCalls + 1)
  })

  it('学习失败提示错误', async () => {
    requestMock.mockImplementation((_m, cmd) => cmd === 'learn' ? Promise.reject(new Error('busy')) : defaultImpl(_m, cmd))
    const w = await mountView()
    const vm = w.vm as any
    vm.learnSource = 'x'
    await vm.learnSkill()
    expect(useToast().toasts.some(t => t.message.includes('学习失败'))).toBe(true)
    expect(vm.learning).toBe(false)
  })
})

describe('SkillsView 页签切换', () => {
  it('config 页签触发加载；shop 页签首次进入触发 browse', async () => {
    const w = await mountView()
    const vm = w.vm as any
    vm.browseResults = []
    await gotoConfig(w)
    expect(requestMock).toHaveBeenCalledWith('skills', 'config.get')
    await gotoShop(w)
    expect(requestMock.mock.calls.filter(c => c[1] === 'browse').length).toBeGreaterThan(0)
    // 再次切换不重复 browse（已有结果）
    const calls = requestMock.mock.calls.filter(c => c[1] === 'browse').length
    vm.activeTab = 'installed'
    await gotoShop(w)
    expect(requestMock.mock.calls.filter(c => c[1] === 'browse').length).toBe(calls)
  })
})
