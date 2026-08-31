import { mount } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'

// useUsageChanged（A3 usage-changed 推送订阅，与 useBoardChanged 同款语义）：
// - 200ms 尾沿防抖：事件流末尾后等 debounceMs 再回调，一次 LLM 轮次
//   连续写多条明细只刷一次；
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

import { useUsageChanged } from '../useUsageChanged'

function harness(handler: () => void, debounceMs?: number) {
  const Comp = {
    setup() {
      if (debounceMs === undefined) useUsageChanged(handler)
      else useUsageChanged(handler, debounceMs)
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

describe('useUsageChanged', () => {
  it('订阅 usage-changed；事件后不立即回调，200ms 防抖窗口后才触发一次', () => {
    const handler = vi.fn()
    const w = harness(handler)

    expect(registered.has('usage-changed')).toBe(true)
    registered.get('usage-changed')!()
    expect(handler).not.toHaveBeenCalled()

    vi.advanceTimersByTime(200)
    expect(handler).toHaveBeenCalledTimes(1)
    w.unmount()
  })

  it('防抖窗口内连发多次 → 只在尾沿后回调一次', () => {
    const handler = vi.fn()
    const w = harness(handler)

    const fire = () => registered.get('usage-changed')!()
    fire()
    vi.advanceTimersByTime(100)
    fire()
    vi.advanceTimersByTime(100) // 距首次 200ms，但距上次仅 100ms
    expect(handler).not.toHaveBeenCalled()
    vi.advanceTimersByTime(200)
    expect(handler).toHaveBeenCalledTimes(1)
    w.unmount()
  })

  it('自定义防抖时长生效', () => {
    const handler = vi.fn()
    const w = harness(handler, 500)

    registered.get('usage-changed')!()
    vi.advanceTimersByTime(200)
    expect(handler).not.toHaveBeenCalled()
    vi.advanceTimersByTime(300)
    expect(handler).toHaveBeenCalledTimes(1)
    w.unmount()
  })

  it('卸载 → 注销订阅 + 取消未触发定时器（回调不再发生）', () => {
    const handler = vi.fn()
    const w = harness(handler)

    registered.get('usage-changed')!()
    w.unmount()
    expect(registered.has('usage-changed')).toBe(false)
    vi.advanceTimersByTime(1000)
    expect(handler).not.toHaveBeenCalled()
  })
})
