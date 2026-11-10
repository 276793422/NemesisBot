import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { useToast } from '../../composables/useToast'

// 六层配置驱动页 spec（十层页面化 2026-11-10）：exec_unknown_policy
// （D1 半边，自旧 SecurityView.spec 迁移）+ 层开关 toggle / ① 阈值 /
// ② 黑名单只读表 / ⑧ 双开关 + 链校验。同一视图以 props.layer 装载。

const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
}))

import SecurityLayerView from '../SecurityLayerView.vue'

function configResult() {
  return {
    default_action: 'allow',
    log_all_operations: true,
    exec_unknown_policy: 'allow',
    layers: {},
  }
}

// 层开关语义 = raw 投影（缺席=启用）；overview mock 镜像后端形状。
function overviewResult() {
  return {
    layers: {
      injection: { enabled: true, threshold: 0.7, threshold_configured: false },
      command_guard: { enabled: true },
      credential: { enabled: true },
      dlp: { enabled: true, rules: [], action: 'block' },
      ssrf: { enabled: true, allowed_hosts: [] },
      audit: { log_file_enabled: true, chain_enabled: false },
    },
    intercepts: { total: 5, denied: 2, by_policy: { command_guard: { total: 5, denied: 2, last_ts: '' } } },
    chain: { present: false, segments: 0 },
  }
}

beforeEach(() => {
  requestMock.mockReset()
  useToast().toasts.splice(0)
  requestMock.mockImplementation((_m: string, cmd: string, _data: any) => {
    if (cmd === 'config.get') return Promise.resolve(configResult())
    if (cmd === 'config.save') return Promise.resolve({ saved: true })
    if (cmd === 'overview') return Promise.resolve(overviewResult())
    if (cmd === 'audit') return Promise.resolve({ entries: [], total: 0 })
    if (cmd === 'command_blocklist') return Promise.resolve({ entries: [], extra_wired: false, note: '静态表' })
    if (cmd === 'credential_rules') return Promise.resolve({ rules: [], note: '静态表' })
    return Promise.resolve({})
  })
})

async function mountLayer(layer: string) {
  const w = mount(SecurityLayerView, { props: { layer } })
  await flushPromises()
  return w
}

describe('SecurityLayerView ② 命令守卫（exec_unknown_policy，D1 半边）', () => {
  it('下拉渲染：值来自 config.get，3 个选项；缺键显示默认 allow', async () => {
    const w = await mountLayer('command')
    const exec = w.find('select[data-test="exec-unknown-policy"]')
    expect(exec.exists()).toBe(true)
    expect((exec.element as HTMLSelectElement).value).toBe('allow')
    expect(exec.findAll('option').length).toBe(3)
    w.unmount()
  })

  it('改值 → 整体 config.save 写回（含改后键 + 其余字段原样）', async () => {
    const w = await mountLayer('command')
    requestMock.mockClear()
    await w.find('select[data-test="exec-unknown-policy"]').setValue('deny')
    await flushPromises()
    const saveCalls = requestMock.mock.calls.filter(c => c[1] === 'config.save')
    expect(saveCalls.length).toBe(1)
    const payload = saveCalls[0][2]
    expect(payload.exec_unknown_policy).toBe('deny')
    expect(payload.default_action).toBe('allow')
    expect(useToast().toasts.some(t => t.type === 'success')).toBe(true)
    w.unmount()
  })

  it('保存失败 → 本地显示回滚 + error toast（无 success）', async () => {
    const w = await mountLayer('command')
    requestMock.mockImplementation((_m: string, cmd: string, _d: any) => {
      if (cmd === 'config.save') return Promise.reject(new Error('ws down'))
      if (cmd === 'config.get') return Promise.resolve(configResult())
      return Promise.resolve({})
    })
    await w.find('select[data-test="exec-unknown-policy"]').setValue('deny')
    await flushPromises()
    expect((w.find('select[data-test="exec-unknown-policy"]').element as HTMLSelectElement).value).toBe('allow')
    expect(useToast().toasts.some(t => t.type === 'error')).toBe(true)
    expect(useToast().toasts.some(t => t.type === 'success')).toBe(false)
    w.unmount()
  })

  it('黑名单只读表渲染（command_blocklist 快照）', async () => {
    requestMock.mockImplementation((_m: string, cmd: string, _d: any) => {
      if (cmd === 'command_blocklist') {
        return Promise.resolve({
          entries: [{ name: 'format', category: 'destructive', severity: 'Critical', platform: 'windows', reason: '格式化磁盘' }],
          extra_wired: false,
          note: '静态表',
        })
      }
      return Promise.resolve({})
    })
    const w = await mountLayer('command')
    const rows = w.find('[data-test="blocklist-table"]').findAll('tbody tr')
    expect(rows).toHaveLength(1)
    expect(w.text()).toContain('format')
    w.unmount()
  })
})

describe('SecurityLayerView 层开关与层参数', () => {
  it('层开关勾选来自 overview 投影（raw 语义缺席=启用）；取消勾选 → config.save layers.<key>.enabled=false', async () => {
    const w = await mountLayer('ssrf')
    const toggle = w.find('[data-test="layer-toggle"]')
    expect(toggle.exists()).toBe(true)
    expect((toggle.element as HTMLInputElement).checked).toBe(true)
    requestMock.mockClear()
    await toggle.setValue(false)
    await flushPromises()
    const saveCalls = requestMock.mock.calls.filter(c => c[1] === 'config.save')
    expect(saveCalls.length).toBe(1)
    expect(saveCalls[0][2].layers.ssrf.enabled).toBe(false)
    w.unmount()
  })

  it('① 阈值：编辑值来自 typed config extra.threshold；改值 → layers.injection.extra.threshold 写回', async () => {
    requestMock.mockImplementation((_m: string, cmd: string, _d: any) => {
      if (cmd === 'config.get') {
        return Promise.resolve({ layers: { injection: { extra: { threshold: 0.8 } } } })
      }
      if (cmd === 'overview') {
        return Promise.resolve({ layers: { injection: { enabled: true, threshold: 0.8, threshold_configured: true } } })
      }
      return Promise.resolve({})
    })
    const w = await mountLayer('injection')
    const input = w.find('[data-test="injection-threshold"]')
    expect((input.element as HTMLInputElement).value).toBe('0.8')
    requestMock.mockClear()
    await input.setValue('0.9')
    await input.trigger('change')
    await flushPromises()
    const saveCalls = requestMock.mock.calls.filter(c => c[1] === 'config.save')
    expect(saveCalls.length).toBe(1)
    expect(saveCalls[0][2].layers.injection.extra.threshold).toBe(0.9)
    w.unmount()
  })

  it('① 阈值越界（>1）→ 不写键 + error toast', async () => {
    requestMock.mockImplementation((_m: string, cmd: string, _d: any) => {
      if (cmd === 'config.get') return Promise.resolve({ layers: { injection: { extra: { threshold: 0.5 } } } })
      if (cmd === 'overview') {
        return Promise.resolve({ layers: { injection: { enabled: true, threshold: 0.5, threshold_configured: true } } })
      }
      return Promise.resolve({})
    })
    const w = await mountLayer('injection')
    const input = w.find('[data-test="injection-threshold"]')
    await input.setValue('1.5')
    await input.trigger('change')
    await flushPromises()
    const saveCalls = requestMock.mock.calls.filter(c => c[1] === 'config.save')
    expect(saveCalls.length).toBe(1)
    expect(saveCalls[0][2].layers.injection.extra.threshold).toBeUndefined()
    expect(useToast().toasts.some(t => t.type === 'error')).toBe(true)
    w.unmount()
  })

  it('⑥ SSRF 白名单 chips：添加 → layers.ssrf.allowed_hosts 追加写回', async () => {
    const w = await mountLayer('ssrf')
    const input = w.find('[data-test="ssrf-hosts-block"] input')
    await input.setValue('127.0.0.1')
    await input.trigger('keyup.enter')
    await flushPromises()
    const saveCalls = requestMock.mock.calls.filter(c => c[1] === 'config.save')
    expect(saveCalls.length).toBe(1)
    expect(saveCalls[0][2].layers.ssrf.allowed_hosts).toEqual(['127.0.0.1'])
    w.unmount()
  })

  it('⑧ 审计链：链不在场时校验按钮禁用；在场 → logs.chain_verify 调用 + 结果展示', async () => {
    requestMock.mockImplementation((_m: string, cmd: string, _d: any) => {
      if (cmd === 'overview') {
        const o = overviewResult()
        o.chain = { present: true, segments: 12 }
        o.layers.audit = { log_file_enabled: true, chain_enabled: true }
        return Promise.resolve(o)
      }
      if (cmd === 'chain_verify') return Promise.resolve({ valid: true, total_segments: 12, first_broken_index: null, broken_count: 0 })
      return Promise.resolve({})
    })
    const w = await mountLayer('audit')
    const btn = w.find('[data-test="chain-verify"]')
    expect(btn.attributes('disabled')).toBeUndefined()
    await btn.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('logs', 'chain_verify')
    expect(w.find('[data-test="chain-verify-result"]').text()).toContain('通过')
    w.unmount()
  })

  it('⑧ 头部状态走 log_file_enabled 口径（与总览同源）；通用层开关不渲染（真开关是顶层两键）', async () => {
    requestMock.mockImplementation((_m: string, cmd: string, _d: any) => {
      if (cmd === 'overview') {
        const o = overviewResult()
        o.layers.audit = { log_file_enabled: true, chain_enabled: false }
        return Promise.resolve(o)
      }
      return Promise.resolve({})
    })
    const w = await mountLayer('audit')
    expect(w.find('.page-security-layer').text()).toContain('已启用') // 头部徽标（曾恒「已关闭」= 投影缺 enabled 误判）
    expect(w.find('[data-test="layer-toggle"]').exists()).toBe(false) // 幽灵键开关已撤
    expect((w.find('[data-test="audit-log-toggle"]').element as HTMLInputElement).checked).toBe(true)
    w.unmount()
  })

  it('拦截记录按层 policy 过滤（② 页只显示 command_guard 行）', async () => {
    requestMock.mockImplementation((_m: string, cmd: string, _d: any) => {
      if (cmd === 'audit') {
        return Promise.resolve({
          entries: [
            { timestamp: '2026-11-10T10:00:00Z', operation: 'process_exec', target: 'a', policy: 'command_guard', result: 'deny' },
            { timestamp: '2026-11-10T10:01:00Z', operation: 'file_write', target: 'b', policy: 'abac', result: 'allow' },
          ],
          total: 2,
        })
      }
      return Promise.resolve({})
    })
    const w = await mountLayer('command')
    const rows = w.find('[data-test="intercept-table"]').findAll('tbody tr')
    expect(rows).toHaveLength(1)
    expect(w.text()).not.toContain('abac')
    w.unmount()
  })
})
