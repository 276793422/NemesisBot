import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { useToast } from '../../composables/useToast'

// 人工审批专页 spec（十层页面化 2026-11-10，自旧 SecurityView.spec 迁移）：
// §2 免打扰开关 / 审批记忆规则条目级管理 / eval 规则节（用户拍板 3：先挂
// 本页，位置后议）。请求形态与渲染分支逐断言平移；后端契约由 security
// approvals/eval_rules handler 测试钉住。

const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
}))

import SecurityApprovalView from '../SecurityApprovalView.vue'

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
describe('SecurityApprovalView 免打扰开关（§2）', () => {
  async function mountWith(cfg: any, status: any) {
    requestMock.mockImplementation((_m: string, cmd: string, _d: any) => {
      if (cmd === 'config.get') return Promise.resolve(cfg)
      if (cmd === 'approvals.status') return Promise.resolve(status)
      if (cmd === 'config.save') return Promise.resolve({ saved: true })
      if (cmd === 'approvals.list') return Promise.resolve({ rules: [] })
      if (cmd === 'eval_rules.list') return Promise.resolve({ rules: [], version: 1 })
      return Promise.resolve({})
    })
    const w = mount(SecurityApprovalView)
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
    if (cmd === 'approvals.status') return Promise.resolve({ injected: false })
    if (cmd === 'approvals.list') return Promise.resolve({ rules: [] })
    if (cmd === 'eval_rules.list') return Promise.resolve({ rules: [], version: 1 })
    return Promise.resolve({})
  })
})

async function mountView() {
  const w = mount(SecurityApprovalView)
  await flushPromises()
  return w
}

// ── 审批记忆规则 + eval 规则节（批次 3 迁移）──────────────────────────
describe('SecurityApprovalView 审批记忆规则 + eval 规则', () => {
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
      if (cmd === 'approvals.list') return Promise.resolve({ rules: opts.approvalRules ?? [] })
      if (cmd === 'eval_rules.list') return Promise.resolve({ rules: opts.evalRules ?? [], version: 3 })
      if (cmd === 'approvals.add') return Promise.resolve({ added: true, pattern: _d?.pattern })
      if (cmd === 'eval_rules.add') return Promise.resolve({ added: true })
      if (cmd === 'eval_rules.set_enabled') return Promise.resolve({ ok: true })
      if (cmd === 'eval_rules.reset') return Promise.resolve({ rules: [], version: 1 })
      return Promise.resolve({})
    })
    const w = mount(SecurityApprovalView)
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
    await w.find('[data-test="approval-rule-add"]').trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('security', 'approvals.add', { op: 'process_exec', pattern: 'cargo test *', action: 'allow' })
    expect(useToast().toasts.some(t => t.type === 'success' && t.message.includes('规则已添加'))).toBe(true)
    expect((opInput.element as HTMLInputElement).value).toBe('') // 表单清空
    w.unmount()
  })

  it('空 op 或 pattern → 本地拦截（不发 approvals.add）', async () => {
    const w = await mountWithRules()
    await w.find('input[placeholder*="匹配 pattern"]').setValue('cargo test *')
    await w.find('[data-test="approval-rule-add"]').trigger('click')
    await flushPromises()
    expect(requestMock).not.toHaveBeenCalledWith('security', 'approvals.add', expect.anything())
    expect(useToast().toasts.some(t => t.type === 'error' && t.message.includes('必填'))).toBe(true)
    w.unmount()
  })

  it('eval 规则节：列表渲染（table 行数 + 版本计数）；toggle 启用 → eval_rules.set_enabled {id,enabled}', async () => {
    const w = await mountWithRules({ evalRules: [evalRule, { ...evalRule, id: 'rm-rf', enabled: false, level: 'critical' }] })
    expect(w.find('[data-test="eval-rules-card"]').exists()).toBe(true)
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
