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

    // C6：分母改为动态 lspLangs.length（mock 3 语言 → 2/3），不再硬编码 5。
    expect(w.text()).toContain('2/3 可用')
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
  it('切开关 → 500ms 后一次性写 12 个字段（8 原有 + E2 并发模式 2 + N2 small_model + C6 auto_install）', async () => {
    const w = await mountView()
    await w.findAll('input[type="checkbox"]')[0].setValue(false)
    await w.findAll('input[type="checkbox"]')[1].setValue(false)

    // 未到防抖窗口：不写
    await vi.advanceTimersByTimeAsync(200)
    expect(requestMock.mock.calls.filter(c => c[1] === 'set_field').length).toBe(0)

    await vi.advanceTimersByTimeAsync(400)
    const writes = requestMock.mock.calls.filter(c => c[1] === 'set_field')
    expect(writes.length).toBe(12)
    const paths = writes.map(c => c[2].path)
    expect(paths).toContain('agents.lsp_tool.enabled')
    // C6 静默自举开关随批写入
    expect(paths).toContain('agents.lsp_tool.auto_install')
    expect(paths).toContain('agents.claude_code_tool.enabled')
    expect(paths).toContain('agents.claude_code_tool.permission_mode')
    expect(paths).toContain('agents.codex_tool.enabled')
    expect(paths).toContain('agents.codex_tool.sandbox')
    // C4 诊断闭环三字段（2026-09-05 防抖保存随组件扩展——旧断言 5 已过时）。
    expect(paths).toContain('agents.defaults.diagnostics_loop.enabled')
    expect(paths).toContain('agents.defaults.diagnostics_loop.max_errors')
    expect(paths).toContain('agents.defaults.diagnostics_loop.wait_max_ms')
    // E2 并发模式两字段
    expect(paths).toContain('agents.defaults.concurrent_request_mode')
    expect(paths).toContain('agents.defaults.queue_size')
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
    expect(requestMock.mock.calls.filter(c => c[1] === 'set_field').length).toBe(12)
  })
})

describe('CodingView 小模型杂务通道（N2）', () => {
  it('未配置回显空、写 null（= 未配置语义）；填别名写 trim 值', async () => {
    const w = await mountView(cfg({
      small_model: { configured: false, model: null, model_names: ['cheap-mini', 'main-model'] },
    }))
    const input = w.find('[data-test="small-model"]')
    expect((input.element as HTMLInputElement).value).toBe('')
    // datalist 候选来自后端 model_names
    const opts = w.findAll('#small-model-names option').map(o => (o.element as HTMLOptionElement).value)
    expect(opts).toEqual(['cheap-mini', 'main-model'])

    requestMock.mockClear()
    await input.setValue('cheap-mini')
    await vi.advanceTimersByTimeAsync(600)
    let writes = requestMock.mock.calls.filter(c => c[1] === 'set_field')
    const setWrite = writes.find(c => c[2].path === 'agents.small_model')!
    expect(setWrite[2].value).toBe('cheap-mini')

    // 清空 → 写 null（恢复未配置，serde 反序列化为 None）
    requestMock.mockClear()
    await input.setValue('  ')
    await vi.advanceTimersByTimeAsync(600)
    writes = requestMock.mock.calls.filter(c => c[1] === 'set_field')
    const clearWrite = writes.find(c => c[2].path === 'agents.small_model')!
    expect(clearWrite[2].value).toBe(null)
  })
})

describe('CodingView 并发请求模式（E2）', () => {
  it('三档回显 + reject 时容量禁用 + 切 steer 写对路径', async () => {
    const w = await mountView(cfg({
      concurrent: { mode: 'reject', queue_size: 4 },
    }))
    const modeSel = w.find('[data-test="concurrent-mode"]')
    const sizeInput = w.find('[data-test="concurrent-queue-size"]')
    expect((modeSel.element as HTMLSelectElement).value).toBe('reject')
    // 三档齐全
    const opts = modeSel.findAll('option').map(o => o.element.value)
    expect(opts).toEqual(['reject', 'queue', 'steer'])
    expect((sizeInput.element as HTMLInputElement).value).toBe('4')
    expect((sizeInput.element as HTMLInputElement).disabled).toBe(true)

    requestMock.mockClear()
    await modeSel.setValue('steer')
    await sizeInput.setValue('6')
    await vi.advanceTimersByTimeAsync(600)
    const writes = requestMock.mock.calls.filter(c => c[1] === 'set_field')
    const modeWrite = writes.find(c => c[2].path === 'agents.defaults.concurrent_request_mode')!
    const sizeWrite = writes.find(c => c[2].path === 'agents.defaults.queue_size')!
    expect(modeWrite[2].value).toBe('steer')
    expect(sizeWrite[2].value).toBe(6)
  })

  it('queue 档容量可编辑；缺省回显 queue/8（E1 默认）', async () => {
    const w = await mountView(cfg({})) // 无 concurrent 段 → 前端缺省回显
    const modeSel = w.find('[data-test="concurrent-mode"]')
    const sizeInput = w.find('[data-test="concurrent-queue-size"]')
    expect((modeSel.element as HTMLSelectElement).value).toBe('queue')
    expect((sizeInput.element as HTMLInputElement).value).toBe('8')
    expect((sizeInput.element as HTMLInputElement).disabled).toBe(false)
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
