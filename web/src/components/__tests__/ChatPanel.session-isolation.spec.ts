import { describe, it, expect, vi, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount, flushPromises } from '@vue/test-utils'

// D-3（2026-09-23 多会话并行清账）：ChatPanel 会话状态按会话隔离。
//
// 钉住三件事：
// 1. busy 表按会话隔离——嵌入面板（sessionId prop）只见自己的 busy，
//    主聊天页 turn 的 busy 不再泄漏进嵌入视图（吞发送/假占位回归钉）；
// 2. 异会话 assistant 帧先落账再过滤——异会话 turn 的完成语义照常清算
//    （busy + B2 在飞登记），但内容不进当前视图；
// 3. 面板无会话锚（standalone/未选中）时保持 legacy 接受语义。

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

import { onMessage } from '../../composables/useWebSocket'
import ChatPanel from '../ChatPanel.vue'
import { useChatStore } from '../../stores/chat'
import { useSessionStore } from '../../stores/session'

function wsHandler(): (frame: any) => void {
  const calls = vi.mocked(onMessage).mock.calls
  expect(calls.length, 'onMessage must have been called on mount').toBeGreaterThan(0)
  return calls[calls.length - 1][0] as (frame: any) => void
}

beforeEach(() => {
  setActivePinia(createPinia())
  requestMock.mockReset()
  requestMock.mockResolvedValue({})
  vi.mocked(onMessage).mockClear()
})

async function mountEmbedded(sid: string) {
  const wrapper = mount(ChatPanel, { props: { sessionId: sid } })
  await flushPromises()
  return wrapper
}

describe('ChatPanel busy 按会话隔离 (D-3)', () => {
  it('异会话 busy 不泄漏：主聊天 busy=true 时嵌入面板输入可用', async () => {
    const sessionStore = useSessionStore()
    sessionStore.currentId = 'main-sess'
    const chat = useChatStore()

    const wrapper = await mountEmbedded('gen-1')
    chat.setBusy('main-sess', true)
    await flushPromises()

    expect(chat.isBusy('main-sess')).toBe(true)
    expect(chat.isBusy('gen-1')).toBe(false)
    expect(wrapper.find('textarea').attributes('disabled')).toBeUndefined()

    // 自身 busy → 输入禁用（inbox mock 无 queue → reject 语义）。
    chat.setBusy('gen-1', true)
    await flushPromises()
    expect(wrapper.find('textarea').attributes('disabled')).toBeDefined()

    wrapper.unmount()
  })

  it('异会话 assistant 帧先落账再过滤：busy 清算、内容不进视图', async () => {
    const sessionStore = useSessionStore()
    sessionStore.currentId = 'main-sess'
    const chat = useChatStore()
    chat.setBusy('main-sess', true)
    chat.markInflightTurn('main-sess', '正在做的事')

    const wrapper = await mountEmbedded('gen-1')
    const h = wsHandler()

    // 主聊天页的回复帧到达（嵌入面板在挂）→ 异会话帧：
    // busy + 在飞登记照常清算，内容不进本视图。
    h({
      type: 'message',
      module: 'chat',
      cmd: 'receive',
      data: { role: 'assistant', content: '主聊天页的回复', session_id: 'main-sess' },
    })
    await flushPromises()

    expect(chat.isBusy('main-sess')).toBe(false)
    expect(chat.inflightTurnOf('main-sess')).toBeUndefined()
    expect(chat.messages).toHaveLength(0)

    wrapper.unmount()
  })

  it('本会话 assistant 帧照常落地并清自身 busy', async () => {
    const sessionStore = useSessionStore()
    sessionStore.currentId = 'main-sess'
    const chat = useChatStore()
    chat.setBusy('gen-1', true)

    const wrapper = await mountEmbedded('gen-1')
    const h = wsHandler()

    h({
      type: 'message',
      module: 'chat',
      cmd: 'receive',
      data: { role: 'assistant', content: '生成会话的回复', session_id: 'gen-1' },
    })
    await flushPromises()

    expect(chat.isBusy('gen-1')).toBe(false)
    expect(chat.messages).toHaveLength(1)
    expect(chat.messages[0].content).toBe('生成会话的回复')

    wrapper.unmount()
  })

  it('prop 锚与全局选中解耦：currentId=null 不影响嵌入面板收发', async () => {
    const sessionStore = useSessionStore()
    sessionStore.currentId = null
    const chat = useChatStore()

    const wrapper = await mountEmbedded('gen-1')
    const h = wsHandler()
    h({
      type: 'message',
      module: 'chat',
      cmd: 'receive',
      data: { role: 'assistant', content: '生成回复', session_id: 'gen-1' },
    })
    await flushPromises()

    expect(chat.messages).toHaveLength(1)
    wrapper.unmount()
  })
})
