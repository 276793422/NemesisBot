import { mount } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'

// QuestionCard（F7 提问卡模态）：
// - 无 pending 时不渲染；
// - SSE 事件后渲染卡（问题/选项/单选多选徽标/倒计时）；
// - 未选择时提交禁用；选择后提交 → respondTo → WSAPI question.respond；
// - 单选互斥（radio）/ 多选可叠加（checkbox）；
// - respond 成功后卡片消失；resolved 广播静默摘卡。
// B8（提问两形态）：表单卡（表单徽标/text+select 字段/必填红星/必填齐活
// 才可提交/只送非空值）+ 单选选项 >6 收纳为下拉（6 项仍 radio）。

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

  // -------------------------------------------------------------------------
  // B8（提问两形态）：表单卡 + 单选下拉收纳
  // -------------------------------------------------------------------------

  function makeFormPayload(questionId: string, overrides: Record<string, unknown> = {}) {
    return makePayload(questionId, {
      options: [],
      fields: [
        { key: 'env', label: '环境', type: 'select', required: true, options: ['prod', 'staging'] },
        { key: 'note', label: '备注', type: 'text' },
      ],
      ...overrides,
    })
  }

  it('表单卡：表单徽标 + text 输入框与 select 下拉 + 必填红星', async () => {
    const w = await mounted()
    fireQuestion(makeFormPayload('q-form'))
    await vi.advanceTimersByTimeAsync(0)

    expect(w.text()).toContain('表单')
    expect(w.text()).toContain('环境')
    expect(w.text()).toContain('备注')
    expect(w.find('input[type="text"].question-field-input').exists()).toBe(true)
    expect(w.findAll('select.question-field-input')).toHaveLength(1)
    expect(w.find('.required-mark').exists()).toBe(true)
    expect(w.find('input[type="radio"]').exists()).toBe(false)
    w.unmount()
  })

  it('表单卡：必填未齐提交禁用；齐活后启用，提交只送非空字段', async () => {
    const w = await mounted()
    fireQuestion(makeFormPayload('q-fsub'))
    await vi.advanceTimersByTimeAsync(0)

    const submit = () => w.findAll('button').find(b => b.text() === '提交')!
    expect(submit().attributes('disabled')).toBeDefined()

    // 填 text（必填仍是 select）→ 仍禁用。
    await w.find('input[type="text"]').setValue('hello')
    await vi.advanceTimersByTimeAsync(0)
    expect(submit().attributes('disabled')).toBeDefined()

    // select 选 prod → 启用。
    await w.findAll('select.question-field-input')[0].setValue('prod')
    await vi.advanceTimersByTimeAsync(0)
    expect(submit().attributes('disabled')).toBeUndefined()

    await submit().trigger('click')
    await vi.advanceTimersByTimeAsync(0)
    expect(wsapiRequest).toHaveBeenCalledWith('question', 'respond', {
      question_id: 'q-fsub',
      values: { env: 'prod', note: 'hello' },
    })
    w.unmount()
  })

  it('表单卡：全部可选未填即可提交（空 values 合法）', async () => {
    const w = await mounted()
    fireQuestion(makeFormPayload('q-fopt', {
      fields: [{ key: 'note', label: '备注', type: 'text' }],
    }))
    await vi.advanceTimersByTimeAsync(0)

    const submit = () => w.findAll('button').find(b => b.text() === '提交')!
    expect(submit().attributes('disabled')).toBeUndefined()
    await submit().trigger('click')
    await vi.advanceTimersByTimeAsync(0)
    expect(wsapiRequest).toHaveBeenCalledWith('question', 'respond', {
      question_id: 'q-fopt',
      values: {},
    })
    w.unmount()
  })

  it('单选选项超过 6 项收纳为下拉 select（radio 消失），选择后提交', async () => {
    const w = await mounted()
    fireQuestion(makePayload('q-dd', { options: ['1', '2', '3', '4', '5', '6', '7'] }))
    await vi.advanceTimersByTimeAsync(0)

    expect(w.find('select.question-dropdown').exists()).toBe(true)
    expect(w.find('input[type="radio"]').exists()).toBe(false)

    const submit = () => w.findAll('button').find(b => b.text() === '提交')!
    expect(submit().attributes('disabled')).toBeDefined()

    await w.find('select.question-dropdown').setValue('7')
    await vi.advanceTimersByTimeAsync(0)
    expect(submit().attributes('disabled')).toBeUndefined()
    await submit().trigger('click')
    await vi.advanceTimersByTimeAsync(0)
    expect(wsapiRequest).toHaveBeenCalledWith('question', 'respond', {
      question_id: 'q-dd',
      selected: ['7'],
    })
    w.unmount()
  })

  it('单选恰好 6 项仍是 radio 列表（不收纳）', async () => {
    const w = await mounted()
    fireQuestion(makePayload('q-six', { options: ['1', '2', '3', '4', '5', '6'] }))
    await vi.advanceTimersByTimeAsync(0)
    expect(w.find('select.question-dropdown').exists()).toBe(false)
    expect(w.findAll('input[type="radio"]')).toHaveLength(6)
    w.unmount()
  })
})
