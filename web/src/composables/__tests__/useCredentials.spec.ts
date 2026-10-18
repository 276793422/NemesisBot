import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'

// useCredentials（B4 凭据卡状态单例）：
// - SSE `credential-requested` → pendingCredentials 入列（去重保原到期时刻）；
// - `credential.pending` 补拉（断线漏事件）；
// - submitCredential 成功 → 本地移除 + 成功 toast；unknown/not wired/不存在
//   错误 → 也移除；其他错误（vault 落库拒绝，请求保留可重试）→ 卡片保留；
// - `credential-resolved` 广播 → 静默摘卡；
// - 倒计时到期 → pruneExpired 摘卡片（服务端已超时放行）。
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

import { useCredentials, _resetCredentialsForTest } from '../useCredentials'

function fireCredential(data: Record<string, unknown>) {
  sseHandlers.get('credential-requested')!(data)
}

function makePayload(requestId: string, overrides: Record<string, unknown> = {}) {
  return {
    request_id: requestId,
    service: 'model:glm-5.3-flash',
    key_name: 'model/glm-5.3-flash',
    reason: '认证失败（401）',
    timeout_secs: 300,
    age_secs: 0,
    ...overrides,
  }
}

beforeEach(() => {
  _resetCredentialsForTest()
  sseHandlers.clear()
  wsapiRequest.mockReset()
  toasts.length = 0
  vi.useFakeTimers()
})

afterEach(() => {
  vi.useRealTimers()
})

describe('useCredentials', () => {
  it('SSE 事件入列；重复 request_id 去重且不重置到期时刻', () => {
    const { pendingCredentials, initCredentials } = useCredentials()
    initCredentials()

    vi.setSystemTime(new Date('2026-10-18T08:00:00'))
    fireCredential(makePayload('c1'))
    expect(pendingCredentials).toHaveLength(1)
    const firstExpiry = pendingCredentials[0].expiresAt

    vi.setSystemTime(new Date('2026-10-18T08:00:05'))
    // 同一请求重放（重连重放场景）：age 更大也不改已有到期时刻。
    fireCredential(makePayload('c1', { age_secs: 5 }))
    expect(pendingCredentials).toHaveLength(1)
    expect(pendingCredentials[0].expiresAt).toBe(firstExpiry)
  })

  it('expiresAt = now + (timeout_secs - age_secs)*1000；缺字段回落', () => {
    const { pendingCredentials, initCredentials } = useCredentials()
    initCredentials()

    vi.setSystemTime(new Date('2026-10-18T08:00:00'))
    fireCredential(makePayload('c-age', { timeout_secs: 60, age_secs: 10 }))
    expect(pendingCredentials[0].expiresAt - Date.now()).toBe(50_000)
    fireCredential(makePayload('c-bare', { timeout_secs: undefined, reason: undefined }))
    // timeout 缺省 300s；reason 缺省 undefined。
    expect(pendingCredentials[1].timeout_secs).toBe(300)
    expect(pendingCredentials[1].reason).toBeUndefined()
  })

  it('initCredentials 幂等：重复调用不重复订阅/补拉', () => {
    const { initCredentials } = useCredentials()
    initCredentials()
    initCredentials()
    expect(wsapiRequest).toHaveBeenCalledTimes(1)
    expect(wsapiRequest).toHaveBeenCalledWith('credential', 'pending', {}, 5000)
  })

  it('补拉 pending 列表入列（断线漏事件恢复）', async () => {
    wsapiRequest.mockResolvedValue({ pending: [makePayload('seed-1'), makePayload('seed-2')] })
    const { pendingCredentials, initCredentials } = useCredentials()
    initCredentials()
    await vi.waitFor(() => expect(pendingCredentials).toHaveLength(2))
    expect(pendingCredentials.map(c => c.request_id)).toEqual(['seed-1', 'seed-2'])
  })

  it('submitCredential 成功 → 本地移除 + 成功 toast；值原样透传', async () => {
    wsapiRequest.mockResolvedValue({ stored: true, rotated: false })
    const { pendingCredentials, submitCredential, initCredentials } = useCredentials()
    initCredentials()
    fireCredential(makePayload('c-ok'))
    expect(pendingCredentials).toHaveLength(1)

    await submitCredential('c-ok', 'sk-test-123')
    expect(wsapiRequest).toHaveBeenCalledWith('credential', 'respond', {
      request_id: 'c-ok',
      value: 'sk-test-123',
    })
    expect(pendingCredentials).toHaveLength(0)
    expect(toasts.some(t => t.type === 'success')).toBe(true)
  })

  it('credential-resolved 事件 → 静默摘卡（竞速败方/服务端超时无感关闭）', () => {
    const { pendingCredentials, initCredentials } = useCredentials()
    initCredentials()
    fireCredential(makePayload('c-loser'))
    fireCredential(makePayload('c-keep'))
    expect(pendingCredentials).toHaveLength(2)

    sseHandlers.get('credential-resolved')!({ request_id: 'c-loser', decision: 'timeout' })
    expect(pendingCredentials.map(c => c.request_id)).toEqual(['c-keep'])
    // 非法 payload（缺 request_id）被忽略。
    sseHandlers.get('credential-resolved')!({ decision: 'stored' })
    expect(pendingCredentials).toHaveLength(1)
  })

  it('submitCredential unknown/不存在 → 卡片同步移除 + 错误 toast', async () => {
    wsapiRequest.mockRejectedValue('凭据请求不存在（已超时或已裁决）: c-gone')
    const { pendingCredentials, submitCredential, initCredentials } = useCredentials()
    initCredentials()
    fireCredential(makePayload('c-gone'))
    expect(pendingCredentials).toHaveLength(1)

    await submitCredential('c-gone', 'sk-x')
    expect(pendingCredentials).toHaveLength(0)
    expect(toasts.some(t => t.type === 'error')).toBe(true)
  })

  it('submitCredential vault 落库拒绝（其他错误）→ 卡片保留（可修正重试）', async () => {
    wsapiRequest.mockRejectedValue('凭据落库失败（请求保留，可修正后重试）: vault 解锁失败')
    const { pendingCredentials, submitCredential, initCredentials } = useCredentials()
    initCredentials()
    fireCredential(makePayload('c-retry'))

    await submitCredential('c-retry', 'sk-wrong')
    expect(pendingCredentials).toHaveLength(1)
    expect(toasts.some(t => t.type === 'error')).toBe(true)
  })

  it('dismissCredential → 只摘本地卡（服务端等待窗口自然超时）', () => {
    const { pendingCredentials, dismissCredential, initCredentials } = useCredentials()
    initCredentials()
    fireCredential(makePayload('c-dismiss'))
    expect(pendingCredentials).toHaveLength(1)

    dismissCredential('c-dismiss')
    expect(pendingCredentials).toHaveLength(0)
    expect(toasts.some(t => t.type === 'info')).toBe(true)
  })

  it('倒计时到期 → pruneExpired 摘卡片（服务端已超时放行）', async () => {
    const { pendingCredentials, initCredentials } = useCredentials()
    initCredentials()
    fireCredential(makePayload('c-exp', { timeout_secs: 2 }))
    expect(pendingCredentials).toHaveLength(1)

    vi.advanceTimersByTime(1000)
    expect(pendingCredentials).toHaveLength(1)
    vi.advanceTimersByTime(1100)
    expect(pendingCredentials).toHaveLength(0)
  })
})
