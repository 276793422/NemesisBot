import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { useToast } from '../../composables/useToast'

// G6 (Phase4-a / Y1)：工具页折叠开关卡 ——
// config.get 回显 enabled/expand_top_n；toggle 走 config.set_field；
// top_n 编辑校验正整数；配置读取失败显示「不可用」。

const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
}))

import ToolsView from '../ToolsView.vue'

beforeEach(() => {
  requestMock.mockReset()
  useToast().toasts.splice(0)
  requestMock.mockImplementation((_m: string, cmd: string) => {
    if (cmd === 'get') {
      // tools.get（笔记内容）与 config.get 用同一 cmd 名 —— 按模块区分。
      return Promise.resolve({ content: '' })
    }
    return Promise.resolve({})
  })
})

// tools 模块和 config 模块的 get 都叫 "get"，用模块参数区分实现。
function withConfig(folding: unknown) {
  requestMock.mockImplementation((m: string, cmd: string) => {
    if (m === 'tools' && cmd === 'get') return Promise.resolve({ content: '# notes' })
    if (m === 'config' && cmd === 'get') return Promise.resolve({ agents: { tool_doc_folding: folding } })
    return Promise.resolve({ updated: true })
  })
}

async function mountView() {
  const w = mount(ToolsView)
  await flushPromises()
  return w
}

describe('ToolsView 工具文档折叠开关（G6）', () => {
  it('回显 enabled=false + expand_top_n=8；开关走 set_field', async () => {
    withConfig({ enabled: false, expand_top_n: 8 })
    const w = await mountView()

    expect(w.find('.toggle.active').exists()).toBe(false)
    expect(w.text()).toContain('已关闭')

    await w.find('.toggle').trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('config', 'set_field', {
      path: 'agents.tool_doc_folding.enabled', value: true,
    })
    expect(w.find('.toggle.active').exists()).toBe(true)
    expect(w.text()).toContain('已开启')
    expect(useToast().toasts.some(t => t.type === 'success' && t.message.includes('即时生效'))).toBe(true)
    // 开启后出现保留数 + 调整入口
    expect(w.text()).toContain('保留 8 个')
    wrapper_clean(w)
  })

  it('开启态回显 expand_top_n；调整 → saveTopN 非法值拒绝、合法值 set_field', async () => {
    withConfig({ enabled: true, expand_top_n: 12 })
    const w = await mountView()
    expect(w.find('.toggle.active').exists()).toBe(true)
    expect(w.text()).toContain('保留 12 个')

    await w.findAll('button').find(b => b.text() === '调整')!.trigger('click')
    const input = w.find('.folding-topn-input')

    // 非法：0 / 小数
    await input.setValue('0')
    await w.findAll('button').find(b => b.text() === '保存')!.trigger('click')
    await flushPromises()
    expect(requestMock.mock.calls.filter(c => c[2]?.path === 'agents.tool_doc_folding.expand_top_n').length).toBe(0)
    expect(useToast().toasts.some(t => t.type === 'error' && t.message.includes('正整数'))).toBe(true)

    // 合法：5
    await input.setValue('5')
    await w.findAll('button').find(b => b.text() === '保存')!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('config', 'set_field', {
      path: 'agents.tool_doc_folding.expand_top_n', value: 5,
    })
    expect(w.text()).toContain('保留 5 个')
    wrapper_clean(w)
  })

  it('config.get 失败 → 「配置读取不可用」，无开关可点', async () => {
    requestMock.mockImplementation((m: string, cmd: string) => {
      if (m === 'tools' && cmd === 'get') return Promise.resolve({ content: '' })
      if (m === 'config' && cmd === 'get') return Promise.reject(new Error('down'))
      return Promise.resolve({})
    })
    const w = await mountView()
    expect(w.find('.folding-unavailable').exists()).toBe(true)
    expect(w.find('.toggle').exists()).toBe(false)
    wrapper_clean(w)
  })
})

function wrapper_clean(w: { unmount: () => void }) {
  w.unmount()
}
