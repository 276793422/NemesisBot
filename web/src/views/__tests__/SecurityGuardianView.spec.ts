import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { useToast } from '../../composables/useToast'

// Guardian 二审专页 spec（十层页面化 2026-11-10，自旧 SecurityView.spec
// 策略开关 describe 迁移）：guardian_failure_policy / guardian_mode 两
// 下拉——改值即整体 security.config.save 写回；保存失败回滚显示；缺键
// 显示后端 serde 默认。

const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
}))

import SecurityGuardianView from '../SecurityGuardianView.vue'

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

beforeEach(() => {
  requestMock.mockReset()
  useToast().toasts.splice(0)
  requestMock.mockImplementation((_m: string, cmd: string, _data: any) => {
    if (cmd === 'config.get') return Promise.resolve(configResult())
    if (cmd === 'config.save') return Promise.resolve({ saved: true })
    if (cmd === 'audit') return Promise.resolve({ entries: [], total: 0 })
    return Promise.resolve({})
  })
})

async function mountView() {
  const w = mount(SecurityGuardianView)
  await flushPromises()
  return w
}

function policySelect(w: { find: (s: string) => any }, key: string) {
  return w.find(`select[data-test="${key}"]`)
}

describe('SecurityGuardianView 策略开关（D1/D2 guardian 半边）', () => {
  it('渲染两下拉：值来自 config.get，各 3 个选项', async () => {
    const w = await mountView()
    const guardian = policySelect(w, 'guardian-failure-policy')
    const mode = policySelect(w, 'guardian-mode')
    expect(guardian.exists()).toBe(true)
    expect(mode.exists()).toBe(true)
    expect((guardian.element as HTMLSelectElement).value).toBe('ask')
    expect((mode.element as HTMLSelectElement).value).toBe('off')
    expect(guardian.findAll('option').length).toBe(3)
    expect(mode.findAll('option').length).toBe(3)
    w.unmount()
  })

  it('改值 → 整体 config.save 写回（含改后键 + 其余字段原样）', async () => {
    const w = await mountView()
    requestMock.mockClear()
    await policySelect(w, 'guardian-failure-policy').setValue('deny')
    await flushPromises()
    const saveCalls = requestMock.mock.calls.filter(c => c[1] === 'config.save')
    expect(saveCalls.length).toBe(1)
    const payload = saveCalls[0][2]
    expect(payload.guardian_failure_policy).toBe('deny')
    // 整体写回：其余字段不被丢弃
    expect(payload.default_action).toBe('allow')
    expect(payload.dir_rules).toEqual({ create: [] })
    expect(useToast().toasts.some(t => t.type === 'success')).toBe(true)
    w.unmount()
  })

  it('保存失败 → 本地显示回滚 + error toast（无 success）', async () => {
    const w = await mountView()
    requestMock.mockImplementation((_m: string, cmd: string, _d: any) => {
      if (cmd === 'config.save') return Promise.reject(new Error('ws down'))
      if (cmd === 'config.get') return Promise.resolve(configResult())
      return Promise.resolve({})
    })
    await policySelect(w, 'guardian-failure-policy').setValue('deny')
    await flushPromises()
    expect((policySelect(w, 'guardian-failure-policy').element as HTMLSelectElement).value).toBe('ask')
    expect(useToast().toasts.some(t => t.type === 'error')).toBe(true)
    expect(useToast().toasts.some(t => t.type === 'success')).toBe(false)
    w.unmount()
  })

  it('旧配置缺键 → 下拉显示后端 serde 默认（ask / off），不空白', async () => {
    requestMock.mockImplementation((_m: string, cmd: string, _d: any) => {
      if (cmd === 'config.get') {
        // Partial 化：模拟"旧配置缺键"要 delete 字段（TS2790 同款处理）。
        const c: Partial<ReturnType<typeof configResult>> = configResult()
        delete c.guardian_failure_policy
        delete c.guardian_mode
        return Promise.resolve(c)
      }
      return Promise.resolve({})
    })
    const w = await mountView()
    expect((policySelect(w, 'guardian-failure-policy').element as HTMLSelectElement).value).toBe('ask')
    expect((policySelect(w, 'guardian-mode').element as HTMLSelectElement).value).toBe('off')
    w.unmount()
  })

  it('guardian_mode 改值 → config.save 写回 + 成功 toast', async () => {
    const w = await mountView()
    requestMock.mockClear()
    await policySelect(w, 'guardian-mode').setValue('critical')
    await flushPromises()
    const saveCalls = requestMock.mock.calls.filter(c => c[1] === 'config.save')
    expect(saveCalls.length).toBe(1)
    expect(saveCalls[0][2].guardian_mode).toBe('critical')
    expect(useToast().toasts.some(t => t.type === 'success')).toBe(true)
    w.unmount()
  })

  it('guardian 审计事件过滤：policy_rule 以 guardian 开头才进裁决表', async () => {
    requestMock.mockImplementation((_m: string, cmd: string, _d: any) => {
      if (cmd === 'config.get') return Promise.resolve(configResult())
      if (cmd === 'audit') {
        return Promise.resolve({
          entries: [
            { timestamp: '2026-11-10T10:00:00Z', operation: 'process_exec', target: 'cmd', policy: 'guardian_approved', result: 'allow' },
            { timestamp: '2026-11-10T10:01:00Z', operation: 'file_write', target: 'x', policy: 'abac', result: 'deny' },
            { timestamp: '2026-11-10T10:02:00Z', operation: 'process_exec', target: 'y', policy: 'guardian_denied', result: 'deny' },
          ],
          total: 3,
        })
      }
      return Promise.resolve({})
    })
    const w = await mountView()
    const rows = w.find('[data-test="guardian-audit-table"]').findAll('tbody tr')
    expect(rows).toHaveLength(2)
    expect(w.text()).not.toContain('abac')
    w.unmount()
  })
})
