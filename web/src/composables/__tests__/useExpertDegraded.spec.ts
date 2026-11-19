import { describe, it, expect, vi, beforeEach } from 'vitest'

// useExpertDegraded（专家降级 toast 单例，2026-11-13 总开关 + 自动降级）：
// - SSE `expert-degraded` → info toast「专家 X 不可用，已自动降级…」；
// - 载荷为展平内层 `{requested, reason, ...}`（server.rs pump 展平）；
// - requested 缺失/非串 → 静默（防御面，不弹空名 toast）；
// - 幂等初始化（重复 init 不重复订阅）。

const sseHandlers = new Map<string, (data?: unknown) => void>()

vi.mock('../useSSE', () => ({
  on: vi.fn((type: string, handler: (data?: unknown) => void) => {
    sseHandlers.set(type, handler)
  }),
  off: vi.fn(),
}))

const toasts: Array<{ msg: string; type: string }> = []
vi.mock('../useToast', () => ({
  useToast: () => ({
    toasts: [],
    info: (m: string) => toasts.push({ msg: m, type: 'info' }),
    success: (m: string) => toasts.push({ msg: m, type: 'success' }),
    warn: (m: string) => toasts.push({ msg: m, type: 'warn' }),
    error: (m: string) => toasts.push({ msg: m, type: 'error' }),
    remove: vi.fn(),
  }),
}))

import { useExpertDegraded, _resetExpertDegradedForTest } from '../useExpertDegraded'

beforeEach(() => {
  _resetExpertDegradedForTest()
  sseHandlers.clear()
  toasts.splice(0)
})

describe('useExpertDegraded', () => {
  it('expert-degraded 事件 → info toast 带专家名', () => {
    const { initExpertDegraded } = useExpertDegraded()
    initExpertDegraded()
    sseHandlers.get('expert-degraded')!({
      session_key: 'agent:main:session:s1',
      chat_id: 's1',
      requested: '数据分析师',
      reason: "Unknown expert '数据分析师'. Hired experts: none.",
    })
    expect(toasts.length).toBe(1)
    expect(toasts[0]!.type).toBe('info')
    expect(toasts[0]!.msg).toContain('数据分析师')
    expect(toasts[0]!.msg).toContain('降级')
  })

  it('requested 缺失/空串/非串 → 静默不弹', () => {
    const { initExpertDegraded } = useExpertDegraded()
    initExpertDegraded()
    sseHandlers.get('expert-degraded')!({})
    sseHandlers.get('expert-degraded')!({ requested: '   ' })
    sseHandlers.get('expert-degraded')!(null)
    expect(toasts.length).toBe(0)
  })

  it('幂等初始化：重复 init 不重复注册 handler', () => {
    const a = useExpertDegraded()
    const b = useExpertDegraded()
    a.initExpertDegraded()
    b.initExpertDegraded()
    sseHandlers.get('expert-degraded')!({ requested: 'X' })
    expect(toasts.length).toBe(1)
  })
})
