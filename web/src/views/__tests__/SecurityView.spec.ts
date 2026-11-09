import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { useToast } from '../../composables/useToast'

// D1/D2（2026-09-16 横扫存量加固）：安全页策略开关下拉。
// exec_unknown_policy（默认 allow）/ guardian_failure_policy（默认 ask）
// —— 改值即整体 security.config.save 写回；保存失败回滚显示。
// guardian_mode（同日无上下文 LLM 命令审计）：覆盖面开关，默认 off。

const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
}))

import SecurityView from '../SecurityView.vue'

function configResult() {
  return {
    default_action: 'allow',
    log_all_operations: true,
    exec_unknown_policy: 'allow',
    guardian_failure_policy: 'ask',
    guardian_mode: 'off',
    file_rules: { read: [] },
    dir_rules: { create: [] },
  }
}

// §2 免打扰（2026-10-11 无人值守计划 Part 2）审批自动批准开关。
describe('SecurityView 免打扰开关（§2）', () => {
  async function mountWith(cfg: any, status: any) {
    requestMock.mockImplementation((_m: string, cmd: string, _d: any) => {
      if (cmd === 'config.get') return Promise.resolve(cfg)
      if (cmd === 'approvals.status') return Promise.resolve(status)
      if (cmd === 'config.save') return Promise.resolve({ saved: true })
      if (cmd === 'audit') return Promise.resolve({ entries: [], total: 0 })
      if (cmd === 'stats') return Promise.resolve({ total_events: 0, by_level: {} })
      return Promise.resolve({})
    })
    const w = mount(SecurityView)
    await flushPromises()
    return w
  }

  it('老配置缺 approvals 段 → 开关不勾选；状态未装配显示诚实提示', async () => {
    const w = await mountWith(configResult(), { injected: false })
    const toggle = w.find('[data-test="auto-approve-toggle"]')
    expect(toggle.exists()).toBe(true)
    expect((toggle.element as HTMLInputElement).checked).toBe(false)
    expect(w.text()).toContain('未装配')
    w.unmount()
  })

  it('approvals.auto_approve=true + status enabled → 勾选 + 计数行显示', async () => {
    const cfg = { ...configResult(), approvals: { auto_approve: true } }
    const w = await mountWith(cfg, { injected: true, enabled: true, auto_approved_count: 7 })
    expect((w.find('[data-test="auto-approve-toggle"]').element as HTMLInputElement).checked).toBe(true)
    expect(w.find('[data-test="auto-approve-status"]').text()).toContain('自动批准 7 条')
    w.unmount()
  })

  it('勾选切换 → config.save 载荷带 approvals.auto_approve（其余字段原样）+ 状态刷新', async () => {
    const w = await mountWith(configResult(), { injected: true, enabled: false, auto_approved_count: 0 })
    requestMock.mockClear()
    await w.find('[data-test="auto-approve-toggle"]').setValue(true)
    await flushPromises()
    const saveCalls = requestMock.mock.calls.filter((c) => c[1] === 'config.save')
    expect(saveCalls.length).toBe(1)
    expect(saveCalls[0][2].approvals).toEqual({ auto_approve: true })
    expect(saveCalls[0][2].default_action).toBe('allow')
    expect(requestMock.mock.calls.some((c) => c[1] === 'approvals.status')).toBe(true)
    expect(useToast().toasts.some((t) => t.type === 'success')).toBe(true)
    w.unmount()
  })

  it('保存失败 → checkbox 回滚 + error toast，配置回退', async () => {
    const w = await mountWith(configResult(), { injected: true, enabled: false, auto_approved_count: 0 })
    requestMock.mockImplementation((_m: string, cmd: string, _d: any) => {
      if (cmd === 'config.save') return Promise.reject(new Error('ws down'))
      if (cmd === 'config.get') return Promise.resolve(configResult())
      return Promise.resolve({})
    })
    await w.find('[data-test="auto-approve-toggle"]').setValue(true)
    await flushPromises()
    expect((w.find('[data-test="auto-approve-toggle"]').element as HTMLInputElement).checked).toBe(false)
    expect(useToast().toasts.some((t) => t.type === 'error')).toBe(true)
    w.unmount()
  })
})

beforeEach(() => {
  requestMock.mockReset()
  useToast().toasts.splice(0)
  requestMock.mockImplementation((_m: string, cmd: string, _data: any) => {
    if (cmd === 'config.get') return Promise.resolve(configResult())
    if (cmd === 'config.save') return Promise.resolve({ saved: true })
    if (cmd === 'audit') return Promise.resolve({ entries: [], total: 0 })
    if (cmd === 'stats') return Promise.resolve({ total_events: 0, by_level: {} })
    return Promise.resolve({})
  })
})

async function mountView() {
  const w = mount(SecurityView)
  await flushPromises()
  return w
}

function policySelect(w: { find: (s: string) => any }, key: string) {
  return w.find(`select[data-test="${key}"]`)
}

describe('SecurityView 策略开关（D1/D2）', () => {
  it('渲染三个下拉：值来自 config.get，各 3 个选项', async () => {
    const w = await mountView()
    const exec = policySelect(w, 'exec_unknown_policy')
    const guardian = policySelect(w, 'guardian_failure_policy')
    const mode = policySelect(w, 'guardian_mode')
    expect(exec.exists()).toBe(true)
    expect(guardian.exists()).toBe(true)
    expect(mode.exists()).toBe(true)
    expect((exec.element as HTMLSelectElement).value).toBe('allow')
    expect((guardian.element as HTMLSelectElement).value).toBe('ask')
    expect((mode.element as HTMLSelectElement).value).toBe('off')
    expect(exec.findAll('option').length).toBe(3)
    expect(guardian.findAll('option').length).toBe(3)
    expect(mode.findAll('option').length).toBe(3)
    w.unmount()
  })

  it('改值 → 整体 config.save 写回（含改后键 + 其余字段原样）', async () => {
    const w = await mountView()
    requestMock.mockClear()
    await policySelect(w, 'exec_unknown_policy').setValue('deny')
    await flushPromises()
    const saveCalls = requestMock.mock.calls.filter(c => c[1] === 'config.save')
    expect(saveCalls.length).toBe(1)
    const payload = saveCalls[0][2]
    expect(payload.exec_unknown_policy).toBe('deny')
    // 整体写回：其余字段不被丢弃
    expect(payload.default_action).toBe('allow')
    expect(payload.dir_rules).toEqual({ create: [] })
    expect(useToast().toasts.some(t => t.type === 'success')).toBe(true)
    w.unmount()
  })

  it('保存失败 → 本地显示回滚 + error toast', async () => {
    const w = await mountView()
    requestMock.mockImplementation((_m: string, cmd: string, _d: any) => {
      if (cmd === 'config.save') return Promise.reject(new Error('ws down'))
      if (cmd === 'config.get') return Promise.resolve(configResult())
      return Promise.resolve({})
    })
    await policySelect(w, 'guardian_failure_policy').setValue('deny')
    await flushPromises()
    expect((policySelect(w, 'guardian_failure_policy').element as HTMLSelectElement).value).toBe('ask')
    expect(useToast().toasts.some(t => t.type === 'error')).toBe(true)
    expect(useToast().toasts.some(t => t.type === 'success')).toBe(false)
    w.unmount()
  })

  it('旧配置缺键 → 下拉显示后端 serde 默认（allow / ask / off），不空白', async () => {
    requestMock.mockImplementation((_m: string, cmd: string, _d: any) => {
      if (cmd === 'config.get') {
        // Partial 化：模拟"旧配置缺键"要 delete 字段——推断类型的必有属性
        // 不可 delete（vue-tsc TS2790），本用例语义就是缺键。
        const c: Partial<ReturnType<typeof configResult>> = configResult()
        delete c.exec_unknown_policy
        delete c.guardian_failure_policy
        delete c.guardian_mode
        return Promise.resolve(c)
      }
      return Promise.resolve({})
    })
    const w = await mountView()
    expect((policySelect(w, 'exec_unknown_policy').element as HTMLSelectElement).value).toBe('allow')
    expect((policySelect(w, 'guardian_failure_policy').element as HTMLSelectElement).value).toBe('ask')
    expect((policySelect(w, 'guardian_mode').element as HTMLSelectElement).value).toBe('off')
    w.unmount()
  })

  it('guardian_mode 改值 → config.save 写回 + 成功 toast', async () => {
    const w = await mountView()
    requestMock.mockClear()
    await policySelect(w, 'guardian_mode').setValue('critical')
    await flushPromises()
    const saveCalls = requestMock.mock.calls.filter(c => c[1] === 'config.save')
    expect(saveCalls.length).toBe(1)
    expect(saveCalls[0][2].guardian_mode).toBe('critical')
    expect(useToast().toasts.some(t => t.type === 'success')).toBe(true)
    w.unmount()
  })

  it('JSON 编辑模式下拉收起（由编辑器接管，显示提示）', async () => {
    const w = await mountView()
    expect(policySelect(w, 'exec_unknown_policy').exists()).toBe(true)
    await w.findAll('button').find(b => b.text() === '编辑')!.trigger('click')
    expect(policySelect(w, 'exec_unknown_policy').exists()).toBe(false)
    expect(w.text()).toContain('JSON 编辑模式下')
    w.unmount()
  })

  it('策略键不再重复出现在通用标量网格中', async () => {
    const w = await mountView()
    const grid = w.find('.settings-grid:not([data-test="policy-switches"])')
    const keys = grid.findAll('.settings-key').map(k => k.text())
    expect(keys).not.toContain('exec_unknown_policy')
    expect(keys).not.toContain('guardian_failure_policy')
    expect(keys).toContain('default_action')
    w.unmount()
  })
})

// ── 批次 3（2026-11-08 配置页补全）：审批记忆规则条目级管理 + eval 规则 TAB。
// 条目级走 approvals.add/remove/clear（与「总是允许」自动写入同一闸）；
// eval 规则走 eval_rules.list/add/update/set_enabled/remove/reset（与 CLI
// `eval rules *` 同源，真相源 nemesis-config::eval_rules）。本 spec 钉前端
// 请求形态与渲染分支；后端契约由 security approvals/eval_rules handler 测试钉住。
describe('SecurityView 审批记忆规则 + eval 规则 TAB（批次 3）', () => {
  const evalRule = {
    id: 'exfil-path', description: '可疑外传路径', level: 'high', enabled: true,
    source: 'tool_trace', min_count: 1,
    conditions: [{ field: 'path', op: 'contains', value: '/etc' }],
  }

  async function mountWithRules(opts: { approvalRules?: any[]; evalRules?: any[] } = {}) {
    requestMock.mockReset()
    requestMock.mockImplementation((_m: string, cmd: string, _d?: any) => {
      if (cmd === 'config.get') return Promise.resolve(configResult())
      if (cmd === 'approvals.status') return Promise.resolve({ injected: false })
      if (cmd === 'config.save') return Promise.resolve({ saved: true })
      if (cmd === 'audit') return Promise.resolve({ entries: [], total: 0 })
      if (cmd === 'stats') return Promise.resolve({ total_events: 0, by_level: {} })
      if (cmd === 'approvals.list') return Promise.resolve({ rules: opts.approvalRules ?? [] })
      if (cmd === 'eval_rules.list') return Promise.resolve({ rules: opts.evalRules ?? [], version: 3 })
      if (cmd === 'approvals.add') return Promise.resolve({ added: true, pattern: _d?.pattern })
      if (cmd === 'eval_rules.add') return Promise.resolve({ added: true })
      if (cmd === 'eval_rules.set_enabled') return Promise.resolve({ ok: true })
      if (cmd === 'eval_rules.reset') return Promise.resolve({ rules: [], version: 1 })
      return Promise.resolve({})
    })
    const w = mount(SecurityView)
    await flushPromises()
    return w
  }

  it('审批规则表：空表渲染空态；表单填 op/pattern → 添加 → approvals.add {op,pattern,action:allow} + 表单清空', async () => {
    const w = await mountWithRules()
    expect(w.find('[data-test="approval-rules-card"]').exists()).toBe(true)
    expect(w.text()).toContain('暂无规则')
    const opInput = w.find('input[placeholder*="操作类型"]')
    const patInput = w.find('input[placeholder*="匹配 pattern"]')
    await opInput.setValue('process_exec')
    await patInput.setValue('cargo test *')
    await w.findAll('button').find(b => b.text() === '添加')!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('security', 'approvals.add', { op: 'process_exec', pattern: 'cargo test *', action: 'allow' })
    expect(useToast().toasts.some(t => t.type === 'success' && t.message.includes('规则已添加'))).toBe(true)
    expect((opInput.element as HTMLInputElement).value).toBe('') // 表单清空
    w.unmount()
  })

  it('空 op 或 pattern → 本地拦截（不发 approvals.add）', async () => {
    const w = await mountWithRules()
    await w.find('input[placeholder*="匹配 pattern"]').setValue('cargo test *')
    await w.findAll('button').find(b => b.text() === '添加')!.trigger('click')
    await flushPromises()
    expect(requestMock).not.toHaveBeenCalledWith('security', 'approvals.add', expect.anything())
    expect(useToast().toasts.some(t => t.type === 'error' && t.message.includes('必填'))).toBe(true)
    w.unmount()
  })

  it('eval 规则 TAB：列表渲染（table 行数 + 版本计数）；toggle 启用 → eval_rules.set_enabled {id,enabled}', async () => {
    const w = await mountWithRules({ evalRules: [evalRule, { ...evalRule, id: 'rm-rf', enabled: false, level: 'critical' }] })
    await w.findAll('button.tab').find(b => b.text() === '评估规则')!.trigger('click')
    await flushPromises()
    expect(w.find('[data-test="eval-rules-tab"]').exists()).toBe(true)
    const rows = w.find('[data-test="eval-rules-table"]').findAll('tbody tr')
    expect(rows).toHaveLength(2)
    expect(w.text()).toContain('共 2 条，1 条启用')
    const firstToggle = rows[0].find('input[type="checkbox"]')
    await firstToggle.setValue(false)
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('security', 'eval_rules.set_enabled', { id: 'exfil-path', enabled: false })
    w.unmount()
  })

  it('eval 规则新增：modal 打开 → 填 id → eval-rule-save → eval_rules.add {rule}（conditions/level/source 完整载荷）', async () => {
    const w = await mountWithRules()
    await w.findAll('button.tab').find(b => b.text() === '评估规则')!.trigger('click')
    await flushPromises()
    await w.find('[data-test="eval-rule-new"]').trigger('click')
    await flushPromises()
    expect(w.find('[data-test="eval-rule-modal"]').exists()).toBe(true)
    // 填必填 id + 条件字段（placeholder 精确定位：id=probe-aws-cli / field=字段路径）
    const modal = w.find('[data-test="eval-rule-modal"]')
    await modal.find('input[placeholder="probe-aws-cli"]').setValue('my-rule')
    await modal.find('input[placeholder="字段路径（如 arguments.command）"]').setValue('command')
    await w.find('[data-test="eval-rule-save"]').trigger('click')
    await flushPromises()
    const addCall = requestMock.mock.calls.find(c => c[1] === 'eval_rules.add')
    expect(addCall).toBeTruthy()
    expect(addCall![2].rule.id).toBe('my-rule')
    expect(addCall![2].rule.conditions).toEqual([{ field: 'command', op: 'contains', value: '' }])
    expect(useToast().toasts.some(t => t.type === 'success' && t.message.includes('规则已添加'))).toBe(true)
    w.unmount()
  })
})
