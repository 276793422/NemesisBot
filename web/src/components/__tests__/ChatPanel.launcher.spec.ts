import { describe, it, expect, vi, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount, flushPromises } from '@vue/test-utils'

// 皮肤骨架槽位 3/3：主页启动器——空会话时品牌 + 场景标签（WB home 形态）。
// 皮肤未激活（skinState.id 空）时零渲染，默认观感零变化。

const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
}))

vi.mock('../../composables/useWebSocket', async () => {
  const { ref } = await import('vue')
  return {
    connect: vi.fn(),
    send: vi.fn(),
    sendHistoryRequest: vi.fn(),
    onMessage: vi.fn(),
    addMessageHandler: vi.fn(),
    removeMessageHandler: vi.fn(),
    wsStatus: ref('connected'),
    httpGet: vi.fn().mockResolvedValue({}),
  }
})

import ChatPanel from '../ChatPanel.vue'
import { useChatStore } from '../../stores/chat'
import { useSessionStore } from '../../stores/session'
import { skinState } from '../../composables/useSkin'

beforeEach(() => {
  setActivePinia(createPinia())
  useSessionStore().currentId = 's1'
  requestMock.mockReset()
  requestMock.mockResolvedValue({})
  skinState.id = ''
  skinState.meta = null
})

async function mountPanel() {
  const w = mount(ChatPanel)
  await flushPromises()
  // mock 的 sendHistoryRequest 永不回落 → historyLoading 卡 true（生产里
  // 由历史加载完成置 false）；启动器在历史加载中不渲染是有意行为，这里
  // 直接落定以测「加载完成且会话为空」的启动器形态。
  useChatStore().historyLoading = false
  await flushPromises()
  return w
}

describe('ChatPanel 主页启动器（皮肤骨架槽位）', () => {
  it('皮肤未激活 → 启动器不渲染，欢迎语照常（默认观感零变化）', async () => {
    const w = await mountPanel()
    expect(w.find('.nb-launcher').exists()).toBe(false)
    expect(w.find('.nb-launcher-mode').exists()).toBe(false)
    expect(w.text()).toContain('你好！我是 NemesisBot')
    w.unmount()
  })

  it('皮肤激活 + 空会话 → 启动器渲染（品牌 + 场景标签），欢迎语退场', async () => {
    skinState.id = 'openlikebuddy'
    skinState.meta = { brand: 'Buddy', scenes: ['日常办公', '代码开发'], version: '0.5.0' }
    const w = await mountPanel()
    expect(w.find('.nb-launcher').exists()).toBe(true)
    expect(w.find('.nb-launcher-brand').text()).toContain('Buddy')
    const chips = w.findAll('.nb-scene-chip')
    expect(chips.length).toBe(2)
    expect(chips[0].text()).toBe('日常办公')
    expect(w.text()).not.toContain('你好！我是 NemesisBot')
    w.unmount()
  })

  it('meta 未就绪 → 品牌回落 skinState.id，标签行不渲染', async () => {
    skinState.id = 'myskin'
    skinState.meta = null
    const w = await mountPanel()
    expect(w.find('.nb-launcher-brand').text()).toContain('myskin')
    expect(w.find('.nb-launcher-scenes').exists()).toBe(false)
    w.unmount()
  })

  it('点击场景标签 → 输入框预填「场景：」', async () => {
    skinState.id = 'openlikebuddy'
    skinState.meta = { brand: 'Buddy', scenes: ['代码开发'], version: '0.5.0' }
    const w = await mountPanel()
    await w.find('.nb-scene-chip').trigger('click')
    expect(useChatStore().input).toBe('代码开发：')
    w.unmount()
  })

  it('非默认 chat 模块（workflow_chat）不渲染启动器', async () => {
    skinState.id = 'openlikebuddy'
    skinState.meta = { brand: 'Buddy', scenes: ['日常办公'], version: '0.5.0' }
    const w = mount(ChatPanel, { props: { module: 'workflow_chat' } })
    await flushPromises()
    expect(w.find('.nb-launcher').exists()).toBe(false)
    w.unmount()
  })
})
