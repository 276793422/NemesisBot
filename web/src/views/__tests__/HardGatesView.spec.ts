import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { useToast } from '../../composables/useToast'

// 危险闸页 spec（2026-11-20）：security.exec_deny_list 渲染契约（分组
// 表格 + enabled 投影 + 追加规则回显）+ exec_deny_set 提交载荷 + 保存
// 失败回滚 + 追加规则增删 + 高级出口横幅。42 条全量契约归后端
// exec_deny_tests（nemesis-web）与 builtin_deny_entries_metadata_contract
// （nemesis-tools），此处用小 fixture 钉渲染行为。

const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
}))

import HardGatesView from '../HardGatesView.vue'

function entry(name: string, category: string, severity = 'high', enabled = true) {
  return { name, category, severity, reason: `${name} 拦截说明`, pattern: `\\b${name}\\b`, enabled }
}

function listResult() {
  return {
    entries: [
      entry('rm_recursive_force', 'destructive'),
      entry('sudo', 'privilege', 'medium', false),
      entry('ssh_remote_login', 'remote', 'medium'),
      entry('docker_exec', 'container', 'medium'),
    ],
    extra: ['forbidden\\.exe'],
    custom_replacement_active: false,
  }
}

beforeEach(() => {
  requestMock.mockReset()
  useToast().toasts.splice(0)
  requestMock.mockImplementation((_m: string, cmd: string, _d: any) => {
    if (cmd === 'exec_deny_list') return Promise.resolve(listResult())
    if (cmd === 'exec_deny_set') return Promise.resolve({ saved: true, hot: true })
    return Promise.resolve({})
  })
})

async function mountView() {
  const w = mount(HardGatesView)
  await flushPromises()
  return w
}

describe('HardGatesView 工具执行词表', () => {
  it('list 渲染契约：分组表格 + 开关初始态（enabled 投影）+ 追加规则回显', async () => {
    const w = await mountView()
    expect(w.find('[data-test="deny-table-destructive"]').exists()).toBe(true)
    expect(w.find('[data-test="deny-table-privilege"]').exists()).toBe(true)
    expect(w.find('[data-test="deny-table-remote"]').exists()).toBe(true)
    // disabled 条目 toggle 无 active；enabled 条目有
    expect(w.find('[data-test="rule-toggle-sudo"]').classes()).not.toContain('active')
    expect(w.find('[data-test="rule-toggle-rm_recursive_force"]').classes()).toContain('active')
    // 追加规则回显
    expect(w.findAll('[data-test="extra-list"] .extra-row')).toHaveLength(1)
    // 初始即快照态：无 dirty、保存钮禁用
    expect(w.find('[data-test="dirty-badge"]').exists()).toBe(false)
    expect(w.find('[data-test="save-btn"]').attributes('disabled')).toBeDefined()
    // 无高级出口：横幅不渲染
    expect(w.find('[data-test="custom-replacement-banner"]').exists()).toBe(false)
    w.unmount()
  })

  it('开关翻转 → dirty；保存提交 disabled/extra 全量载荷 + success toast', async () => {
    const w = await mountView()
    requestMock.mockClear()
    // 开启 sudo（从 disabled 摘除）
    await w.find('[data-test="rule-toggle-sudo"]').trigger('click')
    // 关闭 rm_recursive_force（加入 disabled）
    await w.find('[data-test="rule-toggle-rm_recursive_force"]').trigger('click')
    await flushPromises()
    expect(w.find('[data-test="dirty-badge"]').exists()).toBe(true)
    await w.find('[data-test="save-btn"]').trigger('click')
    await flushPromises()
    const saveCalls = requestMock.mock.calls.filter((c) => c[1] === 'exec_deny_set')
    expect(saveCalls).toHaveLength(1)
    expect(saveCalls[0][0]).toBe('security')
    expect(saveCalls[0][2]).toEqual({ disabled: ['rm_recursive_force'], extra: ['forbidden\\.exe'] })
    expect(useToast().toasts.some((t) => t.type === 'success')).toBe(true)
    // 保存后快照同步 → dirty 消失、保存钮回到禁用
    expect(w.find('[data-test="dirty-badge"]').exists()).toBe(false)
    expect(w.find('[data-test="save-btn"]').attributes('disabled')).toBeDefined()
    w.unmount()
  })

  it('保存失败 → 开关回滚 + error toast（无 success）', async () => {
    const w = await mountView()
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'exec_deny_list') return Promise.resolve(listResult())
      if (cmd === 'exec_deny_set') return Promise.reject(new Error('ws down'))
      return Promise.resolve({})
    })
    await w.find('[data-test="rule-toggle-sudo"]').trigger('click')
    await flushPromises()
    await w.find('[data-test="save-btn"]').trigger('click')
    await flushPromises()
    // 回滚：sudo 回到 disabled 态（无 active）
    expect(w.find('[data-test="rule-toggle-sudo"]').classes()).not.toContain('active')
    expect(useToast().toasts.some((t) => t.type === 'error')).toBe(true)
    expect(useToast().toasts.some((t) => t.type === 'success')).toBe(false)
    w.unmount()
  })

  it('追加规则增删：添加进列表（去重拒绝）、删除摘行、载荷跟随', async () => {
    const w = await mountView()
    const input = w.find('[data-test="extra-input"]')
    await input.setValue('evil\\.exe')
    await w.find('[data-test="extra-add"]').trigger('click')
    await flushPromises()
    expect(w.findAll('[data-test="extra-list"] .extra-row')).toHaveLength(2)
    // 重复添加被拒
    await input.setValue('evil\\.exe')
    await w.find('[data-test="extra-add"]').trigger('click')
    expect(useToast().toasts.some((t) => t.type === 'error')).toBe(true)
    expect(w.findAll('[data-test="extra-list"] .extra-row')).toHaveLength(2)
    // 删除第一行
    await w.find('[data-test="extra-remove-0"]').trigger('click')
    expect(w.findAll('[data-test="extra-list"] .extra-row')).toHaveLength(1)
    // 保存载荷含追加列表终态
    requestMock.mockClear()
    await w.find('[data-test="save-btn"]').trigger('click')
    await flushPromises()
    const saveCalls = requestMock.mock.calls.filter((c) => c[1] === 'exec_deny_set')
    expect(saveCalls[0][2].extra).toEqual(['evil\\.exe'])
    w.unmount()
  })

  it('高级出口激活 → 横幅诚实注记', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'exec_deny_list') return Promise.resolve({ ...listResult(), custom_replacement_active: true })
      return Promise.resolve({})
    })
    const w = await mountView()
    expect(w.find('[data-test="custom-replacement-banner"]').exists()).toBe(true)
    w.unmount()
  })
})
