// W2（2026-09-23）回归钉：显式 disconnect() 必须取消已排程的重连定时器。
//
// 修复前：reconnect() 裸 setTimeout 不留句柄，disconnect() 只置 manualClose——
// 定时器到点照样 connect()，显式断开后的连接「死而复生」（审计场景：切换
// 页面 / 登出 / 换 token 时旧连接带着旧 query 参数爬回来）。
// 修复后：reconnectTimer 句柄 + disconnect() clearTimeout + 回调内复查
// manualClose（排程与触发之间的窗口双保险）。

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
    this.readyState = FakeWebSocket.CLOSED
    this.onclose?.({ code: 1006 })
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
  disconnect() // 单例模块状态复位（manualClose / 定时器 / 连接）
})

afterEach(() => {
  disconnect()
  vi.unstubAllGlobals()
  vi.useRealTimers()
})

describe('W2：disconnect 取消已排程重连', () => {
  it('异常断开 → 定时重连发生（基线：重连机制本身工作）', async () => {
    connect('ws://test/ws', 'tok')
    expect(FakeWebSocket.instances).toHaveLength(1)

    lastSocket().simulateAbnormalClose()
    expect(wsStatus.value).toBe('disconnected')

    await vi.advanceTimersByTimeAsync(1000)
    expect(FakeWebSocket.instances, '重连定时器到点应发起新连接').toHaveLength(2)
  })

  it('断开 → 重连已排程 → disconnect() → 定时器到点不再重连（修复本体）', async () => {
    connect('ws://test/ws', 'tok')
    lastSocket().simulateAbnormalClose() // 排程 1s 后重连

    // 排程与触发之间显式断开（登出/换页面场景）。
    disconnect()

    await vi.advanceTimersByTimeAsync(30_000)
    expect(FakeWebSocket.instances, 'disconnect 后重连定时器必须被取消').toHaveLength(1)
    expect(wsStatus.value).toBe('disconnected')
  })

  it('指数退避后再排程的定时器同样可被 disconnect 取消', async () => {
    connect('ws://test/ws', 'tok')
    // 第 1 轮：断开 → 1s 重连（reconnectDelay 是模块级状态：基线测试已把它
    // 翻倍到 2s，这里推进 5s 覆盖任意已累积的退避值）。
    lastSocket().simulateAbnormalClose()
    await vi.advanceTimersByTimeAsync(5_000)
    expect(FakeWebSocket.instances).toHaveLength(2)

    // 第 2 轮：断开 → 退避重连已排程 → disconnect 打断
    lastSocket().simulateAbnormalClose()
    disconnect()
    await vi.advanceTimersByTimeAsync(60_000)
    expect(FakeWebSocket.instances).toHaveLength(2)
  })

  it('disconnect 后手动 connect 可恢复（取消定时器不破坏正常路径）', async () => {
    connect('ws://test/ws', 'tok')
    lastSocket().simulateAbnormalClose()
    disconnect()

    connect('ws://test/ws', 'tok2')
    // 手动 connect 重置 manualClose=false——若旧定时器未被取消，到点会
    // 叠加一个幽灵重连（实例数 3）。推进完整退避窗口验证不发生。
    expect(FakeWebSocket.instances).toHaveLength(2)
    expect(wsStatus.value).toBe('connecting')
    expect(lastSocket().url).toContain('tok2')
    await vi.advanceTimersByTimeAsync(60_000)
    expect(FakeWebSocket.instances, '不得出现幽灵重连').toHaveLength(2)
  })
})

// W7（2026-10-14）回归钉：connect 守卫放行 CLOSING 态旧 socket 重入后，新
// socket 已赋模块变量 ws；旧 socket 迟到的 onclose/onerror 若直接清 ws/停
// 心跳/报断线，会误杀新连接（sendRaw 永久入队、下次 connect 双活 socket
// 帧双发、状态假断线）。各回调只在自己仍是当前连接时才拥有模块状态。
describe('W7：被取代的旧 socket 迟到回调不得误杀新连接', () => {
  /** 造「旧 socket CLOSING → connect 守卫放行重入 → 新 socket 接管」现场。 */
  function staleReplacedByFresh(): FakeWebSocket {
    connect('ws://test/ws', 'tok')
    const stale = lastSocket()
    stale.readyState = FakeWebSocket.CLOSING
    connect('ws://test/ws', 'tok')
    expect(FakeWebSocket.instances).toHaveLength(2)
    return stale
  }

  it('旧 socket 迟到 onclose：不清 ws、不报假断线、不误停心跳、不叠幽灵重连', async () => {
    const stale = staleReplacedByFresh()
    const fresh = lastSocket()

    fresh.readyState = FakeWebSocket.OPEN
    fresh.onopen?.() // 新连接就绪，心跳已排程

    stale.onclose?.({ code: 1006 }) // 旧 socket 的关闭事件此刻才迟到

    expect(wsStatus.value, '必须保持新连接的 connected').toBe('connected')
    await vi.advanceTimersByTimeAsync(30_000)
    expect(fresh.sent, '新连接心跳必须存活（旧 bug 的迟到 onclose 会误停心跳）').toHaveLength(1)
    expect(fresh.sent[0]).toContain('heartbeat')
    await vi.advanceTimersByTimeAsync(30_000) // 覆盖最大退避窗口（delay≤30s）
    expect(FakeWebSocket.instances, '不得出现第 3 个 socket（幽灵重连）').toHaveLength(2)
  })

  it('旧 socket 迟到 onclose 后新连接照常可用（sendRaw 经新 socket 收发）', () => {
    const stale = staleReplacedByFresh()
    const fresh = lastSocket()

    // 症状链起点：迟到 onclose。修复前把 ws 清 null → 后续 sendRaw 永久
    // 入队且 connect() 会再造第 3 个 socket（双活）；修复后 ws === fresh，
    // 消息入队后在 onopen 时经它冲出。
    stale.onclose?.({ code: 1006 })

    sendRaw({ type: 'message', module: 'chat', cmd: 'send', data: { content: 'hi' } })
    expect(fresh.sent, 'CONNECTING 态不得直发').toHaveLength(0)

    fresh.readyState = FakeWebSocket.OPEN
    fresh.onopen?.()
    expect(fresh.sent, '入队消息必须在 onopen 后经新 socket 冲出').toHaveLength(1)
    expect(FakeWebSocket.instances, '不得再造第 3 个 socket').toHaveLength(2)
  })

  it('旧 socket 迟到 onerror 不伪造新连接的断线状态', () => {
    const stale = staleReplacedByFresh()
    stale.onerror?.()
    expect(wsStatus.value).toBe('connecting')
  })

  it('对照：当前连接自身的关闭仍正常走断线+重连（守卫不伤正常路径）', async () => {
    connect('ws://test/ws', 'tok')
    lastSocket().simulateAbnormalClose()
    expect(wsStatus.value).toBe('disconnected')
    await vi.advanceTimersByTimeAsync(60_000) // reconnectDelay 可能已被前组用例推到 30s
    expect(FakeWebSocket.instances).toHaveLength(2)
  })
})
