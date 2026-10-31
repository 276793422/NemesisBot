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

/** 批次二：带思考链的 RoundText 帧（reasoning 可选字段）。 */
function roundTextR(content: string, reasoning: string): any {
  return {
    type: 'push',
    cmd: 'tool_event',
    data: { kind: 'RoundText', data: { chat_id: 'web:s1', content, reasoning } },
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

  // --- 批次二（2026-10-31）：思考链折叠卡 ---

  it('思考链：thinking-only RoundText 帧 → live 块默认收起的 💭 折叠卡 + 头部徽标', async () => {
    const wrapper = await mountPanel()
    const chat = useChatStore()
    const h = wsHandler()

    h(roundTextR('', '先想清楚改哪里再动手'))
    await flushPromises()

    // 缓冲：thinking-only 轮照建（content 空、reasoning 有值）。
    expect(chat.pendingWork?.rounds).toHaveLength(1)
    expect(chat.pendingWork?.rounds[0]).toMatchObject({ text: '', reasoning: '先想清楚改哪里再动手' })

    // live 块：头部 💭 徽标；思考卡默认收起（无 open 属性）。jsdom 下
    // details 的子节点恒在 DOM（无 CSS 隐藏语义），收起与否只由 open 属性
    // 表达——正文渲染内容直接可断言；浏览器原生展开交互无 JS 参与，无需模拟。
    const live = wrapper.find('.wt-block.live')
    expect(live.exists()).toBe(true)
    expect(live.find('span.wt-think').text()).toContain('💭 1')
    const think = live.find('details.wt-think')
    expect(think.exists()).toBe(true)
    expect(think.find('summary').text()).toContain('💭')
    expect(think.attributes('open')).toBeUndefined()
    expect(think.find('.wt-think-body').text()).toBe('先想清楚改哪里再动手')
    wrapper.unmount()
  })

  it('思考链：随回复落地折叠挂载；无思考的轮不渲染折叠卡', async () => {
    const wrapper = await mountPanel()
    const chat = useChatStore()
    const h = wsHandler()

    // 轮 0：思考 + 工具；轮 1：纯叙述（无思考）。
    h(roundTextR('', '推敲方案'))
    h(started('c1'))
    h(finished('c1'))
    h(roundText('纯叙述轮'))
    h(assistantReply('完成'))
    await flushPromises()

    expect(chat.messages[0].work?.rounds).toHaveLength(2)
    expect(chat.messages[0].work?.rounds[0].reasoning).toBe('推敲方案')
    expect(chat.messages[0].work?.rounds[1].reasoning).toBeUndefined()

    // 落地折叠块：头部徽标 💭 1（只有 1 轮带思考）。
    const block = wrapper.find('.wt-block:not(.live)')
    expect(block.find('.wt-header span.wt-think').text()).toContain('💭 1')

    // 展开后：仅轮 0 有 details.wt-think；轮 1 无。
    await block.find('.wt-header').trigger('click')
    await flushPromises()
    const thinks = wrapper.findAll('.wt-block:not(.live) details.wt-think')
    expect(thinks).toHaveLength(1)
    expect(thinks[0].find('.wt-think-body').text()).toBe('推敲方案')
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

  // -------------------------------------------------------------------------
  // BUG-20261030-208（R263）：错误帧 / 重复恢复帧不清 work/交付物缓冲——
  // live 块永挂 + 下一 turn 事件混进同一缓冲 + 下一回复落地整包误挂。
  // -------------------------------------------------------------------------

  function errorFrame(content: string): any {
    return { type: 'message', module: 'chat', cmd: 'error', data: { content }, timestamp: 't2' }
  }

  function deliverablesPush(files: any[]): any {
    return { type: 'push', cmd: 'deliverables', data: { chat_id: 'web:s1', files } }
  }

  it('BUG-208：错误帧终结 turn——时间线/交付物 flush 挂到错误消息，live 块撤下', async () => {
    const wrapper = await mountPanel()
    const chat = useChatStore()
    const h = wsHandler()

    h(roundText('准备执行'))
    h(started('c1'))
    h(deliverablesPush([{ path: 'out/report.md', title: '报告', note: '' }]))
    h(errorFrame('模型调用失败'))
    await flushPromises()

    // 缓冲已排空（live 块 v-if 含 pendingWork → 撤下）。
    expect(chat.pendingWork).toBeNull()
    expect(chat.pendingDeliverables).toHaveLength(0)
    // 错误消息挂载时间线与交付物（与后端错误行落盘 work 键的重载语义对齐；
    // 模板 work/deliverables 块 role-agnostic，error 角色照常渲染）。
    expect(chat.messages).toHaveLength(1)
    expect(chat.messages[0].role).toBe('error')
    expect(chat.messages[0].work?.rounds[0].text).toBe('准备执行')
    expect(chat.messages[0].deliverables).toHaveLength(1)
    const blocks = wrapper.findAll('.wt-block')
    expect(blocks).toHaveLength(1)
    expect(blocks[0].classes()).not.toContain('live')
    wrapper.unmount()
  })

  it('BUG-208：错误帧后下一 turn 事件不混包——新回复只带自己的时间线', async () => {
    const wrapper = await mountPanel()
    const chat = useChatStore()
    const h = wsHandler()

    // 第一轮：事件积累后错误终结。
    h(started('c1'))
    h(errorFrame('挂了'))
    await flushPromises()
    // 第二轮：新事件 + 正常回复。
    h(roundText('第二轮重来'))
    h(started('c2', 'grep'))
    h(finished('c2', 'grep'))
    h(assistantReply('这回成了'))
    await flushPromises()

    expect(chat.pendingWork).toBeNull()
    expect(chat.messages).toHaveLength(2)
    expect(chat.messages[0].role).toBe('error')
    expect(chat.messages[0].work?.rounds.flatMap(r => r.tools).map(t => t.call_id)).toEqual(['c1'])
    // 新回复时间线只含第二轮事件（修复前：混包含 c1）。
    expect(chat.messages[1].work?.rounds[0].text).toBe('第二轮重来')
    expect(chat.messages[1].work?.rounds.flatMap(r => r.tools).map(t => t.call_id)).toEqual(['c2'])
    wrapper.unmount()
  })

  it('BUG-208 横向：重复恢复帧（watchdog 已落同尾消息）跳过挂载但排空缓冲', async () => {
    const wrapper = await mountPanel()
    const chat = useChatStore()
    const h = wsHandler()

    // watchdog 已从磁盘恢复同尾 assistant 消息（视图已有回复）。
    chat.addMessage({ role: 'assistant', content: '已恢复的回复', timestamp: 't0' })
    // 迟到的实时事件仍在摄取（缓冲非空）。
    h(started('c1'))
    h(finished('c1'))
    await flushPromises()
    expect(chat.pendingWork).not.toBeNull()

    // 迟到 live 帧与尾部同文 → isDuplicateRecovery 跳过挂载，但缓冲排空
    // （时间线已随磁盘行渲染，live 缓冲与磁盘行重复，留着只会混入下轮）。
    h(assistantReply('已恢复的回复'))
    await flushPromises()

    expect(chat.messages).toHaveLength(1) // 不重复插行
    expect(chat.pendingWork).toBeNull()
    expect(chat.pendingDeliverables).toHaveLength(0)
    wrapper.unmount()
  })
})
