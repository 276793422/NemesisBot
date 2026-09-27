import { describe, it, expect, vi, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount, flushPromises } from '@vue/test-utils'

// 皮肤槽位：主页启动器（v2 结构引擎渲染）——空会话时 SkinSlot 挂载皮肤包
// 自带的 launcher 结构（品牌 + 场景标签，fill-input 动作预填）。CSS-only
// 包 / 无皮肤（slots 空）零渲染，默认观感零变化。
//
// 测试结构 = 最小 launcher 模板（与 skins/bot/skin/structure.html 同原语
// 语义），走 parseSkinStructure → engine.load → SkinSlot 挂载全链；真实
// 包-引擎契约由 src/skins/__tests__/bot-package.spec.ts 钉。

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
import { getSkinEngine, skinState } from '../../composables/useSkin'
import { parseSkinStructure } from '../../skins/sanitize'
import type { SkinProjection } from '../../skins/types'

const LAUNCHER_MIN =
  `<template data-nb-slot="launcher" data-nb-engine="1">` +
  `<div class="nb-launcher">` +
  `<div class="nb-launcher-brand"><span data-nb-bind="brand"></span></div>` +
  `<div class="nb-launcher-scenes">` +
  `<button data-nb-for="s in scenes" class="nb-scene-chip" data-nb-action="fill-input:{s}" data-nb-bind="s"></button>` +
  `</div></div></template>`

/** 装载测试 launcher 结构（真实 parse → load 链；structRev 驱动 SkinSlot 重挂）。
 * 投影同样直注（生产由 setupSkinProjection 装配，此处只供 bind/for 数据）。 */
function loadLauncherStructure(): void {
  const parsed = parseSkinStructure(LAUNCHER_MIN)
  expect(parsed).not.toBeNull()
  const engine = getSkinEngine()
  engine.load(parsed!.slots)
  engine.projection = {
    brand: 'Buddy',
    scenes: ['日常办公', '代码开发'],
  } as unknown as SkinProjection
  skinState.slots = [...parsed!.slots.keys()]
  skinState.structRev++
}

function resetStructure(): void {
  getSkinEngine().load(new Map())
  skinState.slots = []
  skinState.structRev++
  skinState.id = ''
  skinState.meta = null
}

beforeEach(() => {
  setActivePinia(createPinia())
  useSessionStore().currentId = 's1'
  requestMock.mockReset()
  requestMock.mockResolvedValue({})
  resetStructure()
})

async function mountPanel(props?: Record<string, unknown>) {
  const w = mount(ChatPanel, { props })
  await flushPromises()
  // mock 的 sendHistoryRequest 永不回落 → historyLoading 卡 true（生产里
  // 由历史加载完成置 false）；启动器在历史加载中不渲染是有意行为，这里
  // 直接落定以测「加载完成且会话为空」的启动器形态。
  useChatStore().historyLoading = false
  await flushPromises()
  return w
}

describe('ChatPanel 主页启动器（皮肤槽位）', () => {
  it('无皮肤 → 启动器不渲染，欢迎语照常（默认观感零变化）', async () => {
    const w = await mountPanel()
    expect(w.find('.nb-launcher').exists()).toBe(false)
    expect(w.find('.nb-launcher-mode').exists()).toBe(false)
    expect(w.text()).toContain('你好！我是 NemesisBot')
    w.unmount()
  })

  it('CSS-only 包（id 在场但无 structure）→ 回落原生布局（v2 语义）', async () => {
    skinState.id = 'csstheme'
    const w = await mountPanel()
    expect(w.find('.nb-launcher').exists()).toBe(false)
    expect(w.text()).toContain('你好！我是 NemesisBot')
    w.unmount()
  })

  it('结构皮肤 + 空会话 → 启动器渲染（品牌 + 场景标签），欢迎语退场', async () => {
    skinState.id = 'bot'
    skinState.meta = { brand: 'Buddy', scenes: ['日常办公', '代码开发'], version: '1.0.0' }
    loadLauncherStructure()
    const w = await mountPanel()
    expect(w.find('.nb-launcher').exists()).toBe(true)
    expect(w.find('.nb-launcher-brand').text()).toBe('Buddy')
    const chips = w.findAll('.nb-scene-chip')
    expect(chips.length).toBe(2)
    expect(chips[0].text()).toBe('日常办公')
    expect(w.text()).not.toContain('你好！我是 NemesisBot')
    w.unmount()
  })

  it('点击场景 chip → fill-input 动作预填「场景：」', async () => {
    skinState.id = 'bot'
    skinState.meta = { brand: 'Buddy', scenes: ['代码开发'], version: '1.0.0' }
    loadLauncherStructure()
    // fill-input 处理器在生产由 setupSkinProjection 装配（AppLayout setup
    // 调用，依赖 router 全家桶）；这里按同一语义直注，测「点击 → 动作 →
    // store 预填」链路本身。
    const chatStore = useChatStore()
    getSkinEngine().actionHandlers.set('fill-input', (arg) => {
      chatStore.input = arg ? `${arg}：` : ''
      chatStore.focusInputNonce++
    })
    const w = await mountPanel()
    await w.find('.nb-scene-chip').trigger('click')
    expect(chatStore.input).toBe('日常办公：')
    w.unmount()
  })

  it('非默认 chat 模块（workflow_chat）不渲染启动器', async () => {
    skinState.id = 'bot'
    loadLauncherStructure()
    const w = await mountPanel({ module: 'workflow_chat' })
    expect(w.find('.nb-launcher').exists()).toBe(false)
    w.unmount()
  })
})
