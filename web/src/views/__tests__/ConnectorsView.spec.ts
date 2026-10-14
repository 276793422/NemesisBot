import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'

// 行动连接器页（2026-10-14 AI 原生行动代理）最小渲染 spec：损坏条目诚实
// 展示、risk 徽标色映射、回执空态 + 列表失败错误态。WSAPI 契约
// （connectors.list / connectors.receipts.list / paperbroker.ledger 响应形态）由后端钉住，这里只锁前端
// 渲染分支。回执非空渲染与纸面账本属展示面，后端契约测试覆盖。

const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
}))

import ConnectorsView from '../ConnectorsView.vue'

const OPS = [
  { id: 'get_quote', intent: ['查询行情'], risk: 'low', class: 'network', confirm_required: false, finance_budget: false },
  { id: 'place_order', intent: ['下单'], risk: 'critical', class: 'write', confirm_required: true, finance_budget: true },
]

function defaultImpl() {
  requestMock.mockImplementation((_m: string, cmd: string) => {
    if (cmd === 'list') {
      return Promise.resolve({
        connectors: [
          { id: 'paper-broker', name: '纸面券商', version: '0.1.0', kind: 'rest', enabled: true, description: '零账号纸面下单', ops: OPS },
          { id: 'broken-c', name: '', version: '', kind: '', enabled: false, description: '', ops: [], error: 'connector.toml 解析失败: 意外键' },
        ],
      })
    }
    if (cmd === 'receipts.list') return Promise.resolve({ receipts: [] })
    if (cmd === 'ledger') return Promise.resolve({ cash: 999000, positions: [], orders: [] })
    return Promise.resolve({})
  })
}

beforeEach(() => {
  requestMock.mockReset()
  defaultImpl()
})

async function mountView() {
  const w = mount(ConnectorsView)
  await flushPromises()
  return w
}

describe('ConnectorsView 渲染分支', () => {
  it('契约：挂载即发 connectors.list/receipts.list/paperbroker.ledger 三请求；损坏条目红字诚实展示 error 原文不隐藏；停用条目展示「已停用」', async () => {
    const w = await mountView()
    const cmds = requestMock.mock.calls.map(c => c[1])
    expect(cmds).toContain('list')
    expect(cmds).toContain('receipts.list')
    expect(cmds).toContain('ledger')

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

  it('risk 徽标色映射：low/medium/high/critical 各落对应色类；约束徽标在场', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'list') {
        return Promise.resolve({
          connectors: [{
            id: 'c1', name: 'C1', version: '1.0.0', kind: 'rest', enabled: true, description: '',
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
      if (cmd === 'ledger') return Promise.resolve({ cash: 0, positions: [], orders: [] })
      return Promise.resolve({})
    })
    const w = await mountView()
    expect(w.find('[data-risk="low"]').classes()).toContain('risk-low')
    expect(w.find('[data-risk="medium"]').classes()).toContain('risk-medium')
    expect(w.find('[data-risk="high"]').classes()).toContain('risk-high')
    expect(w.find('[data-risk="critical"]').classes()).toContain('risk-critical')
    expect(w.text()).toContain('需确认')
    expect(w.text()).toContain('资金限额')
    w.unmount()
  })

  it('回执空态 + 主数据源失败整页错误态（带原文与重试）', async () => {
    const w = await mountView()
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
