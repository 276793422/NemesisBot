import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { useToast } from '../../composables/useToast'

// D1/D2（2026-09-16 横扫存量加固）：安全页策略开关下拉。
// exec_unknown_policy（默认 allow）/ guardian_failure_policy（默认 ask）
// —— 改值即整体 security.config.save 写回；保存失败回滚显示。

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
  it('渲染两个下拉：值来自 config.get，各 3 个选项', async () => {
    const w = await mountView()
    const exec = policySelect(w, 'exec_unknown_policy')
    const guardian = policySelect(w, 'guardian_failure_policy')
    expect(exec.exists()).toBe(true)
    expect(guardian.exists()).toBe(true)
    expect((exec.element as HTMLSelectElement).value).toBe('allow')
    expect((guardian.element as HTMLSelectElement).value).toBe('ask')
    expect(exec.findAll('option').length).toBe(3)
    expect(guardian.findAll('option').length).toBe(3)
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

  it('旧配置缺键 → 下拉显示后端 serde 默认（allow / ask），不空白', async () => {
    requestMock.mockImplementation((_m: string, cmd: string, _d: any) => {
      if (cmd === 'config.get') {
        const c = configResult()
        delete c.exec_unknown_policy
        delete c.guardian_failure_policy
        return Promise.resolve(c)
      }
      return Promise.resolve({})
    })
    const w = await mountView()
    expect((policySelect(w, 'exec_unknown_policy').element as HTMLSelectElement).value).toBe('allow')
    expect((policySelect(w, 'guardian_failure_policy').element as HTMLSelectElement).value).toBe('ask')
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
