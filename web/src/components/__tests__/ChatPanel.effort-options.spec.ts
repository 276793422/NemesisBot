import { describe, it, expect, vi, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount, flushPromises } from '@vue/test-utils'

// G3 会话级思考强度下拉（7 档值域扩档，2026-11-09）：选项集 =
// ''+EFFORT_TIERS 七档；get_effort 挂载对齐；set_effort 乐观更新 +
// 失败回滚；非默认 chat 模块不渲染。

const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
}))

vi.mock('../../composables/useWebSocket', async () => {
  const { ref } = await import('vue')
  return {
    connect: vi.fn(),
    send: vi.fn(),
    sendHistoryRequest: vi.fn(),
    onMessage: vi.fn(),
    addMessageHandler: vi.fn(),
    removeMessageHandler: vi.fn(),
    wsStatus: ref('connected'),
  }
})

import ChatPanel from '../ChatPanel.vue'
import { useSessionStore } from '../../stores/session'

beforeEach(() => {
  setActivePinia(createPinia())
  useSessionStore().currentId = 's1'
  requestMock.mockReset()
  requestMock.mockResolvedValue({})
})

async function mountPanel() {
  const w = mount(ChatPanel)
  await flushPromises()
  return w
}

function effortSelect(w: ReturnType<typeof mount>) {
  return w.find('select.effort-select')
}

describe('ChatPanel 会话思考强度下拉（7 档扩档）', () => {
  it('默认 chat 渲染下拉：8 项选项集（默认 + off + 6 wire 档）顺序钉死', async () => {
    const w = await mountPanel()
    const sel = effortSelect(w)
    expect(sel.exists()).toBe(true)
    const values = sel.findAll('option').map((o) => (o.element as HTMLOptionElement).value)
    expect(values).toEqual(['', 'off', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max'])
    expect((sel.element as HTMLSelectElement).value).toBe('')
    w.unmount()
  })

  it('挂载即 chat.get_effort 对齐 override', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'get_effort') return Promise.resolve({ effort: 'high' })
      return Promise.resolve({})
    })
    const w = await mountPanel()
    expect(requestMock).toHaveBeenCalledWith('chat', 'get_effort', { session_id: 's1' })
    expect((effortSelect(w).element as HTMLSelectElement).value).toBe('high')
    w.unmount()
  })

  it('选 xhigh → set_effort {effort:"xhigh"}；选回默认 → effort:null', async () => {
    const w = await mountPanel()
    const sel = effortSelect(w)
    await sel.setValue('xhigh')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('chat', 'set_effort', {
      session_id: 's1',
      effort: 'xhigh',
    })
    await sel.setValue('')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('chat', 'set_effort', {
      session_id: 's1',
      effort: null,
    })
    w.unmount()
  })

  it('set_effort 失败 → 回滚到前值（不炸 UI）', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'set_effort') return Promise.reject(new Error('unknown effort'))
      return Promise.resolve({})
    })
    const w = await mountPanel()
    await effortSelect(w).setValue('xhigh')
    await flushPromises()
    expect((effortSelect(w).element as HTMLSelectElement).value).toBe('')
    w.unmount()
  })

  it('非默认 chat 模块（workflow_chat）不渲染下拉', async () => {
    const w = mount(ChatPanel, { props: { module: 'workflow_chat' } })
    await flushPromises()
    expect(effortSelect(w).exists()).toBe(false)
    w.unmount()
  })
})

describe('ChatPanel 会话思考强度下拉 · effort 探针标记（probe 二期）', () => {
  function markText(w: ReturnType<typeof mount>, value: string) {
    const opt = effortSelect(w)
      .findAll('option')
      .find((o) => (o.element as HTMLOptionElement).value === value)!
    return opt.text()
  }

  it('models.list 默认模型 effort_marks → ✓/✗/? 渲染（后端方言映射，前端零重建）', async () => {
    requestMock.mockImplementation((_m: string, cmd: string, data?: any) => {
      if (cmd === 'list') {
        return Promise.resolve({
          models: [
            {
              model_name: 'main',
              is_default: true,
              // 与 EFFORT_TIERS 平行：off/minimal/low/medium/high/xhigh/max
              effort_marks: ['ok', 'unknown', 'ok', 'bad', 'unknown', 'ok', 'ok'],
              effort_notes: ['', '', '', 'status 400: unsupported value medium', '', '', ''],
            },
          ],
        })
      }
      if (cmd === 'get_effort') return Promise.resolve({ effort: '' })
      return Promise.resolve({})
    })
    const w = await mountPanel()
    expect(markText(w, 'low')).toContain('✓')
    expect(markText(w, 'medium')).toContain('✗')
    expect(markText(w, 'high')).toContain('?')
    expect(markText(w, 'xhigh')).toContain('✓')
    // off 恒 NotSent（不发送=无验证对象）：不渲染标记。
    expect(markText(w, 'off')).not.toContain('✓')
    expect(markText(w, 'off')).not.toContain('?')
    // 拒绝原文进 option title（tooltip）。
    const med = effortSelect(w)
      .findAll('option')
      .find((o) => (o.element as HTMLOptionElement).value === 'medium')!
    expect(med.attributes('title')).toContain('unsupported value medium')
    w.unmount()
  })

  it('models.list 失败/缺默认 → 全部无标记回落（诚实未知，不炸 UI）', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'list') return Promise.reject(new Error('ws closed'))
      return Promise.resolve({})
    })
    const w = await mountPanel()
    expect(markText(w, 'low')).toBe('思考·低')
    expect(markText(w, 'max')).toBe('思考·最大')
    w.unmount()
  })
})
