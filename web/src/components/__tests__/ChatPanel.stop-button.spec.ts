import { describe, it, expect, vi, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount, flushPromises } from '@vue/test-utils'

// 2026-10-31 停止按钮 BUG（项目会话停止失效 + 假完成 UX）前端修复回归锁：
//
// 根因①（F1）：stopGeneration 调 agent.cancel 不带 session_id → 后端恒取
// 主 loop cancel_all_sessions；项目会话跑在独立 AgentLoop（L6++），取消
// 令牌表各自一份 → 项目 loop 永远收不到取消 = 停止按钮对项目会话完全失效
// （任务跑到自然结束）。修复：带 session_id（后端走 resolve_session_loop
// 单一裁决点，主/项目两态都正确路由）。
//
// 根因②（F2）：cancelled>0 受理后前端立即假完成（清 busy + 加「已停止
// 生成」行）而后端还要跑到检查点（LLM 中断/当前工具跑完/并行批批尾）——
// UI 状态还在动、回复还在蹦，与「已停止」宣告割裂。修复：受理后进入
// 「停止中」诚实过渡态（按钮置灰防重复点击），收尾交给后端「已取消」
// assistant 帧 / error 帧沿既有清账路径；20s 兜底强制复位（防收尾帧丢失）。
//
// W5（cancelled=0）语义保持：复位不加假行。

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
import { useSessionStore } from '../../stores/session'

beforeEach(() => {
  setActivePinia(createPinia())
  requestMock.mockReset()
  requestMock.mockImplementation((_mod: string, _cmd: string) => Promise.resolve({}))
  vi.mocked(onMessage).mockClear()
})

function mountedHandler(): (msg: any) => void {
  return vi.mocked(onMessage).mock.calls[0][0] as any
}

function prepareSession() {
  const sessionStore = useSessionStore()
  sessionStore.sessions = [
    { id: 's1', channel: 'web', startTime: '', lastTime: '', messageCount: 0, firstMessage: 'A', model: '' },
  ] as any
  sessionStore.currentId = 's1'
}

function cancelCalls(): any[][] {
  return requestMock.mock.calls.filter((c: any[]) => c[0] === 'agent' && c[1] === 'cancel')
}

async function mountWithBusyTurn(cancelResult: any) {
  const chatStore = useChatStore()
  const w = mount(ChatPanel)
  await flushPromises()
  prepareSession()
  // 在飞轮次：busy=true → 停止按钮出现。
  chatStore.setBusy('s1', true)
  await flushPromises()
  requestMock.mockImplementation((_mod: string, cmd: string) => {
    if (cmd === 'cancel') return Promise.resolve(cancelResult)
    return Promise.resolve({})
  })
  return { w, chatStore }
}

describe('停止按钮（2026-10-31 BUG 修复回归）', () => {
  it('F1：点击停止 → agent.cancel 携带当前会话 session_id', async () => {
    const { w } = await mountWithBusyTurn({ cancelled: 1 })
    expect(w.find('.btn-stop').exists()).toBe(true)
    await w.find('.btn-stop').trigger('click')
    await flushPromises()
    const calls = cancelCalls()
    expect(calls.length).toBe(1)
    expect(calls[0][2]).toEqual({ session_id: 's1' })
    w.unmount()
  })

  it('F2：cancelled>0 → 诚实过渡态：busy 保持、不加「已停止生成」行、按钮置灰', async () => {
    const { w, chatStore } = await mountWithBusyTurn({ cancelled: 1 })
    await w.find('.btn-stop').trigger('click')
    await flushPromises()
    // 不假完成：busy 仍在（收尾交给后端帧）。
    expect(chatStore.isBusy('s1')).toBe(true)
    // 不加假行。
    expect(chatStore.messages.some((m: any) => m.content === '已停止生成')).toBe(false)
    // 按钮置灰防重复点击。
    expect(w.find('.btn-stop').attributes('disabled')).toBeDefined()
    w.unmount()
  })

  it('F2 收尾：后端「已取消」assistant 帧到达 → busy 清、过渡态摘除', async () => {
    const { w, chatStore } = await mountWithBusyTurn({ cancelled: 1 })
    await w.find('.btn-stop').trigger('click')
    await flushPromises()
    expect(chatStore.isBusy('s1')).toBe(true)
    // 后端取消检查点命中 →「已取消」assistant 帧回面板。
    mountedHandler()({
      type: 'message',
      module: 'chat',
      cmd: 'receive',
      data: { session_id: 's1', role: 'assistant', content: '已取消', timestamp: new Date().toISOString() },
    })
    await flushPromises()
    expect(chatStore.isBusy('s1')).toBe(false)
    expect(chatStore.messages.some((m: any) => m.content === '已取消')).toBe(true)
    w.unmount()
  })

  it('W5：cancelled=0（后端无在跑轮次）→ 立即复位 busy，不加假行', async () => {
    const { w, chatStore } = await mountWithBusyTurn({ cancelled: 0 })
    await w.find('.btn-stop').trigger('click')
    await flushPromises()
    expect(chatStore.isBusy('s1')).toBe(false)
    expect(chatStore.messages.some((m: any) => m.content === '已停止生成')).toBe(false)
    w.unmount()
  })

  it('F2 兜底：受理成功但收尾帧丢失 → 20s 强制复位', async () => {
    vi.useFakeTimers()
    const { w, chatStore } = await mountWithBusyTurn({ cancelled: 1 })
    await w.find('.btn-stop').trigger('click')
    await flushPromises()
    expect(chatStore.isBusy('s1')).toBe(true)
    // 收尾帧永远不来 → 兜底定时器强制复位（不加假行）。
    await vi.advanceTimersByTimeAsync(20000)
    await flushPromises()
    expect(chatStore.isBusy('s1')).toBe(false)
    expect(chatStore.messages.some((m: any) => m.content === '已停止生成')).toBe(false)
    w.unmount()
  })

  it('F2 兜底不误伤：点停止后切走会话 → 20s 到期只清被取消的会话', async () => {
    // 自查发现并修复的闭包捕获回归锁：timer/回调闭包引用执行时刻的
    // effectiveSid.value 会误清切过去的会话；捕获 sid 后兜底精确指向被
    // 取消的那个会话。
    vi.useFakeTimers()
    const sessionStore = useSessionStore()
    sessionStore.sessions = [
      { id: 's1', channel: 'web', startTime: '', lastTime: '', messageCount: 0, firstMessage: 'A', model: '' },
      { id: 's2', channel: 'web', startTime: '', lastTime: '', messageCount: 0, firstMessage: 'B', model: '' },
    ] as any
    sessionStore.currentId = 's1'
    const chatStore = useChatStore()
    const w = mount(ChatPanel)
    await flushPromises()
    chatStore.setBusy('s1', true)
    chatStore.setBusy('s2', true)
    await flushPromises()
    requestMock.mockImplementation((_mod: string, cmd: string) => {
      if (cmd === 'cancel') return Promise.resolve({ cancelled: 1 })
      return Promise.resolve({})
    })
    await w.find('.btn-stop').trigger('click')
    await flushPromises()
    // 切到 s2（视图跟随 s2 的 busy 投影）。
    sessionStore.currentId = 's2'
    await flushPromises()
    await vi.advanceTimersByTimeAsync(20000)
    await flushPromises()
    // s2 的 busy 不被误清；s1（被取消会话）被兜底复位。
    expect(chatStore.isBusy('s2')).toBe(true)
    expect(chatStore.isBusy('s1')).toBe(false)
    w.unmount()
  })

  it('F2 过渡态不跨会话污染：A 停止中切到 B（B busy）→ B 按钮不置灰可点', async () => {
    // 三轮自查回归锁：stopping 是会话归属的（stoppingSid），A 的「停止中」
    // 过渡态不污染 B 自己的在跑任务——切到 B 后按钮必须可点（否则 B 的
    // 停止操作被 A 的过渡态锁死）。
    vi.useFakeTimers()
    const sessionStore = useSessionStore()
    sessionStore.sessions = [
      { id: 's1', channel: 'web', startTime: '', lastTime: '', messageCount: 0, firstMessage: 'A', model: '' },
      { id: 's2', channel: 'web', startTime: '', lastTime: '', messageCount: 0, firstMessage: 'B', model: '' },
    ] as any
    sessionStore.currentId = 's1'
    const chatStore = useChatStore()
    const w = mount(ChatPanel)
    await flushPromises()
    chatStore.setBusy('s1', true)
    chatStore.setBusy('s2', true)
    await flushPromises()
    requestMock.mockImplementation((_mod: string, cmd: string) => {
      if (cmd === 'cancel') return Promise.resolve({ cancelled: 1 })
      return Promise.resolve({})
    })
    // A 视图点停止 → A 按钮置灰。
    await w.find('.btn-stop').trigger('click')
    await flushPromises()
    expect(w.find('.btn-stop').attributes('disabled')).toBeDefined()
    // 切到 B（B busy → 按钮在）→ B 按钮不置灰（A 的过渡态不跨会话）。
    sessionStore.currentId = 's2'
    await flushPromises()
    expect(w.find('.btn-stop').exists()).toBe(true)
    expect(w.find('.btn-stop').attributes('disabled')).toBeUndefined()
    // 切回 A → A 按钮仍置灰（过渡态还挂着）。
    sessionStore.currentId = 's1'
    await flushPromises()
    expect(w.find('.btn-stop').attributes('disabled')).toBeDefined()
    w.unmount()
  })
})
