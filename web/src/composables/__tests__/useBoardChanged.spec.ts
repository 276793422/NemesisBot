import { mount } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'

// useBoardChanged（W2.5 board-changed 推送订阅）：
// - 200ms 尾沿防抖：事件流末尾后等 debounceMs 再回调，突发连发只刷一次；
// - 组件卸载 → 注销 SSE 订阅 + 取消未触发的定时器。
// useSSE 的 on/off 打桩（连接层不在本测试范围）。

const registered = new Map<string, (data?: unknown) => void>()

vi.mock('../useSSE', () => ({
  on: vi.fn((type: string, handler: (data?: unknown) => void) => {
    registered.set(type, handler)
  }),
  off: vi.fn((type: string) => {
    registered.delete(type)
  }),
}))

import { useBoardChanged } from '../useBoardChanged'

function harness(handler: () => void, debounceMs?: number) {
  const Comp = {
    setup() {
      if (debounceMs === undefined) useBoardChanged(handler)
      else useBoardChanged(handler, debounceMs)
      return () => null
    },
  }
  return mount(Comp)
}

beforeEach(() => {
  registered.clear()
  vi.useFakeTimers()
})

afterEach(() => {
  vi.useRealTimers()
})

describe('useBoardChanged', () => {
  it('订阅 board-changed；事件后不立即回调，200ms 防抖窗口后才触发一次', () => {
    const handler = vi.fn()
    const w = harness(handler)

    expect(registered.has('board-changed')).toBe(true)
    registered.get('board-changed')!()
    expect(handler).not.toHaveBeenCalled()

    vi.advanceTimersByTime(200)
    expect(handler).toHaveBeenCalledTimes(1)
    w.unmount()
  })

  it('防抖窗口内连发多次 → 只在尾沿后回调一次（cc-switch 同款语义）', () => {
    const handler = vi.fn()
    const w = harness(handler)

    const fire = () => registered.get('board-changed')!()
    fire()
    vi.advanceTimersByTime(100)
    fire()
    vi.advanceTimersByTime(100) // 距首次 200ms，但距上次仅 100ms
    expect(handler).not.toHaveBeenCalled()
    fire()
    vi.advanceTimersByTime(199)
    expect(handler).not.toHaveBeenCalled()
    vi.advanceTimersByTime(1)
    expect(handler).toHaveBeenCalledTimes(1)
    w.unmount()
  })

  it('自定义防抖时长生效', () => {
    const handler = vi.fn()
    const w = harness(handler, 500)

    registered.get('board-changed')!()
    vi.advanceTimersByTime(200)
    expect(handler).not.toHaveBeenCalled()
    vi.advanceTimersByTime(300)
    expect(handler).toHaveBeenCalledTimes(1)
    w.unmount()
  })

  it('卸载 → 注销订阅 + 取消未触发定时器（回调不再发生）', () => {
    const handler = vi.fn()
    const w = harness(handler)

    registered.get('board-changed')!()
    w.unmount()
    expect(registered.has('board-changed')).toBe(false)
    vi.advanceTimersByTime(1000)
    expect(handler).not.toHaveBeenCalled()
  })
})
