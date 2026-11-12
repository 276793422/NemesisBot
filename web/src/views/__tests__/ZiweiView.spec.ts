import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { useToast } from '../../composables/useToast'

// 紫微总闸（批次 3 配置页补全）：门关横幅 + 立即启用（config.set_field
// ziwei.enabled）→ 开启后主视图（总闸卡）出现；高级折叠 5 键整批保存。
// 后端 GATE_CLOSED_MSG 单一真相源（前端只展示不另维护文案）。

const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
  initWSAPI: vi.fn(),
}))

import ZiweiView from '../ZiweiView.vue'

// 有状态 mock：config 树 + ziwei 门状态联动（enabled=false 时 status
// reject「未启用」字符串——组件按字符串消息处理门关）。
function makeImpl(initial: Record<string, unknown>) {
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
    if (cmd === 'set_field') {
      setPath(data.path, data.value)
      return Promise.resolve({ ok: true })
    }
    if (cmd === 'status') {
      const enabled = (tree as any).ziwei?.enabled === true
      if (!enabled) return Promise.reject('紫微系统未启用：在 config.json 的 ziwei.enabled 设 true 后重启生效')
      return Promise.resolve({ enabled: true, profiles: 0, sessions: [] })
    }
    if (cmd === 'list') return Promise.resolve({ profiles: [] })
    if (cmd === 'setup.status') return Promise.resolve({ state: 'idle' })
    return Promise.resolve({})
  }
}

beforeEach(() => {
  requestMock.mockReset()
  requestMock.mockImplementation(makeImpl({}))
  useToast().toasts.splice(0)
})

async function mountView() {
  const w = mount(ZiweiView)
  await flushPromises()
  return w
}

describe('ZiweiView 紫微总闸（批次 3）', () => {
  it('门关：横幅 + 立即启用按钮；点击 → set_field ziwei.enabled true → 主视图（总闸卡）出现', async () => {
    const w = await mountView()
    expect(w.text()).toContain('紫微系统未启用')
    expect(w.find('[data-test="ziwei-enable"]').exists()).toBe(true)
    await w.find('[data-test="ziwei-enable"]').trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('config', 'set_field', { path: 'ziwei.enabled', value: true })
    // enabled=true 后 status mock 放行 → 横幅消失、总闸卡出现
    expect(w.find('[data-test="ziwei-enable"]').exists()).toBe(false)
    expect(w.find('[data-test="ziwei-config-card"]').exists()).toBe(true)
    expect((w.find('[data-test="ziwei-enabled"]').element as HTMLInputElement).checked).toBe(true)
    w.unmount()
  })

  it('已启用：无横幅，总闸卡渲染；checkbox uncheck → set_field ziwei.enabled false', async () => {
    requestMock.mockImplementation(makeImpl({ ziwei: { enabled: true } }))
    const w = await mountView()
    expect(w.find('[data-test="ziwei-enable"]').exists()).toBe(false)
    expect(w.find('[data-test="ziwei-config-card"]').exists()).toBe(true)
    const cb = w.find('[data-test="ziwei-enabled"]')
    expect((cb.element as HTMLInputElement).checked).toBe(true)
    await cb.setValue(false)
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('config', 'set_field', { path: 'ziwei.enabled', value: false })
    w.unmount()
  })

  it('高级折叠（details）展开 → 保存 → 5 次 set_field（payload_dir 空落 null；数值缺省兜底）', async () => {
    requestMock.mockImplementation(makeImpl({ ziwei: { enabled: true } }))
    const w = await mountView()
    const details = w.find('details')
    expect(details.exists()).toBe(true)
    // jsdom 手动开 details 再派发 toggle（v-if 由 @toggle 处理器驱动）
    ;(details.element as HTMLDetailsElement).open = true
    await details.trigger('toggle')
    await flushPromises()
    await w.find('[data-test="ziwei-adv-save"]').trigger('click')
    await flushPromises()
    const writes = requestMock.mock.calls.filter(c => c[1] === 'set_field')
    expect(writes.map(c => c[2].path)).toEqual([
      'ziwei.allowed_targets',
      'ziwei.payload_dir',
      'ziwei.max_sessions',
      'ziwei.snapshots_ttl_days',
      'ziwei.runs_ttl_days',
    ])
    const byPath = Object.fromEntries(writes.map(c => [c[2].path, c[2].value]))
    expect(byPath['ziwei.allowed_targets']).toEqual([]) // 空 → 空数组
    expect(byPath['ziwei.payload_dir']).toBe(null) // 留空 = 回落默认（Option → null）
    expect(byPath['ziwei.max_sessions']).toBe(4)
    expect(byPath['ziwei.snapshots_ttl_days']).toBe(30)
    expect(byPath['ziwei.runs_ttl_days']).toBe(7)
    expect(w.text()).toContain('已保存')
    w.unmount()
  })
})

// ─────────────────────────────────────────────────────────────────────────────
// 以下为增量补测（M2 会话面 / M3 配置向导六步 / M5 失效找回 / 状态卡与列表）。
// 独立于上面批次 3 的门关测试：makeImpl2 提供带 profiles/sessions 的全量后端。
// ─────────────────────────────────────────────────────────────────────────────

afterEach(() => {
  vi.unstubAllGlobals()
})

const P1 = { profile_id: 'pid-1', name: 'P1', status: 'active', exe_name: 'game.exe', machine: 'x64', tool_count: 2 }
const P2 = { profile_id: 'pid-2', name: 'P2', status: 'stale', exe_name: 'game.exe', machine: 'x64', tool_count: 1 }
const P3 = { profile_id: 'pid-3', name: 'P3', status: 'revoked', exe_name: 'old.exe', machine: 'x86', tool_count: 0 }
const P4 = { profile_id: 'pid-4', name: 'P4', status: 'future-status', exe_name: 'new.exe', machine: 'arm64', tool_count: 0 }
const S1 = { profile_id: 'pid-1', pid: 1234, state: 'ready', arch: 'x64', target_exe: 'game.exe' }
const SHOW_P1 = {
  profile: {
    profile_id: 'pid-1',
    name: 'P1',
    target: { exe_name: 'game.exe', machine: 'x64' },
    functions: [{ rva: 1 }],
    data_structures: [{ name: 'D' }],
    tools: [{ tool_name: 't1' }, { tool_name: 't2' }],
  },
  notes: '备注N',
}

/** 全量后端：status/list/show/attach/detach/invoke/refresh/recover/wizard 全命令分发。 */
function makeImpl2(overrides: Record<string, any> = {}) {
  return (_m: string, cmd: string, data?: any) => {
    if (cmd === 'get') return Promise.resolve(JSON.parse(JSON.stringify(overrides.config ?? { ziwei: { enabled: true } })))
    if (cmd === 'set_field') return Promise.resolve({ ok: true })
    if (cmd === 'status') return Promise.resolve(overrides.status ?? { enabled: true, profiles: 3, sessions: [S1] })
    if (cmd === 'list') return Promise.resolve(overrides.list ?? { profiles: [P1, P2, P3, P4] })
    if (cmd === 'show') return Promise.resolve(overrides.show ?? SHOW_P1)
    if (overrides[cmd] !== undefined) return overrides[cmd](data)
    return Promise.resolve(overrides[cmd] ?? {})
  }
}

async function mountFull(overrides: Record<string, any> = {}) {
  requestMock.mockReset()
  requestMock.mockImplementation(makeImpl2(overrides))
  useToast().toasts.splice(0)
  const w = mount(ZiweiView)
  await flushPromises()
  return w
}

/** 按行文本定位 profile 表格行。 */
function rowOf(w: ReturnType<typeof mount>, needle: string) {
  return w.findAll('tbody tr').find(r => r.text().includes(needle))!
}

describe('ZiweiView 主视图：状态卡 / 列表 / 会话面', () => {
  it('状态卡计数 + 徽标三色（可用/已失效/已吊销）+ 失效横幅（授权找回 / 引导重配）', async () => {
    const w = await mountFull({
      status: { enabled: true, profiles: 4, sessions: [S1], setup_engine: { available: true }, manager_present: true },
    })
    expect(w.text()).toContain('Profile 总数')
    const nums = w.findAll('.stat-num').map(n => n.text())
    expect(nums).toEqual(['4', '1', '1']) // 总数 / 可用 / 已失效
    // 徽标：active=可用 badge-ok；stale=已失效 badge-warn；revoked=已吊销 badge-off
    const b1 = rowOf(w, 'pid-1').find('.badge')
    expect(b1!.classes()).toContain('badge-ok')
    expect(b1!.text()).toBe('可用')
    expect(rowOf(w, 'pid-2').find('.badge')!.classes()).toContain('badge-warn')
    expect(rowOf(w, 'pid-2').find('.badge')!.text()).toBe('已失效')
    expect(rowOf(w, 'pid-3').find('.badge')!.classes()).toContain('badge-off')
    expect(rowOf(w, 'pid-3').find('.badge')!.text()).toBe('已吊销')
    // 未知状态：badge-off 兜底 + 原文直出
    expect(rowOf(w, 'pid-4').find('.badge')!.classes()).toContain('badge-off')
    expect(rowOf(w, 'pid-4').find('.badge')!.text()).toBe('future-status')
    // 失效横幅 + 双入口
    expect(w.text()).toContain('有 1 个 profile 已失效')
    expect(w.findAll('button').find(b => b.text() === '授权找回（pid-2）')).toBeTruthy()
    expect(w.findAll('button').find(b => b.text() === '引导重配')).toBeTruthy()
    // pid-1 有活动会话 → 卸载按钮（warn）；pid-2/3 无会话 → 附加
    expect(rowOf(w, 'pid-1').findAll('button').find(b => b.text() === '卸载')).toBeTruthy()
    expect(rowOf(w, 'pid-2').findAll('button').find(b => b.text() === '附加')).toBeTruthy()
    // 活动会话表 + 子工具按钮（refresh 逐会话 show 取 toolCache）
    expect(w.text()).toContain('活动会话')
    const srow = rowOf(w, '1234')
    expect(srow.text()).toContain('ready')
    expect(srow.findAll('button').map(b => b.text())).toEqual(['t1', 't2'])
    // 脚注（配置期引擎可用 / 会话管理器已装配 / 保留天数）
    expect(w.text()).toContain('配置期引擎：可用')
    expect(w.text()).toContain('会话管理器：已装配')
    expect(w.text()).toContain('capture 回流保留 7 天')
    w.unmount()
  })

  it('脚注降级：setup_engine 未装配 / manager 缺席 → 未装配·未装配', async () => {
    const w = await mountFull({
      status: { enabled: true, profiles: 0, sessions: [], setup_engine: { available: false, note: '引擎未编译' } },
      list: { profiles: [] },
    })
    expect(w.text()).toContain('配置期引擎：引擎未编译')
    expect(w.text()).toContain('会话管理器：未装配')
    expect(w.text()).toContain('暂无 profile')
    // 刷新按钮可重复触发 refresh
    requestMock.mockClear()
    await w.findAll('button').find(b => b.text() === '刷新')!.trigger('click')
    await flushPromises()
    expect(requestMock.mock.calls.some(c => c[1] === 'status')).toBe(true)
    expect(requestMock.mock.calls.some(c => c[1] === 'list')).toBe(true)
    w.unmount()
  })

  it('详情：show → selected 详情卡（目标/函数/结构/子工具计数 + notes）；show 失败不出卡', async () => {
    const w = await mountFull()
    await rowOf(w, 'pid-1').findAll('button').find(b => b.text() === '详情')!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('ziwei', 'show', { profile_id: 'pid-1' })
    expect(w.text()).toContain('备注N')
    const card = w.findAll('.detail-card').find(c => c.text().includes('P1'))!
    expect(card.text()).toContain('game.exe（x64）')
    expect(card.text()).toContain('1 个') // 函数
    expect(card.text()).toContain('2 个') // 子工具

    // 失败：__err 形态 → 不出卡（先关掉已开的卡）
    requestMock.mockImplementation((_m, cmd, data) => {
      if (cmd === 'show' && data?.profile_id === 'pid-2') return Promise.reject('后端炸了')
      return Promise.resolve(cmd === 'show' ? SHOW_P1 : makeImpl2()(_m, cmd, data))
    })
    await rowOf(w, 'pid-2').findAll('button').find(b => b.text() === '详情')!.trigger('click')
    await flushPromises()
    expect(w.text()).not.toContain('后端炸了')
    w.unmount()
  })

  it('删除：confirm 拦截不发请求；确认后 remove + 刷新；删中已选中的 profile 清详情卡', async () => {
    const confirmSpy = vi.fn(() => false)
    vi.stubGlobal('confirm', confirmSpy)
    const w = await mountFull()
    // 先开 pid-1 详情
    await rowOf(w, 'pid-1').findAll('button').find(b => b.text() === '详情')!.trigger('click')
    await flushPromises()
    expect(w.text()).toContain('备注N')

    // confirm=false → 无 remove 请求
    await rowOf(w, 'pid-1').findAll('button').find(b => b.text() === '删除')!.trigger('click')
    expect(confirmSpy).toHaveBeenCalledWith('删除 profile「P1」及其全部快照/运行记录？')
    expect(requestMock.mock.calls.some(c => c[1] === 'remove')).toBe(false)

    // confirm=true → remove + refresh + 详情卡清空（selected 同 profile）
    vi.stubGlobal('confirm', vi.fn(() => true))
    await rowOf(w, 'pid-1').findAll('button').find(b => b.text() === '删除')!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('ziwei', 'remove', { profile_id: 'pid-1' })
    expect(w.text()).not.toContain('备注N')
    w.unmount()
  })

  it('验证：成功 sessionMsg 带裁决；失败 ✗ 原文', async () => {
    const w = await mountFull({
      verify: () => Promise.resolve({ report: { verdict: 'match' } }),
    })
    await rowOf(w, 'pid-1').findAll('button').find(b => b.text() === '验证')!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('ziwei', 'verify', { profile_id: 'pid-1' })
    expect(w.text()).toContain('✓ 验证 pid-1：裁决 match')

    requestMock.mockImplementation((_m, cmd, data) => {
      if (cmd === 'verify') return Promise.reject('busy')
      return Promise.resolve(makeImpl2()(_m, cmd, data))
    })
    await rowOf(w, 'pid-2').findAll('button').find(b => b.text() === '验证')!.trigger('click')
    await flushPromises()
    expect(w.text()).toContain('✗ busy')
    w.unmount()
  })

  it('附加：空 PID 拦截；结构地址 JSON 坏解析拦截；成功带 struct_addrs + 详情选中；失败 ✗', async () => {
    const w = await mountFull()
    const attachBtn = () => rowOf(w, 'pid-2').findAll('button').find(b => b.text() === '附加')!

    // 空 PID
    await attachBtn().trigger('click')
    await flushPromises()
    expect(w.text()).toContain('请先在上方填目标进程 PID')
    expect(requestMock.mock.calls.some(c => c[1] === 'attach')).toBe(false)

    // 坏 JSON
    const pidInput = w.findAll('input').find(i => i.attributes('placeholder')?.includes('12345'))!
    await pidInput.setValue('999')
    const addrInput = w.findAll('input').find(i => i.attributes('placeholder')?.includes('MonitorState'))!
    await addrInput.setValue('{bad json')
    await attachBtn().trigger('click')
    await flushPromises()
    expect(w.text()).toContain('结构地址 JSON 解析失败')

    // 成功（attach 返回 session 状态；struct_addrs 原样透传 JSON.parse 结果）
    requestMock.mockImplementation((_m, cmd, data) => {
      if (cmd === 'attach') return Promise.resolve({ session: { pid: 999, state: 'ready' } })
      return Promise.resolve(makeImpl2()(_m, cmd, data))
    })
    await addrInput.setValue('{"MonitorState": "0x100"}')
    await attachBtn().trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('ziwei', 'attach', { profile_id: 'pid-2', pid: 999, struct_addrs: { MonitorState: '0x100' } })
    expect(w.text()).toContain('✓ 已附加 pid=999（ready）')

    // 失败
    requestMock.mockImplementation((_m, cmd, data) => {
      if (cmd === 'attach') return Promise.reject('权限不足')
      return Promise.resolve(makeImpl2()(_m, cmd, data))
    })
    await attachBtn().trigger('click')
    await flushPromises()
    expect(w.text()).toContain('✗ 权限不足')
    w.unmount()
  })

  it('卸载：confirm 拦截；确认后 detach + 快照清空 + 消息；失败 ✗', async () => {
    const confirmSpy = vi.fn(() => true)
    vi.stubGlobal('confirm', confirmSpy)
    const w = await mountFull()
    // 先造一个 invoke 快照，detach 成功后应清空
    await rowOf(w, '1234').findAll('button').find(b => b.text() === 't1')!.trigger('click')
    await flushPromises()
    expect(w.text()).toContain('最近调用快照')
    const detachBtn = () => rowOf(w, 'pid-1').findAll('button').find(b => b.text() === '卸载')!

    confirmSpy.mockReturnValueOnce(false)
    await detachBtn().trigger('click')
    expect(requestMock.mock.calls.some(c => c[1] === 'detach')).toBe(false)

    requestMock.mockImplementation((_m, cmd) => {
      if (cmd === 'detach') return Promise.resolve({ message: 'payload 已自卸' })
      return Promise.resolve(makeImpl2()(_m, cmd))
    })
    await detachBtn().trigger('click')
    await flushPromises()
    expect(confirmSpy).toHaveBeenCalledWith('卸载会话 pid-1（pid=1234）？payload 将自卸。')
    expect(requestMock).toHaveBeenCalledWith('ziwei', 'detach', { profile_id: 'pid-1' })
    expect(w.text()).toContain('✓ payload 已自卸')
    expect(w.text()).not.toContain('最近调用快照')

    // 失败
    requestMock.mockImplementation((_m, cmd) => {
      if (cmd === 'detach') return Promise.reject('still attached')
      return Promise.resolve(makeImpl2()(_m, cmd))
    })
    await detachBtn().trigger('click')
    await flushPromises()
    expect(w.text()).toContain('✗ still attached')
    w.unmount()
  })

  it('子工具调用：invoke 成功出快照卡；失败 ✗ 且快照卡消失', async () => {
    const w = await mountFull({ invoke: () => Promise.resolve({ tool: 't2', data: [1] }) })
    await rowOf(w, '1234').findAll('button').find(b => b.text() === 't2')!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('ziwei', 'invoke', { profile_id: 'pid-1', tool: 't2' })
    expect(w.text()).toContain('✓ t2 快照已取得')
    expect(w.text()).toContain('最近调用快照 t2')

    requestMock.mockImplementation((_m, cmd, data) => {
      if (cmd === 'invoke') return Promise.reject('tool gone')
      return Promise.resolve(makeImpl2()(_m, cmd, data))
    })
    await rowOf(w, '1234').findAll('button').find(b => b.text() === 't2')!.trigger('click')
    await flushPromises()
    expect(w.text()).toContain('✗ tool gone')
    expect(w.text()).not.toContain('最近调用快照')
    w.unmount()
  })

  it('重注册动态工具：成功计数 / 失败 ✗', async () => {
    const w = await mountFull({ refresh: () => Promise.resolve({ registered: 5 }) })
    await w.findAll('button').find(b => b.text() === '重注册动态工具')!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('ziwei', 'refresh')
    expect(w.text()).toContain('✓ 动态工具重注册：5 个')

    requestMock.mockImplementation((_m, cmd, data) => {
      if (cmd === 'refresh') return Promise.reject('no engine')
      return Promise.resolve(makeImpl2()(_m, cmd, data))
    })
    await w.findAll('button').find(b => b.text() === '重注册动态工具')!.trigger('click')
    await flushPromises()
    expect(w.text()).toContain('✗ no engine')
    w.unmount()
  })
})

describe('ZiweiView 失效找回（M5）', () => {
  const RECOVER_OK = {
    report: {
      verdict: {
        verdict: 'drift',
        per_anchor: [
          { function: 'fnA', kind: 'byte_pattern', hit: true, found_rva: 0x2a10, note: '锚命中' },
          { function: 'fnB', kind: 'str_ref', hit: false, found_rva: null, note: '' },
        ],
      },
    },
    report_md: '# 报告',
  }

  it('授权找回：recover 报告模态（逐锚表 + 命中预勾选 + md）；确认入库走 recover_confirm + 刷新', async () => {
    const w = await mountFull({
      recover: () => Promise.resolve(RECOVER_OK),
      recover_confirm: () => Promise.resolve({ all_recovered: true }),
    })
    await w.findAll('button').find(b => b.text() === '授权找回（pid-2）')!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('ziwei', 'recover', { profile_id: 'pid-2' })
    expect(w.text()).toContain('授权找回 — pid-2')
    expect(w.text()).toContain('总判定：drift')
    expect(w.text()).toContain('# 报告')
    const rows = w.findAll('tbody tr').filter(r => r.text().includes('fnA') || r.text().includes('fnB'))
    expect(rows).toHaveLength(2)
    // 命中行：✓ 徽标 + checkbox（预勾选）；未命中行：✗ 无 checkbox；rva 十六进制
    const rowA = rows.find(r => r.text().includes('fnA'))!
    expect(rowA.text()).toContain('0x2a10')
    expect(rowA.text()).toContain('✓')
    const cbA = rowA.find('input[type="checkbox"]')
    expect(cbA.exists()).toBe(true)
    expect((cbA.element as HTMLInputElement).checked).toBe(true)
    const rowB = rows.find(r => r.text().includes('fnB'))!
    expect(rowB.text()).toContain('✗')
    expect(rowB.find('input[type="checkbox"]').exists()).toBe(false)

    // 确认入库（fnA 预勾选）→ 全部恢复 → 模态关闭（msg 渲染在模态内，随关闭不可见）
    await w.findAll('button').find(b => b.text().includes('确认入库'))!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('ziwei', 'recover_confirm', { profile_id: 'pid-2', accept: ['fnA'] })
    expect(w.text()).not.toContain('授权找回 — pid-2')
    // refresh 重拉
    expect(requestMock.mock.calls.some(c => c[1] === 'list')).toBe(true)
    w.unmount()
  })

  it('全不勾选拦截（模态保持）；部分恢复关模态；recover 请求失败 ✗', async () => {
    const w = await mountFull({ recover: () => Promise.resolve(RECOVER_OK) })
    await w.findAll('button').find(b => b.text() === '授权找回（pid-2）')!.trigger('click')
    await flushPromises()

    // 取消 fnA 勾选 → 全不勾选拦截（模态保持、错误文案可见）
    const cbA = w.findAll('tbody tr').find(r => r.text().includes('fnA'))!.find('input[type="checkbox"]')
    await cbA.setValue(false)
    await w.findAll('button').find(b => b.text().includes('确认入库'))!.trigger('click')
    await flushPromises()
    expect(w.text()).toContain('至少勾选一个要找回的函数（全部不恢复请直接重配）')
    expect(requestMock.mock.calls.some(c => c[1] === 'recover_confirm')).toBe(false)

    // 部分恢复：请求发出 → 模态关闭（all_recovered=false 走部分恢复分支）
    await cbA.setValue(true)
    requestMock.mockImplementation((_m, cmd, data) => {
      if (cmd === 'recover_confirm') return Promise.resolve({ all_recovered: false })
      return Promise.resolve(makeImpl2()(_m, cmd, data))
    })
    await w.findAll('button').find(b => b.text().includes('确认入库'))!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('ziwei', 'recover_confirm', { profile_id: 'pid-2', accept: ['fnA'] })
    expect(w.text()).not.toContain('授权找回 — pid-2')

    // 失败：模态内 ✗ 文案
    requestMock.mockImplementation((_m, cmd, data) => {
      if (cmd === 'recover') return Promise.reject('后端拒绝')
      return Promise.resolve(makeImpl2()(_m, cmd, data))
    })
    await w.findAll('button').find(b => b.text() === '授权找回（pid-2）')!.trigger('click')
    await flushPromises()
    expect(w.text()).toContain('✗ 后端拒绝')
    w.unmount()
  })
})

describe('ZiweiView 配置向导（M3 六步）', () => {
  it('启动 / 收起：setup.status → 面板展开；收起按钮关闭；setup.status 失败 info 落 null', async () => {
    const w = await mountFull()
    await w.findAll('button').find(b => b.text() === '启动 / 恢复向导')!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('ziwei', 'setup.status', {})
    expect(w.text()).toContain('① 选目标')
    expect(w.findAll('button').find(b => b.text() === '收起')).toBeTruthy()
    await w.findAll('button').find(b => b.text() === '收起')!.trigger('click')
    await flushPromises()
    expect(w.findAll('button').find(b => b.text() === '启动 / 恢复向导')).toBeTruthy()

    // setup.status 拒绝 → info=null（catch 分支），面板仍展开
    requestMock.mockImplementation((_m, cmd, data) => {
      if (cmd === 'setup.status') return Promise.reject('ws down')
      return Promise.resolve(makeImpl2()(_m, cmd, data))
    })
    await w.findAll('button').find(b => b.text() === '启动 / 恢复向导')!.trigger('click')
    await flushPromises()
    expect(w.text()).toContain('① 选目标')
    expect(w.text()).not.toContain('检测到未收尾向导草稿')
    w.unmount()
  })

  it('恢复未收尾草稿：step 4 直达 + 草稿信息条 + 提交块出现', async () => {
    const w = await mountFull({
      'setup.status': () => Promise.resolve({ profile_id: 'draft-9', pid: 5, exe_name: 'g.exe', file_sha256: 'abcdef1234567890' }),
    })
    await w.findAll('button').find(b => b.text() === '启动 / 恢复向导')!.trigger('click')
    await flushPromises()
    expect(w.text()).toContain('检测到未收尾向导草稿（draft-9）——已恢复；快照类步骤需重选目标')
    expect(w.text()).toContain('草稿 draft-9 · pid 5 · g.exe · sha abcdef12')
    expect(w.text()).toContain('④ 候选与标注')
    expect(w.text()).toContain('⑥ 提交')
    w.unmount()
  })

  it('① 选目标：空值拦截；exe 与 PID 两种载荷；失败 ✗', async () => {
    const w = await mountFull()
    await w.findAll('button').find(b => b.text() === '启动 / 恢复向导')!.trigger('click')
    await flushPromises()
    const targetInput = w.findAll('input').find(i => i.attributes('placeholder')?.includes('dummy.exe'))!
    const selectBtn = () => w.findAll('button').find(b => b.text() === '选择目标')!

    await selectBtn().trigger('click')
    await flushPromises()
    expect(w.text()).toContain('填目标进程名或 PID')

    await targetInput.setValue('game.exe')
    requestMock.mockImplementation((_m, cmd, data) => {
      if (cmd === 'setup.select_target') return Promise.resolve({ profile_id: 'd1', pid: 10, exe_name: 'game.exe', file_sha256: '0123456789abcdef' })
      return Promise.resolve(makeImpl2()(_m, cmd, data))
    })
    await selectBtn().trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('ziwei', 'setup.select_target', { exe: 'game.exe' })
    expect(w.text()).toContain('✓ 草稿 d1 已建（pid=10，game.exe）')
    expect(w.text()).toContain('②③ 快照 + 分析')

    // PID 形态
    await targetInput.setValue('123')
    await selectBtn().trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('ziwei', 'setup.select_target', { pid: 123 })

    // 失败
    requestMock.mockImplementation((_m, cmd, data) => {
      if (cmd === 'setup.select_target') return Promise.reject('进程不存在')
      return Promise.resolve(makeImpl2()(_m, cmd, data))
    })
    await targetInput.setValue('nope.exe')
    await selectBtn().trigger('click')
    await flushPromises()
    expect(w.text()).toContain('✗ 进程不存在')
    w.unmount()
  })

  it('②③ 快照并分析：落盘消息 + 分析行 + LLM 建议 chip（点击预填）；失败 ✗', async () => {
    const w = await mountFull({
      'setup.status': () => Promise.resolve({ profile_id: 'd1', pid: 3, exe_name: 'g.exe', file_sha256: 'f'.repeat(64) }),
      'setup.snapshot': () => Promise.resolve({ ok: true }),
      'setup.analyze': () =>
        Promise.resolve({
          functions: 5,
          crt_folded: 2,
          import_dlls: 3,
          strings: 40,
          analysis_json: 'analysis.json',
          llm_suggestions: [{ rva: '2A10', name: 'fnS' }],
        }),
    })
    await w.findAll('button').find(b => b.text() === '启动 / 恢复向导')!.trigger('click')
    await flushPromises()
    await w.findAll('button').find(b => b.text() === '快照并分析')!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('ziwei', 'setup.snapshot', {})
    expect(requestMock).toHaveBeenCalledWith('ziwei', 'setup.analyze', {})
    // 落盘消息被随后的分析完成消息覆盖（同一 wiz.msg 单槽）——只断言最终文案
    expect(w.text()).toContain('✓ 分析完成：函数 5，CRT 折叠 2（analysis.json）')
    expect(w.text()).toContain('函数 5 · CRT 折叠 2 · 导入 DLL 3 · 字符串 40')
    // LLM chip 点击 → 预填 byte_pattern + RVA 十六进制转十进制
    const chip = w.findAll('.sugg-chip').find(c => c.text().includes('2A10'))!
    await chip.trigger('click')
    await flushPromises()
    expect(w.text()).toContain('已预填候选（byte_pattern @ 0x2A10）')
    const rvaInput = w.findAll('.ann-form input').find(i => i.attributes('placeholder') === '0x2A10')!
    expect((rvaInput.element as HTMLInputElement).value).toBe('10768')

    // 失败
    requestMock.mockImplementation((_m, cmd, data) => {
      if (cmd === 'setup.snapshot') return Promise.reject('无权限')
      return Promise.resolve(makeImpl2()(_m, cmd, data))
    })
    await w.findAll('button').find(b => b.text() === '快照并分析')!.trigger('click')
    await flushPromises()
    expect(w.text()).toContain('✗ 无权限')
    w.unmount()
  })

  it('仅重跑分析：llm_suggestions 条数文案 / llm_suggest_error 文案 / 普通完成', async () => {
    const w = await mountFull({
      'setup.status': () => Promise.resolve({ profile_id: 'd1', pid: 3, exe_name: 'g.exe', file_sha256: 'f'.repeat(64) }),
    })
    await w.findAll('button').find(b => b.text() === '启动 / 恢复向导')!.trigger('click')
    await flushPromises()
    const rerun = () => w.findAll('button').find(b => b.text() === '仅重跑分析')!.trigger('click')

    requestMock.mockImplementation((_m, cmd, data) => {
      if (cmd === 'setup.analyze') return Promise.resolve({ functions: 7, crt_folded: 3, llm_suggestions: [{ rva: '10', name: 'x' }, { rva: '20', name: 'y' }] })
      return Promise.resolve(makeImpl2()(_m, cmd, data))
    })
    await rerun()
    await flushPromises()
    expect(w.text()).toContain('✓ 分析完成：函数 7，CRT 折叠 3（LLM 建议 2 条）')

    requestMock.mockImplementation((_m, cmd, data) => {
      if (cmd === 'setup.analyze') return Promise.resolve({ functions: 8, crt_folded: 0, llm_suggest_error: 'llm 超时' })
      return Promise.resolve(makeImpl2()(_m, cmd, data))
    })
    await rerun()
    await flushPromises()
    expect(w.text()).toContain('✓ 分析完成：函数 8，CRT 折叠 0（LLM 建议：llm 超时）')

    requestMock.mockImplementation((_m, cmd, data) => {
      if (cmd === 'setup.analyze') return Promise.resolve({ functions: 9, crt_folded: 1 })
      return Promise.resolve(makeImpl2()(_m, cmd, data))
    })
    await rerun()
    await flushPromises()
    expect(w.text()).toContain('✓ 分析完成：函数 9，CRT 折叠 1')

    // 失败 → ✗
    requestMock.mockImplementation((_m, cmd, data) => {
      if (cmd === 'setup.analyze') return Promise.reject('分析器崩溃')
      return Promise.resolve(makeImpl2()(_m, cmd, data))
    })
    await rerun()
    await flushPromises()
    expect(w.text()).toContain('✗ 分析器崩溃')
    w.unmount()
  })

  it('④ 拉取候选：候选表（RVA 十六进制 / null 显示 —）+ 标注预填（byte_pattern / str_ref / imm_const / import_call_seq）', async () => {
    const w = await mountFull({
      'setup.status': () => Promise.resolve({ profile_id: 'd1', pid: 3, exe_name: 'g.exe', file_sha256: 'f'.repeat(64) }),
      'setup.candidates': () =>
        Promise.resolve({
          count: 4,
          candidates: [
            { kind: 'byte_pattern', function_rva: 10768, payload: 'B8 8D' },
            { kind: 'str_ref', function_rva: null, payload: 'SIG' },
            { kind: 'imm_const', function_rva: 255, payload: '0xFFFF' },
            { kind: 'import_call_seq', function_rva: 16, payload: 'OpenA,CloseA' },
          ],
        }),
    })
    await w.findAll('button').find(b => b.text() === '启动 / 恢复向导')!.trigger('click')
    await flushPromises()
    await w.findAll('button').find(b => b.text() === '拉取候选')!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('ziwei', 'setup.candidates', {})
    expect(w.text()).toContain('✓ 候选 4 条——点行内「标注」进入 Step 4b')
    const candRows = w.findAll('tbody tr').filter(r => r.text().includes('byte_pattern') || r.text().includes('str_ref') || r.text().includes('imm_const') || r.text().includes('import_call_seq'))
    expect(candRows).toHaveLength(4)
    expect(candRows[0]!.text()).toContain('0x2A10')
    expect(candRows[1]!.text()).toContain('—') // function_rva null

    // 逐行标注 → prefill 分支
    const annRva = () => w.findAll('.ann-form input').find(i => i.attributes('placeholder') === '0x2A10')!
    const kindSelect = w.find('.ann-form select')

    await candRows[0]!.findAll('button').find(b => b.text() === '标注')!.trigger('click')
    expect((annRva().element as HTMLInputElement).value).toBe('10768')
    expect(w.text()).toContain('已预填候选（byte_pattern @ 0x2A10）')

    await candRows[1]!.findAll('button').find(b => b.text() === '标注')!.trigger('click')
    expect(w.text()).toContain('已预填候选（str_ref @ ?）')
    const textInput = w.findAll('.ann-form input').find(i => i.attributes('placeholder') === 'ZIWEI_DUMMY_DRAW_V1')!
    expect(textInput.exists()).toBe(true)

    await candRows[2]!.findAll('button').find(b => b.text() === '标注')!.trigger('click')
    expect(w.text()).toContain('已预填候选（imm_const @ 0xFF）')
    const valueInput = w.findAll('.ann-form input').find(i => i.attributes('placeholder') === '0x5A6B7C8D')!
    expect(valueInput.exists()).toBe(true)

    await candRows[3]!.findAll('button').find(b => b.text() === '标注')!.trigger('click')
    expect(w.text()).toContain('已预填候选（import_call_seq @ 0x10）')
    const importsInput = w.findAll('.ann-form input').find(i => i.attributes('placeholder')?.includes('CreateCompatibleDC'))!
    expect(importsInput.exists()).toBe(true)
    expect((importsInput.element as HTMLInputElement).value).toBe('OpenA,CloseA')
    // 锚 kind 跟随候选
    expect((kindSelect.element as HTMLSelectElement).value).toBe('import_call_seq')

    // 拉取候选失败 → ✗
    requestMock.mockImplementation((_m, cmd, data) => {
      if (cmd === 'setup.candidates') return Promise.reject('分析未跑')
      return Promise.resolve(makeImpl2()(_m, cmd, data))
    })
    await w.findAll('button').find(b => b.text() === '拉取候选')!.trigger('click')
    await flushPromises()
    expect(w.text()).toContain('✗ 分析未跑')
    w.unmount()
  })

  it('④ 差分窗口：waitSecs 数字化（空回 5）+ 数据页 chip；失败 ✗', async () => {
    const w = await mountFull({
      'setup.status': () => Promise.resolve({ profile_id: 'd1', pid: 3, exe_name: 'g.exe', file_sha256: 'f'.repeat(64) }),
      'setup.diff_window': (_data: any) => Promise.resolve({ pages: [{ rva: 1 }], data_pages: [{ section: '.data', rva: 69632, bytes_changed: 64 }] }),
    })
    await w.findAll('button').find(b => b.text() === '启动 / 恢复向导')!.trigger('click')
    await flushPromises()
    await w.findAll('button').find(b => b.text() === '差分窗口')!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('ziwei', 'setup.diff_window', { wait_secs: 5 })
    expect(w.text()).toContain('✓ 差分窗口（等 5s）：变化页 1，可写数据页 1（DataArea 线索）')
    expect(w.text()).toContain('.data @0x11000（64B）')

    // 自定义等待秒数
    const waitInput = w.findAll('input').find(i => i.classes().includes('tiny'))!
    await waitInput.setValue('9')
    await w.findAll('button').find(b => b.text() === '差分窗口')!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('ziwei', 'setup.diff_window', { wait_secs: 9 })

    // 空 → 回 5；失败 ✗
    await waitInput.setValue('')
    requestMock.mockImplementation((_m, cmd, data) => {
      if (cmd === 'setup.diff_window') return Promise.reject('窗口超时')
      return Promise.resolve(makeImpl2()(_m, cmd, data))
    })
    await w.findAll('button').find(b => b.text() === '差分窗口')!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('ziwei', 'setup.diff_window', { wait_secs: 5 })
    expect(w.text()).toContain('✗ 窗口超时')
    w.unmount()
  })

  it('④ 提交标注：四种锚 payload 直发 + note 可选；失败 ✗', async () => {
    const w = await mountFull({
      'setup.status': () => Promise.resolve({ profile_id: 'd1', pid: 3, exe_name: 'g.exe', file_sha256: 'f'.repeat(64) }),
      'setup.annotate': (data: any) => Promise.resolve({ annotated: 1, anchors: 1, __data: data }),
    })
    await w.findAll('button').find(b => b.text() === '启动 / 恢复向导')!.trigger('click')
    await flushPromises()
    const rvaInput = w.findAll('.ann-form input').find(i => i.attributes('placeholder') === '0x2A10')!
    const semanticInput = w.findAll('.ann-form input').find(i => i.attributes('placeholder') === 'push_record')!
    const noteInput = w.findAll('.ann-form label').filter(l => l.text().includes('语义名')) // 占位：语义名即 semantic
    void noteInput
    const kindSelect = w.find('.ann-form select')
    const submit = () => w.findAll('button').find(b => b.text() === '提交标注')!.trigger('click')

    // byte_pattern（默认 kind）
    await rvaInput.setValue('0x2A10')
    await semanticInput.setValue('push_record')
    const patternInput = w.findAll('.ann-form input').find(i => i.attributes('placeholder') === 'B8 8D 7C 6B 5A')!
    await patternInput.setValue('B8 8D')
    await submit()
    await flushPromises()
    let calls = requestMock.mock.calls.filter(c => c[1] === 'setup.annotate')
    expect(calls).toHaveLength(1)
    expect(calls[0]![2]).toEqual({ rva: 10768, semantic: 'push_record', conv: 'ms_x64', note: undefined, anchors: [{ kind: 'byte_pattern', pattern: 'B8 8D' }] })
    expect(w.text()).toContain('✓ 已标注 1（锚 1）——继续下一个函数或进 Step 5')

    // str_ref
    await kindSelect.setValue('str_ref')
    const textInput = w.findAll('.ann-form input').find(i => i.attributes('placeholder') === 'ZIWEI_DUMMY_DRAW_V1')!
    await textInput.setValue('SIG1')
    await submit()
    await flushPromises()
    calls = requestMock.mock.calls.filter(c => c[1] === 'setup.annotate')
    expect(calls[1]![2].anchors).toEqual([{ kind: 'str_ref', text: 'SIG1' }])

    // imm_const
    await kindSelect.setValue('imm_const')
    const valueInput = w.findAll('.ann-form input').find(i => i.attributes('placeholder') === '0x5A6B7C8D')!
    await valueInput.setValue('65536')
    await submit()
    await flushPromises()
    calls = requestMock.mock.calls.filter(c => c[1] === 'setup.annotate')
    expect(calls[2]![2].anchors).toEqual([{ kind: 'imm_const', value: 65536 }])

    // import_call_seq
    await kindSelect.setValue('import_call_seq')
    const importsInput = w.findAll('.ann-form input').find(i => i.attributes('placeholder')?.includes('CreateCompatibleDC'))!
    await importsInput.setValue('A,B')
    await submit()
    await flushPromises()
    calls = requestMock.mock.calls.filter(c => c[1] === 'setup.annotate')
    expect(calls[3]![2].anchors).toEqual([{ kind: 'import_call_seq', imports: 'A,B' }])

    // 失败
    requestMock.mockImplementation((_m, cmd, data) => {
      if (cmd === 'setup.annotate') return Promise.reject('锚重复')
      return Promise.resolve(makeImpl2()(_m, cmd, data))
    })
    await submit()
    await flushPromises()
    expect(w.text()).toContain('✗ 锚重复')
    w.unmount()
  })

  it('⑤ 定义结构：坏 fields JSON 拦截；成功入列（重名不重复）；失败 ✗', async () => {
    const w = await mountFull({
      'setup.status': () => Promise.resolve({ profile_id: 'd1', pid: 3, exe_name: 'g.exe', file_sha256: 'f'.repeat(64) }),
      'setup.define_struct': (data: any) => Promise.resolve({ struct: data.name }),
    })
    await w.findAll('button').find(b => b.text() === '启动 / 恢复向导')!.trigger('click')
    await flushPromises()
    const nameInput = w.findAll('input').find(i => i.attributes('placeholder') === 'DataHeader')!
    const sizeInput = w.findAll('input').find(i => i.attributes('placeholder') === '24')!
    const fieldsInput = w.findAll('input').find(i => i.attributes('placeholder')?.includes('"offset":0'))!
    const structBtn = () => w.findAll('button').find(b => b.text() === '定义结构')!

    await fieldsInput.setValue('{bad')
    await structBtn().trigger('click')
    await flushPromises()
    expect(w.text()).toContain('✗ fields JSON 解析失败（形如 [{"offset":0,"name":"magic","type":"u32"}]）')
    expect(requestMock.mock.calls.some(c => c[1] === 'setup.define_struct')).toBe(false)

    await nameInput.setValue('DataHeader')
    await sizeInput.setValue('24')
    await fieldsInput.setValue('[{"offset":0,"name":"magic","type":"u32"}]')
    await structBtn().trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('ziwei', 'setup.define_struct', {
      name: 'DataHeader',
      size: 24,
      fields: [{ offset: 0, name: 'magic', type: 'u32' }],
    })
    expect(w.text()).toContain('✓ 结构 DataHeader 已定义')
    expect(w.text()).toContain('本页已定义：结构 DataHeader · 工具 —')

    // 重名不重复入列
    await structBtn().trigger('click')
    await flushPromises()
    expect(w.text()).toContain('本页已定义：结构 DataHeader · 工具 —')

    // 失败
    requestMock.mockImplementation((_m, cmd, data) => {
      if (cmd === 'setup.define_struct') return Promise.reject('结构冲突')
      return Promise.resolve(makeImpl2()(_m, cmd, data))
    })
    await structBtn().trigger('click')
    await flushPromises()
    expect(w.text()).toContain('✗ 结构冲突')
    w.unmount()
  })

  it('⑤ 定义工具：成功入列（重名不重复）；失败 ✗', async () => {
    const w = await mountFull({
      'setup.status': () => Promise.resolve({ profile_id: 'd1', pid: 3, exe_name: 'g.exe', file_sha256: 'f'.repeat(64) }),
    })
    await w.findAll('button').find(b => b.text() === '启动 / 恢复向导')!.trigger('click')
    await flushPromises()
    const toolNameInput = w.findAll('input').find(i => i.attributes('placeholder') === 'dummy_area_read')!
    const descInput = w.findAll('input').find(i => i.attributes('placeholder')?.includes('读 DataHeader'))!
    const addrInput = w.findAll('input').find(i => i.attributes('placeholder')?.includes('struct:DataHeader'))!
    const toolBtn = () => w.findAll('button').find(b => b.text() === '定义工具')!

    await toolNameInput.setValue('dummy_read')
    await descInput.setValue('读数据')
    await addrInput.setValue('struct:DataHeader')
    await toolBtn().trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('ziwei', 'setup.define_tool', {
      tool_name: 'dummy_read',
      description: '读数据',
      action: { primitive: 'poll_report', addr: 'struct:DataHeader', mode: 'on_change' },
    })
    expect(w.text()).toContain('✓ 工具 dummy_read 已定义')
    expect(w.text()).toContain('工具 dummy_read')

    // 重名不重复
    await toolBtn().trigger('click')
    await flushPromises()
    expect(w.text()).toContain('结构 — · 工具 dummy_read')

    // 切 primitive
    const primitiveSelect = w.findAll('select').find(s => (s.element as HTMLSelectElement).value === 'poll_report')!
    await primitiveSelect.setValue('call_fn')
    await toolBtn().trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('ziwei', 'setup.define_tool', {
      tool_name: 'dummy_read',
      description: '读数据',
      action: { primitive: 'call_fn', addr: 'struct:DataHeader', mode: 'on_change' },
    })

    // 失败
    requestMock.mockImplementation((_m, cmd, data) => {
      if (cmd === 'setup.define_tool') return Promise.reject('地址非法')
      return Promise.resolve(makeImpl2()(_m, cmd, data))
    })
    await toolBtn().trigger('click')
    await flushPromises()
    expect(w.text()).toContain('✗ 地址非法')
    w.unmount()
  })

  it('⑥ 提交为正式 profile：confirm 拦截；确认后 commit + wizReset + 刷新；失败 ✗', async () => {
    const confirmSpy = vi.fn(() => true)
    vi.stubGlobal('confirm', confirmSpy)
    const w = await mountFull({
      'setup.status': () => Promise.resolve({ profile_id: 'd1', pid: 3, exe_name: 'g.exe', file_sha256: 'f'.repeat(64) }),
      'setup.commit': () => Promise.resolve({ committed: 'd1', functions: 5, tools: 2, anchors: 3, tools_registered: 2 }),
    })
    await w.findAll('button').find(b => b.text() === '启动 / 恢复向导')!.trigger('click')
    await flushPromises()

    // confirm=false → 不发请求
    confirmSpy.mockReturnValueOnce(false)
    await w.findAll('button').find(b => b.text() === '提交为正式 profile')!.trigger('click')
    await flushPromises()
    expect(confirmSpy).toHaveBeenCalledWith('提交草稿为正式 profile？（全量校验 + 迁移入库 + 动态工具注册）')
    expect(requestMock.mock.calls.some(c => c[1] === 'setup.commit')).toBe(false)

    // confirm=true → commit + reset + refresh（成功消息被随后的 wizReset 清空——不可见）
    const statusCallsBefore = requestMock.mock.calls.filter(c => c[1] === 'status').length
    await w.findAll('button').find(b => b.text() === '提交为正式 profile')!.trigger('click')
    await flushPromises()
    expect(confirmSpy).toHaveBeenCalledWith('提交草稿为正式 profile？（全量校验 + 迁移入库 + 动态工具注册）')
    expect(requestMock).toHaveBeenCalledWith('ziwei', 'setup.commit', {})
    // wizReset：面板收起（msg 同步清空）+ refresh 重拉
    expect(w.findAll('button').find(b => b.text() === '启动 / 恢复向导')).toBeTruthy()
    expect(requestMock.mock.calls.filter(c => c[1] === 'status').length).toBe(statusCallsBefore + 1)

    // 失败：不 reset，✗ 文案（委派到带草稿 setup.status 的基础实现）
    const baseCommit = makeImpl2({
      'setup.status': () => Promise.resolve({ profile_id: 'd1', pid: 3, exe_name: 'g.exe', file_sha256: 'f'.repeat(64) }),
    })
    requestMock.mockImplementation((_m, cmd, data) => {
      if (cmd === 'setup.commit') return Promise.reject('校验失败')
      return Promise.resolve(baseCommit(_m, cmd, data))
    })
    await w.findAll('button').find(b => b.text() === '启动 / 恢复向导')!.trigger('click')
    await flushPromises()
    await w.findAll('button').find(b => b.text() === '提交为正式 profile')!.trigger('click')
    await flushPromises()
    expect(w.text()).toContain('✗ 校验失败')
    expect(w.findAll('button').find(b => b.text() === '提交为正式 profile')).toBeTruthy() // 未 reset
    w.unmount()
  })

  it('⑥ 作废草稿：confirm 拦截；确认后 abort + reset；失败 ✗', async () => {
    const confirmSpy = vi.fn(() => true)
    vi.stubGlobal('confirm', confirmSpy)
    const w = await mountFull({
      'setup.status': () => Promise.resolve({ profile_id: 'd1', pid: 3, exe_name: 'g.exe', file_sha256: 'f'.repeat(64) }),
      'setup.abort': () => Promise.resolve({ aborted: 'd1' }),
    })
    await w.findAll('button').find(b => b.text() === '启动 / 恢复向导')!.trigger('click')
    await flushPromises()

    confirmSpy.mockReturnValueOnce(false)
    await w.findAll('button').find(b => b.text() === '作废草稿')!.trigger('click')
    await flushPromises()
    expect(confirmSpy).toHaveBeenCalledWith('作废当前向导草稿？（草稿目录与快照一并删除）')
    expect(requestMock.mock.calls.some(c => c[1] === 'setup.abort')).toBe(false)

    await w.findAll('button').find(b => b.text() === '作废草稿')!.trigger('click')
    await flushPromises()
    expect(confirmSpy).toHaveBeenCalledWith('作废当前向导草稿？（草稿目录与快照一并删除）')
    expect(requestMock).toHaveBeenCalledWith('ziwei', 'setup.abort', {})
    // wizReset 同步清空 msg → 面板收起即最终可见态
    expect(w.findAll('button').find(b => b.text() === '启动 / 恢复向导')).toBeTruthy()

    // 失败（委派到带草稿 setup.status 的基础实现）
    const baseAbort = makeImpl2({
      'setup.status': () => Promise.resolve({ profile_id: 'd1', pid: 3, exe_name: 'g.exe', file_sha256: 'f'.repeat(64) }),
    })
    requestMock.mockImplementation((_m, cmd, data) => {
      if (cmd === 'setup.abort') return Promise.reject('IO 错误')
      return Promise.resolve(baseAbort(_m, cmd, data))
    })
    await w.findAll('button').find(b => b.text() === '启动 / 恢复向导')!.trigger('click')
    await flushPromises()
    await w.findAll('button').find(b => b.text() === '作废草稿')!.trigger('click')
    await flushPromises()
    expect(w.text()).toContain('✗ IO 错误')
    w.unmount()
  })
})

describe('ZiweiView 总闸 + 高级配置（增量）', () => {
  it('高级配置：展开显示缺省表单；保存五连写 + 回读对账（数组 join 回显）；保存失败 ✗', async () => {
    const w = await mountFull({
      config: {
        ziwei: {
          enabled: true,
          allowed_targets: ['a.exe', 'b.exe'],
          payload_dir: '/tmp/p',
          max_sessions: 8,
          snapshots_ttl_days: 15,
          runs_ttl_days: 3,
        },
      },
    })
    const details = w.find('details')
    ;(details.element as HTMLDetailsElement).open = true
    await details.trigger('toggle')
    await flushPromises()
    // 展开时表单是缺省值（loadZiweiAdvanced 只在保存后回读，源码现状）
    const inputs = () => w.findAll('.adv-grid input')
    expect((inputs()[0]!.element as HTMLInputElement).value).toBe('')
    expect((inputs()[2]!.element as HTMLInputElement).value).toBe('4')

    // 填写 + 保存：5 次 set_field（payload_dir 留空 → null）
    await inputs()[0]!.setValue('x.exe, y.exe')
    await w.find('[data-test="ziwei-adv-save"]').trigger('click')
    await flushPromises()
    const writes = requestMock.mock.calls.filter(c => c[1] === 'set_field')
    expect(writes.map(c => c[2].path)).toEqual([
      'ziwei.allowed_targets',
      'ziwei.payload_dir',
      'ziwei.max_sessions',
      'ziwei.snapshots_ttl_days',
      'ziwei.runs_ttl_days',
    ])
    expect(writes[0]![2].value).toEqual(['x.exe', 'y.exe'])
    expect(writes[1]![2].value).toBe(null)
    expect(writes[2]![2].value).toBe(4)
    expect(writes[3]![2].value).toBe(30)
    expect(writes[4]![2].value).toBe(7)
    expect(w.text()).toContain('✓ 已保存')
    // 保存后 loadZiweiAdvanced 回读 config → 数组 join / 各字段回显
    expect((inputs()[0]!.element as HTMLInputElement).value).toBe('a.exe, b.exe')
    expect((inputs()[1]!.element as HTMLInputElement).value).toBe('/tmp/p')
    expect((inputs()[2]!.element as HTMLInputElement).value).toBe('8')
    expect((inputs()[3]!.element as HTMLInputElement).value).toBe('15')
    expect((inputs()[4]!.element as HTMLInputElement).value).toBe('3')

    // 保存失败 → ✗ 文案
    requestMock.mockImplementation((_m, cmd, data) => {
      if (cmd === 'set_field') return Promise.reject('磁盘只读')
      return Promise.resolve(makeImpl2()(_m, cmd, data))
    })
    await w.find('[data-test="ziwei-adv-save"]').trigger('click')
    await flushPromises()
    expect(w.text()).toContain('✗ 磁盘只读')
    w.unmount()
  })

  it('总闸关闭失败：set_field reject → ✗ 文案（adv 展开可见）+ 开关还原（不翻 false）', async () => {
    const w = await mountFull()
    const cb = w.find('[data-test="ziwei-enabled"]')
    expect((cb.element as HTMLInputElement).checked).toBe(true)
    // ziweiAdvMsg 渲染在高级配置面板内 → 先展开
    const details = w.find('details')
    ;(details.element as HTMLDetailsElement).open = true
    await details.trigger('toggle')
    requestMock.mockImplementation((_m, cmd, data) => {
      if (cmd === 'set_field') return Promise.reject('config 锁定')
      return Promise.resolve(makeImpl2()(_m, cmd, data))
    })
    await cb.setValue(false)
    await flushPromises()
    expect(w.text()).toContain('✗ config 锁定')
    // 失败还原开关：ziweiEnabled 拉回 true（:checked 属性未变时 Vue 不重写
    // DOM，故断言 label 文案而非 element.checked）
    expect(w.text()).toContain('已启用')
    expect(w.text()).not.toContain('紫微系统未启用。')
    w.unmount()
  })
})

