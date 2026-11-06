// 子代理活动胶囊（SubagentDrawer）组件测试（2026-11-06 胶囊化改造）。
//
// 核心回归锁：**终态行不进 live 容器**——BUG 根因（2026-11-06 截图）是
// 旧版只按会话过滤，台账终态行（ledger 语义，容量 500）每轮在 live 容器
// 重放。硬闸 status === 'running' 必须钉死。
// 其余：会话过滤 / 胶囊形态（spinner+任务+用时+停止钮）/ stop 调
// subagents.cancel / 入场动画静止约束 / push 帧触发刷新。
// WSAPI 与 WebSocket composables 全 mock，不碰真实后端。
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
    vi.spyOn(window, 'confirm').mockReturnValue(true)
    vi.useFakeTimers()
  })
  afterEach(() => {
    vi.useRealTimers()
    vi.restoreAllMocks()
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
    expect(w.find('.subagent-capsules').exists()).toBe(false)
  })

  // —— 核心回归锁（BUG 根因）：终态行绝不进 live 容器 ——
  it('终态行不进 live 容器（done/failed/cancelled/interrupted 全闸掉）', async () => {
    requestMock.mockResolvedValue({
      tasks: [
        row({ id: 'subagent:done', status: 'done', running: false }),
        row({ id: 'subagent:failed', status: 'failed', running: false }),
        row({ id: 'subagent:cancelled', status: 'cancelled', running: false }),
        row({ id: 'subagent:interrupted', status: 'interrupted', running: false }),
      ],
    })
    const w = await mountDrawer()
    expect(w.find('.subagent-capsules').exists()).toBe(false)
    expect(w.text()).not.toContain('已完成')
  })

  it('运行中行与终态行混合时只渲染运行中行', async () => {
    requestMock.mockResolvedValue({
      tasks: [
        row({ id: 'subagent:live', task: '正在跑的任务' }),
        row({ id: 'subagent:dead', status: 'done', running: false, task: '早完成的任务' }),
      ],
    })
    const w = await mountDrawer()
    const caps = w.findAll('.subagent-capsule')
    expect(caps.length).toBe(1)
    expect(w.text()).toContain('正在跑的任务')
    expect(w.text()).not.toContain('早完成的任务')
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
    expect(w.findAll('.subagent-capsule').length).toBe(2)
    const text = w.find('.subagent-capsules').text()
    expect(text).toContain('帮我编译')
    expect(text).not.toContain('sess-2')
  })

  it('多任务并列：N 个运行中行渲染 N 条胶囊', async () => {
    requestMock.mockResolvedValue({
      tasks: [row({ id: 'subagent:a' }), row({ id: 'subagent:b', task: '第二个任务' })],
    })
    const w = await mountDrawer()
    expect(w.findAll('.subagent-capsule').length).toBe(2)
  })

  it('胶囊形态：spinner + 任务名 + 用时 + 停止钮，无徽标无页脚', async () => {
    requestMock.mockResolvedValue({ tasks: [row({})] })
    const w = await mountDrawer()
    expect(w.find('.sc-spinner').exists()).toBe(true)
    expect(w.find('.sc-task').text()).toBe('帮我编译')
    expect(w.find('.sc-elapsed').text()).toMatch(/^\d+h? ?\d*[ms]?/)
    expect(w.find('.sc-stop').exists()).toBe(true)
    // 旧形态四件套全数退役
    expect(w.find('.drawer-row').exists()).toBe(false)
    expect(w.find('.drawer-foot').exists()).toBe(false)
    expect(w.text()).not.toContain('详情见')
    expect(w.find('.badge').exists()).toBe(false)
  })

  it('停止钮经确认后调 subagents.cancel', async () => {
    requestMock.mockResolvedValue({ tasks: [row({})] })
    const w = await mountDrawer()
    await w.find('.sc-stop').trigger('click')
    expect(window.confirm).toHaveBeenCalled()
    expect(requestMock).toHaveBeenCalledWith('subagents', 'cancel', { id: 'subagent:u1' })
  })

  it('确认拒绝时不调 cancel', async () => {
    vi.spyOn(window, 'confirm').mockReturnValue(false)
    requestMock.mockResolvedValue({ tasks: [row({})] })
    const w = await mountDrawer()
    await w.find('.sc-stop').trigger('click')
    expect(requestMock).not.toHaveBeenCalledWith('subagents', 'cancel', expect.anything())
  })

  it('入场动画静止约束：首拉在场行不动画，随后新出现的行淡入', async () => {
    requestMock.mockResolvedValue({ tasks: [row({ id: 'subagent:a' })] })
    const w = await mountDrawer()
    expect(w.find('.subagent-capsule').classes()).not.toContain('sc-anim')
    // push 帧触发刷新，新行出现 → 淡入
    const handler = vi.mocked(addMessageHandler).mock.calls[0][0] as (f: unknown) => void
    requestMock.mockResolvedValue({
      tasks: [row({ id: 'subagent:a' }), row({ id: 'subagent:b', task: '新任务' })],
    })
    handler({ type: 'push', cmd: 'tool_event', data: { subagent_id: 'subagent:b', kind: 'ToolStarted', data: {} } })
    await flushPromises()
    const caps = w.findAll('.subagent-capsule')
    expect(caps.length).toBe(2)
    expect(caps[0].classes()).not.toContain('sc-anim')
    expect(caps[1].classes()).toContain('sc-anim')
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
