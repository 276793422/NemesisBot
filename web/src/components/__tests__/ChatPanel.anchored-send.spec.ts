import { describe, it, expect, vi, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount, flushPromises } from '@vue/test-utils'

// 2026-09-26「launcher 态发送右侧无反应」BUG 回归钉死（三形态）：
// A) 无会话锚发送 → 先建会话再发（chat.send 带 session_id），watch 切换链
//    不 reset 毁掉本地回显（justCreatedSid 守卫）
// B) tool_event 无锚豁免——空锚不再把新建会话的实时工具帧全量丢弃
// C) tool_event 异会话照常过滤（豁免不放宽同域纪律）

const { requestMock, sendMock, messageHandlers } = vi.hoisted(() => ({
  requestMock: vi.fn(),
  sendMock: vi.fn(),
  messageHandlers: [] as ((msg: any) => void)[],
}))

vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
}))

vi.mock('../../composables/useWebSocket', async () => {
  const { ref } = await import('vue')
  return {
    connect: vi.fn(),
    send: sendMock,
    sendHistoryRequest: vi.fn(),
    onMessage: (cb: (m: any) => void) => { messageHandlers.push(cb) },
    addMessageHandler: (cb: (m: any) => void) => { messageHandlers.push(cb) },
    removeMessageHandler: (cb: (m: any) => void) => {
      const i = messageHandlers.indexOf(cb)
      if (i >= 0) messageHandlers.splice(i, 1)
    },
    wsStatus: ref('connected'),
    httpGet: vi.fn().mockResolvedValue({}),
  }
})

import ChatPanel from '../ChatPanel.vue'
import { useChatStore } from '../../stores/chat'
import { useSessionStore } from '../../stores/session'

function wsFrame(msg: any) {
  for (const h of [...messageHandlers]) h(msg)
}

function toolFrame(sid: string | null, callId = 'c1') {
  wsFrame({
    type: 'push',
    cmd: 'tool_event',
    data: {
      kind: 'ToolStarted',
      data: {
        ...(sid ? { session_id: sid } : {}),
        call_id: callId,
        tool: 'exec',
        args_preview: 'echo hi',
      },
      seq: 1,
    },
  })
}

beforeEach(() => {
  setActivePinia(createPinia())
  messageHandlers.length = 0
  sendMock.mockReset()
  requestMock.mockReset()
  requestMock.mockImplementation(async (module: string, cmd: string) => {
    if (module === 'sessions' && cmd === 'create') return { session_id: 's-new', title: '新对话' }
    if (module === 'sessions' && cmd === 'list') return { sessions: [] }
    if (module === 'projects') return { projects: [] }
    return {}
  })
})

async function mountPanel() {
  const w = mount(ChatPanel)
  await flushPromises()
  // mock 的 sendHistoryRequest 永不回落 → 手工落定（生产由历史加载完成置 false）
  useChatStore().historyLoading = false
  await flushPromises()
  return w
}

describe('ChatPanel 无会话锚发送链（2026-09-26 BUG 回归）', () => {
  it('A: 未选中态发送 → 先 sessions.create，chat.send 带 session_id，回显保留、标记消费', async () => {
    expect(useSessionStore().currentId).toBeNull()
    const w = await mountPanel()
    await w.find('.chat-input-area textarea').setValue('你好，帮我查一下')
    await w.find('.chat-input-area button.btn-primary').trigger('click')
    await flushPromises()

    // 会话先建（WSAPI sessions.create 出现过）
    const createCalls = requestMock.mock.calls.filter((c: any[]) => c[0] === 'sessions' && c[1] === 'create')
    expect(createCalls.length).toBeGreaterThan(0)
    // chat.send 带上了新建会话 id
    const chatSend = sendMock.mock.calls.find((c: any[]) => c[2]?.moduleData?.session_id !== undefined)
    expect(chatSend).toBeTruthy()
    expect(chatSend![0]).toBe('你好，帮我查一下')
    expect(chatSend![2].moduleData.session_id).toBe('s-new')
    // 本地回显保留（watch 切换链未 reset 毁掉）+ 当前会话已锚定 + 标记消费
    const chat = useChatStore()
    expect(chat.messages.some((m) => m.role === 'user' && m.content.includes('你好，帮我查一下'))).toBe(true)
    expect(useSessionStore().currentId).toBe('s-new')
    expect(useSessionStore().justCreatedSid).toBeNull()
    w.unmount()
  })

  it('B: tool_event 无锚豁免——空锚时新建会话的实时工具帧被接受', async () => {
    const w = await mountPanel()
    expect(useSessionStore().currentId).toBeNull()
    toolFrame('s-backend', 'c-b1')
    await flushPromises()
    const evs = useChatStore().pendingToolEvents
    expect(evs.length).toBe(1)
    expect(evs[0].callId).toBe('c-b1')
    expect(evs[0].tool).toBe('exec')
    w.unmount()
  })

  it('C: tool_event 异会话照常过滤——豁免不放宽同域纪律', async () => {
    useSessionStore().currentId = 's1'
    const w = await mountPanel()
    toolFrame('s2', 'c-other') // 异会话 → 丢弃
    await flushPromises()
    expect(useChatStore().pendingToolEvents.length).toBe(0)
    toolFrame('s1', 'c-mine') // 同会话 → 接受
    await flushPromises()
    expect(useChatStore().pendingToolEvents.length).toBe(1)
    expect(useChatStore().pendingToolEvents[0].callId).toBe('c-mine')
    w.unmount()
  })

  it('D: 锚定发送闭环——发送建会话后同域工具帧实时入库', async () => {
    const w = await mountPanel()
    await w.find('.chat-input-area textarea').setValue('跑个工具')
    await w.find('.chat-input-area button.btn-primary').trigger('click')
    await flushPromises()
    // 锚定完成（s-new），同域工具帧实时进 pending
    toolFrame('s-new', 'c-live')
    await flushPromises()
    const evs = useChatStore().pendingToolEvents
    expect(evs.length).toBe(1)
    expect(evs[0].callId).toBe('c-live')
    // 进行中反馈：typing 或工具卡区可见
    expect(w.find('.typing-indicator').exists() || w.find('.tool-cards').exists()).toBe(true)
    w.unmount()
  })
})
