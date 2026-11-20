// BUG-20261121-005（半开连接检测）+ BUG-20261121-006（disconnect 清积压队列）
// 回归钉。
//
// 005 修复前：heartbeat.ping 每 30s 发出，但服务端的 pong 帧前端从不消费
// （handleWSResponse 只认 type==='response'）——休眠唤醒/网络切换后的半开
// TCP 下 send() 静默进内核缓冲，status 恒 connected，chat.send 永久无响应。
// 修复后：ping 后一个完整心跳周期（30s）未见 pong → 判半开，主动断开走
// 重连循环。
//
// 006 修复前：手动 disconnect()（logout / standalone 切会话）不清
// messageQueue——断线期间积压的旧消息（含上个认证会话的 chat.send）在下次
// connect 的 onopen flushQueue 里复活发出。修复后：终结语义清队列；自动
// 重连路径不经 disconnect()，flush 语义不变。

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'

class FakeWebSocket {
  static instances: FakeWebSocket[] = []
  static CONNECTING = 0
  static OPEN = 1
  static CLOSING = 2
  static CLOSED = 3

  url: string
  readyState = FakeWebSocket.CONNECTING
  sent: string[] = []
  closed = false
  onopen: ((ev?: any) => void) | null = null
  onclose: ((ev?: any) => void) | null = null
  onerror: ((ev?: any) => void) | null = null
  onmessage: ((ev?: any) => void) | null = null

  constructor(url: string) {
    this.url = url
    FakeWebSocket.instances.push(this)
  }

  send(json: string) {
    this.sent.push(json)
  }

  close() {
    this.closed = true
    this.readyState = FakeWebSocket.CLOSED
    this.onclose?.({ code: 1006 })
  }

  /** 服务端 heartbeat.pong 帧到达。 */
  deliverPong() {
    this.onmessage?.({
      data: JSON.stringify({ type: 'system', module: 'heartbeat', cmd: 'pong', data: {} }),
    })
  }

  /** 测试辅助：模拟服务端异常断开（非 1008/4001 → 触发重连路径）。 */
  simulateAbnormalClose() {
    this.readyState = FakeWebSocket.CLOSED
    this.onclose?.({ code: 1006 })
  }
}

vi.mock('../wsResponseHandler', () => ({ handleWSResponse: () => false }))
vi.mock('../useWSAPI', () => ({ initWSAPI: () => {} }))

import {
  connect,
  disconnect,
  sendRaw,
  wsStatus,
} from '../useWebSocket'

function lastSocket(): FakeWebSocket {
  return FakeWebSocket.instances[FakeWebSocket.instances.length - 1]
}

beforeEach(() => {
  vi.useFakeTimers()
  FakeWebSocket.instances = []
  vi.stubGlobal('WebSocket', FakeWebSocket as any)
  disconnect() // 单例模块状态复位（manualClose / 定时器 / 队列 / 连接）
})

afterEach(() => {
  disconnect()
  vi.unstubAllGlobals()
  vi.useRealTimers()
})

describe('BUG-20261121-005：heartbeat pong 半开连接检测', () => {
  it('pong 正常到达：连续心跳周期不误杀', async () => {
    connect('ws://test/ws', 'tok')
    const sock = lastSocket()
    sock.readyState = FakeWebSocket.OPEN
    sock.onopen?.()

    // 第 1 个周期：发出 ping。
    await vi.advanceTimersByTimeAsync(30_000)
    expect(sock.sent, '周期 1 发出 heartbeat.ping').toHaveLength(1)
    expect(sock.sent[0]).toContain('"ping"')
    // pong 到达，解除半开嫌疑。
    sock.deliverPong()

    // 第 2 个周期：正常继续（修复前 pong 被无视也无妨；修复后必须有 pong
    // 才能持续存活——这里验证 pong 消费路径本身工作）。
    await vi.advanceTimersByTimeAsync(30_000)
    expect(sock.sent, 'pong 到达后周期 2 照常 ping').toHaveLength(2)
    sock.deliverPong()
    expect(FakeWebSocket.instances, 'pong 正常时不重连').toHaveLength(1)
    expect(wsStatus.value).toBe('connected')
  })

  it('ping 后一个周期未见 pong：判半开 → close + 状态断开 + 排程重连（修复本体）', async () => {
    connect('ws://test/ws', 'tok')
    const sock = lastSocket()
    sock.readyState = FakeWebSocket.OPEN
    sock.onopen?.()

    // 周期 1：ping 发出，无 pong。
    await vi.advanceTimersByTimeAsync(30_000)
    expect(sock.sent).toHaveLength(1)

    // 周期 2 tick：上个 ping 仍无 pong → 半开判定。
    await vi.advanceTimersByTimeAsync(30_000)
    expect(sock.closed, '半开连接必须被主动 close').toBe(true)
    expect(wsStatus.value, '半开必须置断线状态（修复前恒 connected）').toBe('disconnected')

    // 重连排程生效（退避 1s 起）。
    await vi.advanceTimersByTimeAsync(1000)
    expect(FakeWebSocket.instances, '半开判定后必须走重连').toHaveLength(2)
  })

  it('半开重连后的新连接恢复完整心跳周期（pendingPing 状态不跨连接残留）', async () => {
    connect('ws://test/ws', 'tok')
    const first = lastSocket()
    first.readyState = FakeWebSocket.OPEN
    first.onopen?.()

    // 半开链：ping 1（无 pong）→ 30s 后半开 close → 重连。
    await vi.advanceTimersByTimeAsync(30_000)
    await vi.advanceTimersByTimeAsync(30_000)
    await vi.advanceTimersByTimeAsync(1000)
    const second = lastSocket()
    expect(second).not.toBe(first)
    second.readyState = FakeWebSocket.OPEN
    second.onopen?.()

    // 新连接：周期 1 正常 ping（pendingPing 已被 onopen 重置）。
    await vi.advanceTimersByTimeAsync(30_000)
    expect(second.sent, '新连接周期 1 照常 ping').toHaveLength(1)
    second.deliverPong()
    await vi.advanceTimersByTimeAsync(30_000)
    expect(second.sent, 'pong 后周期 2 照常 ping').toHaveLength(2)
  })
})

describe('BUG-20261121-006：disconnect 清积压队列', () => {
  it('断线入队 → disconnect → 重新 connect + onopen：旧消息不得复活', () => {
    // 断线状态发消息（sendRaw 入队 + 触发 connect）。
    sendRaw({ type: 'message', module: 'chat', cmd: 'send', data: { content: 'stale' } })
    const orphaned = lastSocket()
    expect(orphaned.sent).toHaveLength(0)

    // 终结语义断开（logout 场景）——积压必须作废。
    disconnect()

    // 新认证会话重新 connect。
    connect('ws://test/ws', 'tok2')
    const fresh = lastSocket()
    fresh.readyState = FakeWebSocket.OPEN
    fresh.onopen?.()
    expect(fresh.sent, '上个会话积压不得经 flushQueue 复活').toHaveLength(0)
  })

  it('对照：自动重连路径不经 disconnect——flush 语义保留', () => {
    connect('ws://test/ws', 'tok')
    const first = lastSocket()
    first.readyState = FakeWebSocket.OPEN
    first.onopen?.()

    // 服务端异常断开 → 自动重连（不走 disconnect()）。
    first.simulateAbnormalClose()
    sendRaw({ type: 'message', module: 'chat', cmd: 'send', data: { content: 'queued' } })
    // 断线窗口 sendRaw 触发的 connect() 直接建第二个 socket（守卫放行：
    // ws 已为 null）；排程中的重连定时器到点时被 OPEN 守卫拦截不重建。
    return vi.advanceTimersByTimeAsync(1000).then(() => {
      const second = lastSocket()
      second.readyState = FakeWebSocket.OPEN
      second.onopen?.()
      expect(second.sent, '自动重连后积压必须照常冲出').toHaveLength(1)
      expect(second.sent[0]).toContain('queued')
    })
  })

  it('对照：断线窗口未 disconnect 直接 onopen 的既有 flush 行为不变', () => {
    // CONNECTING 态入队（不经 simulateAbnormalClose，不排程重连）。
    connect('ws://test/ws', 'tok')
    const sock = lastSocket()
    sendRaw({ type: 'message', module: 'chat', cmd: 'send', data: { content: 'hi' } })
    expect(sock.sent).toHaveLength(0)
    sock.readyState = FakeWebSocket.OPEN
    sock.onopen?.()
    expect(sock.sent, '未终结的断线窗口 flush 语义不变').toHaveLength(1)
  })
})
