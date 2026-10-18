// F1 跟进回归钉（2026-10-18）：autoLogin 认证会话下 SSE URL 必须携带 token。
//
// 事故：桌面构建托盘/CLI 打开的 Dashboard 走插件窗口 __DASHBOARD_TOKEN__ 注入
//（或 URL hash token / 桌面 runtime 事件）→ auth.autoLogin 成功但从不写
// localStorage（TerminalView 注释自证的设计事实）→ useSSE 只读 localStorage →
// EventSource 生成不带 ?token= 的请求 → REST 统一鉴权（2026-09-22）恒 401。
// EventSource 对非 200 fail-fast 关闭且不自动重试，Logs 页「正在连接 SSE…」
// 永挂；同刻 WS 携 connect() 注入的内存 token 正常——「WS 连上、SSE 401」
// 即此缺陷的现场指纹。修复：useSSE 与 httpGet 同源（useWebSocket 模块级
// token，currentAuthToken() 只读视图），localStorage 兜底浏览器手动登录路径。
//
// 本 spec 钉住四个面：注入会话带 token；无 token 既有形态不变；内存 token
// 优先于 localStorage 残留；保留字符正确编码。

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'

class FakeEventSource {
  static CLOSED = 2
  static instances: FakeEventSource[] = []
  url: string
  readyState = 1
  onopen: ((ev?: unknown) => void) | null = null
  onerror: ((ev?: unknown) => void) | null = null
  constructor(url: string) {
    this.url = url
    FakeEventSource.instances.push(this)
  }
  addEventListener() {}
  close() {
    this.readyState = FakeEventSource.CLOSED
  }
}

class FakeWebSocket {
  static instances: FakeWebSocket[] = []
  url: string
  readyState = 0
  onopen: ((ev?: unknown) => void) | null = null
  onclose: ((ev?: unknown) => void) | null = null
  onerror: ((ev?: unknown) => void) | null = null
  onmessage: ((ev?: unknown) => void) | null = null
  constructor(url: string) {
    this.url = url
    FakeWebSocket.instances.push(this)
  }
  send() {}
  close() {}
}

vi.mock('../../lib/appBase', () => ({
  apiUrl: (p: string) => `http://test.local${p}`,
  wsUrl: (p: string) => `ws://test.local${p}`,
}))
vi.mock('../../wsResponseHandler', () => ({ handleWSResponse: () => false }))
vi.mock('../../useWSAPI', () => ({ initWSAPI: () => {} }))

beforeEach(() => {
  vi.resetModules()
  FakeEventSource.instances = []
  FakeWebSocket.instances = []
  vi.stubGlobal('EventSource', FakeEventSource as unknown as typeof EventSource)
  vi.stubGlobal('WebSocket', FakeWebSocket as unknown as typeof WebSocket)
  localStorage.clear()
})

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('useSSE 会话 token 来源（autoLogin 注入会话回归钉）', () => {
  it('connect 先注入内存 token（autoLogin 链序）→ EventSource URL 带 ?token=', async () => {
    const ws = await import('../useWebSocket')
    const sse = await import('../useSSE')

    ws.connect(null, 'tok-abc') // auth.ts autoLogin：connect(null, testToken) 先于 connectEvents()
    sse.connectEvents()

    expect(FakeEventSource.instances[0].url).toBe(
      'http://test.local/api/events/stream?token=tok-abc'
    )
  })

  it('无内存 token 且 localStorage 空 → URL 不带参数（对照：修复前 401 死循环形态）', async () => {
    const sse = await import('../useSSE')

    sse.connectEvents()

    expect(FakeEventSource.instances[0].url).toBe('http://test.local/api/events/stream')
  })

  it('内存 token 优先于 localStorage 残留（换号/登出残留不盖会话令牌）', async () => {
    const ws = await import('../useWebSocket')
    const sse = await import('../useSSE')

    localStorage.setItem('nemesisbot_auth_token', 'stale-local')
    ws.connect(null, 'tok-new')
    sse.connectEvents()

    expect(FakeEventSource.instances[0].url).toContain('token=tok-new')
    expect(FakeEventSource.instances[0].url).not.toContain('stale-local')
  })

  it('localStorage 兜底（浏览器手动登录路径既有行为不变）', async () => {
    const sse = await import('../useSSE')

    localStorage.setItem('nemesisbot_auth_token', 'tok-local')
    sse.connectEvents()

    expect(FakeEventSource.instances[0].url).toContain('token=tok-local')
  })

  it('token 保留字符经 encodeURIComponent 编码', async () => {
    const ws = await import('../useWebSocket')
    const sse = await import('../useSSE')

    ws.connect(null, 'a b&c=d')
    sse.connectEvents()

    expect(FakeEventSource.instances[0].url).toBe(
      'http://test.local/api/events/stream?token=a%20b%26c%3Dd'
    )
  })
})
