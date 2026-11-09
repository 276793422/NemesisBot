import { describe, it, expect, vi, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount, flushPromises } from '@vue/test-utils'

// 2026-11-09 滚动/焦点/选择批：ChatPanel 组件级行为测试——
// 消息区点击不抢焦点（拖选可正常复制）、键入路由（焦点不在可编辑元素时
// 打印字符落入输入框）、滚动跟随浮钮（wheel 上滚出现 / 追加消息计未读 /
// 点击回底清零 / 历史前插不计未读）。

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

async function mountPanel(msgs: Array<{ role: string; content: string }>) {
  const store = useChatStore()
  for (const m of msgs) {
    store.messages.push({ role: m.role, content: m.content, timestamp: 't1' } as any)
  }
  // attachTo：焦点断言需要组件真实进文档（test-utils 默认挂到脱离
  // document 的 wrapper，jsdom 对不在文档中的元素 focus() 是 no-op）。
  const w = mount(ChatPanel, { attachTo: document.body })
  await flushPromises()
  return w
}

// jsdom 无布局：offsetParent 恒 null（=不可见）。键入路由的「多面板共存
// 只响应可见实例」守卫在浏览器里正确，在 jsdom 里需按可见实例桩掉。
function markVisible(w: ReturnType<typeof mount>) {
  const el = w.find('.chat-messages').element as HTMLElement
  Object.defineProperty(el, 'offsetParent', { value: document.body, configurable: true })
}

describe('ChatPanel 焦点/键入路由（2026-11-09）', () => {
  it('消息区点击不抢焦点——拖选结束松开鼠标后选区不被输入框打掉', async () => {
    const w = await mountPanel([{ role: 'assistant', content: '可被拖选的回复正文' }])
    await w.find('.chat-messages').trigger('click')
    expect(document.activeElement).not.toBe(w.find('textarea').element)
    expect(useChatStore().input).toBe('')
    w.unmount()
  })

  it('键入路由：焦点不在可编辑元素时，可打印字符落入输入框并聚焦', async () => {
    const w = await mountPanel([{ role: 'assistant', content: '回复' }])
    markVisible(w)
    document.dispatchEvent(new KeyboardEvent('keydown', { key: 'h' }))
    expect(useChatStore().input).toBe('h')
    expect(document.activeElement).toBe(w.find('textarea').element)
    w.unmount()
  })

  it('修饰键组合（Ctrl+A 全选）不路由', async () => {
    const w = await mountPanel([{ role: 'assistant', content: '回复' }])
    markVisible(w)
    document.dispatchEvent(new KeyboardEvent('keydown', { key: 'a', ctrlKey: true }))
    expect(useChatStore().input).toBe('')
    w.unmount()
  })

  it('焦点已在输入框时不二次路由（textarea 上的键入走原生路径）', async () => {
    const w = await mountPanel([{ role: 'assistant', content: '回复' }])
    markVisible(w)
    const ta = w.find('textarea').element as HTMLTextAreaElement
    ta.focus()
    ta.dispatchEvent(new KeyboardEvent('keydown', { key: 'x', bubbles: true }))
    expect(useChatStore().input).toBe('')
    w.unmount()
  })

  it('按钮上的空格是激活语义，不劫持', async () => {
    const w = await mountPanel([{ role: 'assistant', content: '回复' }])
    markVisible(w)
    const btn = document.createElement('button')
    btn.textContent = '占位按钮'
    document.body.appendChild(btn)
    try {
      btn.dispatchEvent(new KeyboardEvent('keydown', { key: ' ', bubbles: true }))
      expect(useChatStore().input).toBe('')
    } finally {
      btn.remove()
    }
    w.unmount()
  })

  it('IME 首击（key=Process）在非编辑元素上：焦点搬进输入框、不代插字符', async () => {
    // 中文输入法用户的键入路由正道：组合首击时 key='Process'（组合尚未
    // 开始），同步 focus 让随后的 compositionstart 落在输入框。
    const w = await mountPanel([{ role: 'assistant', content: '回复' }])
    markVisible(w)
    document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Process' }))
    expect(useChatStore().input).toBe('')
    expect(document.activeElement).toBe(w.find('textarea').element)
    w.unmount()
  })

  it('组合中（isComposing）在非编辑元素上：焦点搬进输入框、内容仍归 IME', async () => {
    const w = await mountPanel([{ role: 'assistant', content: '回复' }])
    markVisible(w)
    document.dispatchEvent(new KeyboardEvent('keydown', { key: 'n', isComposing: true }))
    expect(useChatStore().input).toBe('')
    expect(document.activeElement).toBe(w.find('textarea').element)
    w.unmount()
  })
})

describe('ChatPanel 滚动跟随浮钮（2026-11-09）', () => {
  it('跟随态不显示浮钮；wheel 上滚即时放弃跟随并显示', async () => {
    const w = await mountPanel([
      { role: 'user', content: '问' },
      { role: 'assistant', content: '答' },
    ])
    expect(w.find('.back-to-bottom-wrap').exists()).toBe(false)
    await w.find('.chat-messages').trigger('wheel', { deltaY: -100 })
    expect(w.find('.back-to-bottom-wrap').exists()).toBe(true)
    expect(w.find('.btt-unread').exists()).toBe(false) // 无新消息时无徽标
    w.unmount()
  })

  it('离底期间追加消息 → 未读徽标 +1', async () => {
    const store = useChatStore()
    const w = await mountPanel([
      { role: 'user', content: '问' },
      { role: 'assistant', content: '答' },
    ])
    await w.find('.chat-messages').trigger('wheel', { deltaY: -100 })
    store.messages.push({ role: 'assistant', content: '迟到的回复', timestamp: 't2' } as any)
    await flushPromises()
    expect(w.find('.btt-unread').text()).toBe('1')
    w.unmount()
  })

  it('点击浮钮 → 回底、徽标清零、浮钮消失', async () => {
    const store = useChatStore()
    const w = await mountPanel([
      { role: 'user', content: '问' },
      { role: 'assistant', content: '答' },
    ])
    await w.find('.chat-messages').trigger('wheel', { deltaY: -100 })
    store.messages.push({ role: 'assistant', content: '新回复', timestamp: 't2' } as any)
    await flushPromises()
    expect(w.find('.btt-unread').exists()).toBe(true)
    await w.find('.back-to-bottom').trigger('click')
    expect(w.find('.back-to-bottom-wrap').exists()).toBe(false)
    w.unmount()
  })

  it('历史前插（首元素 identity 变化）不计未读——装载旧消息不是「新消息」', async () => {
    const store = useChatStore()
    const w = await mountPanel([
      { role: 'user', content: '问' },
      { role: 'assistant', content: '答' },
    ])
    await w.find('.chat-messages').trigger('wheel', { deltaY: -100 })
    store.prependHistory([{ role: 'user', content: '更早的旧消息', timestamp: 't0' } as any], 0)
    await flushPromises()
    expect(store.messages.length).toBe(3)
    expect(w.find('.btt-unread').exists()).toBe(false)
    expect(w.find('.back-to-bottom-wrap').exists()).toBe(true) // 仍离底
    w.unmount()
  })
})
