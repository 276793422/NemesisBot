// 子代理抽屉（SubagentDrawer）组件测试：零行隐藏 / 父会话过滤 /
// 深链跳转 / push 帧触发刷新。WSAPI 与 WebSocket composables 全 mock，
// 不碰真实后端。
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { createPinia, setActivePinia } from 'pinia'

const requestMock = vi.fn()

// 组件对 useWebSocket 是**具名导入** addMessageHandler/removeMessageHandler
// （非 useWebSocket() 解构）——mock 必须同时给这三个 export；共享 fn 走
// vi.hoisted（vi.mock 工厂提升先于 const 声明）。
const wsMocks = vi.hoisted(() => ({
  addMessageHandler: vi.fn(),
  removeMessageHandler: vi.fn(),
}))
const addMessageHandler = wsMocks.addMessageHandler
const removeMessageHandler = wsMocks.removeMessageHandler
vi.mock('../../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: requestMock, connected: { value: true } }),
}))
vi.mock('../../../composables/useWebSocket', () => ({
  addMessageHandler: wsMocks.addMessageHandler,
  removeMessageHandler: wsMocks.removeMessageHandler,
  useWebSocket: () => ({
    addMessageHandler: wsMocks.addMessageHandler,
    removeMessageHandler: wsMocks.removeMessageHandler,
    connect: vi.fn(),
    sendMessage: vi.fn(),
  }),
}))

const pushMock = vi.fn()
vi.mock('vue-router', () => ({
  useRouter: () => ({ push: pushMock }),
}))

import SubagentDrawer from '../SubagentDrawer.vue'

function row(overrides: Record<string, unknown>) {
  return {
    id: 'subagent:u1',
    parent_session_key: 'web:sess-1',
    parent_chat_id: 'sess-1',
    origin: 'detached',
    task: '帮我编译',
    status: 'running',
    running: true,
    summary: null,
    error: null,
    started_at_ms: 1,
    ...overrides,
  }
}

async function mountDrawer(sessionSid = 'sess-1') {
  const w = mount(SubagentDrawer, { props: { sessionSid } })
  await flushPromises()
  return w
}

describe('SubagentDrawer', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    requestMock.mockReset()
    addMessageHandler.mockClear()
    removeMessageHandler.mockClear()
    pushMock.mockReset()
    vi.useFakeTimers()
  })
  afterEach(() => {
    vi.useRealTimers()
  })

  it('注册消息处理器并拉取列表', async () => {
    requestMock.mockResolvedValue({ tasks: [] })
    await mountDrawer()
    expect(addMessageHandler).toHaveBeenCalledTimes(1)
    expect(requestMock).toHaveBeenCalledWith('subagents', 'list', {})
  })

  it('零行隐藏（v-if 整块不渲染）', async () => {
    requestMock.mockResolvedValue({ tasks: [] })
    const w = await mountDrawer()
    expect(w.find('.subagent-drawer').exists()).toBe(false)
  })

  it('按父会话过滤：只显示当前会话派生的行', async () => {
    requestMock.mockResolvedValue({
      tasks: [
        row({ id: 'subagent:a', parent_session_key: 'web:sess-1', parent_chat_id: 'sess-1' }),
        row({ id: 'subagent:b', parent_session_key: 'web:sess-2', parent_chat_id: 'sess-2' }),
        row({ id: 'subagent:c', origin: 'delegate', parent_session_key: 'web:sess-1', parent_chat_id: 'sess-1' }),
      ],
    })
    const w = await mountDrawer()
    const rows = w.findAll('.drawer-row')
    expect(rows.length).toBe(2)
    const text = w.find('.subagent-drawer').text()
    expect(text).toContain('帮我编译')
    expect(text).not.toContain('sess-2')
  })

  it('点击行深链 /subagents?focus=<id>', async () => {
    requestMock.mockResolvedValue({ tasks: [row({})] })
    const w = await mountDrawer()
    await w.find('.drawer-row').trigger('click')
    expect(pushMock).toHaveBeenCalledWith({ path: '/subagents', query: { focus: 'subagent:u1' } })
  })

  it('subagent push 帧触发列表刷新', async () => {
    requestMock.mockResolvedValue({ tasks: [] })
    await mountDrawer()
    const handler = vi.mocked(addMessageHandler).mock.calls[0][0] as (f: unknown) => void
    requestMock.mockClear()
    requestMock.mockResolvedValue({ tasks: [row({})] })
    handler({ type: 'push', cmd: 'tool_event', data: { subagent_id: 'subagent:x', kind: 'ToolStarted', data: {} } })
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('subagents', 'list', {})
    // 非子代理 push 帧不触发
    requestMock.mockClear()
    handler({ type: 'push', cmd: 'tool_event', data: { kind: 'ToolStarted', data: {} } })
    await flushPromises()
    expect(requestMock).not.toHaveBeenCalled()
  })
})
