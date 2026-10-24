// QUIC 中继页组件测试（2026-10-21；2026-10-23 实时开关接线；2026-10-23
// 与集群解耦改版——删总开关卡①、删 cluster_gate 分支、加中结名册表，
// WSAPI 从 cluster.relay.* 挪到 quic.relay.*）。
// 后端行为由 crates/nemesis-web/src/handlers/quic/tests.rs 钉住
// （后端唯一真相源）；此处只测组件接线：
// - 降级面板（relay.status available:false → 诚实提示，无保存按钮）
// - 卡①渲染（运行中/待启动徽标、对外 relay URL、中继服务滑动开关）
// - 卡①头部「中继服务」开关 → quic.relay.enable/disable（实时生效，
//   免重启网关）
// - 保存只提交变更字段（端口改动单键 payload；全未变零调用）
// - token/relay_token 留空不改（真实值永不上前端）；显式清除发空串
// - 卡③名册表（quic.registry.members：成员行渲染 / 空态）
import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'

const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
  initWSAPI: vi.fn(),
}))

import QuicRelayTab from '../QuicRelayTab.vue'

function relayData(over: Record<string, any> = {}) {
  return {
    available: true,
    enabled: true,
    active: true,
    requires_restart: false,
    http_addr: '0.0.0.0:3340',
    quic_addr: '0.0.0.0:7842',
    relay: {
      enabled: true,
      http_port: 3340,
      quic_port: 7842,
      external_host: 'vps.example.com',
      tls: 'self-signed',
      token_set: true,
      cert_path: 'C:/ws/cluster/relay/relay_cert.pem',
      cert_state: 'generated',
      key_path: 'C:/ws/cluster/relay/relay_key.pem',
      key_state: 'generated',
      advertised_url: 'http://vps.example.com:3340',
    },
    client: {
      roaming_enabled: true,
      relay_url: 'http://vps.example.com:3340',
      relay_token_set: true,
    },
    ...over,
  }
}

function registryData(over: Record<string, any> = {}) {
  return {
    available: true,
    count: 2,
    members: [
      {
        node_id: 'node-laptop-abc',
        name: 'Laptop',
        role: 'worker',
        iroh_id: 'b'.repeat(64),
        build: 'aeb1221c',
        features: ['cluster'],
        last_seen_ms: 1761150000000,
      },
      {
        node_id: 'node-vps-01',
        name: 'VPS Relay',
        role: 'coordinator',
        iroh_id: 'c'.repeat(64),
        build: '',
        features: [],
        last_seen_ms: 0,
      },
    ],
    ...over,
  }
}

beforeEach(() => {
  vi.clearAllMocks()
  setActivePinia(createPinia())
  requestMock.mockImplementation(async (module: string, cmd: string) => {
    if (module === 'quic' && cmd === 'relay.status') return relayData()
    if (module === 'quic' && cmd === 'registry.members') return registryData()
    return {}
  })
})

function relayCalls(): any[] {
  return requestMock.mock.calls.filter(
    (c: any[]) => c[0] === 'quic' && c[1] === 'relay.config',
  )
}

describe('QuicRelayTab 降级面板', () => {
  it('relay.status available:false → 诚实提示 + 无保存按钮', async () => {
    requestMock.mockImplementation(async (_m: string, cmd: string) =>
      cmd === 'relay.status'
        ? {
            available: false,
            enabled: false,
            active: false,
            requires_restart: false,
            reason: 'QUIC 管理面未就绪（网关为裁剪构建）',
          }
        : {},
    )
    const w = mount(QuicRelayTab)
    await flushPromises()
    expect(w.text()).toContain('QUIC 中继（内网穿透）')
    expect(w.text()).toContain('QUIC 管理面未就绪')
    expect(w.findAll('button').filter(b => b.text() === '保存配置').length).toBe(0)
  })
})

describe('QuicRelayTab 卡渲染', () => {
  it('卡①：中继运行中徽标 + 对外 relay URL + 配置真相源提示（config.quic.json）', async () => {
    const w = mount(QuicRelayTab)
    await flushPromises()
    expect(w.find('.badge-success').exists()).toBe(true)
    expect(w.text()).toContain('中继运行中')
    expect(w.text()).toContain('http://vps.example.com:3340')
    expect(w.text()).toContain('config.quic.json')
  })

  it('与集群解耦：页面不再有跨网漫游开关/集群闸分支（指引文案除外）', async () => {
    // 解耦改版：总开关卡删除——roaming 开关归【集群 → 配置】跨网漫游卡
    // （页面保留一行指引文案指向它）；cluster_gate 分支删除（QUIC 不再
    // 依赖集群闸链）。
    const w = mount(QuicRelayTab)
    await flushPromises()
    // 开关本体只剩卡①「中继服务」一个（总开关已删）
    expect(w.findAll('.toggle-switch').length).toBe(1)
    expect(w.text()).not.toContain('启用跨网漫游')
    expect(w.text()).not.toContain('集群未启用')
    expect(w.text()).not.toContain('token 异常')
  })

  it('config 闸开而未跑 → 卡①待启动徽标（实时开关接线后语义）', async () => {
    requestMock.mockImplementation(async (m: string, cmd: string) => {
      if (m === 'quic' && cmd === 'relay.status')
        return relayData({ active: false, requires_restart: true })
      return {}
    })
    const w = mount(QuicRelayTab)
    await flushPromises()
    expect(w.find('.badge-warning').exists()).toBe(true)
    expect(w.text()).toContain('待启动')
    expect(w.text()).not.toContain('待重启')
  })

  it('中继启用但未设令牌（token_set=false）→ fail-closed 拒启提示', async () => {
    requestMock.mockImplementation(async (m: string, cmd: string) => {
      if (m === 'quic' && cmd === 'relay.status')
        return relayData({
          active: false,
          requires_restart: true,
          relay: {
            enabled: true,
            http_port: 3340,
            quic_port: 7842,
            external_host: '',
            tls: 'self-signed',
            token_set: false,
            cert_path: '',
            cert_state: 'none',
            key_path: '',
            key_state: 'none',
            advertised_url: '',
          },
        })
      return {}
    })
    const w = mount(QuicRelayTab)
    await flushPromises()
    expect(w.text()).toContain('待启动')
    expect(w.text()).toContain('未设访问令牌')
    expect(w.text()).toContain('fail-closed')
  })

  it('端口/主机表单按载荷播种（token 输入恒为空——真实值不上前端）', async () => {
    const w = mount(QuicRelayTab)
    await flushPromises()
    const numbers = w.findAll('input[type="number"]')
    expect(numbers.length).toBe(2)
    expect(String((numbers[0].element as HTMLInputElement).value)).toBe('3340')
    expect(String((numbers[1].element as HTMLInputElement).value)).toBe('7842')
    const passwords = w.findAll('input[type="password"]')
    expect(passwords.length).toBe(2)
    expect((passwords[0].element as HTMLInputElement).value).toBe('')
    expect((passwords[1].element as HTMLInputElement).value).toBe('')
    // 端口说明：HTTP=TCP / QUIC=UDP / 名册=TCP 3341 三条都在
    expect(w.text()).toContain('成员名册')
    expect(w.text()).toContain('3341')
  })

  it('滑动开关（toggle-switch）：卡①「中继服务」开关含 toggle-slider', async () => {
    const w = mount(QuicRelayTab)
    await flushPromises()
    expect(w.find('.toggle-switch').exists()).toBe(true)
    expect(w.find('.toggle-switch .toggle-slider').exists()).toBe(true)
    // 解耦后整页只剩卡①头部这一个开关
    expect(w.findAll('.toggle-switch').length).toBe(1)
  })
})

describe('QuicRelayTab 中结名册（quic.registry.members）', () => {
  it('成员表渲染：节点 ID/名称/角色/iroh 短形/构建/features/最近报到', async () => {
    const w = mount(QuicRelayTab)
    await flushPromises()
    expect(w.text()).toContain('中结名册')
    expect(w.text()).toContain('2 个成员')
    expect(w.text()).toContain('node-laptop-abc')
    expect(w.text()).toContain('Laptop')
    expect(w.text()).toContain('worker')
    // iroh 短形 = 前 12 位 + …；完整 hex 在 title 里
    expect(w.text()).toContain('b'.repeat(12) + '…')
    expect(w.text()).toContain('aeb1221c')
    expect(w.text()).toContain('cluster')
  })

  it('空名册 → 空态提示（中继未运行或暂无设备接入）', async () => {
    requestMock.mockImplementation(async (m: string, cmd: string) => {
      if (m === 'quic' && cmd === 'relay.status') return relayData()
      if (m === 'quic' && cmd === 'registry.members')
        return { available: true, count: 0, members: [] }
      return {}
    })
    const w = mount(QuicRelayTab)
    await flushPromises()
    expect(w.text()).toContain('暂无成员')
  })
})

describe('QuicRelayTab 交互', () => {
  it('卡①头部「中继服务」开关（全页唯一 checkbox）关闭 → quic.relay.disable（实时面，非 relay.config）', async () => {
    const w = mount(QuicRelayTab)
    await flushPromises()
    const boxes = w.findAll('input[type="checkbox"]')
    expect(boxes.length).toBe(1)
    await boxes[0].setValue(false)
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('quic', 'relay.disable')
    // 实时开关不走 relay.config（那是参数面）
    expect(relayCalls().length).toBe(0)
  })

  it('卡①头部「中继服务」开关开启（当前未启用）→ quic.relay.enable', async () => {
    requestMock.mockImplementation(async (m: string, cmd: string) => {
      if (m === 'quic' && cmd === 'relay.status')
        return relayData({ relay: { ...relayData().relay, enabled: false } })
      if (m === 'quic' && cmd === 'registry.members') return registryData()
      return {}
    })
    const w = mount(QuicRelayTab)
    await flushPromises()
    const boxes = w.findAll('input[type="checkbox"]')
    await boxes[0].setValue(true)
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('quic', 'relay.enable')
  })

  it('保存服务端：改端口只提交该键；token 留空不提交', async () => {
    const w = mount(QuicRelayTab)
    await flushPromises()
    await w.findAll('input[type="number"]')[0].setValue('4000')
    const save = w.findAll('button').filter(b => b.text() === '保存配置')[0]
    await save.trigger('click')
    await flushPromises()
    const calls = relayCalls()
    expect(calls.length).toBe(1)
    expect(calls[0][2]).toEqual({ http_port: 4000 })
  })

  it('两张卡全部未变 → 零 relay.config 调用', async () => {
    const w = mount(QuicRelayTab)
    await flushPromises()
    const saves = w.findAll('button').filter(b => b.text() === '保存配置')
    expect(saves.length).toBe(2)
    await saves[0].trigger('click')
    await saves[1].trigger('click')
    await flushPromises()
    expect(relayCalls().length).toBe(0)
  })

  it('token 输入新值 → 提交单键；清除按钮发 {token: ""}', async () => {
    const w = mount(QuicRelayTab)
    await flushPromises()
    await w.findAll('input[type="password"]')[0].setValue('new-token')
    const saves = w.findAll('button').filter(b => b.text() === '保存配置')
    await saves[0].trigger('click')
    await flushPromises()
    expect(relayCalls().length).toBe(1)
    expect(relayCalls()[0][2]).toEqual({ token: 'new-token' })

    // 清除按钮（服务端 + 客户端各一个，token_set 均为 true）
    const clears = w.findAll('button').filter(b => b.text() === '清除')
    expect(clears.length).toBe(2)
    await clears[0].trigger('click')
    await flushPromises()
    expect(relayCalls().length).toBe(2)
    expect(relayCalls()[1][2]).toEqual({ token: '' })
  })

  it('客户端：改 relay_url 只提交该键；relay_token 留空不提交', async () => {
    const w = mount(QuicRelayTab)
    await flushPromises()
    const urlInputs = w.findAll('input.form-input').filter(
      i => (i.element as HTMLInputElement).placeholder.includes('n0 公共中继'),
    )
    expect(urlInputs.length).toBe(1)
    await urlInputs[0].setValue('http://r2.example.com:3340')
    const saves = w.findAll('button').filter(b => b.text() === '保存配置')
    await saves[1].trigger('click')
    await flushPromises()
    const calls = relayCalls()
    expect(calls.length).toBe(1)
    expect(calls[0][2]).toEqual({ relay_url: 'http://r2.example.com:3340' })
  })

  it('客户端清除令牌 → {relay_token: ""}', async () => {
    const w = mount(QuicRelayTab)
    await flushPromises()
    const clears = w.findAll('button').filter(b => b.text() === '清除')
    await clears[1].trigger('click')
    await flushPromises()
    const calls = relayCalls()
    expect(calls.length).toBe(1)
    expect(calls[0][2]).toEqual({ relay_token: '' })
  })
})
