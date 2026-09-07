import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'

// L8（2026-09-07）TerminalView 冒烟：挂载不炸 + /ws/pty URL 带 token +
// 信任级脚注诚实披露 + 状态随 WS open/close 翻转。xterm 在 jsdom 里量不出
// 尺寸（canvas/DOM 测量），两个 xterm 模块整体 mock 掉——行为契约在
// pty/tests.rs（真 PTY roundtrip）钉住，这里只验前端装配面。
//
// token 来源契约（2026-09-08 回归修复）：/ws/pty 的 token 真相源是 auth
// store 的内存 token（本会话主 WS 用它通过认证）；plugin-ui WebView 经
// __DASHBOARD_TOKEN__ 注入认证、从不写 localStorage，只读 localStorage 会
// 拿到空/过期值 → 401「连接已断开」。localStorage 仅作兜底。

const termInstances: any[] = []
vi.mock('@xterm/xterm', () => ({
  Terminal: class {
    cols = 80
    rows = 24
    writeln = vi.fn()
    write = vi.fn()
    clear = vi.fn()
    dispose = vi.fn()
    loadAddon = vi.fn()
    open = vi.fn()
    onData = vi.fn()
    constructor() { termInstances.push(this) }
  },
}))
vi.mock('@xterm/addon-fit', () => ({
  FitAddon: class {
    fit = vi.fn()
  },
}))

import TerminalView from '../TerminalView.vue'
import { useAuthStore } from '../../stores/auth'

// 终端启用开关（2026-09-08）：config.get / config.set_field 走 useWSAPI——
// mock 掉以隔离全局 WS 装配，按用例控制 terminal.enabled 返回值。
// 注意路径：本文件在 views/__tests__/ 下，需两级 ../ 才到 src/composables。
const { requestMock } = vi.hoisted(() => ({ requestMock: vi.fn() }))
// initWSAPI 也要给：auth store（真 store，回归锁需要）→ useWebSocket 顶层
// 会调 initWSAPI(sendRaw)，mock 缺该导出会让整条 import 链炸掉。
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: requestMock }),
  initWSAPI: vi.fn(),
}))

class FakeWebSocket {
  static instances: FakeWebSocket[] = []
  static OPEN = 1
  static CONNECTING = 0
  readyState = 0
  binaryType = ''
  onopen: (() => void) | null = null
  onclose: (() => void) | null = null
  onmessage: ((ev: any) => void) | null = null
  onerror: (() => void) | null = null
  sent: (string | ArrayBuffer | Uint8Array)[] = []
  constructor(public url: string) {
    FakeWebSocket.instances.push(this)
  }
  send(data: string | ArrayBuffer | Uint8Array) { this.sent.push(data) }
  close() { this.readyState = 3; this.onclose?.() }
}

beforeEach(() => {
  termInstances.length = 0
  FakeWebSocket.instances.length = 0
  vi.stubGlobal('WebSocket', FakeWebSocket as any)
  // jsdom 无 ResizeObserver —— 桩掉（onMounted 里 observe 容器）
  vi.stubGlobal('ResizeObserver', class { observe() {} unobserve() {} disconnect() {} })
  localStorage.setItem('nemesisbot_auth_token', 'tok-term')
  // 默认「已启用」→ 挂载自动连接（旧行为）；开关相关用例各自覆盖
  requestMock.mockReset()
  requestMock.mockResolvedValue({ terminal: { enabled: true } })
})

// 每个用例独立 pinia（auth store 不跨用例泄漏）；setActivePinia 让用例内
// useAuthStore() 拿到与组件同一个 store 实例。
function mountView() {
  const pinia = createPinia()
  setActivePinia(pinia)
  return mount(TerminalView, { global: { plugins: [pinia] } })
}

describe('TerminalView', () => {
  it('挂载后连 /ws/pty 且 URL 带 token（store 空 → localStorage 兜底）', async () => {
    const w = mountView()
    await flushPromises()
    expect(FakeWebSocket.instances).toHaveLength(1)
    expect(FakeWebSocket.instances[0].url).toContain('/ws/pty')
    expect(FakeWebSocket.instances[0].url).toContain('token=tok-term')
    expect(termInstances).toHaveLength(1)
    w.unmount()
  })

  it('WebView 注入场景：auth store token 优先于 localStorage（401 回归锁）', async () => {
    // plugin-ui WebView 经 __DASHBOARD_TOKEN__ 注入认证、从不写 localStorage
    // ——旧实现只读 localStorage → 空/过期 token → 服务端 401「连接已断开」。
    localStorage.setItem('nemesisbot_auth_token', 'stale-tok')
    const w = mountView()
    // connect 在 loadTerminalConfig（异步）之后触发——此刻先把有效 token
    // 写进 store，模拟 WebView 已注入认证的真实时序。
    useAuthStore().token = 'store-tok'
    await flushPromises()

    expect(FakeWebSocket.instances).toHaveLength(1)
    const url = FakeWebSocket.instances[0].url
    expect(url).toContain('token=store-tok')
    expect(url).not.toContain('stale-tok')
    w.unmount()
  })

  it('信任级脚注诚实披露：不逐命令审批 + 审计含密码回显', () => {
    const w = mountView()
    const footer = w.find('.terminal-footer').text()
    expect(footer).toContain('不逐命令审批')
    expect(footer).toContain('密码')
    expect(footer).toContain('logs/terminal')
    w.unmount()
  })

  it('状态随 WS open/close 翻转', async () => {
    const w = mountView()
    await flushPromises()
    const ws = FakeWebSocket.instances[0]
    // 挂载即自动连接 → connecting；stub 不 open 则停在这个态
    expect(w.find('.terminal-status').text()).toBe('连接中…')
    ws.readyState = 1
    ws.onopen?.()
    await flushPromises()
    expect(w.find('.terminal-status').text()).toBe('已连接')
    ws.onclose?.()
    await flushPromises()
    expect(w.find('.terminal-status').text()).toBe('未连接')
    w.unmount()
  })
})

// -----------------------------------------------------------------------
// 终端启用开关（2026-09-08）：config.terminal.enabled → 挂载不自动连 +
// 未启用横幅；开关写 config.set_field（terminal.enabled 点路径），切换
// 即生效（后端 pty 闸每次升级 fresh-read config，契约在 pty/tests.rs）。
// -----------------------------------------------------------------------

describe('TerminalView 终端启用开关', () => {
  it('enabled=false：不自动连接 + 未启用横幅 + 状态「未启用」', async () => {
    requestMock.mockResolvedValue({ terminal: { enabled: false } })
    const w = mountView()
    await flushPromises()
    // 核心诉求：未启用时不发起 /ws/pty 连接（避免必然失败的重连噪音）
    expect(FakeWebSocket.instances).toHaveLength(0)
    expect(w.find('.terminal-disabled-banner').exists()).toBe(true)
    expect(w.find('.terminal-status').text()).toBe('未启用')
    const cb = w.find('.terminal-switch input')
    expect((cb.element as HTMLInputElement).checked).toBe(false)
    expect((cb.element as HTMLInputElement).disabled).toBe(false)
    w.unmount()
  })

  it('配置读取失败 → 退回旧行为（自动连接）', async () => {
    requestMock.mockRejectedValue(new Error('WS not initialized'))
    const w = mountView()
    await flushPromises()
    expect(FakeWebSocket.instances).toHaveLength(1)
    expect(w.find('.terminal-disabled-banner').exists()).toBe(false)
    w.unmount()
  })

  it('开关打开：写 config.set_field({terminal.enabled:true}) 后发起连接', async () => {
    requestMock.mockResolvedValue({ terminal: { enabled: false } })
    const w = mountView()
    await flushPromises()
    expect(FakeWebSocket.instances).toHaveLength(0)

    const cb = w.find('.terminal-switch input')
    await cb.setValue(true)
    await flushPromises()

    expect(requestMock).toHaveBeenCalledWith('config', 'set_field', { path: 'terminal.enabled', value: true })
    // 写入成功 → 立即连接
    expect(FakeWebSocket.instances).toHaveLength(1)
    expect(FakeWebSocket.instances[0].url).toContain('/ws/pty')
    // 横幅消失 + 状态芯片离开「未启用」
    expect(w.find('.terminal-disabled-banner').exists()).toBe(false)
    expect(w.find('.terminal-status').text()).not.toBe('未启用')
    w.unmount()
  })

  it('开关关闭：断开现有连接并写 terminal.enabled=false', async () => {
    const w = mountView()
    await flushPromises()
    const ws = FakeWebSocket.instances[0]
    ws.readyState = 1
    ws.onopen?.()
    await flushPromises()
    expect(w.find('.terminal-status').text()).toBe('已连接')

    const closeSpy = vi.spyOn(ws, 'close')
    const cb = w.find('.terminal-switch input')
    await cb.setValue(false)
    await flushPromises()

    expect(requestMock).toHaveBeenCalledWith('config', 'set_field', { path: 'terminal.enabled', value: false })
    expect(closeSpy).toHaveBeenCalled()
    w.unmount()
  })

  it('set_field 失败：toast 报错，连接状态不翻转', async () => {
    requestMock.mockResolvedValue({ terminal: { enabled: false } })
    const w = mountView()
    await flushPromises()
    requestMock.mockRejectedValueOnce(new Error('denied'))

    const cb = w.find('.terminal-switch input')
    await cb.setValue(true)
    await flushPromises()

    expect(FakeWebSocket.instances).toHaveLength(0)
    // 开关回弹为未选中（本地 state 未翻转）
    expect((cb.element as HTMLInputElement).checked).toBe(false)
    expect(w.find('.terminal-disabled-banner').exists()).toBe(true)
    w.unmount()
  })
})
