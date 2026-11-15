/**
 * 浏览器接管前端组件回归钉（2026-11-15，计划 D10/D15）：
 *
 * BrowserView（执行单元第四 TAB 面板）：
 * - 三子页签渲染与切换（列表/设置/日志）；
 * - 列表空态：empty-state + 打开浏览器按钮 → browser.spawn；
 * - 实例卡展开 → tab 行点选 → browser.attach {session_id, target_id}
 *   （点名即授权，圆点行点击语义）；
 * - 关闭按钮带 confirm，取消不发请求；
 * - 设置子页签：detect 卡片渲染 + 保存走 config.set_field + custom
 *   空路径前端拦截。
 *
 * BrowserPickPanel（ChatPanel 点名面板）：
 * - open=false 不渲染（单例 composable 开合）；
 * - open=true 渲染实例 + tab 行；点选行 → browser.attach。
 */
import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'

const wsapiRequest = vi.fn()

vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: wsapiRequest }),
}))

vi.mock('../../composables/useToast', () => ({
  useToast: () => ({
    toasts: [],
    info: vi.fn(),
    success: vi.fn(),
    warn: vi.fn(),
    error: vi.fn(),
    remove: vi.fn(),
  }),
}))

vi.mock('../../composables/useSSE', () => ({
  on: vi.fn(),
  off: vi.fn(),
  useSSE: () => ({
    connectEvents: vi.fn(),
    disconnectEvents: vi.fn(),
    on: vi.fn(),
    off: vi.fn(),
    sseReadyState: () => null,
  }),
}))

import { useBrowserStore } from '../../stores/browser'
import { useBrowserPanel } from '../../composables/useBrowserPanel'
import BrowserView from '../../views/BrowserView.vue'
import BrowserPickPanel from '../../components/chat/BrowserPickPanel.vue'

function sessionSummary(overrides: Record<string, unknown> = {}) {
  return {
    id: 's1',
    seq: 1,
    kind: 'edge',
    kind_name: 'Edge',
    phase2: false,
    executable: 'C:/edge.exe',
    profile: 'C:/home/browser/edge',
    port: 52841,
    started_at: '2026-11-15T00:00:00Z',
    pid: 123,
    working_tab: null,
    operating: false,
    alive: true,
    ...overrides,
  }
}

function tabsPayload(sessionId = 's1') {
  return {
    session_id: sessionId,
    working_tab_id: null,
    tabs: [
      { target_id: 't1', title: '淘宝购物车', url: 'https://taobao.com/cart', attached: false, working: false, load_state: 'detached', operating: false },
      { target_id: 't2', title: '搜索页', url: 'https://taobao.com/s', attached: false, working: false, load_state: 'detached', operating: false },
    ],
  }
}

function mountView() {
  return mount(BrowserView, { global: { plugins: [createPinia()] } })
}

beforeEach(() => {
  setActivePinia(createPinia())
  wsapiRequest.mockReset()
})

describe('BrowserView 三子页签与列表面', () => {
  it('默认列表子页签；三页签切换渲染', async () => {
    wsapiRequest.mockImplementation(async (_m: string, cmd: string) =>
      cmd === 'sessions' ? { sessions: [] } : {},
    )
    const w = mountView()
    expect(w.find('[data-test="browser-tab-list"]').exists()).toBe(true)
    expect(w.find('[data-test="browser-list"]').exists()).toBe(true)
    await w.find('[data-test="browser-tab-settings"]').trigger('click')
    expect(w.find('[data-test="browser-settings"]').exists()).toBe(true)
    await w.find('[data-test="browser-tab-logs"]').trigger('click')
    expect(w.find('[data-test="browser-logs"]').exists()).toBe(true)
  })

  it('空态渲染 + 打开浏览器按钮 → browser.spawn（无 kind）', async () => {
    wsapiRequest.mockImplementation(async (_m: string, cmd: string) =>
      cmd === 'spawn' ? { session: sessionSummary() } : { sessions: [], logs: [] },
    )
    const w = mountView()
    await flushPromises()
    expect(w.find('[data-test="browser-empty"]').exists()).toBe(true)
    await w.find('[data-test="browser-empty"] button').trigger('click')
    await flushPromises()
    const spawnCall = wsapiRequest.mock.calls.find((c) => c[1] === 'spawn')
    expect(spawnCall).toEqual(['browser', 'spawn', {}])
  })

  it('实例卡展开 → tab 行点选 → attach 带双 id', async () => {
    wsapiRequest.mockImplementation(async (_m: string, cmd: string) =>
      cmd === 'sessions'
        ? { sessions: [sessionSummary()] }
        : cmd === 'tabs'
          ? tabsPayload()
          : { session_id: 's1', tab: {} },
    )
    const w = mountView()
    await flushPromises()
    // 展开实例卡（点头部）。
    await w.find('.session-head').trigger('click')
    await flushPromises()
    const rows = w.findAll('[data-test="browser-tab-row"]')
    expect(rows).toHaveLength(2)
    // 点第一行 → attach {session_id: s1, target_id: t1}。
    await rows[0].trigger('click')
    await flushPromises()
    const attachCall = wsapiRequest.mock.calls.find((c) => c[1] === 'attach')
    expect(attachCall).toEqual(['browser', 'attach', { session_id: 's1', target_id: 't1' }])
  })

  it('F27：死实例卡渲染「已退出」置灰 + 不再显示标签页行（不冒充运行中）', async () => {
    wsapiRequest.mockImplementation(async (_m: string, cmd: string) =>
      cmd === 'sessions'
        ? { sessions: [sessionSummary({ alive: false })] }
        : cmd === 'tabs'
          ? tabsPayload()
          : { session_id: 's1', tab: {} },
    )
    const w = mountView()
    await flushPromises()
    // 折叠态：徽标「已退出」（全文提示在 title；展开后才有死态正文）。
    expect(w.find('[data-test="browser-session-dead"]').exists()).toBe(true)
    expect(w.find('[data-test="browser-session-dead"]').text()).toBe('已退出')
    // 展开死卡：死态文案 + 无 tab 行（stale 行不渲染，attach 无从误触）。
    await w.find('.session-head').trigger('click')
    await flushPromises()
    expect(w.text()).toContain('浏览器已退出')
    expect(w.findAll('[data-test="browser-tab-row"]')).toHaveLength(0)
  })

  it('关闭按钮 confirm 取消 → 不发 close', async () => {
    vi.spyOn(window, 'confirm').mockReturnValue(false)
    wsapiRequest.mockImplementation(async (_m: string, cmd: string) =>
      cmd === 'sessions' ? { sessions: [sessionSummary()] } : {},
    )
    const w = mountView()
    await flushPromises()
    await w.find('[data-test="browser-close"]').trigger('click')
    await flushPromises()
    expect(wsapiRequest.mock.calls.some((c) => c[1] === 'close')).toBe(false)
  })

  it('关闭按钮 confirm 通过 → browser.close {session_id}', async () => {
    vi.spyOn(window, 'confirm').mockReturnValue(true)
    wsapiRequest.mockImplementation(async (_m: string, cmd: string) =>
      cmd === 'sessions' ? { sessions: [sessionSummary()] } : { ok: true },
    )
    const w = mountView()
    await flushPromises()
    await w.find('[data-test="browser-close"]').trigger('click')
    await flushPromises()
    const closeCall = wsapiRequest.mock.calls.find((c) => c[1] === 'close')
    expect(closeCall).toEqual(['browser', 'close', { session_id: 's1' }])
  })
})

describe('BrowserView 设置子页签', () => {
  it('detect 卡片渲染 + 保存双字段 + custom 空路径拦截', async () => {
    wsapiRequest.mockImplementation(async (m: string, cmd: string) => {
      if (m === 'config' && cmd === 'get') {
        return { config: { browser: { kind: 'auto', executable: '' } } }
      }
      if (m === 'browser' && cmd === 'detect') {
        return {
          browsers: [
            { kind: 'edge', name: 'Edge', path: 'C:/edge.exe', phase2: false, source: 'app_paths' },
            { kind: 'firefox', name: 'Firefox', path: null, phase2: true, source: 'none' },
          ],
        }
      }
      return { sessions: [], logs: [] }
    })
    const w = mountView()
    await w.find('[data-test="browser-tab-settings"]').trigger('click')
    await flushPromises()
    expect(w.find('[data-test="browser-kind-edge"]').exists()).toBe(true)
    expect(w.find('[data-test="browser-kind-firefox"]').exists()).toBe(true)
    // custom 空路径：点卡片 → 保存被拦（无 config.set_field 调用）。
    await w.find('[data-test="browser-kind-custom"]').trigger('click')
    await w.find('[data-test="browser-save-settings"]').trigger('click')
    await flushPromises()
    expect(wsapiRequest.mock.calls.some((c) => c[0] === 'config' && c[1] === 'set_field')).toBe(false)
    // 填路径后保存 → 双 set_field。
    wsapiRequest.mockResolvedValue({})
    await w.find('[data-test="browser-custom-path"]').setValue('C:/x/b.exe')
    await w.find('[data-test="browser-save-settings"]').trigger('click')
    await flushPromises()
    const setCalls = wsapiRequest.mock.calls.filter((c) => c[0] === 'config' && c[1] === 'set_field')
    expect(setCalls).toHaveLength(2)
    expect(setCalls[0]).toEqual(['config', 'set_field', { path: 'browser.kind', value: 'custom' }])
    expect(setCalls[1]).toEqual(['config', 'set_field', { path: 'browser.executable', value: 'C:/x/b.exe' }])
  })
})

describe('BrowserPickPanel（ChatPanel 点名面板）', () => {
  it('open=false 不渲染；open=true 渲染面板', async () => {
    setActivePinia(createPinia())
    const panel = useBrowserPanel()
    const w = mount(BrowserPickPanel, { global: { plugins: [createPinia()] } })
    expect(w.find('[data-test="browser-pick-panel"]').exists()).toBe(false)
    panel.setOpen(true)
    await flushPromises()
    expect(w.find('[data-test="browser-pick-panel"]').exists()).toBe(true)
  })

  it('有实例时渲染 tab 行；点选行 → attach', async () => {
    setActivePinia(createPinia())
    useBrowserPanel().setOpen(true)
    wsapiRequest.mockImplementation(async (_m: string, cmd: string) =>
      cmd === 'sessions'
        ? { sessions: [sessionSummary()] }
        : cmd === 'tabs'
          ? tabsPayload()
          : { session_id: 's1' },
    )
    const w = mount(BrowserPickPanel, { global: { plugins: [createPinia()] } })
    await flushPromises()
    const rows = w.findAll('.browserpick-row')
    expect(rows.length).toBe(2)
    await rows[1].trigger('click')
    await flushPromises()
    const attachCall = wsapiRequest.mock.calls.find((c) => c[1] === 'attach')
    expect(attachCall).toEqual(['browser', 'attach', { session_id: 's1', target_id: 't2' }])
  })

  it('F26：多实例时 tab 面刷新全部会话（旧实现只刷 s1，s2 恒「读取标签页中…」）', async () => {
    setActivePinia(createPinia())
    useBrowserPanel().setOpen(true)
    wsapiRequest.mockImplementation(async (_m: string, cmd: string, data?: any) =>
      cmd === 'sessions'
        ? {
            sessions: [
              sessionSummary(),
              sessionSummary({ id: 's2', seq: 2, kind: 'chrome', kind_name: 'Google Chrome' }),
            ],
          }
        : cmd === 'tabs'
          ? tabsPayload(data?.session_id ?? 's1')
          : { session_id: data?.session_id },
    )
    const w = mount(BrowserPickPanel, { global: { plugins: [createPinia()] } })
    await flushPromises()
    const tabCalls = wsapiRequest.mock.calls.filter((c) => c[1] === 'tabs')
    const ids = tabCalls.map((c) => c[2]?.session_id)
    expect(ids).toContain('s1')
    expect(ids).toContain('s2')
    // 第二实例的 tab 行真实渲染（每实例 2 行 → 共 4 行）。
    expect(w.findAll('.browserpick-row').length).toBe(4)
  })

  it('F27：死实例在点名面板渲染「已退出」且不轮询其 tabs', async () => {
    setActivePinia(createPinia())
    useBrowserPanel().setOpen(true)
    wsapiRequest.mockImplementation(async (_m: string, cmd: string, data?: any) =>
      cmd === 'sessions'
        ? { sessions: [sessionSummary({ alive: false })] }
        : cmd === 'tabs'
          ? tabsPayload(data?.session_id ?? 's1')
          : { session_id: data?.session_id },
    )
    const w = mount(BrowserPickPanel, { global: { plugins: [createPinia()] } })
    await flushPromises()
    // 死实例不触发 tabs 拉取（拉取必失败被吞——恒「读取中」假象的根因）。
    expect(wsapiRequest.mock.calls.filter((c) => c[1] === 'tabs')).toHaveLength(0)
    // 死实例行渲染「已退出」态（无 tab 行可点）。
    expect(w.text()).toContain('浏览器已退出')
    expect(w.findAll('.browserpick-row')).toHaveLength(0)
  })

  it('无实例空态提供打开按钮 → browser.spawn', async () => {
    setActivePinia(createPinia())
    useBrowserPanel().setOpen(true)
    wsapiRequest.mockImplementation(async (_m: string, cmd: string) =>
      cmd === 'spawn' ? { session: sessionSummary() } : { sessions: [] },
    )
    const w = mount(BrowserPickPanel, { global: { plugins: [createPinia()] } })
    await flushPromises()
    const btn = w.find('.browserpick-empty button')
    expect(btn.exists()).toBe(true)
    await btn.trigger('click')
    await flushPromises()
    expect(wsapiRequest.mock.calls.some((c) => c[1] === 'spawn')).toBe(true)
  })
})
