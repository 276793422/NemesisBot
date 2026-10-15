import { describe, it, expect, vi, beforeEach } from 'vitest'

// useSSE 连接生命周期（重连兜底）：board-changed 是 fire-and-forget 通知
//（无 id/seq，不参与服务端重放）——断线窗口内的事件永久丢失，board 面板
// 会停在旧数据直到下一次无关写入（实证 2026-10-15：gateway 重启后派发链
// 全程列表冻结）。修复 = onopen（含浏览器自动重连成功）补发一次本地合成
// board-changed，各面板静默换新兜底。本 spec 钉住：
// - 每次 onopen 都派发合成事件（载荷带 reconnect:true）；
// - 服务端真实 board-changed 照常透传（不丢原始载荷）。
// EventSource 打桩（连接层假件），每用例重置模块取全新单例状态。

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

describe('useSSE 重连兜底（board-changed 合成补发）', () => {
  it('onopen（含重连成功）每次都派发合成 board-changed（reconnect:true）', async () => {
    const sse = await import('../useSSE')
    const seen: unknown[] = []
    sse.on('board-changed', (d) => seen.push(d))

    sse.connectEvents()
    const es = FakeEventSource.instances[0]
    expect(es).toBeTruthy()

    es.onopen!()
    expect(seen).toHaveLength(1)
    expect((seen[0] as { reconnect?: boolean }).reconnect).toBe(true)

    // 模拟浏览器自动重连成功：onopen 再次触发（同一连接的第二次 open）
    es.onopen!()
    expect(seen).toHaveLength(2)
  })

  it('服务端真实 board-changed 事件照常透传，合成事件不影响原始载荷', async () => {
    const sse = await import('../useSSE')
    const seen: unknown[] = []
    sse.on('board-changed', (d) => seen.push(d))

    sse.connectEvents()
    const es = FakeEventSource.instances[0]
    es.emit('board-changed', { ts: '2026-10-15T00:00:00Z' })
    expect(seen).toHaveLength(1)
    expect(seen[0]).toEqual({ ts: '2026-10-15T00:00:00Z' })

    // 合成兜底与真实事件互不干扰
    es.onopen!()
    expect(seen).toHaveLength(2)
    expect((seen[1] as { reconnect?: boolean }).reconnect).toBe(true)
  })

  it('旧连接 CLOSED 后 connectEvents 重建连接，新连接 onopen 照常兜底', async () => {
    const sse = await import('../useSSE')
    const seen: unknown[] = []
    sse.on('board-changed', (d) => seen.push(d))

    sse.connectEvents()
    const first = FakeEventSource.instances[0]
    first.close()
    first.onopen = null // 已死的连接不再回调

    sse.connectEvents() // readyState=CLOSED → 重置重建
    const second = FakeEventSource.instances[1]
    expect(second).toBeTruthy()
    expect(second).not.toBe(first)

    second.onopen!()
    expect(seen).toHaveLength(1)
    expect((seen[0] as { reconnect?: boolean }).reconnect).toBe(true)
  })
})
