import { describe, it, expect, vi, beforeEach } from 'vitest'

// voice-setup 事件登记回归锁（2026-10-24 契约漂移修复）。
//
// 根因链：后端 voice.rs 安装链 hub.publish("voice-setup", …) 实时播报
// starting/cancelled/complete/error → VoiceTab sseOn('voice-setup', …)
// 订阅 → 但 useSSE 的 EVENT_TYPES 白名单漏登记该 type → EventSource 上
// 从未为它挂 listener → 安装期间（下载可达分钟级）进度横幅全程哑掉，
// 只剩初始静态文案（完成/失败仅靠请求返回后的 toast 兜底）。
//
// 本 spec 钉住行为链：sse.on('voice-setup') 注册 → connectEvents →
// 事件帧 emit → handler 收到。白名单若再漏登记，listener 挂不上、
// emit 不可达，本用例红。

class FakeEventSource {
  static CLOSED = 2
  static instances: FakeEventSource[] = []
  url: string
  readyState = 1
  onopen: ((ev?: unknown) => void) | null = null
  onerror: ((ev?: unknown) => void) | null = null
  closed = false
  private listeners = new Map<string, ((e: { data: string }) => void)[]>()
  constructor(url: string) {
    this.url = url
    FakeEventSource.instances.push(this)
  }
  addEventListener(type: string, cb: (e: { data: string }) => void) {
    const arr = this.listeners.get(type) || []
    arr.push(cb)
    this.listeners.set(type, arr)
  }
  close() {
    this.closed = true
    this.readyState = FakeEventSource.CLOSED
  }
  emit(type: string, data: unknown) {
    for (const cb of this.listeners.get(type) || []) cb({ data: JSON.stringify(data) })
  }
}

vi.mock('../lib/appBase', () => ({
  apiUrl: (p: string) => `http://test.local${p}`,
}))

beforeEach(async () => {
  vi.resetModules()
  FakeEventSource.instances = []
  vi.stubGlobal('EventSource', FakeEventSource as unknown as typeof EventSource)
})

describe('useSSE voice-setup 事件登记（白名单漏登记回归锁）', () => {
  it('voice-setup 帧可达已注册 handler（安装进度链路通）', async () => {
    const sse = await import('../useSSE')
    const seen: unknown[] = []
    sse.on('voice-setup', (d) => seen.push(d))

    sse.connectEvents()
    const es = FakeEventSource.instances[0]
    expect(es).toBeTruthy()

    es.emit('voice-setup', { phase: 'setup', status: 'starting', message: '开始安装语音环境...' })
    es.emit('voice-setup', { phase: 'setup', status: 'complete', message: '语音环境安装完成' })
    expect(seen).toHaveLength(2)
    expect((seen[0] as { status?: string }).status).toBe('starting')
    expect((seen[1] as { status?: string }).status).toBe('complete')
  })

  it("off('voice-setup') 后帧不再可达（退订对称）", async () => {
    const sse = await import('../useSSE')
    const seen: unknown[] = []
    const handler = (d: unknown) => seen.push(d)
    sse.on('voice-setup', handler)
    sse.off('voice-setup', handler)

    sse.connectEvents()
    FakeEventSource.instances[0].emit('voice-setup', { status: 'starting' })
    expect(seen).toHaveLength(0)
  })
})
