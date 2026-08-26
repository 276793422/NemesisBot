import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { useToast } from '../../composables/useToast'

// M6 补测（quality-hardening goal 2026-08-25）：P4 设置页 hooks.json 卡 ——
// get 回显（含 invalid 形态）、前端 JSON 语法自检不落盘、set 保存
// （summary/成功 toast）、后端语义拒绝、一键重启。
// 后端 hooks handler 行为由 handlers/hooks/tests.rs（7 测试）钉住。

const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
}))

import SettingsView from '../SettingsView.vue'

const VALID_HOOKS = JSON.stringify({ hooks: { pre_tool_call: [{ cmd: 'echo', on: 'exec' }] } }, null, 2)

beforeEach(() => {
  requestMock.mockReset()
  useToast().toasts.splice(0)
  vi.useFakeTimers()
  requestMock.mockImplementation((_m: string, cmd: string) => {
    if (cmd === 'get') {
      return Promise.resolve({ content: VALID_HOOKS, exists: true, valid: true, summary: { total: 1, pre_tool_call: 1 } })
    }
    if (cmd === 'set') return Promise.resolve({ summary: { total: 1, pre_tool_call: 1 } })
    return Promise.resolve({})
  })
})

afterEach(() => {
  vi.useRealTimers()
})

async function mountView() {
  const w = mount(SettingsView)
  await flushPromises()
  // hooks 卡在「Hooks」tab 下
  await w.findAll('button').find(b => b.text() === 'Hooks')!.trigger('click')
  await flushPromises()
  return w
}

describe('SettingsView hooks.json 卡', () => {
  it('加载回显内容 + 脚本计数；invalid 形态渲染错误条', async () => {
    const w = await mountView()
    expect(requestMock).toHaveBeenCalledWith('hooks', 'get')
    expect(w.text()).toContain('hooks.json')
    expect(w.text()).toContain('当前 1 个脚本')

    // invalid：内容原文返回 + valid=false + error 说明
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'get') {
        return Promise.resolve({ content: '{ not json', exists: true, valid: false, error: '第 1 行: 期待对象键' })
      }
      return Promise.resolve({})
    })
    const w2 = mount(SettingsView)
    await flushPromises()
    await w2.findAll('button').find(b => b.text() === 'Hooks')!.trigger('click')
    await flushPromises()
    expect(w2.text()).toContain('期待对象键')
  })

  it('保存：语法错误 → 不发 set；合法 → set + 成功 toast（带计数）', async () => {
    const w = await mountView()
    const textarea = w.find('textarea')
    expect(textarea.exists()).toBe(true)

    // 语法坏 → 前端自检拦截，零落盘
    await textarea.setValue('{ broken')
    await w.findAll('button').find(b => b.text() === '校验并保存')!.trigger('click')
    await flushPromises()
    expect(requestMock.mock.calls.filter(c => c[1] === 'set').length).toBe(0)
    expect(useToast().toasts.some(t => t.type === 'error' && t.message.includes('JSON 语法错误'))).toBe(true)

    // 合法 → 落盘
    await textarea.setValue(VALID_HOOKS)
    await w.findAll('button').find(b => b.text() === '校验并保存')!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('hooks', 'set', { content: VALID_HOOKS })
    expect(useToast().toasts.some(t => t.type === 'success' && t.message.includes('1 个脚本'))).toBe(true)
  })

  it('后端语义拒绝 → 错误 toast（文件未写入语义）', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'get') return Promise.resolve({ content: VALID_HOOKS, exists: true, valid: true, summary: { total: 1 } })
      if (cmd === 'set') return Promise.reject(new Error('未知 hook 事件名 foo'))
      return Promise.resolve({})
    })
    const w = await mountView()
    await w.find('textarea').setValue(VALID_HOOKS)
    await w.findAll('button').find(b => b.text() === '校验并保存')!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.type === 'error' && t.message.includes('未写入') && t.message.includes('foo'))).toBe(true)
  })

  it('一键重启：stop → start', async () => {
    const w = await mountView()
    requestMock.mockClear()
    requestMock.mockResolvedValue({})
    await w.findAll('button').find(b => b.text().includes('重启 Agent'))!.trigger('click')
    await vi.advanceTimersByTimeAsync(1500)
    await flushPromises()
    expect(requestMock.mock.calls.filter(c => c[0] === 'agent').map(c => c[1])).toEqual(['stop', 'start'])
  })
})
