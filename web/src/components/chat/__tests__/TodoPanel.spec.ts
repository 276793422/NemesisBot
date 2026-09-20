import { describe, it, expect, vi, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount, flushPromises } from '@vue/test-utils'
import { ref } from 'vue'

// TodoPanel（2026-09-20 BUG-A 回归）：TodoUpdated push 帧按当前会话过滤。
// 帧内层 chat_id 是连接级 id（`web:{连接id}`），与会话 id 不同域——旧过滤
// 恒不等 → TodoUpdated 全部被丢弃 → 清单永不出现。修复：优先按 pump 注入
// 的 `session_id`（会话 id，session_key 末段）过滤；旧帧无该字段时回退
// chat_id 前缀匹配兜底。

const requestMock = vi.fn()
vi.mock('../../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
}))

vi.mock('../../../composables/useWebSocket', async () => {
  const vue = await import('vue')
  return {
    connect: vi.fn(),
    send: vi.fn(),
    sendHistoryRequest: vi.fn(),
    onMessage: vi.fn(),
    addMessageHandler: vi.fn(),
    removeMessageHandler: vi.fn(),
    wsStatus: vue.ref('connected'),
  }
})

import { addMessageHandler } from '../../../composables/useWebSocket'
import TodoPanel from '../TodoPanel.vue'
import { useChatStore } from '../../../stores/chat'
import { useSessionStore } from '../../../stores/session'

/** Grab the handler TodoPanel registered via addMessageHandler(). */
function wsHandler(): (frame: any) => void {
  const calls = vi.mocked(addMessageHandler).mock.calls
  expect(calls.length, 'addMessageHandler must have been called on mount').toBeGreaterThan(0)
  return calls[calls.length - 1][0] as (frame: any) => void
}

function todoUpdated(payload: any): any {
  return { type: 'push', cmd: 'tool_event', data: { kind: 'TodoUpdated', data: payload } }
}

async function mountPanel() {
  const wrapper = mount(TodoPanel, { props: { isDefaultChat: true } })
  await flushPromises()
  return wrapper
}

beforeEach(() => {
  setActivePinia(createPinia())
  requestMock.mockReset()
  requestMock.mockResolvedValue({ todos: [] })
})

describe('TodoPanel TodoUpdated 帧过滤（BUG-A）', () => {
  it('帧带 session_id=currentId 时刷新清单（新帧主过滤路径）', async () => {
    const session = useSessionStore()
    session.currentId = 'sess-42'
    const wrapper = await mountPanel()
    const chat = useChatStore()
    const h = wsHandler()

    // 生产形态：chat_id 是连接级（与 currentId 不同域），session_id 才是会话 id。
    h(todoUpdated({
      chat_id: 'web:conn-99',
      session_id: 'sess-42',
      todos: [{ content: '步骤一', status: 'in_progress' }],
    }))
    await flushPromises()

    expect(chat.todos).toHaveLength(1)
    expect(chat.todos[0].content).toBe('步骤一')
    wrapper.unmount()
  })

  it('帧带 session_id≠currentId 时忽略', async () => {
    const session = useSessionStore()
    session.currentId = 'sess-42'
    const wrapper = await mountPanel()
    const chat = useChatStore()
    const h = wsHandler()

    h(todoUpdated({
      chat_id: 'web:sess-42',
      session_id: 'other-session',
      todos: [{ content: '异会话', status: 'pending' }],
    }))
    await flushPromises()

    expect(chat.todos).toHaveLength(0)
    wrapper.unmount()
  })

  it('旧帧无 session_id：chat_id 匹配时回退通过（兼容）', async () => {
    const session = useSessionStore()
    session.currentId = 's1'
    const wrapper = await mountPanel()
    const chat = useChatStore()
    const h = wsHandler()

    h(todoUpdated({
      chat_id: 'web:s1',
      todos: [{ content: '旧帧', status: 'completed' }],
    }))
    await flushPromises()

    expect(chat.todos).toHaveLength(1)
    expect(chat.todos[0].status).toBe('completed')
    wrapper.unmount()
  })

  it('旧帧无 session_id：chat_id 不匹配时忽略', async () => {
    const session = useSessionStore()
    session.currentId = 's1'
    const wrapper = await mountPanel()
    const chat = useChatStore()
    const h = wsHandler()

    h(todoUpdated({ chat_id: 'web:elsewhere', todos: [{ content: 'x', status: 'pending' }] }))
    await flushPromises()

    expect(chat.todos).toHaveLength(0)
    wrapper.unmount()
  })

  it('清单渲染：todos 非空时面板出现，逐条渲染状态符号', async () => {
    const session = useSessionStore()
    session.currentId = 's1'
    const wrapper = await mountPanel()
    const h = wsHandler()

    expect(wrapper.find('.todo-panel').exists()).toBe(false)

    h(todoUpdated({
      chat_id: 'web:s1',
      todos: [
        { content: '已完成', status: 'completed' },
        { content: '进行中', status: 'in_progress' },
        { content: '待办', status: 'pending' },
      ],
    }))
    await flushPromises()

    const items = wrapper.findAll('.todo-item')
    expect(items.length).toBe(3)
    expect(items[0].classes()).toContain('is-completed')
    expect(items[1].classes()).toContain('is-in_progress')
    expect(items[2].classes()).toContain('is-pending')
    expect(wrapper.find('.todo-count').text()).toBe('1/3')
    wrapper.unmount()
  })
})
