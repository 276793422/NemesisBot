// 中继通道组件测试（goal 批次三）。
// 后端行为由 crates/nemesis-web/src/relay/client_status_tests.rs 钉住
// （后端唯一真相源）；此处只测组件接线：
// - 空态渲染（服务端未配置 / 客户端未启用）
// - overview 有数据：设备表 + 开关态 + 客户端状态徽章
// - 服务端开关 POST /api/relay/enabled payload
// - 保存配置：只提交变更字段（遮蔽值原样跳过——不覆盖真实 token）
// - 手动重连 POST /api/relay/client/reconnect
import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'

const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
  initWSAPI: vi.fn(),
}))

const httpGetMock = vi.fn()
vi.mock('../../composables/useWebSocket', () => ({
  httpGet: (...args: any[]) => httpGetMock(...args),
}))

import RelayTab from '../RelayTab.vue'

const fetchMock = vi.fn()

function overviewData(over: Record<string, any> = {}) {
  return {
    server: {
      enabled: true,
      full_mode: true,
      devices: [
        {
          node_id: 'bridge-homepc',
          name: 'HomePC',
          version: '0.1.0',
          connected_at: 1700000000,
          bytes_up: 2048,
          bytes_down: 4096,
          online: true,
        },
      ],
    },
    client: {
      enabled: true,
      state: 'connected',
      relay_url: 'ws://vps:60600',
      node_id: 'bridge-testnode',
      last_error: null,
      updated_at: 1700000000,
    },
    ...over,
  }
}

beforeEach(() => {
  vi.clearAllMocks()
  vi.stubGlobal('fetch', fetchMock)
  fetchMock.mockResolvedValue(new Response(JSON.stringify({ ok: true }), { status: 200 }))
  // config.get 默认返回：bridge 段带遮蔽值（后端 sanitize 行为）
  requestMock.mockImplementation(async (module: string, cmd: string) => {
    if (module === 'config' && cmd === 'get') {
      return {
        bridge: {
          client: {
            enabled: true,
            relay_url: 'ws://vps:60600',
            token: 'abcd****1234',
            access_token: 'wxyz****5678',
          },
        },
      }
    }
    return {}
  })
  httpGetMock.mockResolvedValue(overviewData())
})

describe('RelayTab 空态', () => {
  it('服务端未配置显示提示；客户端未启用显示提示', async () => {
    httpGetMock.mockResolvedValue({ server: null, client: null })
    const w = mount(RelayTab)
    await flushPromises()
    expect(w.text()).toContain('本机未开启中继服务端')
    expect(w.text()).toContain('客户端未启用')
  })
})

describe('RelayTab 数据渲染', () => {
  it('设备表渲染桥入设备（名称/在线/流量）', async () => {
    const w = mount(RelayTab)
    await flushPromises()
    expect(w.text()).toContain('HomePC')
    expect(w.text()).toContain('bridge-homepc')
    expect(w.text()).toContain('在线')
    expect(w.text()).toContain('2.0 KB / 4.0 KB')
  })

  it('客户端状态徽章 + 节点 ID 展示', async () => {
    const w = mount(RelayTab)
    await flushPromises()
    expect(w.find('.badge-success').exists()).toBe(true)
    expect(w.text()).toContain('已连接')
    expect(w.text()).toContain('bridge-testnode')
  })

  it('被拒绝状态显示 badge-error + last_error', async () => {
    httpGetMock.mockResolvedValue(overviewData({
      client: {
        enabled: true, state: 'rejected', relay_url: 'ws://vps:60600',
        node_id: 'bridge-testnode', last_error: 'token 不匹配', updated_at: 1700000000,
      },
    }))
    const w = mount(RelayTab)
    await flushPromises()
    expect(w.find('.badge-error').exists()).toBe(true)
    expect(w.text()).toContain('被拒绝')
    expect(w.text()).toContain('token 不匹配')
  })
})

describe('RelayTab 交互', () => {
  it('服务端开关切换 POST /api/relay/enabled {on:false}', async () => {
    const w = mount(RelayTab)
    await flushPromises()
    const checkbox = w.find('input[type="checkbox"]')
    expect(checkbox.exists()).toBe(true)
    await checkbox.setValue(false) // 触发 change，checked=false
    await flushPromises()
    expect(fetchMock).toHaveBeenCalledWith(
      expect.stringContaining('/api/relay/enabled'),
      expect.objectContaining({
        method: 'POST',
        body: JSON.stringify({ on: false }),
      }),
    )
  })

  it('手动重连按钮 POST /api/relay/client/reconnect', async () => {
    const w = mount(RelayTab)
    await flushPromises()
    const btn = w.findAll('button').find(b => b.text() === '手动重连')
    expect(btn).toBeTruthy()
    await btn!.trigger('click')
    await flushPromises()
    expect(fetchMock).toHaveBeenCalledWith(
      expect.stringContaining('/api/relay/client/reconnect'),
      expect.objectContaining({ method: 'POST' }),
    )
  })

  it('保存配置：改动 relay_url → set_field 只提交该字段（遮蔽 token 未变不提交）', async () => {
    const w = mount(RelayTab)
    await flushPromises()
    const inputs = w.findAll('input.form-input')
    expect(inputs.length).toBe(3)
    await inputs[0].setValue('ws://new-relay:60600')
    const save = w.findAll('button').find(b => b.text() === '保存配置')
    await save!.trigger('click')
    await flushPromises()
    const setCalls = requestMock.mock.calls.filter(
      (c: any[]) => c[0] === 'config' && c[1] === 'set_field',
    )
    expect(setCalls.length).toBe(1)
    expect(setCalls[0][2]).toEqual({
      path: 'bridge.client.relay_url',
      value: 'ws://new-relay:60600',
    })
  })

  it('保存配置：全部未变 → 零 set_field 调用', async () => {
    const w = mount(RelayTab)
    await flushPromises()
    const save = w.findAll('button').find(b => b.text() === '保存配置')
    await save!.trigger('click')
    await flushPromises()
    const setCalls = requestMock.mock.calls.filter(
      (c: any[]) => c[0] === 'config' && c[1] === 'set_field',
    )
    expect(setCalls.length).toBe(0)
  })
})
