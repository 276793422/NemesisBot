import { mount, flushPromises } from '@vue/test-utils'
import { createPinia } from 'pinia'
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { useToast } from '../../composables/useToast'

// M6 补测（quality-hardening goal 2026-08-25）：P1-2 cron 每任务轮次预算 ——
// 列表徽标、add 默认 null（无预算）、选档保存、edit 播种与「清除预算」语义
// （update 路径 null=清除，与 add 路径 null=不设，语义一致）。
// 后端 max_rounds 落盘 + 执行预算由 handlers/tasks/tests.rs（5 测试）钉住。

const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
  initWSAPI: vi.fn(), // useWebSocket.ts 模块级回注，经 session store 传递导入
}))

import TasksView from '../TasksView.vue'

function job(over: Record<string, unknown> = {}) {
  return {
    id: 'job-1', name: '日报', cron: '0 9 * * *', description: '每天 09:00',
    session_key: 'agent:main:session:web_chat1', enabled: true,
    max_rounds: null, ...over,
  }
}

beforeEach(() => {
  requestMock.mockReset()
  useToast().toasts.splice(0)
  requestMock.mockImplementation((_m: string, cmd: string) => {
    if (cmd === 'cron.list') return Promise.resolve({ jobs: [job()] })
    if (cmd === 'cron.preview') return Promise.resolve({ valid: true, description: '每天 09:00' })
    return Promise.resolve({})
  })
})

async function mountView() {
  const w = mount(TasksView, { global: { plugins: [createPinia()] } })
  await flushPromises()
  await w.findAll('.tab').find(b => b.text() === '定时任务')!.trigger('click')
  await flushPromises()
  return w
}

function maxRoundsSelect(w: ReturnType<typeof mount>) {
  return w.findAll('select.form-select').find(s =>
    s.findAll('option').some(o => o.text().includes('不设预算')),
  )!
}

async function fillNameAndSave(w: ReturnType<typeof mount>, label = '添加') {
  const nameInput = w.findAll('input.form-input').find(i => i.attributes('placeholder')?.includes('每天9点'))!
  await nameInput.setValue('巡检')
  await w.findAll('button').find(b => b.text() === label)!.trigger('click')
  await flushPromises()
}

function selectedOptionText(sel: ReturnType<typeof mount>['findAll'] extends never ? never : any): string {
  return (sel.element as HTMLSelectElement).selectedOptions[0]?.text ?? ''
}

describe('TasksView 轮次预算（P1-2）', () => {
  it('列表：有 max_rounds 显示 ⏱ 徽标；无则不显示', async () => {
    const w = await mountView()
    expect(w.text()).not.toContain('⏱')

    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'cron.list') return Promise.resolve({ jobs: [job({ max_rounds: 5 })] })
      return Promise.resolve({})
    })
    const w2 = await mountView()
    expect(w2.text()).toContain('⏱ 5')
  })

  it('新建：默认不设预算 → cron.add payload max_rounds=null', async () => {
    const w = await mountView()
    await w.findAll('button').find(b => b.text().includes('添加任务'))!.trigger('click')
    await flushPromises()

    const sel = maxRoundsSelect(w)
    expect(selectedOptionText(sel)).toContain('不设预算')

    await fillNameAndSave(w)
    const call = requestMock.mock.calls.find(c => c[1] === 'cron.add')!
    expect(call[2].max_rounds).toBe(null)
    expect(call[2].name).toBe('巡检')
    expect(useToast().toasts.some(t => t.type === 'success' && t.message === '已添加')).toBe(true)
  })

  it('新建：选 10 轮 → payload max_rounds=10', async () => {
    const w = await mountView()
    await w.findAll('button').find(b => b.text().includes('添加任务'))!.trigger('click')
    await flushPromises()
    await maxRoundsSelect(w).setValue('10')
    await fillNameAndSave(w)
    expect(requestMock.mock.calls.find(c => c[1] === 'cron.add')![2].max_rounds).toBe(10)
  })

  it('编辑：播种已有预算 20 → cron.update 原样保存；改回不设 → null=清除预算', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'cron.list') return Promise.resolve({ jobs: [job({ max_rounds: 20 })] })
      if (cmd === 'cron.preview') return Promise.resolve({ valid: true, description: 'x' })
      return Promise.resolve({})
    })
    const w = await mountView()

    await w.findAll('button').find(b => b.attributes('title') === '编辑')!.trigger('click')
    await flushPromises()
    expect(selectedOptionText(maxRoundsSelect(w))).toContain('20 轮')

    // 原样保存（按钮是「保存」）
    const nameInput = w.findAll('input.form-input').find(i => i.attributes('placeholder')?.includes('每天9点'))!
    await nameInput.setValue('日报改')
    await w.findAll('button').find(b => b.text() === '保存')!.trigger('click')
    await flushPromises()
    let call = requestMock.mock.calls.find(c => c[1] === 'cron.update')!
    expect(call[2]).toMatchObject({ id: 'job-1', max_rounds: 20 })

    // 改回不设预算 → update null（清除语义）
    requestMock.mockClear()
    await w.findAll('button').find(b => b.attributes('title') === '编辑')!.trigger('click')
    await flushPromises()
    // null option 的 DOM value 即其文本（:value=null 未落到 value 属性），按真实 DOM 值选
    const nullOpt = maxRoundsSelect(w).findAll('option').find(o => o.text().includes('不设预算'))!
    await maxRoundsSelect(w).setValue((nullOpt.element as HTMLOptionElement).value)
    await nameInput.setValue('日报改2')
    await w.findAll('button').find(b => b.text() === '保存')!.trigger('click')
    await flushPromises()
    call = requestMock.mock.calls.find(c => c[1] === 'cron.update')!
    expect(call[2].max_rounds).toBe(null)
  })
})
