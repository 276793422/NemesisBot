import { describe, it, expect, vi, beforeEach } from 'vitest'
import { defineComponent } from 'vue'
import { mount } from '@vue/test-utils'

// U7 inbox visibility (G1): useInboxStatus — snapshot 刷新、保守降级、
// 派生态（mode/queueEnabled/queuedTotal/queueFull）与轮询生命周期。

const requestMock = vi.fn()
vi.mock('../useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
}))

import { useInboxStatus } from '../useInboxStatus'
import type { InboxStatusData } from '../useInboxStatus'

function steerStatus(over: Partial<InboxStatusData> = {}): InboxStatusData {
  return {
    available: true,
    session_key: 'agent:main:session:s1',
    next_turn: 0,
    next_step: 0,
    capacity: 8,
    busy: true,
    mode: 'steer',
    ...over,
  }
}

/** useInboxStatus 用了 onUnmounted，挂到一个宿主组件上避免无实例警告。 */
function mountHost() {
  let api!: ReturnType<typeof useInboxStatus>
  const Host = defineComponent({
    setup() {
      api = useInboxStatus()
      return () => null
    },
  })
  const wrapper = mount(Host)
  return { api, wrapper }
}

beforeEach(() => {
  requestMock.mockReset()
})

describe('useInboxStatus 快照与派生态', () => {
  it('refresh 成功 → status 落位，steer 派生为真', async () => {
    requestMock.mockResolvedValue(steerStatus({ next_turn: 2, next_step: 1 }))
    const { api, wrapper } = mountHost()

    await api.refresh('s1')
    expect(requestMock).toHaveBeenCalledWith('agent', 'inbox_status', { session_id: 's1' })
    expect(api.status.value?.next_turn).toBe(2)
    expect(api.mode.value).toBe('steer')
    expect(api.steerEnabled.value).toBe(true)
    expect(api.queueEnabled.value).toBe(true)
    expect(api.queuedTotal.value).toBe(3)
    expect(api.queueFull.value).toBe(false)
    wrapper.unmount()
  })

  it('queue 模式 → queueEnabled 真但 steerEnabled 假', async () => {
    requestMock.mockResolvedValue(steerStatus({ mode: 'queue', busy: false }))
    const { api, wrapper } = mountHost()
    await api.refresh()
    expect(api.mode.value).toBe('queue')
    expect(api.queueEnabled.value).toBe(true)
    expect(api.steerEnabled.value).toBe(false)
    wrapper.unmount()
  })

  it('reject 模式 → 不解锁 busy 发送', async () => {
    requestMock.mockResolvedValue(steerStatus({ mode: 'reject' }))
    const { api, wrapper } = mountHost()
    await api.refresh()
    expect(api.queueEnabled.value).toBe(false)
    expect(api.steerEnabled.value).toBe(false)
    wrapper.unmount()
  })

  it('available:false → 保守按 reject 处理', async () => {
    requestMock.mockResolvedValue(steerStatus({ available: false, mode: 'reject' }))
    const { api, wrapper } = mountHost()
    await api.refresh()
    expect(api.mode.value).toBe('reject')
    expect(api.queueEnabled.value).toBe(false)
    expect(api.queuedTotal.value).toBe(0)
    wrapper.unmount()
  })

  it('请求失败 → status 置 null（保守默认，不残留旧快照）', async () => {
    requestMock.mockRejectedValueOnce('WS not initialized')
    const { api, wrapper } = mountHost()
    await api.refresh()
    expect(api.status.value).toBeNull()
    expect(api.mode.value).toBe('reject')
    expect(api.queueEnabled.value).toBe(false)
    wrapper.unmount()
  })

  it('乱序防护：陈旧会话的迟到响应不覆盖当前会话快照', async () => {
    let resolveOld!: (v: InboxStatusData) => void
    const oldSnap = steerStatus({ session_key: 'agent:main:session:old', next_turn: 9 })
    const newSnap = steerStatus({ session_key: 'agent:main:session:new', next_turn: 1 })
    requestMock.mockImplementation((_m: string, _c: string, data: any) => {
      if (data?.session_id === 'old') {
        return new Promise<InboxStatusData>(resolve => { resolveOld = resolve })
      }
      return Promise.resolve(newSnap)
    })
    const { api, wrapper } = mountHost()
    // 在 old 会话上发起一个挂起的请求，再切到 new 会话（立即返回）。
    const stale = api.refresh('old')
    await api.refresh('new')
    expect(api.status.value?.session_key).toContain('new')
    // old 的迟到响应落地 → 必须被丢弃，不得覆盖 new 的快照。
    resolveOld(oldSnap)
    await stale
    expect(api.status.value?.session_key).toContain('new')
    expect(api.status.value?.next_turn).toBe(1)
    wrapper.unmount()
  })

  it('queueFull：两队列合计 ≥ capacity', async () => {
    requestMock.mockResolvedValue(steerStatus({ next_turn: 8, next_step: 0, capacity: 8 }))
    const { api, wrapper } = mountHost()
    await api.refresh()
    expect(api.queueFull.value).toBe(true)
    wrapper.unmount()
  })
})

describe('useInboxStatus 轮询', () => {
  it('startPolling 立即拉一次 + 每 4s 一次；stopPolling 后停；重复 start 不叠加', async () => {
    vi.useFakeTimers()
    try {
      requestMock.mockResolvedValue(steerStatus())
      const { api, wrapper } = mountHost()

      api.startPolling('s1')
      await vi.advanceTimersByTimeAsync(0)
      expect(requestMock).toHaveBeenCalledTimes(1)

      await vi.advanceTimersByTimeAsync(4000)
      expect(requestMock).toHaveBeenCalledTimes(2)

      // 重复 start 不得叠加定时器
      api.startPolling()
      await vi.advanceTimersByTimeAsync(4000)
      expect(requestMock).toHaveBeenCalledTimes(3)

      api.stopPolling()
      await vi.advanceTimersByTimeAsync(12000)
      expect(requestMock).toHaveBeenCalledTimes(3)
      wrapper.unmount()
    } finally {
      vi.useRealTimers()
    }
  })
})
