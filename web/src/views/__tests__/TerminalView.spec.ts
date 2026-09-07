import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'

// L8（2026-09-07）TerminalView 冒烟：挂载不炸 + /ws/pty URL 带 token +
// 信任级脚注诚实披露 + 状态随 WS open/close 翻转。xterm 在 jsdom 里量不出
// 尺寸（canvas/DOM 测量），两个 xterm 模块整体 mock 掉——行为契约在
// pty/tests.rs（真 PTY roundtrip）钉住，这里只验前端装配面。

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
})

describe('TerminalView', () => {
  it('挂载后连 /ws/pty 且 URL 带 localStorage token', async () => {
    const w = mount(TerminalView)
    await flushPromises()
    expect(FakeWebSocket.instances).toHaveLength(1)
    expect(FakeWebSocket.instances[0].url).toContain('/ws/pty')
    expect(FakeWebSocket.instances[0].url).toContain('token=tok-term')
    expect(termInstances).toHaveLength(1)
    w.unmount()
  })

  it('信任级脚注诚实披露：不逐命令审批 + 审计含密码回显', () => {
    const w = mount(TerminalView)
    const footer = w.find('.terminal-footer').text()
    expect(footer).toContain('不逐命令审批')
    expect(footer).toContain('密码')
    expect(footer).toContain('logs/terminal')
    w.unmount()
  })

  it('状态随 WS open/close 翻转', async () => {
    const w = mount(TerminalView)
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
