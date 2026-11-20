import { describe, it, expect, vi, beforeEach } from 'vitest'

// mailbox 事件族登记回归锁（2026-11-21 契约漂移修复，BUG-20261121-019）。
//
// 根因链：SMTP 落信 → 插件 notify_mail_in → post_agent.rs
// publish_global_event("mailbox.updated", …) 广播；发信回执走
// publish_global_event("mail.sent", …)（toast 提示 message_id）。
// MailboxView onSSE('mailbox.updated'/'mail.sent') 订阅都在，但 useSSE 的
// EVENT_TYPES 白名单漏登记这两个 type → EventSource 上从未为它们挂
// listener → 落信列表不实时刷新、回执 toast 永不出现（SSE 是 mailbox
// 唯一实时传输，无 WS push 兜底；voice-setup/expert-degraded 后第三例
// 同族白名单坑）。
//
// 本 spec 钉住行为链：sse.on(type) 注册 → connectEvents → 事件帧 emit →
// handler 收到。白名单若再漏登记，listener 挂不上、emit 不可达，本用例红。

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

describe('useSSE mailbox 事件族登记（白名单漏登记回归锁）', () => {
  it("mailbox.updated 帧可达已注册 handler（落信实时刷新链路通）", async () => {
    const sse = await import('../useSSE')
    const seen: unknown[] = []
    sse.on('mailbox.updated', (d) => seen.push(d))

    sse.connectEvents()
    const es = FakeEventSource.instances[0]
    expect(es).toBeTruthy()

    es.emit('mailbox.updated', { reason: 'notify_mail_in', mailbox: 'inbox' })
    es.emit('mailbox.updated', { reason: 'notify_mail_in', mailbox: 'inbox' })
    expect(seen).toHaveLength(2)
    expect((seen[0] as { mailbox?: string }).mailbox).toBe('inbox')
  })

  it("mail.sent 帧可达已注册 handler（发信回执 toast 链路通）", async () => {
    const sse = await import('../useSSE')
    const seen: unknown[] = []
    sse.on('mail.sent', (d) => seen.push(d))

    sse.connectEvents()
    FakeEventSource.instances[0].emit('mail.sent', { message_id: 'm-1', to: 'a@b.c' })
    expect(seen).toHaveLength(1)
    expect((seen[0] as { message_id?: string }).message_id).toBe('m-1')
  })

  it('两类型退订对称（off 后帧不再可达）', async () => {
    const sse = await import('../useSSE')
    const seen: unknown[] = []
    const h1 = (d: unknown) => seen.push(d)
    const h2 = (d: unknown) => seen.push(d)
    sse.on('mailbox.updated', h1)
    sse.on('mail.sent', h2)
    sse.off('mailbox.updated', h1)
    sse.off('mail.sent', h2)

    sse.connectEvents()
    const es = FakeEventSource.instances[0]
    es.emit('mailbox.updated', {})
    es.emit('mail.sent', {})
    expect(seen).toHaveLength(0)
  })
})
