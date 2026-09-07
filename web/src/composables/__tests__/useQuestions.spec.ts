import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'

// useQuestions（F7 提问卡状态单例）：
// - SSE `question-asked` → pendingQuestions 入列（去重保原到期时刻）；
// - `question.pending` 补拉（断线漏事件）；
// - respond 成功 → 本地移除 + 成功 toast；unknown/not wired 错误 → 也移除；
//   其他错误（后端校验拒绝，请求留在 pending 可重试）→ 卡片保留；
// - `question-resolved` 广播 → 静默摘卡；
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

import { useQuestions, _resetQuestionsForTest } from '../useQuestions'

function fireQuestion(data: Record<string, unknown>) {
  sseHandlers.get('question-asked')!(data)
}

function makePayload(questionId: string, overrides: Record<string, unknown> = {}) {
  return {
    question_id: questionId,
    question: '用哪个包管理器?',
    options: ['pnpm', 'npm'],
    multi: false,
    timeout_secs: 120,
    age_secs: 0,
    chat_id: 'chat-1',
    session_key: 'web:chat-1',
    ...overrides,
  }
}

beforeEach(() => {
  _resetQuestionsForTest()
  sseHandlers.clear()
  wsapiRequest.mockReset()
  toasts.length = 0
  vi.useFakeTimers()
})

afterEach(() => {
  vi.useRealTimers()
})

describe('useQuestions', () => {
  it('SSE 事件入列；重复 question_id 去重且不重置到期时刻', () => {
    const { pendingQuestions, initQuestions } = useQuestions()
    initQuestions()

    vi.setSystemTime(new Date('2026-09-06T08:00:00'))
    fireQuestion(makePayload('q1'))
    expect(pendingQuestions).toHaveLength(1)
    const firstExpiry = pendingQuestions[0].expiresAt

    vi.setSystemTime(new Date('2026-09-06T08:00:05'))
    // 同一问题重放（重连重放场景）：age 更大也不改已有到期时刻。
    fireQuestion(makePayload('q1', { age_secs: 5 }))
    expect(pendingQuestions).toHaveLength(1)
    expect(pendingQuestions[0].expiresAt).toBe(firstExpiry)
  })

  it('expiresAt = now + (timeout_secs - age_secs)*1000；multi 缺省为单选', () => {
    const { pendingQuestions, initQuestions } = useQuestions()
    initQuestions()

    vi.setSystemTime(new Date('2026-09-06T08:00:00'))
    fireQuestion(makePayload('q-age', { timeout_secs: 60, age_secs: 10, multi: true }))
    expect(pendingQuestions[0].expiresAt - Date.now()).toBe(50_000)
    expect(pendingQuestions[0].multi).toBe(true)
    fireQuestion(makePayload('q-single', { multi: undefined }))
    expect(pendingQuestions[1].multi).toBe(false)
  })

  it('initQuestions 幂等：重复调用不重复订阅/补拉', () => {
    const { initQuestions } = useQuestions()
    initQuestions()
    initQuestions()
    expect(wsapiRequest).toHaveBeenCalledTimes(1)
    expect(wsapiRequest).toHaveBeenCalledWith('question', 'pending', {}, 5000)
  })

  it('补拉 pending 列表入列（断线漏事件恢复）', async () => {
    wsapiRequest.mockResolvedValue({ pending: [makePayload('seed-1'), makePayload('seed-2')] })
    const { pendingQuestions, initQuestions } = useQuestions()
    initQuestions()
    await vi.waitFor(() => expect(pendingQuestions).toHaveLength(2))
    expect(pendingQuestions.map(q => q.question_id)).toEqual(['seed-1', 'seed-2'])
  })

  it('respond 成功 → 本地移除 + 成功 toast；selected 原样透传', async () => {
    wsapiRequest.mockResolvedValue({ delivered: true })
    const { pendingQuestions, respondTo, initQuestions } = useQuestions()
    initQuestions()
    fireQuestion(makePayload('q-ok', { multi: true }))
    expect(pendingQuestions).toHaveLength(1)

    await respondTo('q-ok', ['pnpm', 'npm'])
    expect(wsapiRequest).toHaveBeenCalledWith('question', 'respond', {
      question_id: 'q-ok',
      selected: ['pnpm', 'npm'],
    })
    expect(pendingQuestions).toHaveLength(0)
    expect(toasts.some(t => t.type === 'success')).toBe(true)
  })

  it('question-resolved 事件 → 静默摘卡（竞速败方/服务端超时无感关闭）', () => {
    const { pendingQuestions, initQuestions } = useQuestions()
    initQuestions()
    fireQuestion(makePayload('q-loser'))
    fireQuestion(makePayload('q-keep'))
    expect(pendingQuestions).toHaveLength(2)

    sseHandlers.get('question-resolved')!({ question_id: 'q-loser', decision: 'answered' })
    expect(pendingQuestions.map(q => q.question_id)).toEqual(['q-keep'])
    expect(toasts.some(t => t.type === 'error')).toBe(false)
    // 非法 payload（缺 question_id）被忽略。
    sseHandlers.get('question-resolved')!({ decision: 'timeout' })
    expect(pendingQuestions).toHaveLength(1)
  })

  it('respond unknown/not wired → 卡片同步移除 + 错误 toast', async () => {
    wsapiRequest.mockRejectedValue('unknown question: q-gone')
    const { pendingQuestions, respondTo, initQuestions } = useQuestions()
    initQuestions()
    fireQuestion(makePayload('q-gone'))
    expect(pendingQuestions).toHaveLength(1)

    await respondTo('q-gone', ['pnpm'])
    expect(pendingQuestions).toHaveLength(0)
    expect(toasts.some(t => t.type === 'error')).toBe(true)
  })

  it('respond 校验拒绝（其他错误）→ 卡片保留（后端 pending 可重试）', async () => {
    wsapiRequest.mockRejectedValue("'yarn' is not one of the offered options")
    const { pendingQuestions, respondTo, initQuestions } = useQuestions()
    initQuestions()
    fireQuestion(makePayload('q-retry'))

    await respondTo('q-retry', ['yarn'])
    expect(pendingQuestions).toHaveLength(1)
    expect(toasts.some(t => t.type === 'error')).toBe(true)
  })

  it('倒计时到期 → pruneExpired 摘卡片（服务端已超时放行）', async () => {
    const { pendingQuestions, initQuestions } = useQuestions()
    initQuestions()
    fireQuestion(makePayload('q-exp', { timeout_secs: 2 }))
    expect(pendingQuestions).toHaveLength(1)

    vi.advanceTimersByTime(1000)
    expect(pendingQuestions).toHaveLength(1)
    vi.advanceTimersByTime(1100)
    expect(pendingQuestions).toHaveLength(0)
  })
})
