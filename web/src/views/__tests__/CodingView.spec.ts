import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { useToast } from '../../composables/useToast'

// M6 补测（quality-hardening goal 2026-08-25）：P2-1 代码开发页 ——
// 双请求加载（config + lsp_status，语言表来自后端不硬编码）、
// 初始加载不发写、防抖保存 5 字段、一键重启顺序。

const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
}))

import CodingView from '../CodingView.vue'

function cfg(over: Record<string, unknown> = {}) {
  return {
    lsp: { enabled: true },
    claude_code: { enabled: true, permission_mode: 'plan' },
    codex: { enabled: false, sandbox: 'workspace_write' },
    ...over,
  }
}

function lsp(langs: [string, string, string, boolean][], wouldRegister = true) {
  const available = langs.filter(l => l[3]).length
  return {
    languages: langs.map(([lang, label, command, ok]) => ({ lang, label, command, available: ok })),
    available_count: available,
    tool_would_register: wouldRegister && available > 0,
  }
}

beforeEach(() => {
  requestMock.mockReset()
  useToast().toasts.splice(0)
  vi.useFakeTimers()
})

afterEach(() => {
  vi.useRealTimers()
})

async function mountView(config = cfg(), status = lsp([
  ['rust', 'Rust', 'rust-analyzer', true],
  ['typescript', 'TypeScript', 'typescript-language-server', true],
  ['python', 'Python', 'pyright-langserver', false],
])) {
  requestMock.mockImplementation((_m: string, cmd: string) => {
    if (cmd === 'config') return Promise.resolve(config)
    if (cmd === 'lsp_status') return Promise.resolve(status)
    return Promise.resolve({})
  })
  const w = mount(CodingView)
  await flushPromises() // onMounted loadAll
  return w
}

describe('CodingView 加载', () => {
  it('config + lsp_status 双请求，语言表与档位回显来自后端', async () => {
    const w = await mountView()
    expect(requestMock).toHaveBeenCalledWith('coding', 'config')
    expect(requestMock).toHaveBeenCalledWith('coding', 'lsp_status')

    expect(w.text()).toContain('2/5 可用')
    expect(w.text()).toContain('Rust')
    expect(w.text()).toContain('rust-analyzer')
    expect(w.text()).toContain('已安装')
    expect(w.text()).toContain('未安装')
    // 档位回显
    expect((w.findAll('select')[0].element as HTMLSelectElement).value).toBe('plan')
    expect((w.findAll('select')[1].element as HTMLSelectElement).value).toBe('workspace_write')
  })

  it('初始加载不发任何写请求（configInitialized 守卫）', async () => {
    await mountView()
    const writes = requestMock.mock.calls.filter(c => c[1] === 'set_field')
    expect(writes.length).toBe(0)
  })

  it('加载失败 → 错误 toast', async () => {
    requestMock.mockRejectedValue(new Error('WS 断开'))
    mount(CodingView)
    await flushPromises()
    expect(useToast().toasts.some(t => t.type === 'error' && t.message.includes('WS 断开'))).toBe(true)
  })
})

describe('CodingView 防抖保存', () => {
  it('切开关 → 500ms 后一次性写 5 个字段', async () => {
    const w = await mountView()
    await w.findAll('input[type="checkbox"]')[0].setValue(false)
    await w.findAll('input[type="checkbox"]')[1].setValue(false)

    // 未到防抖窗口：不写
    await vi.advanceTimersByTimeAsync(200)
    expect(requestMock.mock.calls.filter(c => c[1] === 'set_field').length).toBe(0)

    await vi.advanceTimersByTimeAsync(400)
    const writes = requestMock.mock.calls.filter(c => c[1] === 'set_field')
    expect(writes.length).toBe(5)
    const paths = writes.map(c => c[2].path)
    expect(paths).toContain('agents.lsp_tool.enabled')
    expect(paths).toContain('agents.claude_code_tool.enabled')
    expect(paths).toContain('agents.claude_code_tool.permission_mode')
    expect(paths).toContain('agents.codex_tool.enabled')
    expect(paths).toContain('agents.codex_tool.sandbox')
    const lspWrite = writes.find(c => c[2].path === 'agents.lsp_tool.enabled')!
    expect(lspWrite[2].value).toBe(false)
    expect(useToast().toasts.some(t => t.type === 'success' && t.message.includes('重启 Agent'))).toBe(true)
  })

  it('连点防抖折叠：两次变更只写一批', async () => {
    const w = await mountView()
    await w.findAll('input[type="checkbox"]')[0].setValue(false)
    await vi.advanceTimersByTimeAsync(300)
    await w.findAll('input[type="checkbox"]')[0].setValue(true)
    await vi.advanceTimersByTimeAsync(600)
    expect(requestMock.mock.calls.filter(c => c[1] === 'set_field').length).toBe(5)
  })
})

describe('CodingView 重启 Agent', () => {
  it('stop → start 顺序调用，成功 toast；失败不崩', async () => {
    const w = await mountView()
    requestMock.mockClear()
    requestMock.mockResolvedValue({})
    await w.findAll('button').find(b => b.text().includes('重启 Agent'))!.trigger('click')
    await vi.advanceTimersByTimeAsync(1500) // 跨过内置 1s 间隔
    await flushPromises()

    const agentCalls = requestMock.mock.calls.filter(c => c[0] === 'agent').map(c => c[1])
    expect(agentCalls).toEqual(['stop', 'start'])
    expect(useToast().toasts.some(t => t.type === 'success' && t.message.includes('已重启'))).toBe(true)

    // 失败路径
    requestMock.mockRejectedValue(new Error('agent 忙'))
    const btn = w.findAll('button').find(b => b.text().includes('重启 Agent'))!
    await btn.trigger('click')
    await vi.advanceTimersByTimeAsync(1500)
    await flushPromises()
    expect(useToast().toasts.some(t => t.type === 'error' && t.message.includes('agent 忙'))).toBe(true)
    expect(btn.attributes('disabled')).toBeUndefined()
  })
})
