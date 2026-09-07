import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'

// useApprovals（M7 审批卡状态单例）：
// - SSE `approval-requested` → pendingApprovals 入列（去重保原到期时刻）；
// - `approval.pending` 补拉（断线漏事件）；
// - respond 成功 → 本地移除 + 成功 toast；unknown/not wired 错误 → 也移除；
// - 倒计时到期 → pruneExpired 摘卡片（服务端已自动拒绝）。
// useSSE / useWSAPI / useToast 全部打桩。

const sseHandlers = new Map<string, (data?: unknown) => void>()
const wsapiRequest = vi.fn()

vi.mock('../useSSE', () => ({
  on: vi.fn((type: string, handler: (data?: unknown) => void) => {
    sseHandlers.set(type, handler)
  }),
  off: vi.fn(),
}))

vi.mock('../useWSAPI', () => ({
  useWSAPI: () => ({ request: wsapiRequest }),
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

import { useApprovals, _resetApprovalsForTest } from '../useApprovals'

function fireApproval(data: Record<string, unknown>) {
  sseHandlers.get('approval-requested')!(data)
}

function makePayload(requestId: string, overrides: Record<string, unknown> = {}) {
  return {
    request_id: requestId,
    operation: 'process_exec',
    target: 'cargo publish',
    risk_level: 'HIGH',
    reason: 'rule: exec-publish',
    timeout_secs: 60,
    age_secs: 0,
    ...overrides,
  }
}

beforeEach(() => {
  _resetApprovalsForTest()
  sseHandlers.clear()
  wsapiRequest.mockReset()
  toasts.length = 0
  vi.useFakeTimers()
})

afterEach(() => {
  vi.useRealTimers()
})

describe('useApprovals', () => {
  it('SSE 事件入列；重复 request_id 去重且不重置到期时刻', () => {
    const { pendingApprovals, initApprovals } = useApprovals()
    initApprovals()

    vi.setSystemTime(new Date('2026-09-06T08:00:00'))
    fireApproval(makePayload('r1'))
    expect(pendingApprovals).toHaveLength(1)
    const firstExpiry = pendingApprovals[0].expiresAt

    vi.setSystemTime(new Date('2026-09-06T08:00:05'))
    // 同一请求重放（重连重放场景）：age 更大也不改已有到期时刻。
    fireApproval(makePayload('r1', { age_secs: 5 }))
    expect(pendingApprovals).toHaveLength(1)
    expect(pendingApprovals[0].expiresAt).toBe(firstExpiry)
  })

  it('expiresAt = now + (timeout_secs - age_secs)*1000', () => {
    const { pendingApprovals, initApprovals } = useApprovals()
    initApprovals()

    vi.setSystemTime(new Date('2026-09-06T08:00:00'))
    fireApproval(makePayload('r-age', { timeout_secs: 60, age_secs: 10 }))
    // 2026-09-06T08:00:00 的 epoch 是确定的，直接比对增量。
    expect(pendingApprovals[0].expiresAt - Date.now()).toBe(50_000)
  })

  it('initApprovals 幂等：重复调用不重复订阅/补拉', () => {
    const { initApprovals } = useApprovals()
    initApprovals()
    initApprovals()
    expect(wsapiRequest).toHaveBeenCalledTimes(1)
    expect(wsapiRequest).toHaveBeenCalledWith('approval', 'pending', {}, 5000)
  })

  it('补拉 pending 列表入列（断线漏事件恢复）', async () => {
    wsapiRequest.mockResolvedValue({ pending: [makePayload('seed-1'), makePayload('seed-2')] })
    const { pendingApprovals, initApprovals } = useApprovals()
    initApprovals()
    await vi.waitFor(() => expect(pendingApprovals).toHaveLength(2))
    expect(pendingApprovals.map(a => a.request_id)).toEqual(['seed-1', 'seed-2'])
  })

  it('respond 成功 → 本地移除 + 成功 toast', async () => {
    wsapiRequest.mockResolvedValue({ delivered: true })
    const { pendingApprovals, respondTo, initApprovals } = useApprovals()
    initApprovals()
    fireApproval(makePayload('r-ok'))
    expect(pendingApprovals).toHaveLength(1)

    await respondTo('r-ok', true)
    expect(wsapiRequest).toHaveBeenCalledWith('approval', 'respond', {
      request_id: 'r-ok',
      approved: true,
      always: false,
    })
    expect(pendingApprovals).toHaveLength(0)
    expect(toasts.some(t => t.type === 'success')).toBe(true)
  })

  it('F3: respondTo(r, true, true) → always 透传 + 「记住」toast', async () => {
    wsapiRequest.mockResolvedValue({ delivered: true })
    const { respondTo, initApprovals } = useApprovals()
    initApprovals()

    await respondTo('r-always', true, true)
    expect(wsapiRequest).toHaveBeenCalledWith('approval', 'respond', {
      request_id: 'r-always',
      approved: true,
      always: true,
    })
    const ok = toasts.find(t => t.type === 'success')
    expect(ok?.msg).toContain('记住')
  })

  it('F3: SSE payload 的 pattern 入列（总是允许确认串）', () => {
    const { pendingApprovals, initApprovals } = useApprovals()
    initApprovals()
    fireApproval(makePayload('r-pat', { pattern: 'cargo publish *' }))
    expect(pendingApprovals[0].pattern).toBe('cargo publish *')
    // 无 pattern = 不适用。
    fireApproval(makePayload('r-nopat'))
    expect(pendingApprovals[1].pattern).toBeUndefined()
  })

  it('F6: approval-resolved 事件 → 静默摘卡（竞速败方无感关闭）', () => {
    const { pendingApprovals, initApprovals } = useApprovals()
    initApprovals()
    fireApproval(makePayload('r-loser'))
    fireApproval(makePayload('r-keep'))
    expect(pendingApprovals).toHaveLength(2)

    // 另一窗口已裁决 r-loser → 广播到达 → 本地卡移除，另一张不受影响。
    sseHandlers.get('approval-resolved')!({ request_id: 'r-loser', decision: 'approved' })
    expect(pendingApprovals.map(a => a.request_id)).toEqual(['r-keep'])
    // 无错误 toast（静默语义）。
    expect(toasts.some(t => t.type === 'error')).toBe(false)
    // 非法 payload（缺 request_id）被忽略。
    sseHandlers.get('approval-resolved')!({ decision: 'denied' })
    expect(pendingApprovals).toHaveLength(1)
  })

  it('F6: respondTo 的 note 只在有值时进 payload', async () => {
    wsapiRequest.mockResolvedValue({ delivered: true })
    const { respondTo, initApprovals } = useApprovals()
    initApprovals()

    await respondTo('r-n1', false, false, '别动生产配置')
    expect(wsapiRequest).toHaveBeenLastCalledWith('approval', 'respond', {
      request_id: 'r-n1',
      approved: false,
      always: false,
      note: '别动生产配置',
    })
    // 未传 note = 缺键（与旧前端 payload 字节兼容）。
    await respondTo('r-n2', true, false)
    expect(wsapiRequest).toHaveBeenLastCalledWith('approval', 'respond', {
      request_id: 'r-n2',
      approved: true,
      always: false,
    })
  })

  it('respond 返回 unknown/not wired → 卡片同步移除 + 错误 toast', async () => {
    wsapiRequest.mockRejectedValue('unknown approval request: r-gone')
    const { pendingApprovals, respondTo, initApprovals } = useApprovals()
    initApprovals()
    fireApproval(makePayload('r-gone'))
    expect(pendingApprovals).toHaveLength(1)

    await respondTo('r-gone', false)
    expect(pendingApprovals).toHaveLength(0)
    expect(toasts.some(t => t.type === 'error')).toBe(true)
  })

  it('respond 其他错误 → 卡片保留（用户可重试）', async () => {
    wsapiRequest.mockRejectedValue('timeout: approval.respond')
    const { pendingApprovals, respondTo, initApprovals } = useApprovals()
    initApprovals()
    fireApproval(makePayload('r-keep'))

    await respondTo('r-keep', true)
    expect(pendingApprovals).toHaveLength(1)
    expect(toasts.some(t => t.type === 'error')).toBe(true)
  })

  it('倒计时到期 → pruneExpired 摘卡片（服务端已自动拒绝）', async () => {
    const { pendingApprovals, initApprovals } = useApprovals()
    initApprovals()
    fireApproval(makePayload('r-exp', { timeout_secs: 2 }))
    expect(pendingApprovals).toHaveLength(1)

    vi.advanceTimersByTime(1000)
    expect(pendingApprovals).toHaveLength(1)
    vi.advanceTimersByTime(1100)
    expect(pendingApprovals).toHaveLength(0)
  })
})
