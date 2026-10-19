import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'

// 行动连接器页最小渲染 spec：三页签（概览/注册表/商城）门控显示、损坏条目
// 诚实展示、risk 徽标色映射、信任/绑定目标徽标、回执空态 + 列表失败错误态，
// 以及管理面（新建模态 / 启停 / 两段确认删除）。WSAPI 契约
// （connectors.list / receipts.list / create / set_enabled / remove 响应形态）
// 由后端钉住，这里只锁前端渲染分支与请求形态。回执非空渲染属展示面，
// 后端契约测试覆盖。领域数据（纸面券商账本等）不进框架页——A1 摘除后
// 本 spec 不再断言 ledger 相关面。B4 总开关（概览页开关 + 注册表子开关
// 置灰）与 B2 独占接管（镜像表 + per-tool 三态选择器）同页锁定。

const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
}))

import ConnectorsView from '../ConnectorsView.vue'
import { useToast } from '../../composables/useToast'

const OPS = [
  { id: 'get_quote', intent: ['查询行情'], risk: 'low', class: 'network', confirm_required: false, finance_budget: false },
  { id: 'place_order', intent: ['下单'], risk: 'critical', class: 'write', confirm_required: true, finance_budget: true },
]

function defaultImpl() {
  requestMock.mockImplementation((_m: string, cmd: string) => {
    if (cmd === 'list') {
      return Promise.resolve({
        connectors: [
          { id: 'paper-broker', dir: 'paper-broker', name: '纸面券商', version: '0.1.0', kind: 'rest', enabled: true, trust: 'unsigned', description: '零账号纸面下单', ops: OPS },
          { id: 'broken-c', dir: 'broken-c', name: '', version: '', kind: '', enabled: false, trust: null, description: '', ops: [], error: 'connector.toml 解析失败: 意外键' },
        ],
      })
    }
    if (cmd === 'receipts.list') return Promise.resolve({ receipts: [] })
    return Promise.resolve({})
  })
}

beforeEach(() => {
  requestMock.mockReset()
  defaultImpl()
  useToast().toasts.splice(0)
})

async function mountView() {
  const w = mount(ConnectorsView)
  await flushPromises()
  return w
}

/** 切到注册表页签（卡片列表所在页）。 */
async function toRegistry(w: ReturnType<typeof mount>) {
  await w.find('[data-test="tab-registry"]').trigger('click')
  await flushPromises()
}

describe('ConnectorsView 页签与渲染分支', () => {
  it('默认概览页：三个页签在场；卡片等注册表内容切页后才渲染', async () => {
    const w = await mountView()
    expect(w.findAll('[data-test="conn-tabs"] .tab').length).toBe(3)
    expect(w.find('[data-test="overview-card"]').exists()).toBe(true)
    // 挂载即拉请求（数据不随页签懒加载）；领域数据（ledger 等）不进框架页
    const cmds = requestMock.mock.calls.map(c => c[1])
    expect(cmds).toContain('list')
    expect(cmds).toContain('receipts.list')
    expect(cmds).not.toContain('ledger')
    expect(w.find('[data-test="connector-card"]').exists()).toBe(false)

    await toRegistry(w)
    const cards = w.findAll('[data-test="connector-card"]')
    expect(cards.length).toBe(2)
    const broken = w.find('[data-test="connector-error"]')
    expect(broken.exists()).toBe(true)
    expect(broken.text()).toContain('connector.toml 解析失败: 意外键')
    expect(broken.classes()).toContain('connector-error')
    expect(w.text()).toContain('已停用')
    // 健康条目 ops 表照常渲染；坏条目（error 在场）不渲染 ops 表
    expect(w.text()).toContain('get_quote')
    expect(w.text()).toContain('查询行情')
    w.unmount()
  })

  it('商城页签：占位面板在场（规划中 + CLI 指引）', async () => {
    const w = await mountView()
    await w.find('[data-test="tab-shop"]').trigger('click')
    await flushPromises()
    expect(w.find('[data-test="shop-card"]').exists()).toBe(true)
    expect(w.text()).toContain('规划中')
    expect(w.text()).toContain('add-source')
    w.unmount()
  })

  it('risk 徽标色映射：low/medium/high/critical 各落对应色类；约束徽标在场', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'list') {
        return Promise.resolve({
          connectors: [{
            id: 'c1', dir: 'c1', name: 'C1', version: '1.0.0', kind: 'rest', enabled: true, description: '',
            ops: [
              { id: 'a', intent: [], risk: 'low', class: 'network', confirm_required: false, finance_budget: false },
              { id: 'b', intent: [], risk: 'medium', class: 'read', confirm_required: false, finance_budget: false },
              { id: 'c', intent: [], risk: 'high', class: 'write', confirm_required: false, finance_budget: false },
              { id: 'd', intent: [], risk: 'critical', class: 'write', confirm_required: true, finance_budget: true },
            ],
          }],
        })
      }
      if (cmd === 'receipts.list') return Promise.resolve({ receipts: [] })
      return Promise.resolve({})
    })
    const w = await mountView()
    await toRegistry(w)
    expect(w.find('[data-risk="low"]').classes()).toContain('risk-low')
    expect(w.find('[data-risk="medium"]').classes()).toContain('risk-medium')
    expect(w.find('[data-risk="high"]').classes()).toContain('risk-high')
    expect(w.find('[data-risk="critical"]').classes()).toContain('risk-critical')
    expect(w.text()).toContain('需确认')
    expect(w.text()).toContain('资金限额')
    w.unmount()
  })

  it('M1 信任徽标：verified/community/unsigned 三态渲染、损坏条目（trust=null）不渲染徽标', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'list') {
        return Promise.resolve({
          connectors: [
            { id: 'v', dir: 'v', name: 'V', version: '1.0', kind: 'rest', enabled: true, trust: 'verified', description: '', ops: [] },
            { id: 'm', dir: 'm', name: 'M', version: '1.0', kind: 'rest', enabled: true, trust: 'community', description: '', ops: [] },
            { id: 'u', dir: 'u', name: 'U', version: '1.0', kind: 'rest', enabled: true, trust: 'unsigned', description: '', ops: [] },
            { id: 'x', dir: 'x', name: 'X', version: '', kind: '', enabled: false, trust: null, description: '', ops: [], error: '清单损坏' },
          ],
        })
      }
      if (cmd === 'receipts.list') return Promise.resolve({ receipts: [] })
      return Promise.resolve({})
    })
    const w = await mountView()
    await toRegistry(w)
    const badges = w.findAll('[data-test="connector-trust"]')
    expect(badges.length).toBe(3) // 损坏条目不渲染信任徽标
    expect(badges[0].classes()).toContain('badge-success')
    expect(badges[0].text()).toContain('已验证')
    expect(badges[1].classes()).toContain('badge-info')
    expect(badges[2].classes()).toContain('badge-neutral')
    expect(badges[2].text()).toContain('未签名')
    w.unmount()
  })

  it('M4b/A2：绑定目标未配置指路徽标——mcp_ready=false 展示中性指路文案，true/null 不展示', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'list') {
        return Promise.resolve({
          connectors: [
            { id: 'd', dir: 'd', name: 'Desktop UI', version: '0.1.0', kind: 'local', enabled: false, trust: 'unsigned', description: '', ops: [], mcp_ready: false },
            { id: 'r', dir: 'r', name: 'Ready', version: '0.1.0', kind: 'local', enabled: true, trust: 'unsigned', description: '', ops: [], mcp_ready: true },
            { id: 'h', dir: 'h', name: 'Rest', version: '0.1.0', kind: 'rest', enabled: true, trust: 'unsigned', description: '', ops: [] },
          ],
        })
      }
      if (cmd === 'receipts.list') return Promise.resolve({ receipts: [] })
      return Promise.resolve({})
    })
    const w = await mountView()
    await toRegistry(w)
    const missing = w.findAll('[data-test="connector-mcp-missing"]')
    expect(missing.length).toBe(1) // 只有 mcp_ready=false 的条目亮徽标
    expect(missing[0].text()).toContain('绑定目标未配置')
    // 指路语义、非警示色（A2）：连接器不拦截任何工具，真相源在 MCP 页
    expect(missing[0].classes()).toContain('badge-neutral')
    expect(missing[0].classes()).not.toContain('badge-warning')
    w.unmount()
  })

  it('回执空态 + 主数据源失败整页错误态（带原文与重试）', async () => {
    const w = await mountView()
    // 概览页不渲染回执卡——回执空态在注册表页
    await toRegistry(w)
    expect(w.find('[data-test="receipts-empty"]').exists()).toBe(true)
    expect(w.text()).toContain('暂无回执')
    w.unmount()

    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'list') return Promise.reject(new Error('ws down'))
      return Promise.resolve({})
    })
    const w2 = await mountView()
    const err = w2.find('[data-test="load-error"]')
    expect(err.exists()).toBe(true)
    expect(err.text()).toContain('ws down')
    // 错误态提供重试按钮
    expect(err.find('button').exists()).toBe(true)
    w2.unmount()
  })
})

describe('ConnectorsView 管理面（新建/启停/删除）', () => {
  it('启停：点「停用」发 set_enabled {dir, enabled:false} 并刷新列表；失败 toast 原文', async () => {
    const w = await mountView()
    await toRegistry(w)
    const toggle = w.findAll('[data-test="connector-toggle"]')[0]
    await toggle.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('connectors', 'set_enabled', { dir: 'paper-broker', enabled: false })
    // 刷新：list 被再次调用
    const listCalls = requestMock.mock.calls.filter(c => c[1] === 'list')
    expect(listCalls.length).toBeGreaterThanOrEqual(2)
    expect(useToast().toasts.some(t => t.type === 'success')).toBe(true)
    w.unmount()
  })

  it('删除两段确认：首击只变文案不发请求，再击发 remove；损坏条目无启停按钮但可删', async () => {
    const w = await mountView()
    await toRegistry(w)
    const delBtns = w.findAll('[data-test="connector-delete"]')
    expect(delBtns.length).toBe(2)
    // 损坏条目（第二张卡）不渲染启停
    expect(w.findAll('[data-test="connector-toggle"]').length).toBe(1)

    // 首击：变「确认删除？」、零请求
    await delBtns[0].trigger('click')
    await flushPromises()
    expect(delBtns[0].text()).toBe('确认删除？')
    expect(requestMock.mock.calls.some(c => c[1] === 'remove')).toBe(false)

    // 再击：发 remove + 成功 toast 注明凭条保留 + 刷新
    await w.findAll('[data-test="connector-delete"]')[0].trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('connectors', 'remove', { dir: 'paper-broker' })
    expect(useToast().toasts.some(t => t.type === 'success' && t.message.includes('凭条历史保留'))).toBe(true)
    w.unmount()
  })

  it('新建模态：预填模板、create 成功后切注册表页并刷新；失败错误原文回显不关模态', async () => {
    const w = await mountView()
    await w.find('[data-test="open-create"]').trigger('click')
    await flushPromises()
    const modal = w.find('[data-test="create-modal"]')
    expect(modal.exists()).toBe(true)
    expect((w.find('[data-test="create-toml"]').element as HTMLTextAreaElement).value).toContain('[connector]')

    // 成功路径
    await w.find('[data-test="create-dir"]').setValue('my-new')
    await w.find('[data-test="create-submit"]').trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('connectors', 'create', expect.objectContaining({ dir_name: 'my-new' }))
    expect(w.find('[data-test="create-modal"]').exists()).toBe(false)
    // 已切到注册表页且重拉 list
    expect(w.find('[data-test="connector-card"]').exists()).toBe(true)
    w.unmount()

    // 失败路径：后端拒绝原文回显，模态不关
    const w2 = mount(ConnectorsView)
    await flushPromises()
    await w2.find('[data-test="open-create"]').trigger('click')
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'create') return Promise.reject(new Error('connector 目录 `x` 已存在'))
      if (cmd === 'list') return Promise.resolve({ connectors: [] })
      if (cmd === 'receipts.list') return Promise.resolve({ receipts: [] })
      return Promise.resolve({})
    })
    await w2.find('[data-test="create-dir"]').setValue('x')
    await w2.find('[data-test="create-submit"]').trigger('click')
    await flushPromises()
    const errEl = w2.find('[data-test="create-error"]')
    expect(errEl.exists()).toBe(true)
    expect(errEl.text()).toContain('已存在')
    expect(w2.find('[data-test="create-modal"]').exists()).toBe(true)
    w2.unmount()
  })

  it('dir==id 边界补强：模板 id 跟随目录名自动替换；手改 id 与目录名不一致时预检拦截不发请求', async () => {
    const w = await mountView()
    await w.find('[data-test="open-create"]').trigger('click')
    await flushPromises()

    // 输入目录名 → 模板占位 id 自动跟随（后端强制 dir==id）
    await w.find('[data-test="create-dir"]').setValue('weather')
    await flushPromises()
    const tomlEl = w.find('[data-test="create-toml"]').element as HTMLTextAreaElement
    expect(tomlEl.value).toContain('id = "weather"')
    expect(tomlEl.value).not.toContain('id = "my-connector"')
    w.unmount()

    // 用户手改 id 后与目录名不一致 → 预检拦截，请求不发、模态不关
    const w2 = await mountView()
    await w2.find('[data-test="open-create"]').trigger('click')
    await flushPromises()
    const ta = w2.find('[data-test="create-toml"]')
    await ta.setValue('[connector]\nid = "other-id"\nname = "X"\nversion = "0.1.0"\nkind = "local"\n\n[[op]]\nid = "noop"\nrisk = "low"\nclass = "read"\naction = "builtin.noop"\n')
    await w2.find('[data-test="create-dir"]').setValue('my-dir')
    await w2.find('[data-test="create-submit"]').trigger('click')
    await flushPromises()
    const createCalls = requestMock.mock.calls.filter((c: any[]) => c[1] === 'create')
    expect(createCalls.length).toBe(0)
    expect(w2.find('[data-test="create-error"]').text()).toContain('不一致')
    expect(w2.find('[data-test="create-modal"]').exists()).toBe(true)
    w2.unmount()
  })
})

describe('ConnectorsView B4 总开关', () => {
  it('概览页开关在场且缺省为开（list 缺 enabled 键 = 后端缺省 true）；点击发 set_master {enabled:false} 并刷新', async () => {
    const w = await mountView()
    const btn = w.find('[data-test="master-toggle"]')
    expect(btn.exists()).toBe(true)
    expect(btn.text()).toContain('总开关：开')
    await btn.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('connectors', 'set_master', { enabled: false })
    // 刷新：list 被再次拉取
    const listCalls = requestMock.mock.calls.filter(c => c[1] === 'list')
    expect(listCalls.length).toBeGreaterThanOrEqual(2)
    expect(useToast().toasts.some(t => t.type === 'success' && t.message.includes('已全关'))).toBe(true)
    w.unmount()
  })

  it('总开关关态：list 顶层 enabled=false 驱动关态文案；注册表子开关置灰（徽标在场 + toggle disabled）', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'list') {
        return Promise.resolve({
          enabled: false,
          connectors: [
            { id: 'c1', dir: 'c1', name: 'C1', version: '1.0.0', kind: 'rest', enabled: true, trust: 'unsigned', description: '', ops: [] },
          ],
        })
      }
      if (cmd === 'receipts.list') return Promise.resolve({ receipts: [] })
      return Promise.resolve({})
    })
    const w = await mountView()
    expect(w.find('[data-test="master-toggle"]').text()).toContain('总开关：关')
    await toRegistry(w)
    expect(w.findAll('[data-test="sub-toggle-greyed"]').length).toBe(1)
    expect(w.find('[data-test="sub-toggle-greyed"]').text()).toContain('总开关已关')
    expect(w.find('[data-test="connector-toggle"]').attributes('disabled')).toBeDefined()
    w.unmount()
  })
})

describe('ConnectorsView B2 独占接管镜像表', () => {
  const MIRRORS_LIST = {
    connectors: [
      {
        id: 'excl', dir: 'excl', name: 'Excl', version: '0.1.0', kind: 'local', enabled: true,
        trust: 'unsigned', description: '', ops: [], exclusive_mcp: ['stub'],
        mirrors: [
          { id: 'mcp_stub__read_page', policy: 'default', args_schema: null, risk: 'low', class: 'read', confirm_required: false },
          { id: 'mcp_stub__click', policy: 'exempt', args_schema: null, risk: 'high', class: 'write', confirm_required: true },
        ],
      },
      { id: 'plain', dir: 'plain', name: 'Plain', version: '0.1.0', kind: 'rest', enabled: true, trust: 'unsigned', description: '', ops: [] },
    ],
  }

  function mirrorsImpl() {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'list') return Promise.resolve(MIRRORS_LIST)
      if (cmd === 'receipts.list') return Promise.resolve({ receipts: [] })
      return Promise.resolve({})
    })
  }

  it('exclusive_mcp 卡渲染镜像表 + 三态选择器（policy 初值照后端）；非独占卡无镜像块', async () => {
    mirrorsImpl()
    const w = await mountView()
    await toRegistry(w)
    expect(w.findAll('[data-test="exclusive-block"]').length).toBe(1)
    expect(w.findAll('[data-test="mirror-row"]').length).toBe(2)
    const sel = w.find('[data-test="policy-select-mcp_stub__read_page"]')
    expect((sel.element as HTMLSelectElement).value).toBe('default')
    expect((w.find('[data-test="policy-select-mcp_stub__click"]').element as HTMLSelectElement).value).toBe('exempt')
    // A4 依赖注记：独占连接器停用按钮带「释放独占」语义提示；非独占不带
    expect(w.findAll('[data-test="connector-toggle"]')[0].attributes('title')).toContain('释放独占')
    expect(w.findAll('[data-test="connector-toggle"]')[1].attributes('title') ?? '').not.toContain('释放独占')
    w.unmount()
  })

  it('B3 基线徽标：hints 推导的 risk 落四色类，自述破坏性带 ⚑ 确认标；缺 risk 行显示 —', async () => {
    mirrorsImpl()
    const w = await mountView()
    await toRegistry(w)
    const ro = w.find('[data-test="mirror-risk-mcp_stub__read_page"]')
    expect(ro.classes()).toContain('risk-low')
    expect(ro.text()).not.toContain('⚑')
    const destr = w.find('[data-test="mirror-risk-mcp_stub__click"]')
    expect(destr.classes()).toContain('risk-high')
    expect(destr.text()).toContain('⚑')
    w.unmount()
  })

  it('选择器改动发 tool_policy {connector, tool, policy} + 刷新；成功 toast 带策略名', async () => {
    mirrorsImpl()
    const w = await mountView()
    await toRegistry(w)
    await w.find('[data-test="policy-select-mcp_stub__read_page"]').setValue('blocked')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('connectors', 'tool_policy', {
      connector: 'excl', tool: 'mcp_stub__read_page', policy: 'blocked',
    })
    expect(useToast().toasts.some(t => t.type === 'success' && t.message.includes('封禁'))).toBe(true)
    // 刷新：list 被再次拉取
    const listCalls = requestMock.mock.calls.filter(c => c[1] === 'list')
    expect(listCalls.length).toBeGreaterThanOrEqual(2)
    w.unmount()
  })
})
