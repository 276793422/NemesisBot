import { mount } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'

// ClusterRoaming（iroh 跨网漫游 TAB，开关+UI 接入）：
// - TAB 清单：'roaming'（跨网）存在且紧随 topology（ClusterTabs）；
// - feature 未编译（available:false）→ 诚实降级面板，无开关可点；
// - 正常态：运行中徽标 + iroh:<hex> 身份展示 + 信任簿计数；
// - 开关点击 → roaming.enable / roaming.disable WSAPI 调用 + 重启提示 toast。

const wsapiRequest = vi.fn()
const toastFns = {
  success: vi.fn(),
  error: vi.fn(),
  warn: vi.fn(),
  info: vi.fn(),
}

vi.mock('../../composables/useWSAPI', () => ({
  initWSAPI: vi.fn(),
  useWSAPI: () => ({ request: wsapiRequest }),
}))

vi.mock('../../composables/useToast', () => ({
  useToast: () => toastFns,
}))

import ClusterTabs from '../cluster/ClusterTabs.vue'
import ClusterRoaming from '../cluster/ClusterRoaming.vue'

beforeEach(() => {
  wsapiRequest.mockReset().mockResolvedValue({})
  for (const f of Object.values(toastFns)) f.mockReset()
})

describe('ClusterTabs', () => {
  it('含跨网 TAB 且紧随拓扑', () => {
    const w = mount(ClusterTabs, { props: { modelValue: 'overview' } })
    // 位置断言：跨网在拓扑之后、身份之前
    const tabIds = w
      .findAll('.tab')
      .map((t) => t.classes().includes('active'))
    expect(tabIds[0]).toBe(true) // overview 默认 active
    const texts = w.findAll('.tab').map((t) => t.text())
    expect(texts.indexOf('拓扑')).toBeLessThan(texts.indexOf('跨网'))
    expect(texts.indexOf('跨网')).toBeLessThan(texts.indexOf('身份'))
    w.unmount()
  })
})

describe('ClusterRoaming', () => {
  it('feature 未编译时渲染诚实降级面板', async () => {
    wsapiRequest.mockResolvedValue({
      available: false,
      enabled: false,
      active: false,
      reason: 'cluster-roaming feature 未编译（iroh 跨网漫游层不可用）',
    })
    const w = mount(ClusterRoaming)
    await vi.waitFor(() => expect(w.text()).toContain('未编译'))
    expect(w.text()).toContain('cluster-roaming feature 关闭')
    // 降级面板无 toggle
    expect(w.find('.toggle').exists()).toBe(false)
    w.unmount()
  })

  it('正常态展示运行中徽标、iroh 身份与信任簿计数', async () => {
    wsapiRequest.mockResolvedValue({
      available: true,
      enabled: true,
      active: true,
      requires_restart: false,
      key_present: true,
      node_id: 'ab'.repeat(32),
      trust: { enabled: true, trusted: 2, revoked: 1 },
    })
    const w = mount(ClusterRoaming)
    await vi.waitFor(() => expect(w.text()).toContain('运行中'))
    expect(w.text()).toContain('iroh:' + 'ab'.repeat(32))
    expect(w.text()).toContain('放行 2 个 / 吊销 1 个')
    expect(w.find('.roam-badge.pass').exists()).toBe(true)
    // 待重启提示不出现
    expect(w.find('.roam-restart-hint').exists()).toBe(false)
    w.unmount()
  })

  it('enabled 未生效（待重启）时显示重启提示', async () => {
    wsapiRequest.mockResolvedValue({
      available: true,
      enabled: true,
      active: false,
      requires_restart: true,
      key_present: true,
      node_id: null,
      trust: { enabled: false, trusted: 0, revoked: 0 },
    })
    const w = mount(ClusterRoaming)
    await vi.waitFor(() => expect(w.text()).toContain('待重启生效'))
    expect(w.find('.roam-restart-hint').exists()).toBe(true)
    w.unmount()
  })

  it('开关点击调用 roaming.disable 并提示重启', async () => {
    wsapiRequest.mockImplementation((_mod: string, cmd: string) => {
      if (cmd === 'roaming.status') {
        return Promise.resolve({
          available: true,
          enabled: true,
          active: true,
          requires_restart: false,
          key_present: true,
          node_id: 'cd'.repeat(32),
          trust: { enabled: false, trusted: 0, revoked: 0 },
        })
      }
      return Promise.resolve({ enabled: false, requires_restart: true })
    })
    const w = mount(ClusterRoaming)
    await vi.waitFor(() => expect(w.find('.toggle').exists()).toBe(true))
    await w.find('.toggle').trigger('click')
    await vi.waitFor(() =>
      expect(wsapiRequest).toHaveBeenCalledWith('cluster', 'roaming.disable'),
    )
    await vi.waitFor(() =>
      expect(toastFns.success).toHaveBeenCalledWith(
        expect.stringContaining('重启网关后生效'),
      ),
    )
    // disable 后 status 重拉
    expect(wsapiRequest).toHaveBeenCalledWith('cluster', 'roaming.status')
    w.unmount()
  })
})
