import { describe, it, expect, vi, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount, flushPromises } from '@vue/test-utils'

// L6++ G5 回归钉：项目会话的 tool_event 不串台。ChatPanel 的 push 帧
// 过滤按 `web:{currentId}`（ChatPanel.handleWSMessage）——当前活跃的是
// 项目会话 s1 时，其他会话（主会话/另一项目会话）的 tool_event 必须被
// 丢弃，只有 `web:s1` 的事件进入 chat store。事件链路本身零改动（按
// session_id 寻址），此 spec 钉住该不变量防未来回归。

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

function started(callId: string, chatId: string, tool = 'exec'): any {
  return {
    type: 'push',
    cmd: 'tool_event',
    data: { kind: 'ToolStarted', data: { chat_id: chatId, call_id: callId, tool, args_preview: '' } },
  }
}

beforeEach(() => {
  setActivePinia(createPinia())
  requestMock.mockReset()
  requestMock.mockResolvedValue({})
  vi.mocked(onMessage).mockClear()
})

describe('项目会话 tool_event 不串台 (L6++ G5)', () => {
  it('活跃=项目会话 s1：web:s1 事件应用；web:其他 会话事件丢弃', async () => {
    const sessionStore = useSessionStore()
    // s1 是项目会话（projectId 在场不改变过滤——过滤只认 session_id）。
    sessionStore.sessions = [
      { id: 's1', channel: 'web', startTime: '', lastTime: '', messageCount: 0, firstMessage: '项目会话', model: '', projectId: 'p1' },
      { id: 's2', channel: 'web', startTime: '', lastTime: '', messageCount: 0, firstMessage: '主会话', model: '' },
    ] as any
    sessionStore.currentId = 's1'

    const wrapper = mount(ChatPanel)
    await flushPromises()
    const chat = useChatStore()
    const h = wsHandler()

    // 另一会话（s2）的工具事件 → 丢弃。
    h(started('other-1', 'web:s2', 'read_file'))
    await flushPromises()
    expect(chat.pendingToolEvents).toHaveLength(0)

    // 非当前项目的任意会话 → 丢弃。
    h(started('other-2', 'web:s99', 'grep'))
    await flushPromises()
    expect(chat.pendingToolEvents).toHaveLength(0)

    // 当前项目会话（s1）的事件 → 应用。
    h(started('mine-1', 'web:s1', 'exec'))
    await flushPromises()
    expect(chat.pendingToolEvents).toHaveLength(1)
    expect(chat.pendingToolEvents[0].callId).toBe('mine-1')

    // 切到 s2 后：ChatPanel watch 触发 chatStore.reset()（pending 清空），
    // 且同一 chat_id=web:s1 的事件反转为丢弃（过滤跟 currentId 走）。
    sessionStore.currentId = 's2'
    await flushPromises()
    h(started('mine-2', 'web:s1', 'exec'))
    await flushPromises()
    expect(chat.pendingToolEvents).toHaveLength(0)

    wrapper.unmount()
  })
})

describe('receive 帧按会话过滤 (L6++：跨组切换不串台)', () => {
  function receiveFrame(sid: string | undefined, content: string): any {
    return {
      type: 'message',
      module: 'chat',
      cmd: 'receive',
      data: sid === undefined ? { role: 'assistant', content } : { role: 'assistant', content, session_id: sid },
    }
  }

  it('活跃=s1：s2 的回复丢弃；s1 的回复应用；无 session_id 帧保持接受（legacy）', async () => {
    const sessionStore = useSessionStore()
    sessionStore.sessions = [
      { id: 's1', channel: 'web', startTime: '', lastTime: '', messageCount: 0, firstMessage: 'A', model: '' },
      { id: 's2', channel: 'web', startTime: '', lastTime: '', messageCount: 0, firstMessage: 'B', model: '' },
    ] as any
    sessionStore.currentId = 's1'

    const wrapper = mount(ChatPanel)
    await flushPromises()
    const chat = useChatStore()
    const h = wsHandler()

    // 异会话（s2）晚到的回复 → 丢弃（后端已持久化，切换时从磁盘加载）。
    h(receiveFrame('s2', '晚到的 s2 回复'))
    await flushPromises()
    expect(chat.messages).toHaveLength(0)

    // 当前会话（s1）的回复 → 应用。
    h(receiveFrame('s1', 's1 的回复'))
    await flushPromises()
    expect(chat.messages).toHaveLength(1)
    expect(chat.messages[0].content).toBe('s1 的回复')

    // 无 session_id 的帧（旧路径/legacy 广播）→ 保持接受。
    h(receiveFrame(undefined, 'legacy 回复'))
    await flushPromises()
    expect(chat.messages).toHaveLength(2)

    wrapper.unmount()
  })

  it('无活跃会话（currentId=null）：带 session_id 的帧也接受（standalone 兼容）', async () => {
    const sessionStore = useSessionStore()
    sessionStore.sessions = [] as any
    sessionStore.currentId = null

    const wrapper = mount(ChatPanel)
    await flushPromises()
    const chat = useChatStore()
    const h = wsHandler()

    h(receiveFrame('s9', 'standalone 回复'))
    await flushPromises()
    expect(chat.messages).toHaveLength(1)

    wrapper.unmount()
  })
})
