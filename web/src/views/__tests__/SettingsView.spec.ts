import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { useToast } from '../../composables/useToast'

// SettingsView 配置页补全批次 2/4 关键写路径：数据治理 TAB（logs 16 族
// 预设档/逐族 + checkpoint.gc + disk_watermark + debug.capture）、gateway
// host/port 可编辑化、tools TAB exec deny 词表（接线激活的死键 UI）、
// agent TAB 高频键。后端契约（set_field round-trip / C-1 CORS manager）
// 由 handlers/config tests 钉住；这里锁前端渲染分支与请求形态。

const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
  initWSAPI: vi.fn(),
}))

vi.mock('../../composables/useEditorMode', () => ({
  useEditorMode: () => ({ fullAccess: { value: true }, externalWrite: { value: false }, editorAvailable: { value: true }, setEditorAccess: vi.fn() }),
}))

import SettingsView from '../SettingsView.vue'

function cfg(over: Record<string, unknown> = {}) {
  return {
    gateway: { host: '127.0.0.1', port: 49000 },
    agents: { defaults: { max_tool_iterations: 100 } },
    logs: {},
    checkpoint: {},
    health: {},
    debug: {},
    tools: {},
    cors: {},
    ...over,
  }
}

// 有状态 mock：set_field 按 path 前缀写进可变 config 树（toggle 后回读
// 能看到新值——gc enabled 开闸后 keep_turns 才解除 disabled）。
// ⚠ get 必须 deep-clone 返回：Vue ref 赋同一对象引用不触发重渲染
// （生产后端每次 get 返回新 JSON，同引用是 mock 伪像）。
function makeStatefulImpl(initial: Record<string, unknown>) {
  const tree = initial
  function setPath(path: string, value: unknown) {
    const parts = path.split('.')
    let node = tree as Record<string, unknown>
    for (let i = 0; i < parts.length - 1; i++) {
      if (typeof node[parts[i]] !== 'object' || node[parts[i]] === null) node[parts[i]] = {}
      node = node[parts[i]] as Record<string, unknown>
    }
    node[parts[parts.length - 1]] = value
  }
  return (_m: string, cmd: string, data?: any) => {
    if (cmd === 'get') return Promise.resolve(JSON.parse(JSON.stringify(tree)))
    if (cmd === 'cors.list') return Promise.resolve({ origins: [], development_mode: false })
    if (cmd === 'lease_status') return Promise.resolve({ supported: false })
    if (cmd === 'set_field') {
      setPath(data.path, data.value)
      return Promise.resolve({ ok: true })
    }
    return Promise.resolve({})
  }
}

beforeEach(() => {
  setActivePinia(createPinia())
  requestMock.mockReset()
  requestMock.mockImplementation(makeStatefulImpl(cfg()))
  useToast().toasts.splice(0)
})

async function mountView() {
  const w = mount(SettingsView)
  await flushPromises()
  return w
}

async function toTab(w: ReturnType<typeof mount>, label: string) {
  await w.findAll('button.tab').find(b => b.text() === label)!.trigger('click')
  await flushPromises()
}

describe('SettingsView 数据治理 TAB（批次 2）', () => {
  it('16 族 TTL grid 渲染：缺席段走同源缺省（session_logs 30 / terminal 7 / exec_checkpoints 1）', async () => {
    const w = await mountView()
    await toTab(w, '数据治理')
    await w.find('[data-test="data-adv-toggle"]').trigger('click') // 展开高级逐族折叠
    const inputs = w.findAll('[data-test="ttl-family-input"]')
    expect(inputs.length).toBe(16)
    expect((inputs[0].element as HTMLInputElement).value).toBe('30') // session_logs
    expect((inputs[2].element as HTMLInputElement).value).toBe('14') // request_logs
    expect((inputs[15].element as HTMLInputElement).value).toBe('1') // exec_checkpoints
    w.unmount()
  })

  it('预设档选「放宽 90 天」→ 16 族逐族 set_field logs.*（预设 = 16 次写入一次 toast）', async () => {
    const w = await mountView()
    await toTab(w, '数据治理')
    await w.find('[data-test="ttl-preset"]').setValue('90')
    await flushPromises()
    const writes = requestMock.mock.calls.filter(c => c[1] === 'set_field')
    expect(writes.length).toBe(16)
    expect(writes.every(c => c[2].value === 90)).toBe(true)
    expect(writes.map(c => c[2].path)).toContain('logs.terminal')
    expect(writes.map(c => c[2].path)).toContain('logs.exec_checkpoints')
    expect(useToast().toasts.some(t => t.type === 'success' && t.message.includes('16 族'))).toBe(true)
    w.unmount()
  })

  it('checkpoint.gc.keep_turns 数字改动 → set_field；disk_watermark.warn_gb → set_field', async () => {
    const w = await mountView()
    await toTab(w, '数据治理')
    // gc 默认关 → keep_turns/keep_days disabled；先开闸再改数字
    // （mock 有状态：set_field 写入后回读 enabled=true，disabled 解除）
    await w.find('[data-test="gc-enable-toggle"]').trigger('click')
    await flushPromises()
    const gcTurns = w.find('[data-test="gc-keep-turns"]')
    expect((gcTurns.element as HTMLInputElement).disabled).toBe(false)
    await gcTurns.setValue('300')
    await gcTurns.trigger('change')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('config', 'set_field', { path: 'checkpoint.gc.keep_turns', value: 300 })

    const wmWarn = w.find('[data-test="wm-warn-gb"]')
    await wmWarn.setValue('10')
    await wmWarn.trigger('change')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('config', 'set_field', { path: 'health.disk_watermark.warn_gb', value: 10 })
    w.unmount()
  })

  it('debug.capture 默认开 → toggle 发 set_field false', async () => {
    const w = await mountView()
    await toTab(w, '数据治理')
    await w.find('[data-test="capture-toggle"]').trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('config', 'set_field', { path: 'debug.capture.enabled', value: false })
    w.unmount()
  })
})

describe('SettingsView Gateway / 工具 / Agent TAB（批次 4）', () => {
  it('gateway port 可编辑 → set_field gateway.port', async () => {
    const w = await mountView()
    await toTab(w, 'Gateway')
    const port = w.find('[data-test="gateway-port"]')
    expect((port.element as HTMLInputElement).value).toBe('49000')
    await port.setValue('49100')
    await port.trigger('change')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('config', 'set_field', { path: 'gateway.port', value: 49100 })
    w.unmount()
  })

  it('exec deny：开关默认关 → toggle 发 enable_deny_patterns true；词表 textarea change 发 custom_deny_patterns', async () => {
    const w = await mountView()
    await toTab(w, '工具')
    const toggle = w.find('[data-test="exec-deny-toggle"]')
    expect(toggle.classes()).not.toContain('active')
    await toggle.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('config', 'set_field', { path: 'tools.exec.enable_deny_patterns', value: true })

    // 词表 textarea 在「自定义词表」折叠内（denyAdvOpen），先展开
    await w.find('[data-test="deny-adv-toggle"]').trigger('click')
    await flushPromises()
    const ta = w.find('[data-test="exec-deny-patterns"]')
    // 转义层级：TS 源 '\\s' = 字面反斜杠+s（正则词表条目形态）
    await ta.setValue('rm -rf\\s+/\ncurl.*\\|.*sh')
    await ta.trigger('change')
    await flushPromises()
    const write = requestMock.mock.calls.filter(c => c[1] === 'set_field').find(c => c[2].path === 'tools.exec.custom_deny_patterns')
    expect(write).toBeTruthy()
    expect(write![2].value).toEqual(['rm -rf\\s+/', 'curl.*\\|.*sh']) // 按行切分、trim 空行
    w.unmount()
  })

  it('agent max_tool_iterations 数字改动 → set_field', async () => {
    const w = await mountView()
    const iters = w.find('[data-test="agent-max-tool-iterations"]')
    await iters.setValue('150')
    await iters.trigger('change')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('config', 'set_field', { path: 'agents.defaults.max_tool_iterations', value: 150 })
    w.unmount()
  })
})
