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

const P1 = { id: 'p1', name: 'Nemesis', emoji: '🤖', division: '开发', description: '主力人格', installed: false, source: 'agency', recommended_for: [] }
const P2 = { id: 'p2', name: 'Guardian', emoji: '🛡️', division: '安全', description: '守卫人格', installed: true, source: 'agency', recommended_for: ['安全'] }
// builtin 条目（2026-11-15 独立页拍板）：不走全部页 browse 网格，由
// 「内置人格」专列 browse{source:'builtin'} 供给。
const PB = { id: 'bug-analyst', name: 'Bug Analyst', emoji: '🔍', division: '内置', description: '根因分析纪律人格', installed: false, source: 'builtin', recommended_for: ['BUG 分析'] }

// 服务端策展橱窗（browse 响应 recommend_sections）：ghost ref 模拟防死链
// 丢弃（agency/ghost-x 不在 items 里，行内不渲染卡片）。
const REC_SECTIONS = [
  { key: 'dev', title: '开发', tier: 'normal', division: '开发', refs: ['agency/p1'] },
  { key: 'bug-analysis', title: 'BUG 分析', tier: null, division: null, refs: ['agency/ghost-x', 'builtin/bug-analyst'] },
]

function defaultImpl(_m: string, cmd: string, p?: any) {
  switch (cmd) {
    case 'shop.browse':
      // 内置专列独立臂（source:'builtin'）与全部页/推荐臂分流
      if (p?.source === 'builtin') return Promise.resolve({ items: [PB], source: 'live' })
      return Promise.resolve({ items: p?.division ? [P2] : [P1, P2, PB], recommend_sections: REC_SECTIONS })
    case 'shop.search':
      return Promise.resolve({ items: [P2] })
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
  // 共享夹具复位（2026-11-15 七轮复查）：就地补丁类生产路径（download/hire
  // 成功后翻转 shopItems/builtinItems 安装态）会改写模块级夹具对象——逐用例
  // 复位防跨用例泄漏（实录：内置下载用例翻转 PB 后，后续聘用按钮可用性断言
  // 读到 installed=true 的 builtin 卡）。
  P1.installed = false
  P2.installed = true
  PB.installed = false
})

async function mountView() {
  const w = mount(PersonaShopView)
  await flushPromises()
  return w
}

/// 切到「全部人格」页（推荐页是默认 TAB——搜索/过滤/网格测试的前置）。
async function mountAllView() {
  const w = await mountView()
  await w.findAll('button.tab').find(b => b.text() === '全部人格')!.trigger('click')
  await flushPromises()
  return w
}

describe('PersonaShopView 推荐橱窗（默认 TAB）', () => {
  it('默认落在推荐 TAB：行结构、tier 注记、防死链丢卡、空行不渲染', async () => {
    const w = await mountView()
    expect((w.vm as any).activeTab).toBe('recommend')
    const rows = w.findAll('.rec-section')
    expect(rows.length).toBe(2)
    // 行 1：开发（六职能，tier=normal → 建议模型注记 + 查看该类全部）
    expect(rows[0].text()).toContain('开发')
    expect(rows[0].text()).toContain('建议模型 标准档')
    expect(rows[0].find('.rec-view-all').exists()).toBe(true)
    // 行 1 卡片 = agency/p1（ghost ref 不渲染）
    expect(rows[0].findAll('.shop-card').length).toBe(1)
    expect(rows[0].text()).toContain('Nemesis')
    // 行 2：BUG 分析（方向行，无 tier 注记、无查看全部跳转）
    expect(rows[1].text()).toContain('BUG 分析')
    expect(rows[1].text()).not.toContain('建议模型')
    expect(rows[1].find('.rec-view-all').exists()).toBe(false)
    // ghost-x 防死链丢弃 → 只剩 builtin 卡
    expect(rows[1].findAll('.shop-card').length).toBe(1)
    expect(rows[1].text()).toContain('Bug Analyst')
    // 全部页内容不在推荐页渲染（全部页网格滤 builtin 后为 2 条）
    expect(w.text()).not.toContain('共 2 个人格')
  })

  it('「查看该类全部 →」切全部页并预置 division 过滤', async () => {
    const w = await mountView()
    await w.find('.rec-view-all').trigger('click')
    await flushPromises()
    expect((w.vm as any).activeTab).toBe('all')
    expect(requestMock).toHaveBeenCalledWith('persona', 'shop.browse', { division: '开发' })
    expect(w.text()).toContain('共 1 个人格')
    expect(w.text()).toContain('守卫人格')
  })

  it('无策展数据显示空态指引', async () => {
    requestMock.mockImplementation((_m, cmd) => cmd === 'shop.browse'
      ? Promise.resolve({ items: [P1] })
      : Promise.resolve({}))
    const w = await mountView()
    expect(w.text()).toContain('暂无推荐内容')
  })

  it('推荐行卡片与全部页同权：点击下载带 source', async () => {
    const w = await mountView()
    await w.findAll('.rec-section')[0].find('.shop-card .btn-primary').trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('persona', 'shop.download', { id: 'p1', source: 'agency' })
  })

  it('挂载先 refresh 再 browse（两 TAB 同一次 browse 供给）', async () => {
    await mountView()
    expect(requestMock).toHaveBeenCalledWith('persona', 'shop.refresh', {})
    expect(requestMock).toHaveBeenCalledWith('persona', 'shop.browse', { division: undefined })
  })
})

describe('PersonaShopView 全部人格页', () => {
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
    await w.findAll('button.tab').find(b => b.text() === '全部人格')!.trigger('click')
    await flushPromises()
    expect(w.text()).toContain('共 2 个人格')
  })

  it('browse 失败提示', async () => {
    requestMock.mockImplementation((_m, cmd) => cmd === 'shop.browse' ? Promise.reject(new Error('net')) : Promise.resolve({}))
    await mountView()
    expect(useToast().toasts.some(t => t.message.includes('加载失败'))).toBe(true)
  })

  it('空关键词搜索回落 browse；有关键词走 shop.search（不掏空推荐橱窗数据）', async () => {
    const w = await mountAllView()
    ;(w.vm as any).searchQuery = '  '
    await (w.vm as any).search()
    expect(requestMock.mock.calls.filter(c => c[1] === 'shop.search').length).toBe(0)
    expect(requestMock.mock.calls.filter(c => c[1] === 'shop.browse').length).toBe(2)

    ;(w.vm as any).searchQuery = 'guard'
    await (w.vm as any).search()
    expect(requestMock).toHaveBeenCalledWith('persona', 'shop.search', { query: 'guard' })
    // 搜索只换全部页渲染列表，shopItems（推荐橱窗数据源）保持 browse 全量
    expect((w.vm as any).items.length).toBe(1)
    expect((w.vm as any).shopItems.length).toBe(3)
    expect((w.vm as any).recommendRows.length).toBe(2)
  })

  it('搜索失败提示；回车触发搜索', async () => {
    requestMock.mockImplementation((_m, cmd) => cmd === 'shop.search' ? Promise.reject(new Error('boom')) : Promise.resolve({}))
    const w = await mountAllView()
    await w.find('input.form-input').setValue('x')
    await w.find('input.form-input').trigger('keyup.enter')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('搜索失败'))).toBe(true)
  })

  it('部门筛选触发 browse；「内置」pill 退场（内置独立页拍板）', async () => {
    const w = await mountAllView()
    const pillTexts = w.findAll('.filter-pill').map(p => p.text())
    expect(pillTexts).not.toContain('内置')
    await w.findAll('.filter-pill').find(p => p.text() === '安全')!.trigger('click')
    expect(requestMock).toHaveBeenCalledWith('persona', 'shop.browse', { division: '安全' })
    await w.findAll('.filter-pill').find(p => p.text() === '全部')!.trigger('click')
    expect(requestMock).toHaveBeenCalledWith('persona', 'shop.browse', { division: undefined })
  })

  it('division 过滤态不覆写 shopItems：切回推荐 TAB 橱窗行完整不裁剪（2026-11-15 七轮复查修）', async () => {
    // 回归钉：过滤态 browse 的 items 是子集，覆写 shopItems 会让推荐橱窗
    // ref 解析（byRef）失联——pill 点一下再切回来，指向其他 division 的
    // 推荐行静默集体消失。shopItems 只承载未过滤全量（与 search 口径对齐）。
    const w = await mountAllView()
    expect((w.vm as any).shopItems.length).toBe(3) // 未过滤全量 P1+P2+PB
    await w.findAll('.filter-pill').find(p => p.text() === '安全')!.trigger('click')
    await flushPromises()
    expect((w.vm as any).items.length).toBe(1) // 过滤子集
    expect((w.vm as any).shopItems.length).toBe(3) // 全量集未被覆写
    await w.findAll('button.tab').find(b => b.text() === '推荐')!.trigger('click')
    await flushPromises()
    // 两行推荐全部存活（开发行 agency/p1 + BUG 分析行 builtin/bug-analyst，
    // 二者都不在「安全」子集里——修复前 rows 归零）
    const rows = w.findAll('.rec-section')
    expect(rows.length).toBe(2)
    expect(rows[0].text()).toContain('Nemesis')
    expect(rows[1].text()).toContain('Bug Analyst')
  })

  it('division 过滤态下载后就地补推荐卡安装态（不等下次全量 browse）', async () => {
    const w = await mountAllView()
    await w.findAll('.filter-pill').find(p => p.text() === '安全')!.trigger('click')
    await flushPromises()
    const p2 = (w.vm as any).shopItems.find((x: any) => x.id === 'p2')
    expect(p2.installed).toBe(true) // fixture 预置 installed: true
    p2.installed = false
    await (w.vm as any).downloadPersona(p2)
    await flushPromises()
    // 过滤态 browse（division=安全）后 shopItems 保持全量集，下载的卡就地翻转
    expect((w.vm as any).shopItems.find((x: any) => x.id === 'p2').installed).toBe(true)
    expect((w.vm as any).items.find((x: any) => x.id === 'p2').installed).toBe(true)
  })

  it('来源徽标：live = · 在线；快照响应 = 内置快照（label）+ 警示色类', async () => {
    const w = await mountView()
    const liveBadge = w.find('.shop-source-badge')
    expect(liveBadge.text()).toBe('Agency Agents · 在线')
    expect(liveBadge.classes()).not.toContain('shop-source-snapshot')

    requestMock.mockImplementation((_m, cmd) => cmd === 'shop.browse'
      ? Promise.resolve({ items: [P1], source: 'snapshot', snapshot_info: { commit: 'abc1234', fetched_at: '2026-11-11T00:00:00Z', file_count: 274, label: 'abc1234 @ 2026-11-11' } })
      : Promise.resolve({}))
    await (w.vm as any).browse()
    await flushPromises()
    const badge = w.find('.shop-source-badge')
    expect(badge.text()).toBe('Agency Agents · 内置快照（abc1234 @ 2026-11-11）')
    expect(badge.classes()).toContain('shop-source-snapshot')
    expect((w.vm as any).dataSource).toBe('snapshot')
  })

  it('全部页网格过滤 builtin：内置条目不进全部页（agency 条目照常）', async () => {
    const w = await mountAllView()
    const cardTexts = w.findAll('.shop-card').map(c => c.text())
    expect(cardTexts.some(t => t.includes('Bug Analyst'))).toBe(false)
    expect(w.text()).toContain('共 2 个人格')
    expect(w.text()).toContain('守卫人格')
  })

  // --- 内置人格专列（2026-11-15 独立页拍板） ---

  it('内置卡片不再出现在全部页网格（回归钉：旧混排形态退场）', async () => {
    // shopItems 仍含 PB（推荐行跨源引用消费），但全部页渲染列表必须滤净
    const w = await mountAllView()
    expect((w.vm as any).shopItems.length).toBe(3)
    expect((w.vm as any).allPageItems.length).toBe(2)
  })
})

describe('PersonaShopView 内置人格页（独立专列）', () => {
  async function mountBuiltinView() {
    const w = await mountView()
    await w.findAll('button.tab').find(b => b.text() === '内置人格')!.trigger('click')
    await flushPromises()
    return w
  }

  it('首次进入懒加载 browse{source:builtin}；网格渲染 + 内置徽标', async () => {
    const w = await mountBuiltinView()
    expect(requestMock).toHaveBeenCalledWith('persona', 'shop.browse', { source: 'builtin' })
    expect(w.text()).toContain('共 1 个人格')
    const card = w.findAll('.shop-card').find(c => c.text().includes('Bug Analyst'))!
    expect(card.text()).toContain('内置')
    // 推荐来源角标跨页保留（recommended_for 照常渲染）
    expect(card.find('.rec-chip').exists()).toBe(true)
    expect(card.find('.rec-chip').text()).toBe('BUG 分析')
    // 推荐臂只发一次 browse（mount 时），builtin 臂一次——两臂分流
    expect(requestMock.mock.calls.filter(c => c[1] === 'shop.browse').length).toBe(2)
  })

  it('已加载再切回不重拉（懒加载守卫）；刷新按钮 force 重拉', async () => {
    const w = await mountBuiltinView()
    expect(requestMock.mock.calls.filter(c => c[1] === 'shop.browse').length).toBe(2)
    await w.findAll('button.tab').find(b => b.text() === '全部人格')!.trigger('click')
    await w.findAll('button.tab').find(b => b.text() === '内置人格')!.trigger('click')
    await flushPromises()
    expect(requestMock.mock.calls.filter(c => c[1] === 'shop.browse').length).toBe(2)
    await w.findAll('button').find(b => b.text() === '刷新')!.trigger('click')
    await flushPromises()
    expect(requestMock.mock.calls.filter(c => c[1] === 'shop.browse').length).toBe(3)
  })

  it('客户端过滤：名称/描述关键词命中与落空，零服务端请求', async () => {
    const w = await mountBuiltinView()
    await w.find('input.form-input').setValue('根因')
    await flushPromises()
    expect(w.text()).toContain('共 1 个人格')
    await w.find('input.form-input').setValue('zzz-no-match')
    await flushPromises()
    expect(w.text()).toContain('没有匹配的内置人格')
    expect(requestMock.mock.calls.filter(c => c[1] === 'shop.search').length).toBe(0)
  })

  it('拉取失败不显假空态：无「共 0 个人格」/「没有匹配」，显拉取指引', async () => {
    // 逐行复查回归钉（2026-11-15）：挂载刷新窗口吞 loadBuiltin 或拉取
    // 失败 → builtinLoaded=false，不得显「共 0 个人格」+「没有匹配的
    // 内置人格」假空态（3df3f89e 全部页同类教训），显未加载指引。
    requestMock.mockImplementation((_m, cmd, p) =>
      p?.source === 'builtin' && cmd === 'shop.browse'
        ? Promise.reject(new Error('network down'))
        : defaultImpl(_m, cmd, p))
    const w = await mountView()
    await w.findAll('button.tab').find(b => b.text() === '内置人格')!.trigger('click')
    await flushPromises()
    expect(w.text()).not.toContain('共 0 个人格')
    expect(w.text()).not.toContain('没有匹配的内置人格')
    expect(w.text()).toContain('尚未加载')
  })

  it('内嵌兜底响应：琥珀警示注记 + 上游标签；live 响应无注记', async () => {
    const w = await mountBuiltinView()
    expect(w.find('.builtin-offline-note').exists()).toBe(false)
    expect((w.vm as any).builtinSource).toBe('live')
    requestMock.mockImplementation((_m, cmd, p) => p?.source === 'builtin' && cmd === 'shop.browse'
      ? Promise.resolve({ items: [PB], source: 'embedded', snapshot_info: { label: '编译期内嵌模板（3 个人格）' } })
      : defaultImpl(_m, cmd, p))
    await (w.vm as any).loadBuiltin(true)
    await flushPromises()
    const note = w.find('.builtin-offline-note')
    expect(note.exists()).toBe(true)
    expect(note.text()).toContain('上游不可达')
    expect(note.text()).toContain('编译期内嵌模板（3 个人格）')
    expect((w.vm as any).builtinSource).toBe('embedded')
  })

  it('下载 builtin 人格后已加载的内置列表同步刷新安装态', async () => {
    const w = await mountBuiltinView()
    const calls = requestMock.mock.calls.filter(c => c[1] === 'shop.browse').length
    await w.findAll('.shop-card').find(c => c.text().includes('Bug Analyst'))!.find('.btn-primary').trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('persona', 'shop.download', { id: 'bug-analyst', source: 'builtin' })
    // 下载后 browse（agency 列表刷新）+ builtin force 重拉，各一次
    expect(requestMock.mock.calls.filter(c => c[1] === 'shop.browse').length).toBe(calls + 2)
  })
})

describe('PersonaShopView 预览', () => {
  it('点击卡片打开预览：数据合并、原始/转换双页签', async () => {
    const w = await mountAllView()
    await w.findAll('.shop-card').find(c => c.text().includes('Nemesis'))!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('persona', 'shop.preview', { id: 'p1', source: 'agency' })
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
    const w = await mountAllView()
    await w.findAll('.shop-card').find(c => c.text().includes('Nemesis'))!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('加载预览失败'))).toBe(true)

    await w.findAll('.shop-card').find(c => c.text().includes('Guardian'))!.trigger('click')
    await flushPromises()
    await w.findAll('.modal-footer button').find(b => b.text() === '关闭')!.trigger('click')
    expect((w.vm as any).previewPersona).toBe(null)
  })

  it('预览加载中关窗：迟到响应/失败都不重开弹窗（八轮复查修）', async () => {
    // 回归钉：× 与遮罩点击在 previewLoading 期未禁用，shop.preview 在线拉取
    // 可达 30s 超时窗——关窗后迟到回填会把已关闭的弹窗无端重开。
    let settlePreview!: (v: any) => void
    let failPreview!: (e: any) => void
    requestMock.mockImplementation((_m, cmd) => {
      if (cmd === 'shop.preview') return new Promise((res, rej) => { settlePreview = res; failPreview = rej })
      return defaultImpl(_m, cmd, {})
    })
    const w = await mountAllView()
    await w.findAll('.shop-card').find(c => c.text().includes('Nemesis'))!.trigger('click')
    await flushPromises()
    expect((w.vm as any).previewLoading).toBe(true)
    await w.find('.modal-close').trigger('click')
    expect((w.vm as any).previewPersona).toBe(null)
    // 迟到成功响应：不重开
    settlePreview({ name: 'Late', installed: true, raw: '# x', converted: {} })
    await flushPromises()
    expect((w.vm as any).previewPersona).toBe(null)

    // 第二轮：关窗后迟到失败——不重开、不弹错误 toast（用户已取消关心）
    useToast().toasts.splice(0)
    await w.findAll('.shop-card').find(c => c.text().includes('Nemesis'))!.trigger('click')
    await flushPromises()
    expect((w.vm as any).previewLoading).toBe(true) // 第二轮在途
    await w.find('.modal-close').trigger('click')
    failPreview(new Error('late fail'))
    await flushPromises()
    expect((w.vm as any).previewPersona).toBe(null)
    expect(useToast().toasts.some(t => t.message.includes('加载预览失败'))).toBe(false)
  })

  it('无转换数据显示空态；原始内容缺失显示暂无内容', async () => {
    requestMock.mockImplementation((_m, cmd) => cmd === 'shop.preview' ? Promise.resolve({ name: 'x' }) : defaultImpl(_m, cmd, {}))
    const w = await mountAllView()
    await w.findAll('.shop-card').find(c => c.text().includes('Nemesis'))!.trigger('click')
    await flushPromises()
    expect(w.text()).toContain('暂无内容')
    await w.findAll('.preview-tab').find(b => b.text() === '转换预览')!.trigger('click')
    expect(w.text()).toContain('暂无转换数据')
  })
})

describe('PersonaShopView 下载与聘用', () => {
  it('下载成功：提示、预览态置 installed、刷新列表', async () => {
    const w = await mountAllView()
    await w.findAll('.shop-card').find(c => c.text().includes('Nemesis'))!.trigger('click')
    await flushPromises()
    const calls = requestMock.mock.calls.filter(c => c[1] === 'shop.browse').length
    await w.findAll('.modal-footer button').find(b => b.text() === '下载')!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('persona', 'shop.download', { id: 'p1', source: 'agency' })
    expect(useToast().toasts.some(t => t.message.includes('下载成功'))).toBe(true)
    expect((w.vm as any).previewPersona.installed).toBe(true)
    expect(requestMock.mock.calls.filter(c => c[1] === 'shop.browse').length).toBe(calls + 1)
  })

  it('下载失败提示；卡片下载按钮', async () => {
    requestMock.mockImplementation((_m, cmd) => cmd === 'shop.download' ? Promise.reject(new Error('deny')) : defaultImpl(_m, cmd, {}))
    const w = await mountAllView()
    await w.findAll('.shop-card').find(c => c.text().includes('Nemesis'))!.find('.btn-primary').trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('下载失败'))).toBe(true)
    expect((w.vm as any).downloading).toBe('')
  })

  it('集群聘用成功（带 note）关预览；失败提示', async () => {
    // 预览合并后 installed=true，聘用按钮才可用
    requestMock.mockImplementation((_m, cmd) => cmd === 'shop.preview' ? Promise.resolve({ ...defaultImpl(_m, cmd, {}), installed: true }) : defaultImpl(_m, cmd, {}))
    const w = await mountAllView()
    await w.findAll('.shop-card').find(c => c.text().includes('Guardian'))!.trigger('click')
    await flushPromises()
    await w.findAll('.modal-footer button').find(b => b.text().includes('集群聘用'))!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('persona', 'shop.hire', { id: 'p2', source: 'agency' })
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
    const w = await mountAllView()
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

  it('雇佣专家不依赖安装态恒可用（原文在线直拉，无需先下载）；无 cluster 也恒渲染', async () => {
    // 原文直用改版（2026-11-15）：原文本地缺席时 shop.hire_expert 自动在线
    // 拉取并保存，雇佣不再要求先下载——按钮对未安装人格同样可点。
    const w = await mountAllView()
    const nemesisBtn = w.findAll('.shop-card').find(c => c.text().includes('Nemesis'))!
      .findAll('button').find(b => b.text().includes('雇佣专家'))!
    expect(nemesisBtn.attributes('disabled')).toBeUndefined()
    const guardBtn = w.findAll('.shop-card').find(c => c.text().includes('Guardian'))!
      .findAll('button').find(b => b.text().includes('雇佣专家'))!
    expect(guardBtn.attributes('disabled')).toBeUndefined()
  })

  it('集群聘用按钮：builtin 免下载直聘可用；agency 未安装禁用（七轮复查修）', async () => {
    // 回归钉：后端 shop.hire builtin 臂支持免下载直聘用（自动取材物化），
    // 此前卡片按钮按 installed 一刀切禁用——特性在 UI 上不可达。
    // builtin 卡在推荐橱窗行渲染（全部页网格滤 builtin）。
    const w = await mountView()
    const bugCard = w.findAll('.shop-card').find(c => c.text().includes('Bug Analyst'))!
    const bugHireBtn = bugCard.findAll('button').find(b => b.text().includes('集群聘用'))!
    expect(bugHireBtn.attributes('disabled')).toBeUndefined()
    expect(bugHireBtn.attributes('title')).toContain('免下载直聘')

    // agency 未安装卡（Nemesis）在全部页网格：维持「先下载后聘用」
    await w.findAll('button.tab').find(b => b.text() === '全部人格')!.trigger('click')
    await flushPromises()
    const nemesisCard = w.findAll('.shop-card').find(c => c.text().includes('Nemesis'))!
    const nemesisHireBtn = nemesisCard.findAll('button').find(b => b.text().includes('集群聘用'))!
    expect(nemesisHireBtn.attributes('disabled')).toBeDefined()
    expect(nemesisHireBtn.attributes('title')).toBe('请先下载')
  })

  it('预览弹窗集群聘用同卡同权：builtin 未安装免下载直聘可点（八轮复查修）', async () => {
    // 回归钉：七轮修卡片聘用闸时漏了预览弹窗同款按钮——builtin 未安装人格
    // 经预览路径仍不可聘。现卡片与弹窗共用 isHireEnabled/hireTitleOf 单一
    // 真相源，两臂必然一致。
    const w = await mountView()
    await w.findAll('.shop-card').find(c => c.text().includes('Bug Analyst'))!.trigger('click')
    await flushPromises()
    const builtinBtn = w.findAll('.modal-footer button').find(b => b.text().includes('集群聘用'))!
    expect(builtinBtn.attributes('disabled')).toBeUndefined()
    expect(builtinBtn.attributes('title')).toContain('免下载直聘')

    // agency 未安装（Nemesis）：弹窗内维持「请先下载」
    await w.find('.modal-close').trigger('click')
    await w.findAll('.shop-card').find(c => c.text().includes('Nemesis'))!.trigger('click')
    await flushPromises()
    const agencyBtn = w.findAll('.modal-footer button').find(b => b.text().includes('集群聘用'))!
    expect(agencyBtn.attributes('disabled')).toBeDefined()
    expect(agencyBtn.attributes('title')).toBe('请先下载')
  })

  it('builtin 免下载直聘用成功后就地翻转安装态（shopItems + builtinItems）', async () => {
    // builtin 臂回克隆对象（服务端两次响应本就是不同实例）——补丁必须独立
    // 翻转两套陈列集，而非共享引用的隐式生效。
    requestMock.mockImplementation((_m, cmd, p) => {
      if (p?.source === 'builtin' && cmd === 'shop.browse') return Promise.resolve({ items: [{ ...PB }], source: 'live' })
      return defaultImpl(_m, cmd, p)
    })
    const w = await mountAllView()
    await w.findAll('button.tab').find(b => b.text() === '内置人格')!.trigger('click')
    await flushPromises()
    const pb = (w.vm as any).shopItems.find((x: any) => x.id === 'bug-analyst')
    expect(pb.installed).toBe(false)
    await (w.vm as any).hirePersona(pb)
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('已聘入集群'))).toBe(true)
    expect((w.vm as any).shopItems.find((x: any) => x.id === 'bug-analyst').installed).toBe(true)
    expect((w.vm as any).builtinItems.find((x: any) => x.id === 'bug-analyst').installed).toBe(true)
  })

  it('预览弹窗内雇佣专家成功关预览', async () => {
    // 预览合并 installed 才可用（默认 preview 响应 installed=false，需覆盖）
    requestMock.mockImplementation((_m, cmd) =>
      cmd === 'shop.preview' ? Promise.resolve({ ...defaultImpl(_m, cmd, {}), installed: true }) : defaultImpl(_m, cmd, {}))
    const w = await mountAllView()
    await w.findAll('.shop-card').find(c => c.text().includes('Guardian'))!.trigger('click')
    await flushPromises()
    await w.findAll('.modal-footer button').find(b => b.text().includes('雇佣专家'))!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('persona', 'shop.hire_expert', { id: 'p2' })
    expect((w.vm as any).previewPersona).toBe(null)
    expect((w.vm as any).hiringExpert).toBe('')
  })
})
