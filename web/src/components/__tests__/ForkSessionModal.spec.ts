import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { useToast } from '../../composables/useToast'

// M6 补测（quality-hardening goal 2026-08-25）：P3-1 会话分叉弹窗。
// mock useChatApi（turns/fork 的 HTTP 契约由后端 fork_route_tests.rs +
// composables/__tests__/useChatApi.spec.ts 钉住，这里测弹窗交互层）。

const turnsMock = vi.fn()
const forkMock = vi.fn()
vi.mock('../../composables/useChatApi', () => ({
  useChatApi: () => ({ turns: (...a: any[]) => turnsMock(...a), fork: (...a: any[]) => forkMock(...a) }),
}))

import ForkSessionModal from '../ForkSessionModal.vue'

const TURNS = {
  session_id: 'sid-1',
  session_key: 'agent:main:session:sid-1',
  total_turns: 2,
  total_messages: 8,
  turns: [
    { turn: 1, preview: '读一下配置', end_preview: '已读取', time: '2026-08-24T10:00:00+08:00', turn_messages: 4, kept_messages: 4 },
    { turn: 2, preview: '再改一处', end_preview: '改好了', time: '2026-08-24T10:05:00+08:00', turn_messages: 4, kept_messages: 8 },
  ],
}

function mountModal() {
  return mount(ForkSessionModal, {
    props: { sessionId: 'sid-1', sessionTitle: '测试会话' },
  })
}

beforeEach(() => {
  turnsMock.mockReset().mockResolvedValue(TURNS)
  forkMock.mockReset()
  // 清空真实 toast 队列（模块级单例，跨用例残留会互相污染断言）
  useToast().toasts.splice(0)
})

describe('ForkSessionModal 轮次表', () => {
  it('打开即拉取并渲染轮次行 + 全量选项 + 末条预览', async () => {
    const w = mountModal()
    await flushPromises()
    expect(turnsMock).toHaveBeenCalledWith('sid-1')

    const rows = w.findAll('.turn-row')
    expect(rows.length).toBe(2)
    expect(rows[0].text()).toContain('第 1 轮')
    expect(rows[0].text()).toContain('读一下配置')
    expect(rows[0].text()).toContain('↳ 分叉末条：已读取')
    expect(rows[0].text()).toContain('4 条')
    // 全量选项默认选中，文案带总消息数
    expect(w.find('.full-option').text()).toContain('全量分叉')
    expect(w.find('.full-option').text()).toContain('8 条消息')
    expect(w.find('.foot-hint').text()).toContain('保留全部轮次')
  })

  it('选中某轮 → 底部提示切换为「保留前 N 轮」', async () => {
    const w = mountModal()
    await flushPromises()
    await w.findAll('input[type="radio"]')[1].setValue()
    expect(w.find('.foot-hint').text()).toContain('保留前 1 轮')
    expect(w.find('.foot-hint').text()).toContain('从第 2 轮起另开分支')
  })

  it('轮次拉取失败 → toast 错误并关闭弹窗', async () => {
    turnsMock.mockRejectedValue(new Error('HTTP 404'))
    const w = mountModal()
    await flushPromises()
    expect(useToast().toasts.some(t => t.type === 'error' && t.message.includes('HTTP 404'))).toBe(true)
    expect(w.emitted('close')).toBeTruthy()
  })
})

describe('ForkSessionModal 分叉', () => {
  it('全量分叉：fork 不带 at_turn，成功后 emit forked(新 id)', async () => {
    forkMock.mockResolvedValue({ session_id: 'sid-new', kept_messages: 8 })
    const w = mountModal()
    await flushPromises()
    await w.findAll('button').find(b => b.text() === '分叉')!.trigger('click')
    await flushPromises()

    expect(forkMock).toHaveBeenCalledWith('sid-1', undefined)
    expect(w.emitted('forked')).toBeTruthy()
    expect(w.emitted('forked')![0]).toEqual(['sid-new'])
    expect(useToast().toasts.some(t => t.type === 'success' && t.message.includes('8 条消息'))).toBe(true)
  })

  it('选轮分叉：fork 带所选轮号', async () => {
    forkMock.mockResolvedValue({ session_id: 'sid-new2', kept_messages: 4 })
    const w = mountModal()
    await flushPromises()
    await w.findAll('input[type="radio"]')[1].setValue()
    await w.findAll('button').find(b => b.text() === '分叉')!.trigger('click')
    await flushPromises()
    expect(forkMock).toHaveBeenCalledWith('sid-1', 1)
    expect(w.emitted('forked')![0]).toEqual(['sid-new2'])
  })

  it('分叉失败：toast 错误、不 emit forked、按钮复位可重试', async () => {
    forkMock.mockRejectedValue(new Error('源会话不存在'))
    const w = mountModal()
    await flushPromises()
    const btn = w.findAll('button').find(b => b.text() === '分叉')!
    await btn.trigger('click')
    await flushPromises()
    expect(w.emitted('forked')).toBeFalsy()
    expect(useToast().toasts.some(t => t.type === 'error' && t.message.includes('源会话不存在'))).toBe(true)
    expect(btn.attributes('disabled')).toBeUndefined()

    // 重试成功
    forkMock.mockResolvedValue({ session_id: 'sid-new3', kept_messages: 8 })
    await btn.trigger('click')
    await flushPromises()
    expect(w.emitted('forked')).toBeTruthy()
  })
})
