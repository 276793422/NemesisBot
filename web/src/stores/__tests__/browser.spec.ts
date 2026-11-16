/**
 * browser store（2026-11-15 浏览器接管，计划 D10/D15）回归钉：
 * - WSAPI 面契约：sessions/tabs/attach/release/close/logs/detect 的
 *   module/cmd/data 形态（handlers/browser.rs 命令清单对齐）；
 * - attach/release 的本地 working 指针即时反馈（点名不等地快照就有徽标）；
 * - spawn 的 kind/executable 按值透传（auto 不带字段 = 后端解析序兜底）；
 * - ingestEvent 头部并入 + 400 条上限（SSE browser_event 实时投影）；
 * - settings 读写走 config.get/config.set_field 双字段（browser.kind +
 *   browser.executable），custom 空路径由视图层拦；非 custom kind 落盘清空
 *   executable 由 store 归一（F47：stale 路径在解析序里压过 kind）。
 */
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'

const wsapiRequest = vi.fn()

vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: wsapiRequest }),
}))

import { useBrowserStore } from '../browser'

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
    ...overrides,
  }
}

beforeEach(() => {
  setActivePinia(createPinia())
  wsapiRequest.mockReset()
})

describe('browser store WSAPI 面契约', () => {
  it('fetchSessions 走 browser.sessions 并投影列表', async () => {
    wsapiRequest.mockResolvedValue({ sessions: [sessionSummary()] })
    const store = useBrowserStore()
    await store.fetchSessions()
    expect(wsapiRequest).toHaveBeenCalledWith('browser', 'sessions')
    expect(store.sessions).toHaveLength(1)
    expect(store.sessions[0].kind_name).toBe('Edge')
  })

  it('fetchTabs 按会话缓存 tab 列表', async () => {
    wsapiRequest.mockResolvedValue({
      session_id: 's1',
      working_tab_id: 't2',
      tabs: [
        { target_id: 't1', title: 'A', url: 'http://a.com', attached: false, working: false, load_state: 'detached', operating: false },
        { target_id: 't2', title: 'B', url: 'http://b.com', attached: true, working: true, load_state: 'complete', operating: false },
      ],
    })
    const store = useBrowserStore()
    await store.fetchTabs('s1')
    expect(wsapiRequest).toHaveBeenCalledWith('browser', 'tabs', { session_id: 's1' })
    expect(store.tabsBySession.s1).toHaveLength(2)
    expect(store.tabsBySession.s1[1].working).toBe(true)
  })

  it('spawn kind=auto 不带字段（后端解析序兜底）、显式 kind 透传', async () => {
    // 调用序：spawn → sessions（spawn 内刷列表）×3 组；spawn 组恒返回会话摘要。
    wsapiRequest.mockImplementation(async (_m: string, cmd: string) =>
      cmd === 'spawn' ? { session: sessionSummary() } : { sessions: [] },
    )
    const store = useBrowserStore()
    await store.spawn()
    expect(wsapiRequest).toHaveBeenNthCalledWith(1, 'browser', 'spawn', {})
    await store.spawn('chrome')
    expect(wsapiRequest).toHaveBeenNthCalledWith(3, 'browser', 'spawn', { kind: 'chrome' })
    await store.spawn(undefined, 'C:/x.exe')
    expect(wsapiRequest).toHaveBeenNthCalledWith(5, 'browser', 'spawn', { executable: 'C:/x.exe' })
  })

  it('attach 更新本地 working 徽标并附 session_id+target_id', async () => {
    wsapiRequest.mockReset()
    wsapiRequest.mockImplementation(async (_m: string, cmd: string) => {
      if (cmd === 'tabs') {
        return {
          session_id: 's1',
          tabs: [
            { target_id: 't1', title: 'A', url: 'http://a.com', attached: false, working: true, load_state: 'complete', operating: false },
            { target_id: 't2', title: 'B', url: 'http://b.com', attached: false, working: false, load_state: 'detached', operating: false },
          ],
        }
      }
      return {}
    })
    const store = useBrowserStore()
    await store.fetchTabs('s1')
    await store.attach('s1', 't2')
    expect(wsapiRequest).toHaveBeenCalledWith('browser', 'attach', { session_id: 's1', target_id: 't2' })
    expect(store.tabsBySession.s1[0].working).toBe(false)
    expect(store.tabsBySession.s1[1].working).toBe(true)
  })

  it('release 清空全部 working 徽标', async () => {
    wsapiRequest.mockImplementation(async (_m: string, cmd: string) => {
      if (cmd === 'tabs') {
        return {
          session_id: 's1',
          tabs: [{ target_id: 't1', title: 'A', url: 'http://a.com', attached: true, working: true, load_state: 'complete', operating: false }],
        }
      }
      return {}
    })
    const store = useBrowserStore()
    await store.fetchTabs('s1')
    await store.release('s1')
    expect(wsapiRequest).toHaveBeenCalledWith('browser', 'release', { session_id: 's1' })
    expect(store.tabsBySession.s1[0].working).toBe(false)
  })

  it('closeSession 带 session_id 并刷新列表', async () => {
    wsapiRequest.mockResolvedValue({ ok: true, graceful: true, killed: false })
    const store = useBrowserStore()
    await store.closeSession('s1')
    expect(wsapiRequest).toHaveBeenCalledWith('browser', 'close', { session_id: 's1' })
  })

  it('fetchLogs 透传 session_id 过滤与 limit', async () => {
    wsapiRequest.mockResolvedValue({ logs: [] })
    const store = useBrowserStore()
    await store.fetchLogs('s1', 50)
    expect(wsapiRequest).toHaveBeenCalledWith('browser', 'logs', { session_id: 's1', limit: 50 })
    await store.fetchLogs()
    expect(wsapiRequest).toHaveBeenLastCalledWith('browser', 'logs', { limit: 200 })
  })
})

describe('browser store 设置与实时事件', () => {
  it('fetchSettings 读 config.browser 段并给 auto 缺省', async () => {
    // BUG-20261116-017：config.get 的 wire 形状 = 配置根对象直出
    // （protocol.rs response_ok 的 data = handler 返回值，无 {config} 包裹；
    // BridgeTab/SettingsView/PluginsView 等全部直读根字段）——旧 mock 包裹
    // 形漏掉了 store 读 r?.config?.browser 的契约漂移。
    wsapiRequest.mockResolvedValue({ browser: { kind: 'edge', executable: '' } })
    const store = useBrowserStore()
    await store.fetchSettings()
    expect(store.settings).toEqual({ kind: 'edge', executable: '' })
    wsapiRequest.mockResolvedValue({})
    await store.fetchSettings()
    expect(store.settings).toEqual({ kind: 'auto', executable: '' })
  })

  it('saveSettings 双字段 set_field（kind + executable）', async () => {
    wsapiRequest.mockResolvedValue({})
    const store = useBrowserStore()
    await store.saveSettings({ kind: 'custom', executable: 'C:/b.exe' })
    expect(wsapiRequest).toHaveBeenNthCalledWith(1, 'config', 'set_field', { path: 'browser.kind', value: 'custom' })
    expect(wsapiRequest).toHaveBeenNthCalledWith(2, 'config', 'set_field', { path: 'browser.executable', value: 'C:/b.exe' })
    expect(store.settings.kind).toBe('custom')
  })

  it('F47：非 custom kind 保存时清空 executable（stale 路径压过 kind）', async () => {
    // 场景：先选 custom 填路径 P 保存，再改选 edge 保存——视图隐藏输入框
    // 仍持有 P，落盘不清空 = 解析序「配置 executable > 配置 kind」让 P 静默
    // 压过 edge（UI 显示 edge、实际打开 P）。归一在 store 单点；custom 形态
    // 不回归（路径原样透传）。
    wsapiRequest.mockResolvedValue({})
    const store = useBrowserStore()
    await store.saveSettings({ kind: 'edge', executable: 'C:/stale.exe' })
    expect(wsapiRequest).toHaveBeenNthCalledWith(1, 'config', 'set_field', { path: 'browser.kind', value: 'edge' })
    expect(wsapiRequest).toHaveBeenNthCalledWith(2, 'config', 'set_field', { path: 'browser.executable', value: '' })
    expect(store.settings).toEqual({ kind: 'edge', executable: '' })
    await store.saveSettings({ kind: 'custom', executable: 'C:/b.exe' })
    expect(wsapiRequest).toHaveBeenNthCalledWith(4, 'config', 'set_field', { path: 'browser.executable', value: 'C:/b.exe' })
  })

  it('ingestEvent 头部并入 + 400 条上限', () => {
    const store = useBrowserStore()
    for (let i = 0; i < 410; i++) {
      store.ingestEvent({ ts_ms: i, session_id: 's1', action: 'spawn', target: '', ok: true, detail: '', duration_ms: 1 })
    }
    expect(store.logs).toHaveLength(400)
    expect(store.logs[0].ts_ms).toBe(409)
  })
})
