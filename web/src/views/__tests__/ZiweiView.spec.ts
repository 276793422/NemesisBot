import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'
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
