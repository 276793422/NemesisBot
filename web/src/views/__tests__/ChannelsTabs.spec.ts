// 通道页 TAB 结构测试（2026-10-21）：第 4 tab 改名【桥接通道】+ 新增
// 第 5 tab【中继通道】（QUIC）。位次与标签钉死，防止后续增删 TAB 漂移。
// - 5 个 TAB 顺序：本地/云端/语音/桥接/中继
// - 桥接通道渲染 BridgeTab（中继服务端卡头）
// - 中继通道渲染 QuicRelayTab 占位面板（QUIC 中继说明）
import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'

const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
  initWSAPI: vi.fn(),
}))

const httpGetMock = vi.fn()
vi.mock('../../composables/useWebSocket', () => ({
  httpGet: (...args: any[]) => httpGetMock(...args),
}))

import ChannelsView from '../ChannelsView.vue'

beforeEach(() => {
  vi.clearAllMocks()
  setActivePinia(createPinia())
  vi.stubGlobal('fetch', vi.fn().mockResolvedValue(
    new Response(JSON.stringify({ ok: true }), { status: 200 }),
  ))
  requestMock.mockResolvedValue({})
  httpGetMock.mockResolvedValue({ server: null, client: null })
})

describe('ChannelsView TAB 结构', () => {
  it('5 个 TAB 按序渲染：本地/云端/语音/桥接/中继', async () => {
    const w = mount(ChannelsView)
    await flushPromises() // TAB 条在 v-if="!loading" 内，等 loadChannels 翻转
    const tabs = w.findAll('.ch-tab').map(b => b.text())
    expect(tabs).toEqual(['本地通道', '云端通道', '语音通道', '桥接通道', '中继通道'])
  })

  it('桥接通道 tab 渲染 BridgeTab（中继服务端卡头）', async () => {
    const w = mount(ChannelsView)
    await flushPromises()
    const bridgeTab = w.findAll('.ch-tab').find(b => b.text() === '桥接通道')
    await bridgeTab!.trigger('click')
    await flushPromises()
    expect(w.text()).toContain('中继服务端')
  })

  it('中继通道 tab 渲染 QUIC 占位面板', async () => {
    const w = mount(ChannelsView)
    await flushPromises()
    const relayTab = w.findAll('.ch-tab').find(b => b.text() === '中继通道')
    await relayTab!.trigger('click')
    await flushPromises()
    expect(w.text()).toContain('QUIC 中继（内网穿透）')
  })
})
