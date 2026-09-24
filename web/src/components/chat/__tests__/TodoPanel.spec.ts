import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount, flushPromises } from '@vue/test-utils'
import { ref } from 'vue'

// TodoPanel（2026-09-20 BUG-A 回归）：TodoUpdated push 帧按当前会话过滤。
// 帧内层 chat_id 是连接级 id（`web:{连接id}`），与会话 id 不同域——旧过滤
// 恒不等 → TodoUpdated 全部被丢弃 → 清单永不出现。修复：优先按 pump 注入
// 的 `session_id`（会话 id，session_key 末段）过滤；旧帧无该字段时回退
// chat_id 前缀匹配兜底。
//
// 2026-09-24 嵌入面板串扰回归：会话域从全局 currentId 改为宿主传入的
// `sessionId` prop（ChatPanel 的 effectiveSid，D-3）——工作流「对话生成」
// 面板钉死绑定会话（module 仍是 chat），面板不得把全局选中会话的清单
// 拉来渲染/收帧。

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

async function mountPanel(sessionId: string | null) {
  const wrapper = mount(TodoPanel, { props: { sessionId } })
  await flushPromises()
  return wrapper
}

beforeEach(() => {
  setActivePinia(createPinia())
  requestMock.mockReset()
  requestMock.mockResolvedValue({ todos: [] })
})

describe('TodoPanel TodoUpdated 帧过滤（BUG-A）', () => {
  it('帧带 session_id=面板会话时刷新清单（新帧主过滤路径）', async () => {
    const wrapper = await mountPanel('sess-42')
    const chat = useChatStore()
    const h = wsHandler()

    // 生产形态：chat_id 是连接级（与面板会话不同域），session_id 才是会话 id。
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

  it('帧带 session_id≠面板会话时忽略', async () => {
    const wrapper = await mountPanel('sess-42')
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
    const wrapper = await mountPanel('s1')
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
    const wrapper = await mountPanel('s1')
    const chat = useChatStore()
    const h = wsHandler()

    h(todoUpdated({ chat_id: 'web:elsewhere', todos: [{ content: 'x', status: 'pending' }] }))
    await flushPromises()

    expect(chat.todos).toHaveLength(0)
    wrapper.unmount()
  })

  it('清单渲染：todos 非空时面板出现，逐条渲染状态符号', async () => {
    const wrapper = await mountPanel('s1')
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

describe('TodoPanel 嵌入面板会话域（2026-09-24 串扰回归）', () => {
  it('面板钉在会话 A：全局选中会话 B 的帧不渲染（工作流对话生成场景）', async () => {
    // 主聊天全局选中 sess-B；工作流「对话生成」面板钉 sess-A。
    const session = useSessionStore()
    session.currentId = 'sess-B'
    const wrapper = await mountPanel('sess-A')
    const chat = useChatStore()
    const h = wsHandler()

    h(todoUpdated({
      chat_id: 'web:sess-B',
      session_id: 'sess-B',
      todos: [{ content: '主聊天的清单', status: 'in_progress' }],
    }))
    await flushPromises()

    expect(chat.todos).toHaveLength(0)
    expect(wrapper.find('.todo-panel').exists()).toBe(false)
    wrapper.unmount()
  })

  it('挂载拉取用面板钉住的会话 id（非全局 currentId）', async () => {
    const session = useSessionStore()
    session.currentId = 'sess-B'
    await mountPanel('sess-A')

    expect(requestMock).toHaveBeenCalledWith('chat', 'todo_get', { session_id: 'sess-A' })
    expect(requestMock).not.toHaveBeenCalledWith('chat', 'todo_get', { session_id: 'sess-B' })
  })

  it('prop 换绑会话（agent-gen 切目标）：重拉新会话，旧回包丢弃', async () => {
    requestMock.mockResolvedValueOnce({ todos: [{ content: '旧会话清单', status: 'pending' }] })
    const wrapper = await mountPanel('sess-A')
    const chat = useChatStore()
    expect(chat.todos).toHaveLength(1)

    // 回包慢于切换：切到 sess-B 后旧会话回包才落地 → 丢弃。
    requestMock.mockImplementation(() => new Promise(() => {}))
    await wrapper.setProps({ sessionId: 'sess-B' })
    await flushPromises()
    expect(requestMock).toHaveBeenLastCalledWith('chat', 'todo_get', { session_id: 'sess-B' })
    expect(chat.todos).toHaveLength(1) // 仍是切换前那份（挂起回包未落地）
    wrapper.unmount()
  })
})

describe('TodoPanel 全完成收起（R2 + 2026-09-24 先判断再展示）', () => {
  beforeEach(() => {
    vi.useFakeTimers()
  })
  afterEach(() => {
    vi.useRealTimers()
  })

  it('挂载拉取到历史全完成清单：不渲染面板（修项目对话框 4/4 闪现）', async () => {
    requestMock.mockResolvedValue({
      todos: [
        { content: 'x', status: 'completed' },
        { content: 'y', status: 'completed' },
      ],
    })
    const wrapper = await mountPanel('s1')
    expect(wrapper.find('.todo-panel').exists()).toBe(false)
    vi.advanceTimersByTime(10000)
    await wrapper.vm.$nextTick()
    expect(wrapper.find('.todo-panel').exists()).toBe(false)
    wrapper.unmount()
  })

  it('实时首帧即全完成（此前无清单在显示）：不闪现', async () => {
    const wrapper = await mountPanel('s1')
    const h = wsHandler()

    h(todoUpdated({
      chat_id: 'web:s1',
      session_id: 's1',
      todos: [{ content: '一步到位', status: 'completed' }],
    }))
    await flushPromises()
    expect(wrapper.find('.todo-panel').exists()).toBe(false)
    vi.advanceTimersByTime(10000)
    await wrapper.vm.$nextTick()
    expect(wrapper.find('.todo-panel').exists()).toBe(false)
    wrapper.unmount()
  })

  it('实时勾完最后一项（此前清单在显示中）：保留 3s 全勾瞬间再收起', async () => {
    const wrapper = await mountPanel('s1')
    const h = wsHandler()

    h(todoUpdated({
      chat_id: 'web:s1',
      session_id: 's1',
      todos: [
        { content: 'a', status: 'completed' },
        { content: 'b', status: 'in_progress' },
      ],
    }))
    await flushPromises()
    expect(wrapper.find('.todo-panel').exists()).toBe(true)

    h(todoUpdated({
      chat_id: 'web:s1',
      session_id: 's1',
      todos: [
        { content: 'a', status: 'completed' },
        { content: 'b', status: 'completed' },
      ],
    }))
    await flushPromises()
    expect(wrapper.find('.todo-panel').exists()).toBe(true)
    vi.advanceTimersByTime(2999)
    await wrapper.vm.$nextTick()
    expect(wrapper.find('.todo-panel').exists()).toBe(true)
    vi.advanceTimersByTime(1)
    await wrapper.vm.$nextTick()
    expect(wrapper.find('.todo-panel').exists()).toBe(false)
    wrapper.unmount()
  })

  it('收起后重复全完成帧不复活面板', async () => {
    const wrapper = await mountPanel('s1')
    const h = wsHandler()

    h(todoUpdated({
      chat_id: 'web:s1',
      session_id: 's1',
      todos: [{ content: 'a', status: 'completed' }],
    }))
    await flushPromises()
    expect(wrapper.find('.todo-panel').exists()).toBe(false)

    h(todoUpdated({
      chat_id: 'web:s1',
      session_id: 's1',
      todos: [{ content: 'a', status: 'completed' }],
    }))
    await flushPromises()
    expect(wrapper.find('.todo-panel').exists()).toBe(false)
    wrapper.unmount()
  })

  it('3s 收起计时器不跨清单泄漏：收起未定时切到未完成清单不会被误收', async () => {
    const wrapper = await mountPanel('s1')
    const h = wsHandler()

    // 立未完成清单 → 实时勾完（进入 3s 展示窗口，计时器挂起）。
    h(todoUpdated({
      chat_id: 'web:s1',
      session_id: 's1',
      todos: [
        { content: 'a', status: 'completed' },
        { content: 'b', status: 'in_progress' },
      ],
    }))
    await flushPromises()
    h(todoUpdated({
      chat_id: 'web:s1',
      session_id: 's1',
      todos: [
        { content: 'a', status: 'completed' },
        { content: 'b', status: 'completed' },
      ],
    }))
    await flushPromises()
    expect(wrapper.find('.todo-panel').exists()).toBe(true)

    // 3s 未到时切会话拉到未完成清单 → 旧计时器必须作废。
    requestMock.mockResolvedValueOnce({
      todos: [{ content: '新会话的活', status: 'pending' }],
    })
    await wrapper.setProps({ sessionId: 's2' })
    await flushPromises()
    vi.advanceTimersByTime(10000)
    await wrapper.vm.$nextTick()
    expect(wrapper.find('.todo-panel').exists()).toBe(true)
    wrapper.unmount()
  })

  it('含未完成项（pending / in_progress）不收起', async () => {
    const wrapper = await mountPanel('s1')
    const h = wsHandler()

    h(todoUpdated({
      chat_id: 'web:s1',
      session_id: 's1',
      todos: [
        { content: 'done', status: 'completed' },
        { content: 'running', status: 'in_progress' },
      ],
    }))
    await flushPromises()
    vi.advanceTimersByTime(10000)
    await wrapper.vm.$nextTick()
    expect(wrapper.find('.todo-panel').exists()).toBe(true)
    wrapper.unmount()
  })

  it('收起后新清单（有未完成项）立即恢复显示', async () => {
    const wrapper = await mountPanel('s1')
    const h = wsHandler()

    h(todoUpdated({ chat_id: 'web:s1', session_id: 's1', todos: [{ content: 'a', status: 'completed' }] }))
    await flushPromises()
    expect(wrapper.find('.todo-panel').exists()).toBe(false)

    h(todoUpdated({
      chat_id: 'web:s1',
      session_id: 's1',
      todos: [
        { content: '新任务', status: 'pending' },
        { content: '旧任务', status: 'completed' },
      ],
    }))
    await flushPromises()
    expect(wrapper.find('.todo-panel').exists()).toBe(true)
    expect(wrapper.text()).toContain('新任务')
    wrapper.unmount()
  })
})
