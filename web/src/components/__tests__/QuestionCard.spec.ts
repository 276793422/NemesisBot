import { mount } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'

// QuestionCard（F7 提问卡模态）：
// - 无 pending 时不渲染；
// - SSE 事件后渲染卡（问题/选项/单选多选徽标/倒计时）；
// - 未选择时提交禁用；选择后提交 → respondTo → WSAPI question.respond；
// - 单选互斥（radio）/ 多选可叠加（checkbox）；
// - respond 成功后卡片消失；resolved 广播静默摘卡。

const sseHandlers = new Map<string, (data?: unknown) => void>()
const wsapiRequest = vi.fn()

vi.mock('../../composables/useSSE', () => ({
  on: vi.fn((type: string, handler: (data?: unknown) => void) => {
    sseHandlers.set(type, handler)
  }),
  off: vi.fn(),
}))

vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: wsapiRequest }),
}))

vi.mock('../../composables/useToast', () => ({
  useToast: () => ({
    toasts: [],
    info: vi.fn(),
    success: vi.fn(),
    warn: vi.fn(),
    error: vi.fn(),
    remove: vi.fn(),
  }),
}))

import { useQuestions, _resetQuestionsForTest } from '../../composables/useQuestions'
import QuestionCard from '../QuestionCard.vue'

function fireQuestion(data: Record<string, unknown>) {
  sseHandlers.get('question-asked')!(data)
}

function makePayload(questionId: string, overrides: Record<string, unknown> = {}) {
  return {
    question_id: questionId,
    question: '用哪个包管理器?',
    options: ['pnpm', 'npm'],
    multi: false,
    timeout_secs: 60,
    age_secs: 0,
    ...overrides,
  }
}

beforeEach(() => {
  _resetQuestionsForTest()
  sseHandlers.clear()
  wsapiRequest.mockReset().mockResolvedValue({ delivered: true })
  vi.useFakeTimers()
})

afterEach(() => {
  vi.useRealTimers()
})

async function mounted() {
  const { initQuestions } = useQuestions()
  const w = mount(QuestionCard)
  initQuestions()
  return w
}

describe('QuestionCard', () => {
  it('无 pending 时不渲染模态', async () => {
    const w = await mounted()
    expect(w.find('.question-backdrop').exists()).toBe(false)
    w.unmount()
  })

  it('SSE 事件后渲染卡：问题/选项/单选徽标齐全，radio 形态', async () => {
    const w = await mounted()
    fireQuestion(makePayload('q-view'))
    await vi.advanceTimersByTimeAsync(0)

    expect(w.find('.question-backdrop').exists()).toBe(true)
    expect(w.text()).toContain('用哪个包管理器?')
    expect(w.text()).toContain('pnpm')
    expect(w.text()).toContain('npm')
    expect(w.text()).toContain('单选')
    expect(w.find('input[type="radio"]').exists()).toBe(true)
    expect(w.find('input[type="checkbox"]').exists()).toBe(false)
    w.unmount()
  })

  it('多选：checkbox 形态 + 多选徽标', async () => {
    const w = await mounted()
    fireQuestion(makePayload('q-multi', { multi: true, options: ['a', 'b', 'c'] }))
    await vi.advanceTimersByTimeAsync(0)

    expect(w.text()).toContain('多选')
    expect(w.findAll('input[type="checkbox"]')).toHaveLength(3)
    w.unmount()
  })

  it('倒计时显示剩余秒数并逐秒递减', async () => {
    const w = await mounted()
    vi.advanceTimersByTime(1000)
    await w.vm.$nextTick()
    fireQuestion(makePayload('q-cd', { timeout_secs: 30 }))
    await vi.advanceTimersByTimeAsync(0)
    expect(w.find('.approval-countdown').text()).toContain('30s')

    vi.advanceTimersByTime(5000)
    await w.vm.$nextTick()
    expect(w.find('.approval-countdown').text()).toContain('25s')
    w.unmount()
  })

  it('未选择时提交禁用；选择后启用', async () => {
    const w = await mounted()
    fireQuestion(makePayload('q-dis'))
    await vi.advanceTimersByTimeAsync(0)

    const submit = () => w.findAll('button').find(b => b.text() === '提交')!
    expect(submit().attributes('disabled')).toBeDefined()

    await w.findAll('input[type="radio"]')[0].setValue()
    await vi.advanceTimersByTimeAsync(0)
    expect(submit().attributes('disabled')).toBeUndefined()
    w.unmount()
  })

  it('单选：改选互斥，提交送最后一选', async () => {
    const w = await mounted()
    fireQuestion(makePayload('q-radio'))
    await vi.advanceTimersByTimeAsync(0)

    const radios = w.findAll('input[type="radio"]')
    await radios[0].setValue()
    await radios[1].setValue()
    await vi.advanceTimersByTimeAsync(0)

    await w.findAll('button').find(b => b.text() === '提交')!.trigger('click')
    await vi.advanceTimersByTimeAsync(0)
    expect(wsapiRequest).toHaveBeenCalledWith('question', 'respond', {
      question_id: 'q-radio',
      selected: ['npm'],
    })
    w.unmount()
  })

  it('多选：勾选两项一起送出', async () => {
    const w = await mounted()
    fireQuestion(makePayload('q-check', { multi: true }))
    await vi.advanceTimersByTimeAsync(0)

    const boxes = w.findAll('input[type="checkbox"]')
    await boxes[0].setValue()
    await boxes[1].setValue()
    await vi.advanceTimersByTimeAsync(0)

    await w.findAll('button').find(b => b.text() === '提交')!.trigger('click')
    await vi.advanceTimersByTimeAsync(0)
    expect(wsapiRequest).toHaveBeenCalledWith('question', 'respond', {
      question_id: 'q-check',
      selected: ['pnpm', 'npm'],
    })
    w.unmount()
  })

  it('提交成功 → 卡片消失', async () => {
    const w = await mounted()
    fireQuestion(makePayload('q-done'))
    await vi.advanceTimersByTimeAsync(0)
    expect(w.find('.question-backdrop').exists()).toBe(true)

    await w.findAll('input[type="radio"]')[0].setValue()
    await w.findAll('button').find(b => b.text() === '提交')!.trigger('click')
    await vi.advanceTimersByTimeAsync(0)
    expect(w.find('.question-backdrop').exists()).toBe(false)
    w.unmount()
  })

  it('question-resolved 广播 → 卡片静默摘除（竞速败方/服务端超时无感关闭）', async () => {
    const w = await mounted()
    fireQuestion(makePayload('q-loser'))
    await vi.advanceTimersByTimeAsync(0)
    expect(w.find('.question-backdrop').exists()).toBe(true)

    sseHandlers.get('question-resolved')!({ question_id: 'q-loser', decision: 'timeout' })
    await vi.advanceTimersByTimeAsync(0)
    expect(w.find('.question-backdrop').exists()).toBe(false)
    w.unmount()
  })
})
