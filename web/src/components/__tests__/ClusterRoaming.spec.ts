import { mount } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'

// ClusterRoaming（跨网漫游卡片，2026-10-23 自独立 TAB 收编进【集群设置】
// 右列顶；QUIC 解耦后开关+观测归集群配置页）：
// - TAB 清单：'roaming'（跨网）已删除（ClusterTabs）；
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
  it('跨网 TAB 已删除（卡片收编进配置页）且不含残留项', () => {
    const w = mount(ClusterTabs, { props: { modelValue: 'overview' } })
    const texts = w.findAll('.tab').map((t) => t.text())
    expect(texts).not.toContain('跨网')
    // 相邻项完好：拓扑之后直接是身份
    expect(texts.indexOf('拓扑')).toBeLessThan(texts.indexOf('身份'))
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
    // 卡片头徽标在 loading 期就渲染「未编译」——必须等降级正文出现，
    // 不能拿徽标当就绪信号（竞态：waitFor 过早放行）
    await vi.waitFor(() =>
      expect(w.text()).toContain('cluster-roaming feature 关闭'),
    )
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
    // rendezvous 段缺席（Null）→ 名册同步块不渲染（诚实降级）
    expect(w.text()).not.toContain('名册同步（自动发现）')
    w.unmount()
  })

  it('rendezvous 名册状态渲染：成员数/注册表地址/同步失败错误', async () => {
    wsapiRequest.mockResolvedValue({
      available: true,
      enabled: true,
      active: true,
      requires_restart: false,
      key_present: true,
      node_id: 'ab'.repeat(32),
      trust: { enabled: false, trusted: 0, revoked: 0 },
      rendezvous: {
        registry_url: 'http://vps.example.com:3341',
        last_sync_ok: true,
        last_sync_ms: 1760000000000,
        member_count: 4,
        last_error: '',
      },
    })
    const w = mount(ClusterRoaming)
    await vi.waitFor(() => expect(w.text()).toContain('名册同步（自动发现）'))
    expect(w.text()).toContain('名册 4 个成员')
    expect(w.text()).toContain('http://vps.example.com:3341')
    expect(w.text()).toContain('已同步')
    // 失败错误行不渲染
    expect(w.text()).not.toContain('限频重试中')
    w.unmount()
  })

  it('rendezvous 同步失败：warn 徽标 + last_error 提示行', async () => {
    wsapiRequest.mockResolvedValue({
      available: true,
      enabled: true,
      active: true,
      requires_restart: false,
      key_present: true,
      node_id: 'ab'.repeat(32),
      trust: { enabled: false, trusted: 0, revoked: 0 },
      rendezvous: {
        registry_url: 'http://vps.example.com:3341',
        last_sync_ok: false,
        last_sync_ms: 1760000000000,
        member_count: 0,
        last_error: 'register 被拒（HTTP 401）',
      },
    })
    const w = mount(ClusterRoaming)
    await vi.waitFor(() => expect(w.text()).toContain('同步失败'))
    expect(w.find('.roam-badge.warn').exists()).toBe(true)
    expect(w.text()).toContain('register 被拒（HTTP 401）')
    expect(w.text()).toContain('限频重试中')
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

  it('集群闸未开（cluster_gate.open=false）时徽标说集群未启用而非待重启', async () => {
    // 2026-10-22 用户实录：只开 roaming.enabled 而集群本体闸未开 → 重启
    // 永远不生效，横幅必须给定向指引而不是「重启网关后生效」无限循环。
    wsapiRequest.mockResolvedValue({
      available: true,
      enabled: true,
      active: false,
      requires_restart: true,
      key_present: true,
      node_id: null,
      trust: { enabled: false, trusted: 0, revoked: 0 },
      cluster_gate: { main_enabled: false, cluster_enabled: true, open: false, token_broken: false },
    })
    const w = mount(ClusterRoaming)
    await vi.waitFor(() => expect(w.text()).toContain('集群未启用'))
    // 徽标不再是「待重启生效」；横幅给闸链指引
    expect(w.text()).not.toContain('待重启生效')
    expect(w.find('.roam-restart-hint').exists()).toBe(true)
    expect(w.find('.roam-restart-hint').text()).toContain('集群本体未启用')
    expect(w.find('.roam-restart-hint').text()).not.toContain('重启网关后生效')
    w.unmount()
  })

  it('token 异常（cluster_gate.token_broken）时徽标与横幅按 token 定性', async () => {
    wsapiRequest.mockResolvedValue({
      available: true,
      enabled: true,
      active: false,
      requires_restart: true,
      key_present: true,
      node_id: null,
      trust: { enabled: false, trusted: 0, revoked: 0 },
      cluster_gate: { main_enabled: true, cluster_enabled: true, open: false, token_broken: true },
    })
    const w = mount(ClusterRoaming)
    await vi.waitFor(() => expect(w.text()).toContain('token 异常'))
    expect(w.find('.roam-restart-hint').text()).toContain('token')
    expect(w.find('.roam-restart-hint').text()).not.toContain('重启网关后生效')
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
