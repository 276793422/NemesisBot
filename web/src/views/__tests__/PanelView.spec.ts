import { describe, it, expect, vi, beforeEach } from 'vitest'
import { flushPromises } from '@vue/test-utils'
import { createRouter, createMemoryHistory, type Router } from 'vue-router'

// PanelView（P2）：/panel/:route 容器——取数分块、动作执行、confirm 门、
// form 提交、未知路由诚实占位。
const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
  initWSAPI: vi.fn(),
}))
const toastSuccess = vi.fn()
const toastError = vi.fn()
vi.mock('../../composables/useToast', () => ({
  useToast: () => ({ success: toastSuccess, error: toastError, warn: vi.fn(), info: vi.fn() }),
}))
// echarts 按需装载 mock：vitest（jsdom 无 canvas）不拉真 echarts，只验容器
// 面取数/渲染接线；echarts 本体的装载与注册归 build 门（vite 构建验证拆包）。
vi.mock('../../components/panel/echartsLoader', () => ({
  loadECharts: () =>
    Promise.resolve({
      init: () => {
        const inst = {
          setOption: vi.fn(),
          resize: vi.fn(),
          dispose: vi.fn(),
        }
        ;(inst as any).__setOption = inst.setOption
        return inst
      },
      use: vi.fn(),
    }),
}))

import PanelView from '../../views/PanelView.vue'
import { usePanels } from '../../composables/usePanels'

const PANEL = {
  slug: 'panel-echo',
  name: '回显面板',
  payload: true,
  pages: [
    {
      route: 'echo',
      title: '回显面板',
      icon: 'puzzle',
      views: [
        {
          kind: 'table',
          bind: 'cmd://rows',
          columns: [
            { key: 'id', label: '编号' },
            { key: 'title', label: '标题' },
          ],
          fields: [],
          buttons: [],
        },
        {
          kind: 'actions',
          bind: '',
          columns: [],
          fields: [],
          buttons: [
            { label: '回显', command: 'echo', confirm: false, danger: false },
            { label: '危险重置', command: 'reset', confirm: true, danger: true },
          ],
        },
        {
          kind: 'form',
          bind: 'cmd://submit',
          columns: [],
          fields: [
            { key: 'text', label: '文本', field_type: 'string' },
            { key: 'num', label: '数量', field_type: 'int' },
          ],
          buttons: [],
        },
      ],
    },
  ],
}

const ROWS = { rows: [{ id: 'NB-1', title: '样例行' }] }

// flow:// 绑定面板（P3）：声明式（payload=false，无 wasm 载荷），取数全串
// 透传宿主流程引擎——前端不剥前缀不拆解。
const FLOW_PANEL = {
  slug: 'flow-panel',
  name: '流程面板',
  payload: false,
  pages: [
    {
      route: 'flow',
      title: '流程面板',
      icon: 'puzzle',
      views: [
        {
          kind: 'table',
          bind: 'flow://bugfix-pipeline/list',
          columns: [
            { key: 'instance', label: '实例' },
            { key: 'state', label: '状态' },
          ],
          fields: [],
          buttons: [],
        },
      ],
    },
  ],
}

// 引擎 list 投影真实 wire 形态（flow_engine.rs answer_list：{rows:[...]}，
// 行带 instance/state 等字段）——夹具镜像运行时，不从断言反推。
const FLOW_LIST = {
  rows: [{ instance: 'I-1', ext_key: 'BUG-1', title: '样例实例', state: 'running' }],
}

// chart 面板（2026-11-01 决策 A）：bind 取数 → PanelChart 渲染（echarts 装载
// mock 化，只验容器接线与空态/未知类型诚实面）。
const CHART_PANEL = {
  slug: 'chart-panel',
  name: '图表面板',
  payload: false,
  pages: [
    {
      route: 'chart',
      title: '图表面板',
      icon: 'puzzle',
      views: [
        {
          kind: 'chart',
          bind: 'flow://bugfix-pipeline/metrics',
          chart: 'line',
          x_key: 'day',
          columns: [],
          fields: [],
          buttons: [],
          series: [
            { key: 'count', label: '数量' },
            { key: 'score', label: '分数' },
          ],
        },
        {
          kind: 'chart',
          bind: 'flow://bugfix-pipeline/ghostchart',
          chart: 'treemap',
          x_key: 'day',
          columns: [],
          fields: [],
          buttons: [],
          series: [{ key: 'count', label: '数量' }],
        },
      ],
    },
  ],
}

const CHART_ROWS = {
  rows: [
    { day: '周一', count: 3, score: 8.5 },
    { day: '周二', count: 5, score: 7 },
  ],
}

/** 匹配 command 的 ui.invoke 调用列表（每个元素 = [module, cmd, data, timeout]）。 */
function invokeCalls(cmd: string): any[][] {
  return requestMock.mock.calls.filter(
    (c: any[]) => c[0] === 'plugins' && c[1] === 'ui.invoke' && c[2]?.command === cmd,
  )
}

function makeRouter(): Router {
  return createRouter({
    history: createMemoryHistory(),
    routes: [
      { path: '/', component: { template: '<div />' } },
      { path: '/panel/:route', component: PanelView },
    ],
  })
}

async function mountAt(route: string) {
  const router = makeRouter()
  await router.push(route)
  await router.isReady()
  const { mount } = await import('@vue/test-utils')
  const wrapper = mount(PanelView, { global: { plugins: [router] } })
  await flushPromises()
  return wrapper
}

beforeEach(() => {
  requestMock.mockReset()
  toastSuccess.mockReset()
  toastError.mockReset()
  usePanels().panels.value = [] // 面板单例复位（模块级状态跨用例残留）
  requestMock.mockImplementation((_module: string, cmd: string, data?: any) => {
    if (cmd === 'ui.registry') return Promise.resolve({ panels: [PANEL, FLOW_PANEL] })
    if (cmd === 'ui.invoke') {
      const command = data?.command
      if (command === 'rows') return Promise.resolve(ROWS)
      if (command === 'echo') return Promise.resolve({ ok: true, echo: data?.args?.text ?? '' })
      if (command === 'fail') return Promise.resolve({ error: '演示性业务失败' })
      if (command === 'flow://bugfix-pipeline/list') return Promise.resolve(FLOW_LIST)
      return Promise.resolve({ ok: true })
    }
    return Promise.resolve({})
  })
  vi.stubGlobal('confirm', vi.fn(() => true))
})

describe('PanelView', () => {
  it('渲染表格块：挂载即取数（table bind）并按声明列序渲染行', async () => {
    const wrapper = await mountAt('/panel/echo')
    expect(wrapper.find('[data-test="panel-view"]').exists()).toBe(true)
    expect(wrapper.text()).toContain('NB-1')
    expect(wrapper.text()).toContain('样例行')
    // 挂载取数：rows 一次；form 的 cmd:// 绝不能空参自动执行（真 bug 门）。
    const rowsCalls = invokeCalls('rows')
    expect(rowsCalls).toHaveLength(1)
    expect(rowsCalls[0][2]).toEqual({ slug: 'panel-echo', command: 'rows', args: {} })
    expect(invokeCalls('submit')).toHaveLength(0)
  })

  it('动作按钮：无 confirm 直发 invoke，成功 toast + 数据面重取', async () => {
    const wrapper = await mountAt('/panel/echo')
    await wrapper.find('[data-test="pa-btn-0"]').trigger('click')
    await flushPromises()
    expect(invokeCalls('echo')).toHaveLength(1)
    expect(invokeCalls('echo')[0][2]).toMatchObject({ slug: 'panel-echo', command: 'echo', args: {} })
    expect(toastSuccess).toHaveBeenCalled()
    // 成功后整页重取（rows 第二次）。
    expect(invokeCalls('rows').length).toBeGreaterThanOrEqual(2)
  })

  it('confirm 门：confirm=true 且拒绝 → 不发 invoke', async () => {
    const wrapper = await mountAt('/panel/echo')
    ;(globalThis.confirm as any).mockReturnValueOnce(false)
    await wrapper.find('[data-test="pa-btn-1"]').trigger('click')
    await flushPromises()
    expect(invokeCalls('reset')).toHaveLength(0)
    ;(globalThis.confirm as any).mockReturnValueOnce(true)
    await wrapper.find('[data-test="pa-btn-1"]').trigger('click')
    await flushPromises()
    expect(invokeCalls('reset')).toHaveLength(1)
  })

  it('业务失败（{"error":...}）：error toast，不重取', async () => {
    requestMock.mockImplementation((_m: string, cmd: string, data?: any) => {
      if (cmd === 'ui.registry') return Promise.resolve({ panels: [PANEL] })
      if (data?.command === 'rows') return Promise.resolve(ROWS)
      if (data?.command === 'echo') return Promise.resolve({ error: '演示性业务失败' })
      return Promise.resolve({ ok: true })
    })
    const wrapper = await mountAt('/panel/echo')
    toastError.mockClear()
    await wrapper.find('[data-test="pa-btn-0"]').trigger('click')
    await flushPromises()
    expect(toastError).toHaveBeenCalledWith(expect.stringContaining('演示性业务失败'))
  })

  it('form 提交：字段值整形上抛（int→Number）', async () => {
    const wrapper = await mountAt('/panel/echo')
    const text = wrapper.find('[data-test="pf-text"]')
    await text.setValue('hello')
    const num = wrapper.find('[data-test="pf-num"]')
    await num.setValue('42')
    await wrapper.find('[data-test="pf-submit"]').trigger('submit')
    await flushPromises()
    expect(invokeCalls('submit')).toHaveLength(1)
    expect(invokeCalls('submit')[0][2].args).toEqual({ text: 'hello', num: 42 })
  })

  it('未知路由：诚实占位（面板未找到）', async () => {
    const wrapper = await mountAt('/panel/ghost')
    expect(wrapper.find('[data-test="panel-missing"]').exists()).toBe(true)
    expect(wrapper.text()).toContain('面板未找到')
  })

  it('flow:// 绑定：全串透传宿主引擎（不剥前缀），声明式面板无载荷警告', async () => {
    const wrapper = await mountAt('/panel/flow')
    expect(wrapper.find('[data-test="panel-view"]').exists()).toBe(true)
    expect(wrapper.text()).toContain('I-1')
    // 契约锚：bind 全串逐字节作为 command 上行（宿主按 <template>/<view> 路由）。
    const calls = invokeCalls('flow://bugfix-pipeline/list')
    expect(calls).toHaveLength(1)
    expect(calls[0][2]).toEqual({
      slug: 'flow-panel',
      command: 'flow://bugfix-pipeline/list',
      args: {},
    })
    // 声明式（payload=false）+ 纯 flow 绑定 = 设计形态，不得显示载荷缺失警告。
    expect(wrapper.text()).not.toContain('载荷缺失')
  })

  it('声明式面板 actions 按钮全为 flow:// 命令：不显示载荷缺失警告（P4）', async () => {
    const flowActionsPanel = {
      slug: 'flow-actions-panel',
      name: '流程动作面板',
      payload: false,
      pages: [
        {
          route: 'flow-actions',
          title: '流程动作面板',
          icon: 'puzzle',
          views: [
            {
              kind: 'table',
              bind: 'flow://bugfix-pipeline/list',
              columns: [{ key: 'instance', label: '实例' }],
              fields: [],
              buttons: [],
            },
            {
              kind: 'actions',
              bind: '',
              fields: [],
              buttons: [{ label: '同步', command: 'flow://bugfix-pipeline/fetch', confirm: false, danger: false }],
            },
          ],
        },
      ],
    }
    requestMock.mockReset()
    requestMock.mockImplementation((_m: string, cmd: string, data?: any) => {
      if (cmd === 'ui.registry') return Promise.resolve({ panels: [flowActionsPanel] })
      if (data?.command === 'flow://bugfix-pipeline/list') return Promise.resolve(FLOW_LIST)
      return Promise.resolve({ ok: true })
    })
    const wrapper = await mountAt('/panel/flow-actions')
    expect(wrapper.text()).toContain('I-1')
    // flow:// 按钮是宿主代答（非 guest 依赖），纯声明式面板不得告警。
    expect(wrapper.text()).not.toContain('载荷缺失')
    await wrapper.find('[data-test="pa-btn-0"]').trigger('click')
    await flushPromises()
    expect(invokeCalls('flow://bugfix-pipeline/fetch')).toHaveLength(1)
  })

  it('注册表晚到：先空后到自动渲染（watch 触发）', async () => {
    requestMock.mockReset()
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'ui.registry') {
        return new Promise((resolve) => setTimeout(() => resolve({ panels: [PANEL] }), 10))
      }
      return Promise.resolve(ROWS)
    })
    const wrapper = await mountAt('/panel/echo')
    expect(wrapper.find('[data-test="panel-missing"]').exists()).toBe(true)
    await flushPromises()
    await new Promise((r) => setTimeout(r, 30))
    await flushPromises()
    expect(wrapper.find('[data-test="panel-view"]').exists()).toBe(true)
    expect(wrapper.text()).toContain('NB-1')
  })

  it('chart 视图：bind 取数 → 图表块渲染；未知图表类型诚实占位', async () => {
    requestMock.mockReset()
    requestMock.mockImplementation((_m: string, cmd: string, data?: any) => {
      if (cmd === 'ui.registry') return Promise.resolve({ panels: [CHART_PANEL] })
      if (data?.command === 'flow://bugfix-pipeline/metrics') return Promise.resolve(CHART_ROWS)
      return Promise.resolve({ rows: [] })
    })
    const wrapper = await mountAt('/panel/chart')
    const blocks = wrapper.findAll('[data-test="panel-chart"]')
    expect(blocks).toHaveLength(2)
    // 第一个块拿到数据（echarts mock 的 init/setOption 被调）——取数接线成立。
    await flushPromises()
    await new Promise((r) => setTimeout(r, 10))
    await flushPromises()
    // 第二个块：treemap 不在词表 → 诚实占位（不猜渲染）。
    expect(blocks[1].text()).toContain('未知图表类型：treemap')
    // bind 取数按 flow:// 全串透传（同 table 语义）。
    expect(invokeCalls('flow://bugfix-pipeline/metrics')).toHaveLength(1)
  })
})
