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

// ---------------------------------------------------------------------------
// gap 补测（覆盖率批次）：CORS、Gateway host/租约探针、工具 TAB
// （secret / i2c / 数字钳制）、服务开关与高级 JSON、日志 TAB、Agent 批次 4
// 键与实验区、数据治理补遗（预设高亮 / 单族非法 / gc keep_days / 水位）、
// 编辑器放行 / 会话放行、配色、原始 JSON、皮肤。
// 遵循上方既有约定（stateful config mock + setActivePinia）。
// ---------------------------------------------------------------------------

import { useSessionStore } from '../../stores/session'
import { _resetSessionAllowForTest } from '../../composables/useSessionAllowAll'
import { mount as rawMount } from '@vue/test-utils'

/** 按 form-group 首个 label 精确定位开关（避免「首个未激活 toggle」误中）。 */
function toggleByLabel(w: ReturnType<typeof mount>, label: string) {
  const g = w.findAll('.form-group').find(x => {
    const l = x.find('label.form-label')
    return l.exists() && l.text() === label
  })
  return g!.find('.toggle')
}

/** 非状态化 impl：get 恒返回 fixed（lease / cors 三态渲染用）。 */
function makeFixedImpl(initial: Record<string, unknown>, handlers: Record<string, (data?: any) => unknown> = {}) {
  return (_m: string, cmd: string, data?: any) => {
    if (cmd === 'get') return Promise.resolve(JSON.parse(JSON.stringify(initial)))
    if (handlers[cmd]) return Promise.resolve(handlers[cmd](data))
    return Promise.resolve({})
  }
}

describe('SettingsView CORS TAB（gap 补测）', () => {
  it('origins 渲染 + 开发模式开关 + 添加/移除/空值守卫', async () => {
    requestMock.mockImplementation(makeFixedImpl(cfg(), {
      'cors.list': () => ({ origins: ['https://a.com', 'https://b.com'], development_mode: true }),
    }))
    const w = await mountView()
    await toTab(w, 'CORS')
    expect(w.text()).toContain('https://a.com')
    expect(w.text()).toContain('https://b.com')
    expect(w.text()).not.toContain('CORS 配置未加载')
    // 开发模式开 → toggle active
    const headerToggle = w.find('.card-header .toggle')
    expect(headerToggle.classes()).toContain('active')

    // 添加：v-model 输入 + 按钮
    const input = w.find('input.form-input')
    await input.setValue('https://c.com')
    await w.findAll('button').find(b => b.text() === '添加')!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('config', 'cors.add', { origin: 'https://c.com' })
    expect(useToast().toasts.some(t => t.type === 'success' && t.message === '已添加')).toBe(true)
    expect((w.element as HTMLElement).querySelector<HTMLInputElement>('input.form-input')!.value).toBe('')

    // 移除第一个 origin
    requestMock.mockClear()
    await w.findAll('button').find(b => b.text() === '移除')!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('config', 'cors.remove', { origin: 'https://a.com' })
    expect(useToast().toasts.some(t => t.type === 'success' && t.message === '已移除')).toBe(true)

    // 空输入守卫：不再发 cors.add
    requestMock.mockClear()
    await w.findAll('button').find(b => b.text() === '添加')!.trigger('click')
    expect(requestMock.mock.calls.filter(c => c[1] === 'cors.add').length).toBe(0)

    // 开发模式 toggle：true → false
    await headerToggle.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('config', 'cors.toggle', { enabled: false })
    expect(useToast().toasts.some(t => t.message === '已禁用')).toBe(true)
    w.unmount()
  })

  it('添加/移除/切换 失败各自提示；cors.list 失败显示错误横幅', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'get') return Promise.resolve(cfg())
      if (cmd === 'cors.list') return Promise.resolve({ origins: ['https://a.com'], development_mode: false })
      if (cmd === 'cors.add' || cmd === 'cors.remove' || cmd === 'cors.toggle') return Promise.reject(new Error('locked'))
      return Promise.resolve({})
    })
    const w = await mountView()
    await toTab(w, 'CORS')
    await w.find('input.form-input').setValue('https://x.com')
    await w.findAll('button').find(b => b.text() === '添加')!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('添加失败'))).toBe(true)
    await w.findAll('button').find(b => b.text() === '移除')!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('移除失败'))).toBe(true)
    await w.find('.card-header .toggle').trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('操作失败'))).toBe(true)

    // cors.list 失败（重新挂载）→ 诚实错误横幅
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'get') return Promise.resolve(cfg())
      if (cmd === 'cors.list') return Promise.reject(new Error('cors.json 解析失败'))
      return Promise.resolve({})
    })
    const w2 = await mountView()
    await toTab(w2, 'CORS')
    expect(w2.text()).toContain('CORS 配置未加载')
    expect(w2.text()).toContain('cors.json 解析失败')
    w2.unmount()
    w.unmount()
  })
})

describe('SettingsView Gateway：host 与租约探针（gap 补测）', () => {
  it('host 空回退 / 成功提示；port 非法回退', async () => {
    const w = await mountView()
    await toTab(w, 'Gateway')
    const host = w.find('[data-test="gateway-host"]')
    // 空值 → 回退 config 值
    await host.setValue('')
    await flushPromises()
    expect((host.element as HTMLInputElement).value).toBe('127.0.0.1')
    expect(useToast().toasts.some(t => t.message.includes('host 不能为空'))).toBe(true)
    // DEBUG
    expect(requestMock.mock.calls.filter(c => c[2]?.path === 'gateway.host').length).toBe(0)

    // 合法 → set_field + info 提示
    await host.setValue('0.0.0.0')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('config', 'set_field', { path: 'gateway.host', value: '0.0.0.0' })
    expect(useToast().toasts.some(t => t.type === 'info' && t.message.includes('host 改动需重启 gateway'))).toBe(true)

    // port 越界 → 回退 config 值 49000
    const port = w.find('[data-test="gateway-port"]')
    await port.setValue('70000')
    await flushPromises()
    expect((port.element as HTMLInputElement).value).toBe('49000')
    expect(useToast().toasts.some(t => t.message.includes('端口须为 1-65535'))).toBe(true)

    // port NaN → 同样回退
    await port.setValue('abc')
    await flushPromises()
    expect((port.element as HTMLInputElement).value).toBe('49000')
    w.unmount()
  })

  it('租约三态：held / free / 未装配 + 刷新按钮 + 时间格式化', async () => {
    requestMock.mockImplementation(makeFixedImpl(cfg(), {
      lease_status: () => ({ supported: true, held: true, holder: 'agent-x', acquired_at: '2026-01-02T03:04:05Z', note: '持有中注记' }),
    }))
    const w = await mountView()
    await toTab(w, 'Gateway')
    expect(w.text()).toContain('被占用')
    expect(w.text()).toContain('agent-x')
    expect(w.text()).toContain('持有中注记')
    // 合法时间 → toLocaleString 渲染（含时分）
    expect(w.find('.lease-held').text()).toMatch(/2026/)

    // 空闲态 + 非法时间原样回显
    requestMock.mockImplementation(makeFixedImpl(cfg(), {
      lease_status: () => ({ supported: true, held: false, acquired_at: 'not-a-date' }),
    }))
    await w.findAll('button').find(b => b.text() === '刷新')!.trigger('click')
    await flushPromises()
    expect(w.text()).toContain('空闲')
    expect(w.find('.lease-held').exists()).toBe(false)

    // 未装配
    requestMock.mockImplementation(makeFixedImpl(cfg(), {
      lease_status: () => ({ supported: false, note: 'headless' }),
    }))
    await w.findAll('button').find(b => b.text() === '刷新')!.trigger('click')
    await flushPromises()
    expect(w.text()).toContain('租约未装配（headless）')

    // 探针失败 → 状态未知
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'get') return Promise.resolve(cfg())
      if (cmd === 'lease_status') return Promise.reject(new Error('x'))
      return Promise.resolve({})
    })
    await w.findAll('button').find(b => b.text() === '刷新')!.trigger('click')
    await flushPromises()
    expect(w.text()).toContain('状态未知')
    w.unmount()
  })
})

describe('SettingsView 工具 TAB：secret / i2c / 数字钳制（gap 补测）', () => {
  it('saveSecret：空不动存量；掩码串拒绝；合法值落盘并清空输入', async () => {
    const w = await mountView()
    await toTab(w, '工具')
    const key = w.find('[data-test="brave-api-key"]')
    // 空输入 = 未表态
    await key.setValue('')
    await flushPromises()
    expect(requestMock.mock.calls.filter(c => c[2]?.path === 'tools.web.brave.api_key').length).toBe(0)

    // 掩码串拒绝
    await key.setValue('sk-abc****def')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('检测到掩码串'))).toBe(true)
    expect(requestMock.mock.calls.filter(c => c[2]?.path === 'tools.web.brave.api_key').length).toBe(0)
    expect((key.element as HTMLInputElement).value).toBe('')

    // 合法值落盘
    await key.setValue('real-key-123')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('config', 'set_field', { path: 'tools.web.brave.api_key', value: 'real-key-123' })
    expect((key.element as HTMLInputElement).value).toBe('') // 写入端掩码：输入框恒空
    w.unmount()
  })

  it('i2c 地址段：合法落盘二维数组；非法各形态拦截', async () => {
    const w = await mountView()
    await toTab(w, '工具')
    await w.findAll('span').find(s => s.text().includes('I2C 地址段高级'))!.trigger('click')
    await flushPromises()
    const areas = w.findAll('textarea')
    expect(areas.length).toBe(2)
    // 合法：两段
    await areas[0]!.setValue('8-119\n120-127')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('config', 'set_field', {
      path: 'tools.hardware.i2c_allow_ranges',
      value: [[8, 119], [120, 127]],
    })
    // 空行跳过
    await areas[0]!.setValue('\n  \n9-9')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('config', 'set_field', {
      path: 'tools.hardware.i2c_allow_ranges',
      value: [[9, 9]],
    })
    // 非法：lo > hi / 非数字 / 越界 → toast + 不落盘
    for (const bad of ['9-8', 'abc', '300-400', '1-2-3']) {
      await areas[0]!.setValue(bad)
      await flushPromises()
      expect(useToast().toasts.some(t => t.message.includes('格式须为 lo-hi'))).toBe(true)
    }
    // deny 侧合法落盘
    await areas[1]!.setValue('0-7')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('config', 'set_field', {
      path: 'tools.hardware.i2c_deny_ranges',
      value: [[0, 7]],
    })
    w.unmount()
  })

  it('搜索源数字钳制与开关；image_gen 模型空值落 null', async () => {
    const w = await mountView()
    await toTab(w, '工具')
    // Brave 结果条数 > 20 → 回退 5
    const nums = w.findAll('input[type="number"]')
    // 工具 TAB number inputs 顺序：brave.max_results(5), ddg.max_results(5), perplexity.max_results(5), image_gen.timeout(120)
    expect((nums[0]!.element as HTMLInputElement).value).toBe('5')
    await nums[0]!.setValue('30')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('≥1') && t.message.includes('≤20') && t.message.includes('已回退 5'))).toBe(true)
    expect(requestMock.mock.calls.filter(c => c[2]?.path === 'tools.web.brave.max_results').length).toBe(0)

    // 合法值落盘
    await nums[1]!.setValue('8')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('config', 'set_field', { path: 'tools.web.duckduckgo.max_results', value: 8 })

    // Brave 开关（默认关）→ 开
    const toggles = w.findAll('.toggle')
    await toggles[0]!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('config', 'set_field', { path: 'tools.web.brave.enabled', value: true })

    // image_gen 模型：非空 trim 落盘；空值落 null
    const modelInput = w.findAll('input.form-input').find(i => i.attributes('placeholder')?.includes('模型别名'))!
    await modelInput.setValue('  images-x  ')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('config', 'set_field', { path: 'tools.image_gen.model', value: 'images-x' })
    await modelInput.setValue('')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('config', 'set_field', { path: 'tools.image_gen.model', value: null })
    w.unmount()
  })
})

describe('SettingsView 服务开关 TAB（gap 补测）', () => {
  it('六个开关双向、心跳间隔钳制、终端并发、shell 覆盖、高级 JSON 展示', async () => {
    requestMock.mockImplementation(makeStatefulImpl(cfg({
      devices: { monitor_usb: false, enabled: true },
      session: { dm_scope: 'per-peer', identity_links: { a: 'b' } },
      bindings: [{ agent_id: 'main' }],
      // executor.persistent 刻意缺席 → prettyOr 走「未配置」fallback 分支
    })))
    const w = await mountView()
    await toTab(w, '服务开关')
    // 开关双向：Heartbeat（undefined ≠ false → active）点击关
    const toggles = w.findAll('.card-body > div .toggle, .form-group .toggle')
    await toggles[0]!.trigger('click') // Heartbeat
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('config', 'set_field', { path: 'heartbeat.enabled', value: false })
    // USB 监控（初始 false → 非 active）按行定位点击 → 开
    const usbRow = w.findAll('div').find(d => d.text() === 'USB 监控')!
    await usbRow.find('.toggle').trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('config', 'set_field', { path: 'devices.monitor_usb', value: true })

    // 心跳间隔：< 5 回退 30；合法落盘
    const hb = w.find('[data-test="heartbeat-interval"]')
    await hb.setValue('2')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('≥5') && t.message.includes('已回退 30'))).toBe(true)
    await hb.setValue('45')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('config', 'set_field', { path: 'heartbeat.interval', value: 45 })

    // 终端并发：> 32 回退 4；合法落盘
    const maxSessions = w.findAll('input[type="number"]').find(i => (i.element as HTMLInputElement).value === '4')!
    await maxSessions.setValue('40')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('≥1') && t.message.includes('≤32') && t.message.includes('已回退 4'))).toBe(true)
    await maxSessions.setValue('8')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('config', 'set_field', { path: 'terminal.max_sessions', value: 8 })

    // 终端 shell 覆盖高级：空值落 null；非空 trim 落盘
    await w.findAll('span').find(s => s.text().includes('终端 shell 覆盖高级'))!.trigger('click')
    await flushPromises()
    const shellInput = w.findAll('input').find(i => i.attributes('placeholder')?.includes('平台默认'))!
    await shellInput.setValue('/bin/zsh')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('config', 'set_field', { path: 'terminal.shell', value: '/bin/zsh' })
    await shellInput.setValue('  ')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('config', 'set_field', { path: 'terminal.shell', value: null })

    // 高级 JSON 只读折叠：三块渲染 + prettyOr 两分支
    await w.find('[data-test="adv-json-toggle"]').trigger('click')
    await flushPromises()
    expect(w.text()).toContain('bindings（agent 绑定表）')
    expect(w.text()).toContain('session（会话身份）')
    expect(w.text()).toContain('executor.persistent（常驻执行体）')
    // bindings/session 配置在场 → JSON；executor.persistent 缺席 → fallback 注记
    expect(w.text()).toContain('（未配置 — 全关缺省')
    expect(w.text()).toContain('"agent_id": "main"')
    expect(w.text()).toContain('"dm_scope": "per-peer"')
    w.unmount()
  })
})

describe('SettingsView 日志 TAB（gap 补测）', () => {
  it('级别下拉 / 通用与 LLM 开关 / 路径与详细度', async () => {
    const w = await mountView()
    await toTab(w, '日志')
    const selects = w.findAll('select.form-select')
    expect(selects.length).toBe(2)
    await selects[0]!.setValue('debug')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('config', 'set_field', { path: 'logging.general.level', value: 'debug' })
    await selects[1]!.setValue('truncated')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('config', 'set_field', { path: 'logging.llm.detail_level', value: 'truncated' })

    // 通用日志开关（默认开）→ 关
    const toggles = w.findAll('.toggle')
    await toggles[0]!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('config', 'set_field', { path: 'logging.general.enabled', value: false })
    // 控制台输出 → 关
    await toggles[1]!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('config', 'set_field', { path: 'logging.general.enable_console', value: false })
    // LLM 通信日志（默认关）→ 开
    await toggles[2]!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('config', 'set_field', { path: 'logging.llm.enabled', value: true })
    // save_raw → 开
    await toggles[3]!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('config', 'set_field', { path: 'logging.llm.save_raw', value: true })

    // 路径输入 trim 落盘
    const dirInput = w.findAll('input.form-input').find(i => i.attributes('placeholder')?.includes('logs/llm'))!
    await dirInput.setValue(' /abs/dir ')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('config', 'set_field', { path: 'logging.llm.log_dir', value: '/abs/dir' })
    const fileInput = w.findAll('input.form-input').find(i => i.attributes('placeholder')?.includes('只落默认位置'))!
    await fileInput.setValue('logs/app.log')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('config', 'set_field', { path: 'logging.general.file', value: 'logs/app.log' })
    w.unmount()
  })
})

describe('SettingsView Agent TAB 批次 4 键与实验区（gap 补测）', () => {
  it('温度/tokens/限流/钳制族/开关/select/hidden_tools/guardian_model', async () => {
    const w = await mountView()
    // 温度（inline saveField parseFloat）
    const temp = w.findAll('input[type="number"]').find(i => (i.element as HTMLInputElement).value === '0.7')!
    await temp.setValue('0.9')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('config', 'set_field', { path: 'agents.defaults.temperature', value: 0.9 })
    // max_tokens
    const tokens = w.findAll('input[type="number"]').find(i => (i.element as HTMLInputElement).value === '4096')!
    await tokens.setValue('2048')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('config', 'set_field', { path: 'agents.defaults.max_tokens', value: 2048 })
    // restrict_to_workspace 开关（默认开）→ 关
    await w.findAll('.toggle')[0]!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('config', 'set_field', { path: 'agents.defaults.restrict_to_workspace', value: false })
    // rate_limit_retries 非法 → 回退 10
    const retries = w.findAll('input[type="number"]').find(i => (i.element as HTMLInputElement).value === '10')!
    await retries.setValue('abc')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('须为 ≥0 的数字（已回退 10）'))).toBe(true)
    expect((retries.element as HTMLInputElement).value).toBe('10')
    // max_output_ceiling < 1024 → 回退 262144
    const ceiling = w.findAll('input[type="number"]').find(i => (i.element as HTMLInputElement).value === '262144')!
    await ceiling.setValue('100')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('≥1024') && t.message.includes('已回退 262144'))).toBe(true)
    // subagent.max_depth > 8 → 回退 1
    const depth = w.findAll('input[type="number"]').find(i => (i.element as HTMLInputElement).value === '1')!
    await depth.setValue('9')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('≥0') && t.message.includes('≤8') && t.message.includes('已回退 1'))).toBe(true)
    // spill_retention_days < 1 → 回退 7
    const spill = w.findAll('input[type="number"]').find(i => (i.element as HTMLInputElement).value === '7')!
    await spill.setValue('0')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('已回退 7'))).toBe(true)
    // session_token_budget 合法
    const budget = w.findAll('input[type="number"]').find(i => (i.element as HTMLInputElement).value === '0')!
    await budget.setValue('5000')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('config', 'set_field', { path: 'agents.defaults.session_token_budget', value: 5000 })
    // tool_result_cap 非法 → 回退 8000
    const cap = w.findAll('input[type="number"]').find(i => (i.element as HTMLInputElement).value === '8000')!
    await cap.setValue('xyz')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('已回退 8000'))).toBe(true)
    // format_on_save 开关（默认关）→ 开
    await toggleByLabel(w, '保存后格式化（format_on_save）').trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('config', 'set_field', { path: 'agents.defaults.format_on_save.enabled', value: true })
    // image_downscale（默认开）→ 关
    await toggleByLabel(w, '图片自动降采样（image_downscale）').trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('config', 'set_field', { path: 'agents.image_downscale', value: false })
    // cache_warmer 开关（默认关）→ 开；cost_limit 非法 → 回退 0.05
    await toggleByLabel(w, 'Prompt 缓存预热（cache_warmer）').trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('config', 'set_field', { path: 'agents.cache_warmer.enabled', value: true })
    const cost = w.findAll('input[type="number"]').find(i => (i.element as HTMLInputElement).value === '0.05')!
    await cost.setValue('nan!')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('已回退 0.05'))).toBe(true)
    // instruction_trust select
    const trust = w.findAll('select.form-select').find(s => s.findAll('option').some(o => o.element.value === 'isolated'))!
    await trust.setValue('isolated')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('config', 'set_field', { path: 'projects.instruction_trust', value: 'isolated' })
    // guardian_model：trim 落盘；空 → null
    const guardian = w.findAll('input.form-input').find(i => i.attributes('placeholder')?.includes('回落阶梯'))!
    await guardian.setValue(' gpt-audit ')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('config', 'set_field', { path: 'agents.guardian_model', value: 'gpt-audit' })
    await guardian.setValue('')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('config', 'set_field', { path: 'agents.guardian_model', value: null })
    // hidden_tools：多分隔符切分
    const hidden = w.find('[data-test="agent-hidden-tools"]')
    await hidden.setValue('exec, registry_write；foo  bar，，baz')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('config', 'set_field', {
      path: 'agents.hidden_tools',
      value: ['exec', 'registry_write', 'foo', 'bar', 'baz'],
    })
    w.unmount()
  })

  it('实验性高级区：prompt_system / discipline / mcp_lazy_supply', async () => {
    const w = await mountView()
    await w.find('[data-test="exp-adv-toggle"]').trigger('click')
    await flushPromises()
    expect(w.text()).toContain('实验面：改动影响提示词形态')
    const ps = w.find('[data-test="prompt-system"]')
    await ps.setValue('classic')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('config', 'set_field', { path: 'agents.prompt_system', value: 'classic' })
    // discipline（默认关）→ 开
    await toggleByLabel(w, '纪律闸（discipline.enabled）').trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('config', 'set_field', { path: 'agents.discipline.enabled', value: true })
    // mcp_lazy_supply → 开
    await toggleByLabel(w, '工具 schema 惰性供给（mcp_lazy_supply）').trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('config', 'set_field', { path: 'agents.mcp_lazy_supply', value: true })
    w.unmount()
  })
})

describe('SettingsView 数据治理补遗（gap 预设高亮 / 非法 / gc / 水位）', () => {
  const allLogs30: Record<string, number> = {}
  for (const k of ['session_logs', 'boundary', 'request_logs', 'terminal', 'event_ledger', 'trash', 'gateway', 'cluster_logs', 'eval', 'capture', 'dreaming', 'calibration', 'deliveries', 'heartbeat', 'ziwei', 'exec_checkpoints']) allLogs30[k] = 30

  it('全族同值 → 预设档高亮 30；选 7 → 16 次 set_field；单族非法回退', async () => {
    requestMock.mockImplementation(makeStatefulImpl(cfg({ logs: allLogs30 })))
    const w = await mountView()
    await toTab(w, '数据治理')
    const preset = w.find('[data-test="ttl-preset"]')
    expect((preset.element as HTMLSelectElement).value).toBe('30')
    await preset.setValue('7')
    await flushPromises()
    const writes = requestMock.mock.calls.filter(c => c[1] === 'set_field' && c[2].path.startsWith('logs.'))
    expect(writes.length).toBe(16)
    expect(writes.every(c => c[2].value === 7)).toBe(true)
    expect(useToast().toasts.some(t => t.type === 'success' && t.message.includes('16 族') && t.message.includes('7 天'))).toBe(true)
    // 写盘后回读 7 → 预设仍高亮 7
    await flushPromises()
    expect((w.find('[data-test="ttl-preset"]').element as HTMLSelectElement).value).toBe('7')

    // 单族非法：NaN 与负数 → 回退 + toast，不落盘
    requestMock.mockClear()
    await w.find('[data-test="data-adv-toggle"]').trigger('click')
    await flushPromises()
    const firstInput = w.find('[data-test="ttl-family-input"]')
    await firstInput.setValue('abc')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('保留天数须为 ≥0 整数'))).toBe(true)
    expect((firstInput.element as HTMLInputElement).value).toBe('7')
    expect(requestMock.mock.calls.filter(c => c[1] === 'set_field' && c[2].path === 'logs.session_logs').length).toBe(0)
    await firstInput.setValue('-3')
    await flushPromises()
    expect(useToast().toasts.filter(t => t.message.includes('保留天数须为 ≥0 整数')).length).toBe(2)
    // 单族合法改动 → saveTtlFamily 有效分支
    await firstInput.setValue('15')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('config', 'set_field', { path: 'logs.session_logs', value: 15 })
    w.unmount()
  })

  it('预设中途失败 → 「预设失败」toast；loadConfig 仍执行', async () => {
    requestMock.mockImplementation((_m: string, cmd: string, data?: any) => {
      if (cmd === 'get') return Promise.resolve(cfg())
      if (cmd === 'cors.list') return Promise.resolve({ origins: [], development_mode: false })
      if (cmd === 'lease_status') return Promise.resolve({ supported: false })
      if (cmd === 'set_field') {
        return data.path === 'logs.trash' ? Promise.reject(new Error('ro')) : Promise.resolve({ ok: true })
      }
      return Promise.resolve({})
    })
    const w = await mountView()
    await toTab(w, '数据治理')
    await w.find('[data-test="ttl-preset"]').setValue('90')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('预设失败'))).toBe(true)
    expect(requestMock.mock.calls.some(c => c[1] === 'get')).toBe(true)
    w.unmount()
  })

  it('checkpoint.gc keep_days 与磁盘水位三键', async () => {
    const w = await mountView()
    await toTab(w, '数据治理')
    await w.find('[data-test="gc-enable-toggle"]').trigger('click')
    await flushPromises()
    const gcCard = w.findAll('.card').find(c => c.text().includes('checkpoint.gc'))!
    const keepDays = gcCard.findAll('input[type="number"]').find(i => (i.element as HTMLInputElement).value === '7')!
    await keepDays.setValue('14')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('config', 'set_field', { path: 'checkpoint.gc.keep_days', value: 14 })

    // 水位开关（默认开）→ 关
    const wmCard = w.findAll('.card').find(c => c.text().includes('disk_watermark'))!
    await wmCard.findAll('.toggle')[0]!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('config', 'set_field', { path: 'health.disk_watermark.enabled', value: false })
    // critical_gb / interval_secs
    const critical = wmCard.findAll('input[type="number"]').find(i => (i.element as HTMLInputElement).value === '1')!
    await critical.setValue('2')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('config', 'set_field', { path: 'health.disk_watermark.critical_gb', value: 2 })
    const interval = wmCard.findAll('input[type="number"]').find(i => (i.element as HTMLInputElement).value === '60')!
    await interval.setValue('120')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('config', 'set_field', { path: 'health.disk_watermark.interval_secs', value: 120 })
    w.unmount()
  })
})

describe('SettingsView 编辑器放行 / 会话放行 / 配色 / 原始 JSON / 皮肤（gap 补测）', () => {
  beforeEach(() => {
    _resetSessionAllowForTest()
  })

  it('编辑器 TAB：Full Access 与 外部写删放行 开关可点', async () => {
    const w = await mountView()
    await toTab(w, '编辑器')
    expect(w.text()).toContain('编辑器放行开关')
    const toggles = w.findAll('.toggle')
    expect(toggles[0]!.classes()).toContain('active') // fullAccess true（mock 固定）
    await toggles[0]!.trigger('click') // toggleEditorFull → setEditorAccess(false, false)
    await flushPromises()
    await toggles[1]!.trigger('click') // toggleEditorExt → fullAccess true → setEditorAccess(true, true)
    await flushPromises()
    // 会话放行（无会话）→ 报错 toast
    await toggles[2]!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message === '当前没有活动会话')).toBe(true)
    w.unmount()
  })

  it('会话放行：状态回显 / 翻转 / 持久化失败告警 / 未装配禁用', async () => {
    const w = await mountView()
    const store = useSessionStore()
    requestMock.mockImplementation((_m: string, cmd: string, data?: any) => {
      if (cmd === 'get') return Promise.resolve(cfg())
      if (cmd === 'cors.list') return Promise.resolve({ origins: [], development_mode: false })
      if (cmd === 'lease_status') return Promise.resolve({ supported: false })
      if (cmd === 'session_allow_status') {
        return Promise.resolve({ enabled: true, since: '2026-02-03T00:00:00Z', count: 5 })
      }
      if (cmd === 'session_allow') {
        return Promise.resolve({ enabled: data.enabled, since: data.enabled ? '2026-02-04T00:00:00Z' : null, count: data.enabled ? 6 : 0, persisted: false, warning: '持久化失败警告' })
      }
      return Promise.resolve({})
    })
    store.currentId = 'sess-1'
    await flushPromises()
    await toTab(w, '编辑器')
    expect(w.text()).toContain('（sess-1）')
    expect(w.text()).toContain('已开启')
    expect(w.text()).toContain('累计自动批准 5 条')
    const allowToggle = w.findAll('.toggle')[2]!
    expect(allowToggle.classes()).toContain('active')
    // 翻转 → set_session_allow + persisted=false 告警
    await allowToggle.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('security', 'session_allow', { session_id: 'sess-1', enabled: false })
    expect(useToast().toasts.some(t => t.type === 'error' && t.message.includes('持久化失败警告'))).toBe(true)

    // 失败（未装配）→ 开关禁用态
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'get') return Promise.resolve(cfg())
      if (cmd === 'session_allow' || cmd === 'session_allow_status') return Promise.reject(new Error('security 未装配'))
      return Promise.resolve({})
    })
    store.currentId = 'sess-2'
    await flushPromises()
    expect((w.vm as any).sessionAllowAvailable).toBe(false)
    w.unmount()
  })

  it('配色 TAB：浅色/深色卡片切换持久化', async () => {
    const w = await mountView()
    await toTab(w, '配色')
    const cards = w.findAll('.theme-card')
    expect(cards.length).toBe(2)
    // 默认深色（localStorage 未设 + prefers-color-scheme 不匹配）
    expect(cards[1]!.classes()).toContain('active')
    await cards[0]!.trigger('click')
    expect(document.documentElement.getAttribute('data-theme')).toBe('light')
    expect(localStorage.getItem('nemesisbot_theme')).toBe('light')
    expect(cards[0]!.classes()).toContain('active')
    expect(cards[0]!.text()).toContain('✓ 当前')
    await cards[1]!.trigger('click')
    expect(document.documentElement.getAttribute('data-theme')).toBe('dark')
    w.unmount()
  })

  it('原始 JSON TAB：sections 渲染、编辑保存成功/失败/取消', async () => {
    const w = await mountView()
    await toTab(w, '原始 JSON')
    expect(w.text()).toContain('gateway')
    expect(w.text()).toContain('127.0.0.1')
    // 编辑
    await w.findAll('button').find(b => b.text() === '编辑')!.trigger('click')
    const ta = w.find('textarea.form-textarea')
    expect((ta.element as HTMLTextAreaElement).value).toContain('"gateway"')
    // 保存成功
    await ta.setValue('{"gateway":{"host":"9.9.9.9","port":1234}}')
    await w.findAll('button').find(b => b.text() === '保存')!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('config', 'save', { gateway: { host: '9.9.9.9', port: 1234 } })
    expect(useToast().toasts.some(t => t.type === 'success' && t.message === '配置已保存')).toBe(true)
    // 再次编辑：非法 JSON → 保存失败 toast
    await w.findAll('button').find(b => b.text() === '编辑')!.trigger('click')
    await w.find('textarea.form-textarea').setValue('{broken')
    await w.findAll('button').find(b => b.text() === '保存')!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('保存失败'))).toBe(true)
    // 取消
    await w.findAll('button').find(b => b.text() === '取消')!.trigger('click')
    expect(w.find('textarea.form-textarea').exists()).toBe(false)
    w.unmount()
  })

  it('config save 失败与加载失败 toast', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'get') return Promise.reject(new Error('gone'))
      if (cmd === 'save' || cmd === 'set_field') return Promise.reject(new Error('ro'))
      return Promise.resolve({})
    })
    const w = await mountView()
    expect(useToast().toasts.some(t => t.message.includes('加载配置失败'))).toBe(true)
    // saveField 错误分支（set_field 拒绝）
    await (w.vm as any).saveField('gateway.port', 1234)
    await flushPromises()
    ;(w.vm as any).editConfig = '{"a":1}'
    await (w.vm as any).saveFullConfig()
    await flushPromises()
    expect(useToast().toasts.filter(t => t.message.includes('保存失败')).length).toBe(2)
    w.unmount()
  })

  it('皮肤 TAB 渲染异步面板（feature 门控默认开；真面板 WSAPI 已 mock）', async () => {
    const w = await rawMount(SettingsView)
    await flushPromises()
    expect(w.findAll('button.tab').map(b => b.text())).toContain('皮肤')
    await toTab(w, '皮肤')
    await flushPromises()
    await flushPromises()
    // 异步面板挂载点渲染（真面板依赖 useSkin/authFetch，WSAPI 走本文件 mock 返回 {}）
    expect(w.findAll('button.tab').find(b => b.text() === '皮肤')!.classes()).toContain('active')
    w.unmount()
  })
})
