// 中继通道组件测试（2026-10-21）：QuicRelayTab 三卡接线。
// 后端行为由 crates/nemesis-web/src/handlers/cluster/tests.rs 钉住
// （后端唯一真相源）；此处只测组件接线：
// - 降级面板（relay.status available:false → 诚实提示，无保存按钮）
// - 三卡渲染（运行中/待重启徽标、iroh:<node_id>、对外 relay URL）
// - 总开关切换 → cluster.roaming.enable/disable
// - 保存只提交变更字段（端口改动单键 payload；全未变零调用）
// - token/relay_token 留空不改（真实值永不上前端）；显式清除发空串
import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'

const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
  initWSAPI: vi.fn(),
}))

import QuicRelayTab from '../QuicRelayTab.vue'

function roamingData(over: Record<string, any> = {}) {
  return {
    available: true,
    enabled: true,
    active: true,
    requires_restart: false,
    key_present: true,
    node_id: 'a'.repeat(64),
    trust: { enabled: true, trusted: 0, revoked: 0 },
    ...over,
  }
}

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

beforeEach(() => {
  vi.clearAllMocks()
  setActivePinia(createPinia())
  requestMock.mockImplementation(async (module: string, cmd: string) => {
    if (module === 'cluster' && cmd === 'roaming.status') return roamingData()
    if (module === 'cluster' && cmd === 'relay.status') return relayData()
    return {}
  })
})

function relayCalls(): any[] {
  return requestMock.mock.calls.filter(
    (c: any[]) => c[0] === 'cluster' && c[1] === 'relay.config',
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
            reason: 'roaming-relay feature 未编译（QUIC 中继层不可用）',
          }
        : {},
    )
    const w = mount(QuicRelayTab)
    await flushPromises()
    expect(w.text()).toContain('QUIC 中继（内网穿透）')
    expect(w.text()).toContain('roaming-relay feature 未编译')
    expect(w.findAll('button').filter(b => b.text() === '保存配置').length).toBe(0)
  })
})

describe('QuicRelayTab 三卡渲染', () => {
  it('总开关卡：运行中徽标 + iroh:<node_id> + 对外 relay URL', async () => {
    const w = mount(QuicRelayTab)
    await flushPromises()
    expect(w.find('.badge-success').exists()).toBe(true)
    expect(w.text()).toContain('运行中')
    expect(w.text()).toContain(`iroh:${'a'.repeat(64)}`)
    expect(w.text()).toContain('http://vps.example.com:3340')
  })

  it('config 闸开而未跑 → 待重启徽标（badge-warning）', async () => {
    requestMock.mockImplementation(async (m: string, cmd: string) => {
      if (m === 'cluster' && cmd === 'roaming.status')
        return roamingData({ active: false })
      if (m === 'cluster' && cmd === 'relay.status')
        return relayData({ active: false, requires_restart: true })
      return {}
    })
    const w = mount(QuicRelayTab)
    await flushPromises()
    expect(w.find('.badge-warning').exists()).toBe(true)
    expect(w.text()).toContain('待重启')
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
  })
})

describe('QuicRelayTab 交互', () => {
  it('总开关切换 → cluster.roaming.disable（卡①的 checkbox 是全页第一个）', async () => {
    const w = mount(QuicRelayTab)
    await flushPromises()
    const checkbox = w.find('input[type="checkbox"]')
    await checkbox.setValue(false)
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('cluster', 'roaming.disable')
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
