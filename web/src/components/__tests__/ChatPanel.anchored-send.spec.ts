import { describe, it, expect, vi, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount, flushPromises } from '@vue/test-utils'

// 2026-09-26「launcher 态发送右侧无反应」BUG 回归钉死（三形态）：
// A) 无会话锚发送 → 先建会话再发（chat.send 带 session_id），watch 切换链
//    不 reset 毁掉本地回显（justCreatedSid 守卫）
// B) tool_event 无锚豁免——空锚不再把新建会话的实时工具帧全量丢弃
// C) tool_event 异会话照常过滤（豁免不放宽同域纪律）
// 2026-11-07「侧栏新建 → 秒发」竞速回归钉死（两形态，FF chat e2e 实录）：
// E) 侧栏新建在途（sessions.create 网络往返未落定）→ 秒发：发送挂起等
//    settle，落定后绑新建会话、回显保留且行号锚建立（rowIndex=0）
// F) 切换到已知零消息会话跳过 loadHistory（空拉只会抹竞速窗口回显）+
//    补行号锚；非零消息会话照常重拉
// G) 新建在飞窗口内输入的草稿随会话落地存活（carryInput 不洗在建输入，
//    wk8 FF 全量实录：fill→click 前落地 → 发送钮恒 disabled）

const { requestMock, sendMock, historyMock, messageHandlers } = vi.hoisted(() => ({
  requestMock: vi.fn(),
  sendMock: vi.fn(),
  historyMock: vi.fn(),
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
    sendHistoryRequest: historyMock,
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
  historyMock.mockReset()
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
    const tools = useChatStore().pendingWork?.rounds.flatMap(r => r.tools) ?? []
    expect(tools.length).toBe(1)
    expect(tools[0].call_id).toBe('c-b1')
    expect(tools[0].tool).toBe('exec')
    w.unmount()
  })

  it('C: tool_event 异会话照常过滤——豁免不放宽同域纪律', async () => {
    useSessionStore().currentId = 's1'
    const w = await mountPanel()
    toolFrame('s2', 'c-other') // 异会话 → 丢弃
    await flushPromises()
    expect(useChatStore().pendingWork).toBeNull()
    toolFrame('s1', 'c-mine') // 同会话 → 接受
    await flushPromises()
    const tools = useChatStore().pendingWork?.rounds.flatMap(r => r.tools) ?? []
    expect(tools.length).toBe(1)
    expect(tools[0].call_id).toBe('c-mine')
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
    const tools = useChatStore().pendingWork?.rounds.flatMap(r => r.tools) ?? []
    expect(tools.length).toBe(1)
    expect(tools[0].call_id).toBe('c-live')
    // 进行中反馈：typing 或 live work 块可见
    expect(w.find('.typing-indicator').exists() || w.find('.wt-block.live').exists()).toBe(true)
    w.unmount()
  })

  // 2026-11-07 侧栏新建→秒发竞速回归（FF chat e2e 实录）：
  // sessionStore.create 的 sessions.create 是网络往返，往返期间 currentId 仍指
  // 旧会话——修复前此窗口内的发送绑旧会话（消息+回复全落旧会话，视图随后切
  // 到新空会话，两者皆不可见）。修复 = sendValidated 先 settlePendingCreates。
  it('E: 侧栏新建在途→秒发：发送挂起等 settle，落定后绑新建会话且回显带行号锚', async () => {
    let resolveCreate!: (v: unknown) => void
    // 可控 deferred：模拟侧栏 newChat() 的 sessions.create 网络往返在途。
    // 全量替换（非 mockImplementationOnce）——Once 按「下一次调用」消费不按
    // 参数匹配，mount 期间的 list/projects 请求会把 Once 槽抢走，create 落到
    // 默认实现立即完成，竞速现场就没了。
    requestMock.mockImplementation(async (module: string, cmd: string) => {
      if (module === 'sessions' && cmd === 'create') {
        return new Promise((r) => {
          resolveCreate = r
        })
      }
      if (module === 'sessions' && cmd === 'list') return { sessions: [] }
      if (module === 'projects') return { projects: [] }
      return {}
    })
    const store = useSessionStore()
    store.currentId = 's-old' // 从旧会话切出（竞速现场：旧会话有历史）
    const w = await mountPanel()
    const histBaseline = historyMock.mock.calls.length // 挂载首拉已计入基线

    // 侧栏「新建对话」点击 → create 在飞（尚未落定，currentId 仍 s-old）
    const createPromise = store.create()
    await flushPromises()
    expect(store.currentId).toBe('s-old')

    // 往返窗口内秒发（竞速窗口本体）
    await w.find('.chat-input-area textarea').setValue('你好，帮我查一下')
    await w.find('.chat-input-area button.btn-primary').trigger('click')
    await flushPromises()
    // create 未落定 → 发送挂起等待 settle，绝不绑旧会话
    expect(sendMock.mock.calls.length).toBe(0)

    // create 落定：乐观行 + switchTo(s-new) → watch reset（零消息跳过重拉+补锚）
    resolveCreate({ session_id: 's-new', title: '新对话' })
    await createPromise
    await flushPromises()

    // chat.send 绑新建会话（非旧会话）
    const chatSend = sendMock.mock.calls.find(
      (c: any[]) => c[2]?.moduleData?.session_id !== undefined,
    )
    expect(chatSend).toBeTruthy()
    expect(chatSend![0]).toBe('你好，帮我查一下')
    expect(chatSend![2].moduleData.session_id).toBe('s-new')
    // 回显保留在新视图（reset 后回显，未绑错旧会话）+ 行号锚建立（rowIndex=0，
    // 回退/重新生成按钮组首轮可用）+ 零消息新会话未重拉历史（空拉只会抹回显）
    const chat = useChatStore()
    expect(chat.messages.length).toBe(1)
    expect(chat.messages[0].role).toBe('user')
    expect(chat.messages[0].content).toContain('你好，帮我查一下')
    expect(chat.messages[0].rowIndex).toBe(0)
    expect(store.currentId).toBe('s-new')
    // 新会话零消息 → 切换 watch 跳过重拉（此后无新历史请求；空拉只会抹回显）
    expect(historyMock.mock.calls.length).toBe(histBaseline)
    w.unmount()
  })

  // 2026-11-07 切换零消息会话跳过重拉（修复件之二）：已知零消息（乐观行恒 0，
  // 服务端 list 计数=jsonl 行数）的 loadHistory 空拉唯一「产出」是空响应回填，
  // 抹掉竞速窗口内的本地回显；同时 reset 后必须补行号锚（M6 回归锁）。
  it('F: 切换零消息会话不重拉历史且补行号锚；非零消息会话照常重拉', async () => {
    const store = useSessionStore()
    const w = await mountPanel()
    const baseline = historyMock.mock.calls.length // 挂载首拉已计入基线
    // 种两行：零消息（乐观行形态）+ 非零（真实历史形态）
    store.sessions.unshift(
      {
        id: 's-fresh',
        channel: 'web',
        startTime: '',
        lastTime: '',
        messageCount: 0,
        firstMessage: '新对话',
        model: '',
      },
      {
        id: 's-full',
        channel: 'web',
        startTime: '',
        lastTime: '',
        messageCount: 3,
        firstMessage: '旧会话',
        model: '',
      },
    )
    const chat = useChatStore()
    // 旧视图残留（切换 reset 应清掉）
    chat.messages = [
      { role: 'user', content: '旧会话残留', timestamp: new Date().toISOString() },
    ] as any

    // 切零消息会话：reset 生效（残留清掉）+ 不发历史请求
    // （补锚语义由测试 E 的回显 rowIndex===0 锁住——nextRowIndex 未导出）
    store.switchTo('s-fresh')
    await flushPromises()
    expect(chat.messages.length).toBe(0)
    expect(historyMock.mock.calls.length).toBe(baseline)

    // 切非零消息会话：照常重拉（旧行为不回归）
    store.switchTo('s-full')
    await flushPromises()
    expect(historyMock.mock.calls.length).toBe(baseline + 1)
    expect(historyMock.mock.calls[baseline]?.[0]).toMatch(/^hist_/)
    w.unmount()
  })

  // 2026-11-07 FF chat e2e 全量实录（wk8）：fill 后 click 前恰逢 create 落定
  // → 切换 watch carryInput 把在建输入当旧会话草稿洗掉 → 发送钮恒 disabled
  // （click 10s 超时；solo 过/全量红的分岔 = create 落地快慢决定窗口先后）。
  // 修复 = create 落地时快照现 input 进新会话草稿表（真实用户慢网下点新建
  // 后立即打字同样命中，属产品缺陷非测试专属）。
  it('G: 新建在飞窗口内输入的草稿随会话落地存活，发送钮可用', async () => {
    let resolveCreate!: (v: unknown) => void
    requestMock.mockImplementation(async (module: string, cmd: string) => {
      if (module === 'sessions' && cmd === 'create') {
        return new Promise((r) => {
          resolveCreate = r
        })
      }
      if (module === 'sessions' && cmd === 'list') return { sessions: [] }
      if (module === 'projects') return { projects: [] }
      return {}
    })
    const store = useSessionStore()
    store.currentId = 's-old'
    const w = await mountPanel()

    // 侧栏「新建对话」在飞（currentId 仍 s-old）
    const createPromise = store.create()
    await flushPromises()
    expect(store.currentId).toBe('s-old')

    // 往返窗口内用户开始打字（实录时序：fill 先于落地）
    await w.find('.chat-input-area textarea').setValue('你好，帮我查一下')

    // create 落定 → switchTo(s-new) → 切换 watch carryInput
    resolveCreate({ session_id: 's-new', title: '新对话' })
    await createPromise
    await flushPromises()

    // 草稿存活（未被当旧会话草稿洗掉）+ 发送按钮可用
    expect(store.currentId).toBe('s-new')
    expect(useChatStore().input).toBe('你好，帮我查一下')
    expect(
      (w.find('.chat-input-area button.btn-primary').element as HTMLButtonElement).disabled,
    ).toBe(false)
    w.unmount()
  })
})
