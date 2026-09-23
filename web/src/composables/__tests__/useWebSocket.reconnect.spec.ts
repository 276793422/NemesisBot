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
  onopen: ((ev?: any) => void) | null = null
  onclose: ((ev?: any) => void) | null = null
  onerror: ((ev?: any) => void) | null = null
  onmessage: ((ev?: any) => void) | null = null

  constructor(url: string) {
    this.url = url
    FakeWebSocket.instances.push(this)
  }

  send() {}

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
