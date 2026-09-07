import { describe, it, expect, vi, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount, flushPromises } from '@vue/test-utils'

// M1b（2026-09-05）：ChatPanel 工具卡片端到端——push 帧（M1a tool_event
// 通道）→ chat store 收集（callId 去重）→ assistant 响应落地 flush 挂载 →
// 卡片组渲染；>=3 个默认折叠为「已运行 N 个工具」计数条，点击展开。

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

/** Grab the handler ChatPanel registered via onMessage(). */
function wsHandler(): (frame: any) => void {
  const calls = vi.mocked(onMessage).mock.calls
  expect(calls.length, 'onMessage must have been called on mount').toBeGreaterThan(0)
  return calls[calls.length - 1][0] as (frame: any) => void
}

function started(callId: string, tool = 'exec'): any {
  return {
    type: 'push',
    cmd: 'tool_event',
    data: { kind: 'ToolStarted', data: { chat_id: 'web:s1', call_id: callId, tool, args_preview: `{"k":"${callId}"}` } },
  }
}

function finished(callId: string, tool = 'exec', ok = true): any {
  return {
    type: 'push',
    cmd: 'tool_event',
    data: {
      kind: 'ToolFinished',
      data: { chat_id: 'web:s1', call_id: callId, tool, duration_ms: 1500, ok, result_preview: ok ? 'done' : 'Tool error: x' },
    },
  }
}

async function mountPanel() {
  const wrapper = mount(ChatPanel)
  await flushPromises()
  return wrapper
}

beforeEach(() => {
  setActivePinia(createPinia())
  requestMock.mockReset()
  // inbox_status / todo_get 等 WSAPI 请求默认空响应。
  requestMock.mockResolvedValue({})
  vi.mocked(onMessage).mockClear()
})

describe('ChatPanel M1b 工具卡片', () => {
  it('push 事件收集：Started→Finished 同 callId 原地合并，pending 渲染卡片', async () => {
    const wrapper = await mountPanel()
    const chat = useChatStore()
    const h = wsHandler()

    h(started('c1'))
    await flushPromises()
    h(started('c2', 'read_file'))
    await flushPromises()
    h(finished('c1'))
    await flushPromises()

    expect(chat.pendingToolEvents).toHaveLength(2)
    expect(chat.pendingToolEvents[0].state).toBe('ok')
    expect(chat.pendingToolEvents[0].durationMs).toBe(1500)
    expect(chat.pendingToolEvents[1].state).toBe('running')

    // pending 区（streaming 未开 → typing 指示条不渲染，但事件也不渲染；
    // pending 卡片挂在 typing 指示条内部）。手动开 streaming 验证渲染。
    chat.streaming = true
    await flushPromises()
    const cards = wrapper.findAll('.tool-card')
    expect(cards.length).toBe(2)
    expect(cards[0].classes()).toContain('is-ok')
    expect(cards[1].find('.tool-spinner').exists()).toBe(true)
    wrapper.unmount()
  })

  it('assistant 响应落地：pending flush 挂载到消息 toolEvents，缓冲清空', async () => {
    const wrapper = await mountPanel()
    const chat = useChatStore()
    const h = wsHandler()

    h(started('c1', 'grep'))
    h(finished('c1', 'grep'))
    h({
      type: 'message',
      module: 'chat',
      cmd: 'receive',
      data: { role: 'assistant', content: '完成回复' },
      timestamp: 't1',
    })
    await flushPromises()

    expect(chat.pendingToolEvents).toEqual([])
    expect(chat.messages).toHaveLength(1)
    expect(chat.messages[0].toolEvents).toHaveLength(1)
    expect(chat.messages[0].toolEvents![0].tool).toBe('grep')

    // 消息上方渲染卡片（<3 个不折叠）。
    const cards = wrapper.findAll('.tool-card')
    expect(cards.length).toBe(1)
    expect(cards[0].classes()).toContain('is-ok')
    wrapper.unmount()
  })

  it('重复推送幂等：同 callId 重复 Finished 不产生重复卡片', async () => {
    const wrapper = await mountPanel()
    const chat = useChatStore()
    const h = wsHandler()

    h(started('c1'))
    h(finished('c1'))
    h(finished('c1'))
    await flushPromises()

    expect(chat.pendingToolEvents).toHaveLength(1)
    wrapper.unmount()
  })

  it('非当前会话 chat_id 的事件被过滤（currentId 活跃时精确匹配）', async () => {
    const { useSessionStore } = await import('../../stores/session')
    useSessionStore().currentId = 's1'
    const wrapper = await mountPanel()
    const chat = useChatStore()
    const h = wsHandler()

    h({
      type: 'push',
      cmd: 'tool_event',
      data: { kind: 'ToolStarted', data: { chat_id: 'web:other-session', call_id: 'c9', tool: 'exec' } },
    })
    h(started('c1'))
    await flushPromises()

    expect(chat.pendingToolEvents).toHaveLength(1)
    expect(chat.pendingToolEvents[0].callId).toBe('c1')
    wrapper.unmount()
  })

  it('>=3 个事件默认折叠为计数条，点击展开', async () => {
    const wrapper = await mountPanel()
    const chat = useChatStore()
    const h = wsHandler()

    for (const id of ['c1', 'c2', 'c3']) {
      h(started(id))
      h(finished(id, 'exec', true))
    }
    h({
      type: 'message',
      module: 'chat',
      cmd: 'receive',
      data: { role: 'assistant', content: '三轮工具完成' },
      timestamp: 't1',
    })
    await flushPromises()

    // 默认折叠：计数条在，卡片不在。
    const toggle = wrapper.find('.tool-group-toggle')
    expect(toggle.exists()).toBe(true)
    expect(toggle.text()).toContain('已运行 3 个工具')
    expect(wrapper.findAll('.tool-card').length).toBe(0)

    // 点击展开。
    await toggle.trigger('click')
    await flushPromises()
    expect(wrapper.findAll('.tool-card').length).toBe(3)
    wrapper.unmount()
  })

  it('watchdog 重同步（replaceMessages）清 pending，旧事件不误挂下一轮', async () => {
    const wrapper = await mountPanel()
    const chat = useChatStore()
    const h = wsHandler()

    h(started('c1'))
    chat.replaceMessages([{ role: 'assistant', content: 'recovered', timestamp: 't0' }])
    h({
      type: 'message',
      module: 'chat',
      cmd: 'receive',
      data: { role: 'assistant', content: '下一轮回复' },
      timestamp: 't1',
    })
    await flushPromises()

    expect(chat.messages).toHaveLength(2)
    expect(chat.messages[1].toolEvents).toBeUndefined()
    wrapper.unmount()
  })
})
