import { describe, it, expect, vi, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount, flushPromises } from '@vue/test-utils'

// 2026-10-30（ChatPanel 长任务时间线改造）：ChatPanel 端到端——push 帧
// （tool_event 通道）→ chat store work 时间线摄取（分组/幂等）→ assistant
// 响应落地 flush 挂载 → WorkTimelineBlock 渲染（live 展开 / 落地折叠）。

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

import { onMessage, wsStatus } from '../../composables/useWebSocket'
import ChatPanel from '../ChatPanel.vue'
import { useChatStore } from '../../stores/chat'

/** Grab the handler ChatPanel registered via onMessage(). */
function wsHandler(): (frame: any) => void {
  const calls = vi.mocked(onMessage).mock.calls
  expect(calls.length, 'onMessage must have been called on mount').toBeGreaterThan(0)
  return calls[calls.length - 1][0] as (frame: any) => void
}

function started(callId: string, tool = 'exec', sessionId?: string): any {
  return {
    type: 'push',
    cmd: 'tool_event',
    data: {
      kind: 'ToolStarted',
      data: {
        chat_id: 'web:s1',
        ...(sessionId ? { session_id: sessionId } : {}),
        call_id: callId,
        tool,
        args_preview: `{"k":"${callId}"}`,
      },
    },
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

function roundText(content: string): any {
  return {
    type: 'push',
    cmd: 'tool_event',
    data: { kind: 'RoundText', data: { chat_id: 'web:s1', content } },
  }
}

function assistantReply(content: string): any {
  return {
    type: 'message',
    module: 'chat',
    cmd: 'receive',
    data: { role: 'assistant', content },
    timestamp: 't1',
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
  // inbox_status / todo_get / chat.work_current 等 WSAPI 请求默认空响应。
  requestMock.mockResolvedValue({})
  vi.mocked(onMessage).mockClear()
})

describe('ChatPanel work 时间线（2026-10-30）', () => {
  it('push 事件摄取：Started→Finished 同 call_id 原地合并，live 块展开渲染', async () => {
    const wrapper = await mountPanel()
    const chat = useChatStore()
    const h = wsHandler()

    h(roundText('我先看下目录'))
    h(started('c1'))
    await flushPromises()
    h(finished('c1'))
    h(started('c2', 'read_file'))
    await flushPromises()

    // 缓冲：叙述轮承接 c1 批次；c2 无新叙述 → 纯工具轮。
    expect(chat.pendingWork?.rounds).toHaveLength(2)
    expect(chat.pendingWork?.rounds[0].tools[0]).toMatchObject({ call_id: 'c1', ok: true })
    expect(chat.pendingWork?.rounds[1].tools[0]).toMatchObject({ tool: 'read_file', running: true })

    // live 块默认展开（外层 v-if 仅凭 pendingWork 即渲染，无需 streaming）。
    const live = wrapper.find('.wt-block.live')
    expect(live.exists()).toBe(true)
    expect(live.find('.wt-round-text').text()).toBe('我先看下目录')
    expect(live.findAll('.wt-tool').length).toBe(2)
    wrapper.unmount()
  })

  it('assistant 响应落地：flush 挂载 msg.work，缓冲清空，落地折叠块默认收起', async () => {
    const wrapper = await mountPanel()
    const chat = useChatStore()
    const h = wsHandler()

    h(started('c1', 'grep'))
    h(finished('c1', 'grep'))
    h(assistantReply('完成回复'))
    await flushPromises()

    expect(chat.pendingWork).toBeNull()
    expect(chat.messages).toHaveLength(1)
    expect(chat.messages[0].work?.rounds[0].tools[0].tool).toBe('grep')

    // 落地态：默认收起（头部在，body 不在），点击展开。
    const block = wrapper.find('.wt-block:not(.live)')
    expect(block.exists()).toBe(true)
    expect(block.find('.wt-header').text()).toContain('用时')
    expect(block.find('.wt-body').exists()).toBe(false)
    await block.find('.wt-header').trigger('click')
    await flushPromises()
    expect(wrapper.find('.wt-block:not(.live) .wt-body').exists()).toBe(true)
    wrapper.unmount()
  })

  it('直答（无中间事件）落地：消息不带 work 字段', async () => {
    const wrapper = await mountPanel()
    const chat = useChatStore()
    const h = wsHandler()

    h(assistantReply('直接回答'))
    await flushPromises()

    expect(chat.messages).toHaveLength(1)
    expect(chat.messages[0].work).toBeUndefined()
    expect(wrapper.find('.wt-block').exists()).toBe(false)
    wrapper.unmount()
  })

  it('失败徽标：终态失败工具计入头部徽标', async () => {
    const wrapper = await mountPanel()
    const chat = useChatStore()
    const h = wsHandler()

    h(started('c1'))
    h(finished('c1', 'exec', false))
    h(assistantReply('修不好'))
    await flushPromises()

    const header = wrapper.find('.wt-block:not(.live) .wt-header')
    expect(header.find('.wt-failed').text()).toContain('失败 1')
    wrapper.unmount()
  })

  it('重复推送幂等：同 call_id 重复 Finished 不产生重复条目', async () => {
    const wrapper = await mountPanel()
    const chat = useChatStore()
    const h = wsHandler()

    h(started('c1'))
    h(finished('c1'))
    h(finished('c1'))
    await flushPromises()

    expect(chat.pendingWork?.rounds.flatMap(r => r.tools)).toHaveLength(1)
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

    expect(chat.pendingWork?.rounds.flatMap(r => r.tools)).toHaveLength(1)
    expect(chat.pendingWork?.rounds.flatMap(r => r.tools)[0].call_id).toBe('c1')
    wrapper.unmount()
  })

  // -------------------------------------------------------------------------
  // BUG-A（2026-09-20）：帧内层 chat_id 是连接级 id（`web:{连接id}`），与
  // 会话 id 不同域，旧过滤恒不等 → 全部实时帧被丢弃。修复：优先按 pump
  // 注入的 session_id（会话 id 域）过滤。
  // -------------------------------------------------------------------------

  it('BUG-A：帧带 session_id=currentId 时通过（新帧主过滤路径）', async () => {
    const { useSessionStore } = await import('../../stores/session')
    useSessionStore().currentId = 'sess-42'
    const wrapper = await mountPanel()
    const chat = useChatStore()
    const h = wsHandler()

    // 生产形态：chat_id 是连接级（与 currentId 不同域），session_id 才是会话 id。
    h(started('c1', 'exec', 'sess-42'))
    await flushPromises()

    expect(chat.pendingWork?.rounds.flatMap(r => r.tools)).toHaveLength(1)
    expect(chat.pendingWork?.rounds.flatMap(r => r.tools)[0].call_id).toBe('c1')
    wrapper.unmount()
  })

  it('BUG-A：帧带 session_id≠currentId 时丢弃（chat_id 碰巧匹配也不放行）', async () => {
    const { useSessionStore } = await import('../../stores/session')
    useSessionStore().currentId = 'sess-42'
    const wrapper = await mountPanel()
    const chat = useChatStore()
    const h = wsHandler()

    h(started('c9', 'exec', 'other-session'))
    await flushPromises()

    expect(chat.pendingWork).toBeNull()
    wrapper.unmount()
  })

  it('watchdog 重同步（replaceMessages）清 pendingWork，旧事件不误挂下一轮', async () => {
    const wrapper = await mountPanel()
    const chat = useChatStore()
    const h = wsHandler()

    h(started('c1'))
    chat.replaceMessages([{ role: 'assistant', content: 'recovered', timestamp: 't0' }])
    h(assistantReply('下一轮回复'))
    await flushPromises()

    expect(chat.messages).toHaveLength(2)
    expect(chat.messages[1].work).toBeUndefined()
    wrapper.unmount()
  })

  it('work_current 活查询恢复：切回悬空会话经 sync+adopt 渲染 live 块', async () => {
    const { useSessionStore } = await import('../../stores/session')
    useSessionStore().currentId = 's1'
    const wrapper = await mountPanel()
    const chat = useChatStore()
    const h = wsHandler()

    // 悬空 user 行 + 已载历史标记（重连补拉分支的准入条件）。
    chat.addMessage({ role: 'user', content: '长任务', timestamp: 't0' })
    chat.historyLoaded = true
    // mount 时 onMounted 已发过 loadHistory（mock 的 sendHistoryRequest 永不
    // 回包，historyLoading 挂到 10s 超时）——模拟在飞已收尾，否则
    // syncMissedChat 入口闸直接 bail。
    chat.historyLoading = false

    // chat.sync 补拉回进行中工具事件（无 assistant——turn 未完）；随后
    // detectPendingTurn 起轮 → work_current 拉到活累加器全量快照。
    requestMock.mockImplementation(async (module: string, cmd: string) => {
      if (module === 'chat' && cmd === 'sync') {
        return {
          events: [
            { kind: 'tool', tool: { kind: 'ToolStarted', data: { chat_id: 'web:s1', call_id: 'c0', tool: 'exec', args_preview: 'run' } }, seq: 5 },
          ],
        }
      }
      if (module === 'chat' && cmd === 'work_current') {
        return {
          work: {
            started_at_ms: Date.now() - 60_000,
            ended_at_ms: Date.now(),
            rounds: [
              { text: '切回前的叙述', tools: [{ tool: 'exec', call_id: 'c0', ok: false, duration_ms: 10, running: true }] },
            ],
          },
        }
      }
      return {}
    })

    // 模拟重连：wsStatus 翻转 → watcher 走「已载历史 → 断线补拉」分支。
    wsStatus.value = 'disconnected'
    await flushPromises()
    wsStatus.value = 'connected'
    await flushPromises()
    // detectPendingTurn → refreshWorkCurrent 异步链。
    await new Promise(r => setTimeout(r, 0))
    await flushPromises()

    // 补拉摄取 + 快照接管 → live 块渲染恢复的叙述与工具。
    expect(chat.pendingWork?.rounds[0].text).toBe('切回前的叙述')
    expect(chat.pendingWork?.rounds[0].tools[0].running).toBe(true)
    const live = wrapper.find('.wt-block.live')
    expect(live.exists()).toBe(true)
    expect(live.text()).toContain('切回前的叙述')
    wrapper.unmount()
  })
})
