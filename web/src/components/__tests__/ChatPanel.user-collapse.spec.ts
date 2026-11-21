import { describe, it, expect, vi, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount, flushPromises } from '@vue/test-utils'

// 长用户消息折叠（2026-11-20 用户需求）：用户消息超阈值（600 字符或
// 18 行）默认折叠 + 「展开全文/收起」切换；短消息零渲染变化；
// assistant/system 消息不参与折叠。

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
  }
})

import ChatPanel from '../ChatPanel.vue'
import { useChatStore } from '../../stores/chat'
import { useSessionStore } from '../../stores/session'

beforeEach(() => {
  setActivePinia(createPinia())
  useSessionStore().currentId = 's1'
  requestMock.mockReset()
  requestMock.mockResolvedValue({})
})

function longText(chars: number, lines = 1): string {
  const line = '这是一段用于折叠测试的长文本内容。'.repeat(Math.ceil(chars / 17))
  if (lines <= 1) return line.slice(0, chars)
  return Array.from({ length: lines }, (_, i) => `第${i + 1}行：${'内容'.repeat(20)}`).join('\n')
}

describe('ChatPanel 长用户消息折叠', () => {
  it('超阈值用户消息默认折叠 + 有「展开全文」按钮', async () => {
    const store = useChatStore()
    store.messages.push({ role: 'user', content: longText(800), timestamp: 't1' } as any)
    const w = mount(ChatPanel)
    await flushPromises()
    expect(w.find('.message-text.user-collapsed').exists()).toBe(true)
    const btn = w.find('.msg-collapse-toggle')
    expect(btn.exists()).toBe(true)
    expect(btn.text()).toBe('展开全文')
    w.unmount()
  })

  it('点击「展开全文」→ 展开态 + 按钮变「收起」', async () => {
    const store = useChatStore()
    store.messages.push({ role: 'user', content: longText(800), timestamp: 't1' } as any)
    const w = mount(ChatPanel)
    await flushPromises()
    await w.find('.msg-collapse-toggle').trigger('click')
    await flushPromises()
    expect(w.find('.message-text.user-collapsed').exists()).toBe(false)
    expect(w.find('.msg-collapse-toggle').text()).toBe('收起')
    // 再点回收起
    await w.find('.msg-collapse-toggle').trigger('click')
    await flushPromises()
    expect(w.find('.message-text.user-collapsed').exists()).toBe(true)
    w.unmount()
  })

  it('多行超阈值（>18 行）折叠；短消息无按钮无截断类', async () => {
    const store = useChatStore()
    store.messages.push({ role: 'user', content: longText(0, 25), timestamp: 't1' } as any)
    store.messages.push({ role: 'user', content: '短消息', timestamp: 't2' } as any)
    const w = mount(ChatPanel)
    await flushPromises()
    const texts = w.findAll('.message-text')
    expect(texts[0].classes()).toContain('user-collapsed')
    expect(texts[1].classes()).not.toContain('user-collapsed')
    expect(w.findAll('.msg-collapse-toggle').length).toBe(1)
    w.unmount()
  })

  it('超长 assistant 消息不折叠（不在射程）', async () => {
    const store = useChatStore()
    store.messages.push({ role: 'assistant', content: longText(900), timestamp: 't1' } as any)
    const w = mount(ChatPanel)
    await flushPromises()
    expect(w.find('.message-text.user-collapsed').exists()).toBe(false)
    expect(w.find('.msg-collapse-toggle').exists()).toBe(false)
    w.unmount()
  })
})
