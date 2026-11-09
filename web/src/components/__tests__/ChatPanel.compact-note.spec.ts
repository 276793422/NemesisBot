import { describe, it, expect, vi, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount, flushPromises } from '@vue/test-utils'

// 压缩注记徽标（2026-11-09 徽标化）：压缩过程注记不再渲染蓝底长文
// system 条——与工具调用同族的折叠小徽标（--info 蓝系）。覆盖三路
// 入路（实时 receive 帧 / 历史响应 / chat.sync 补拉）+ 旧数据 regex
// 兜底 + 进行中动画徽标的两条清扫谓词（完成回执到场 / assistant 到场）。

const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
}))

/** 捕获槽（vi.hoisted——mock 工厂在 import 期执行，普通顶层 let 会 TDZ）。 */
const h = vi.hoisted(() => ({
  wsHandler: null as ((data: any) => void) | null,
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  wsStatus: null as any,
}))

vi.mock('../../composables/useWebSocket', async () => {
  const { ref } = await import('vue')
  h.wsStatus = ref<'connected' | 'disconnected'>('connected')
  return {
    connect: vi.fn(),
    send: vi.fn(),
    sendHistoryRequest: vi.fn(),
    onMessage: vi.fn((fn: (data: any) => void) => { h.wsHandler = fn }),
    addMessageHandler: vi.fn(),
    removeMessageHandler: vi.fn(),
    wsStatus: h.wsStatus,
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
  h.wsHandler = null
  h.wsStatus.value = 'connected'
})

async function mountPanel() {
  const w = mount(ChatPanel)
  await flushPromises()
  return w
}

/** 实时 receive 帧构造（后端 send_to_session 帧形状）。 */
function receiveFrame(over: Record<string, any> = {}) {
  return {
    type: 'message',
    module: 'chat',
    cmd: 'receive',
    timestamp: '2026-11-09T10:00:00+08:00',
    data: { role: 'assistant', content: '回复正文', session_id: 's1', ...over },
  }
}

describe('ChatPanel 压缩注记徽标', () => {
  it('receive 帧 note_kind=compaction → 静态折叠徽标（替代气泡，前缀剥离）', async () => {
    const w = await mountPanel()
    h.wsHandler!(receiveFrame({
      role: 'system',
      content: '[自动压缩] 已折叠较早对话以释放上下文窗口',
      note_kind: 'compaction',
      seq: 1,
    }))
    await flushPromises()
    const note = w.find('.compact-note')
    expect(note.exists()).toBe(true)
    // 徽标替代气泡——不再是蓝底长文 system 条。
    expect(w.find('.message-bubble').exists()).toBe(false)
    // 静态形态：🗜️ 图标 + 去档位前缀的正文；title 保留全文供悬停。
    expect(note.find('.compact-note-glyph').exists()).toBe(true)
    expect(note.classes()).not.toContain('running')
    expect(note.find('.compact-note-label').text()).toBe('已折叠较早对话以释放上下文窗口')
    expect(note.attributes('title')).toBe('[自动压缩] 已折叠较早对话以释放上下文窗口')
    w.unmount()
  })

  it('receive 帧 note_kind=compaction_start → 进行中动画徽标（四竖条、无图标）', async () => {
    const w = await mountPanel()
    h.wsHandler!(receiveFrame({
      role: 'system',
      content: '[自动压缩] 正在压缩上下文…',
      note_kind: 'compaction_start',
      seq: 1,
    }))
    await flushPromises()
    const note = w.find('.compact-note.running')
    expect(note.exists()).toBe(true)
    expect(note.find('.compact-note-bars').exists()).toBe(true)
    expect(note.findAll('.compact-note-bars i')).toHaveLength(4)
    expect(note.find('.compact-note-glyph').exists()).toBe(false)
    expect(note.find('.compact-note-label').text()).toBe('正在压缩上下文…')
    // running 态无 title（不可点开）。
    expect(note.attributes('title')).toBeUndefined()
    w.unmount()
  })

  it('进行中徽标末尾实时计时（已压缩时长：秒级/分秒级格式）', async () => {
    // 起点 = 帧级 timestamp（receive 分支读 frame.timestamp，服务端盖章）；
    // label = now - ts，渲染即出。
    const w = await mountPanel()
    const f1 = receiveFrame({
      role: 'system',
      content: '[手动压缩] 正在折叠会话历史为摘要…',
      note_kind: 'compaction_start',
      seq: 1,
    })
    f1.timestamp = new Date(Date.now() - 32_000).toISOString()
    h.wsHandler!(f1)
    await flushPromises()
    const t1 = w.find('.compact-note-timer')
    expect(t1.exists()).toBe(true)
    expect(t1.text()).toBe('32 秒')
    w.unmount()

    // 第二挂载（分秒级）：同测试内共享 pinia——先清掉 f1 的徽标再重放。
    const st = useChatStore()
    st.messages.splice(0, st.messages.length)
    const w2 = await mountPanel()
    const f2 = receiveFrame({
      role: 'system',
      content: '[手动压缩] 正在折叠会话历史为摘要…',
      note_kind: 'compaction_start',
      seq: 1,
    })
    f2.timestamp = new Date(Date.now() - 95_000).toISOString()
    h.wsHandler!(f2)
    await flushPromises()
    expect(w2.find('.compact-note-timer').text()).toBe('1分35秒')
    // 静态折叠徽标（完成态）无计时片段——计时只属于进行中。
    h.wsHandler!(receiveFrame({
      role: 'system',
      content: '✓ 已压缩',
      note_kind: 'compaction',
      seq: 2,
    }))
    await flushPromises()
    expect(w2.find('.compact-note-timer').exists()).toBe(false)
    w2.unmount()
  })

  it('完成回执到场 → 进行中徽标被移除、静态徽标替换（限频闸正常路径）', async () => {
    const w = await mountPanel()
    const chatStore = useChatStore()
    h.wsHandler!(receiveFrame({
      role: 'system', content: '[自动压缩] 正在压缩上下文…', note_kind: 'compaction_start', seq: 1,
    }))
    await flushPromises()
    expect(w.find('.compact-note.running').exists()).toBe(true)
    h.wsHandler!(receiveFrame({
      role: 'system', content: '[自动压缩] 已折叠较早对话以释放上下文窗口', note_kind: 'compaction', seq: 2,
    }))
    await flushPromises()
    expect(w.find('.compact-note.running').exists()).toBe(false)
    expect(w.find('.compact-note').exists()).toBe(true)
    expect(chatStore.messages).toHaveLength(1)
    expect(chatStore.messages[0].noteKind).toBe('compaction')
    w.unmount()
  })

  it('assistant 回复到场 → 进行中徽标兜底扫除（回执被限频闸抑制时）', async () => {
    const w = await mountPanel()
    const chatStore = useChatStore()
    h.wsHandler!(receiveFrame({
      role: 'system', content: '[自动压缩] 正在压缩上下文…', note_kind: 'compaction_start', seq: 1,
    }))
    await flushPromises()
    h.wsHandler!(receiveFrame({ role: 'assistant', content: '压缩完成后的回复', model: 'm1', seq: 2 }))
    await flushPromises()
    expect(w.find('.compact-note').exists()).toBe(false)
    expect(chatStore.messages).toHaveLength(1)
    expect(chatStore.messages[0].role).toBe('assistant')
    w.unmount()
  })

  it('手动压缩全程：start 徽标 → 排队收据（⏳）不扫 → ✓ 回执扫除（BUG 2026-11-09）', async () => {
    // 手动 /compact 场景（compact_session_noticed 徽标 + finish_message ✓ 回执，
    // 全部 role=assistant / note_kind=None 除徽标帧外）：压缩进行中用户发消息
    // → busy 排队收据「⏳ 当前正在处理上一条消息…」role=assistant 到场——
    // 压缩正是「上一条消息」，此收据不得扫掉进行中徽标（否则分钟级压缩
    // 窗口重新失去反馈，用户体感=卡死）。
    const w = await mountPanel()
    const chatStore = useChatStore()
    h.wsHandler!(receiveFrame({ role: 'assistant', content: '⏳ 正在压缩会话…', seq: 1 }))
    await flushPromises()
    h.wsHandler!(receiveFrame({
      role: 'system', content: '[手动压缩] 正在折叠会话历史为摘要…', note_kind: 'compaction_start', seq: 2,
    }))
    await flushPromises()
    expect(w.find('.compact-note.running').exists()).toBe(true)
    expect(w.find('.compact-note.running').find('.compact-note-label').text()).toBe('正在折叠会话历史为摘要…')
    // 压缩中用户发消息 → busy 排队收据（role=assistant，⏳ 前缀）。
    h.wsHandler!(receiveFrame({
      role: 'assistant', content: '⏳ 当前正在处理上一条消息。你的消息已排队，将在本轮结束后继续处理。', seq: 3,
    }))
    await flushPromises()
    // 排队收据不扫徽标——动画保持到压缩真正结束。
    expect(w.find('.compact-note.running').exists()).toBe(true)
    // ✓ 完成回执（无 ⏳ 前缀）到场 → 扫除。
    h.wsHandler!(receiveFrame({
      role: 'assistant', content: '✓ 已压缩：摘要覆盖前 23 条，保留近 0 条', seq: 4,
    }))
    await flushPromises()
    expect(w.find('.compact-note').exists()).toBe(false)
    expect(chatStore.messages.some(m => m.content.includes('已压缩'))).toBe(true)
    w.unmount()
  })

  it('chat.sync 补拉：⏳ 排队收据同样不扫进行中徽标（与 receive 分支同谓词）', async () => {
    const w = await mountPanel()
    const chatStore = useChatStore()
    // 先实时挂一枚进行中徽标。
    h.wsHandler!(receiveFrame({
      role: 'system', content: '[手动压缩] 正在折叠会话历史为摘要…', note_kind: 'compaction_start', seq: 1,
    }))
    await flushPromises()
    chatStore.historyLoading = false
    chatStore.historyLoaded = true
    requestMock.mockImplementation((_mod: string, cmd: string) => {
      if (cmd === 'sync') {
        return Promise.resolve({
          events: [
            { seq: 2, role: 'assistant', content: '⏳ 当前正在处理上一条消息。你的消息已排队，将在本轮结束后继续处理。', ts: '2026-11-09T10:00:03+08:00' },
          ],
        })
      }
      return Promise.resolve({})
    })
    h.wsStatus.value = 'disconnected'
    await flushPromises()
    h.wsStatus.value = 'connected'
    await flushPromises()
    // 补拉只进了排队收据（⏳）——徽标保留。
    expect(w.find('.compact-note.running').exists()).toBe(true)
    w.unmount()
  })

  it('旧数据 regex 兜底：无 note_kind 的压缩注记 system 行仍渲染徽标', async () => {
    const w = await mountPanel()
    h.wsHandler!(receiveFrame({
      role: 'system',
      content: '[自动压缩] 已折叠较早对话以释放上下文窗口',
      seq: 1,
    }))
    await flushPromises()
    expect(w.find('.compact-note').exists()).toBe(true)
    expect(w.find('.message-bubble').exists()).toBe(false)
    w.unmount()
  })

  it('其他 system 行不误判：busy 回执（⏳）与 [System:] 前缀照旧气泡渲染', async () => {
    const w = await mountPanel()
    h.wsHandler!(receiveFrame({ role: 'system', content: '⏳ 消息排队中，请稍候', seq: 1 }))
    await flushPromises()
    expect(w.find('.compact-note').exists()).toBe(false)
    expect(w.find('.message-bubble').exists()).toBe(true)
    h.wsHandler!(receiveFrame({ role: 'system', content: '[System: tester] 后台任务完成', seq: 2 }))
    await flushPromises()
    expect(w.findAll('.compact-note')).toHaveLength(0)
    expect(w.findAll('.message-bubble')).toHaveLength(2)
    w.unmount()
  })

  it('静态徽标点击展开：expanded 类切换（全文可见）', async () => {
    const w = await mountPanel()
    h.wsHandler!(receiveFrame({
      role: 'system',
      content: '[自动压缩] 已折叠较早对话以释放上下文窗口',
      note_kind: 'compaction',
      seq: 1,
    }))
    await flushPromises()
    const note = w.find('.compact-note')
    expect(note.classes()).not.toContain('expanded')
    await note.trigger('click')
    await flushPromises()
    expect(w.find('.compact-note').classes()).toContain('expanded')
    await w.find('.compact-note').trigger('click')
    await flushPromises()
    expect(w.find('.compact-note').classes()).not.toContain('expanded')
    w.unmount()
  })

  it('历史响应透传 note_kind → 切走再切回看到同一枚徽标（不蹦蓝底长文）', async () => {
    const w = await mountPanel()
    // 首连 loadHistory 已发请求（mock 捕获 requestId）——围栏要求响应
    // request_id 匹配在飞登记，按真实调用参数回放。
    const { sendHistoryRequest } = await import('../../composables/useWebSocket')
    // mock 不在 beforeEach 重置（文件级共享）——取最近一次调用才是本测试
    // mount 发出的请求；calls[0] 会拿到首测的过期 requestId 被围栏丢弃。
    const calls = (sendHistoryRequest as ReturnType<typeof vi.fn>).mock.calls
    const requestId = calls[calls.length - 1][0] as string
    h.wsHandler!({
      type: 'message',
      module: 'chat',
      cmd: 'history_response',
      data: {
        request_id: requestId,
        session_id: 's1',
        oldest_index: 0,
        has_more: false,
        messages: [
          { role: 'user', content: '帮我看看这个 bug' },
          { role: 'assistant', content: '好的，我来看' },
          { role: 'system', content: '[自动压缩] 已折叠较早对话以释放上下文窗口', note_kind: 'compaction' },
        ],
      },
    })
    await flushPromises()
    const notes = w.findAll('.compact-note')
    expect(notes).toHaveLength(1)
    expect(notes[0].find('.compact-note-label').text()).toBe('已折叠较早对话以释放上下文窗口')
    // 同行不再渲染气泡。
    expect(w.findAll('.message-bubble')).toHaveLength(2)
    w.unmount()
  })

  it('chat.sync 补拉：重放序列 start→回执 按序处理（动画扫除、静态入列）', async () => {
    const w = await mountPanel()
    const chatStore = useChatStore()
    chatStore.historyLoading = false
    chatStore.historyLoaded = true
    requestMock.mockImplementation((_mod: string, cmd: string) => {
      if (cmd === 'sync') {
        return Promise.resolve({
          events: [
            { seq: 1, role: 'system', content: '[自动压缩] 正在压缩上下文…', note_kind: 'compaction_start', ts: '2026-11-09T10:00:00+08:00' },
            { seq: 2, role: 'system', content: '[自动压缩] 已折叠较早对话以释放上下文窗口', note_kind: 'compaction', ts: '2026-11-09T10:00:05+08:00' },
          ],
        })
      }
      return Promise.resolve({})
    })
    h.wsStatus.value = 'disconnected'
    await flushPromises()
    h.wsStatus.value = 'connected'
    await flushPromises()
    expect(w.find('.compact-note.running').exists()).toBe(false)
    expect(w.find('.compact-note').exists()).toBe(true)
    expect(chatStore.messages).toHaveLength(1)
    expect(chatStore.messages[0].noteKind).toBe('compaction')
    w.unmount()
  })

  it('刷新恢复（方案 A）：context_status compaction_running=true → 徽标+计时+流光按服务端原钟物化；false → 摘除', async () => {
    // 刷新/断连后历史重载，瞬态 start 徽标消失——refreshUsage（进会话即拉）
    // 读后端登记表投影物化徽标：计时起点 = 服务端 started_at（跨刷新连续，
    // 已压 32s → 直接显示 32 秒），usage 条流光由同一 computed 驱动恢复。
    requestMock.mockImplementation((_mod: string, cmd: string) => {
      if (cmd === 'context_status') {
        return Promise.resolve({
          context: {
            // usage-bar 只在 breakdown 在场分支渲染（else=旧后端回落只有
            // 文字徽标，条不渲染）——真实后端 context_status 恒带完整构成。
            pct: 24,
            window: 100000,
            breakdown: { system: 1000, tools: 500, mcp: 0, skill: 0, memory: 0, summary: 0, history: 1000, free: 97500 },
            compaction_running: true,
            compaction_started_at: new Date(Date.now() - 32_000).toISOString(),
            compaction_label: '[手动压缩] 正在折叠会话历史为摘要…',
          },
        })
      }
      return Promise.resolve({})
    })
    const w = await mountPanel()
    await flushPromises()
    const note = w.find('.compact-note.running')
    expect(note.exists()).toBe(true)
    expect(note.find('.compact-note-timer').text()).toBe('32 秒')
    expect(note.find('.compact-note-label').text()).toBe('正在折叠会话历史为摘要…')
    expect(w.find('.usage-bar.compacting').exists()).toBe(true)

    // 完成态同步：running=false → 徽标摘除（完成回执在断连窗口错过时的
    // 30s 轮询兜底）。切会话触发 effectiveSid watch → refreshUsage 重拉。
    requestMock.mockImplementation((_mod: string, cmd: string) => {
      if (cmd === 'context_status') {
        return Promise.resolve({
          context: { compaction_running: false, compaction_started_at: null, compaction_label: null },
        })
      }
      return Promise.resolve({})
    })
    useSessionStore().currentId = 's2'
    await flushPromises()
    expect(w.find('.compact-note.running').exists()).toBe(false)
    w.unmount()
  })
})
