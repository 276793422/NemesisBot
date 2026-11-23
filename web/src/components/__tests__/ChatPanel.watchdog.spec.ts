import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount, flushPromises } from '@vue/test-utils'

// 2026-09-21 watchdog 三修复回归钉（真机 BUG：exec 审批挂起 45s → watchdog
// 误触发恢复 → 窗口错位误判「回复已落」→ 磁盘 50 条重灌视图，工具卡/占位
// 全冲掉，n=22→50）：
// - 修复①：onWatchdog 触发时先看轮次活跃迹象（审批挂起 pendingApprovals /
//   本轮工具事件未 flush）→ 只续期不重灌；活跃续期 12 次上限后走原恢复路径。
// - 修复②：「回复已落」判定语义化——磁盘历史尾部向前，本轮发送文本的
//   user 行之前出现 assistant 行才算已落。原计数式判定（磁盘 50 条 vs 发送
//   时视图 ~20 条的 assistant 数）窗口错位，磁盘计数天然偏大，轮次还在跑
//   也被判已落 → 误重灌。
// - 修复③：重灌只有文本行——replayToolsFromRing 从环回放挂回最后一轮
//   工具卡（primeSeqBaseline 同语义提炼共用）。

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

vi.mock('../../composables/useSSE', () => {
  const handlers: Record<string, any[]> = {}
  ;(globalThis as any).__sseHandlers = handlers
  return {
    on: (t: string, h: any) => {
      ;(handlers[t] ??= []).push(h)
    },
    off: (t: string, h: any) => {
      handlers[t] = (handlers[t] ?? []).filter((x: any) => x !== h)
    },
  }
})

import { onMessage, sendHistoryRequest, send as wsSend } from '../../composables/useWebSocket'
import ChatPanel from '../ChatPanel.vue'
import { useChatStore } from '../../stores/chat'
import { useSessionStore } from '../../stores/session'
import {
  useApprovals,
  _resetApprovalsForTest,
} from '../../composables/useApprovals'

function mountedHandler(): (msg: any) => void {
  return vi.mocked(onMessage).mock.calls[0][0] as any
}

function feedHistory(requestId: string, messages: any[]) {
  mountedHandler()({
    type: 'message',
    module: 'chat',
    cmd: 'history_response',
    data: { request_id: requestId, messages, has_more: false, oldest_index: 0 },
  })
}

function lastRequestId(): string {
  const calls = vi.mocked(sendHistoryRequest).mock.calls
  return calls[calls.length - 1][0] as string
}

function countHistoryRequests(): number {
  return vi.mocked(sendHistoryRequest).mock.calls.length
}

function prepareSession() {
  const sessionStore = useSessionStore()
  sessionStore.sessions = [
    { id: 's1', channel: 'web', startTime: '', lastTime: '', messageCount: 0, firstMessage: 'A', model: '' },
  ] as any
  sessionStore.currentId = 's1'
}

/** 挂载 + 喂入基线历史（q1/a1 一轮完整）→ 返回 wrapper。 */
async function mountWithHistory() {
  const w = mount(ChatPanel)
  await flushPromises()
  feedHistory(lastRequestId(), [
    { role: 'user', content: 'q1' },
    { role: 'assistant', content: 'a1' },
  ])
  await flushPromises()
  return w
}

/** 走真实 sendMessage 路径武装 watchdog（startWatchdog 记录本轮文本锚）。
 * 输入区 Ctrl+Enter——比按钮 click 稳（面板多弹层下 btn-primary 的 click
 * 在 jsdom 里不总是触达 handler）。 */
async function sendFromUI(w: ReturnType<typeof mount>, content: string) {
  const chat = useChatStore()
  const ta = w.find('textarea')
  await ta.setValue(content)
  await ta.trigger('keydown', { key: 'Enter', ctrlKey: true })
  await flushPromises()
  expect(chat.streaming, '发送后进入流式态').toBe(true)
}

/** 模拟 CRITICAL 审批挂起（SSE approval-requested 的等价本地态）。 */
function hangApproval(requestId = 'appr-1') {
  const { pendingApprovals } = useApprovals()
  pendingApprovals.push({
    request_id: requestId,
    operation: 'process_exec',
    target: 'echo x',
    risk_level: 'CRITICAL',
    reason: 'test',
    timeout_secs: 300,
    expiresAt: Date.now() + 300_000,
  } as any)
}

beforeEach(() => {
  setActivePinia(createPinia())
  requestMock.mockReset()
  requestMock.mockImplementation((_mod: string, cmd: string) => {
    if (cmd === 'inbox_status') return Promise.resolve({ available: true, busy: true })
    return Promise.resolve({})
  })
  vi.mocked(sendHistoryRequest).mockClear()
  vi.mocked(wsSend).mockClear()
  vi.mocked(onMessage).mockClear()
  _resetApprovalsForTest()
})

afterEach(() => {
  _resetApprovalsForTest()
  vi.useRealTimers()
})

describe('修复①：审批挂起 / 工具未收尾 → watchdog 续期不重灌', () => {
  it('审批挂起中 45s 触发 → 不发恢复请求、streaming/视图保持', async () => {
    prepareSession()
    vi.useFakeTimers()
    const w = await mountWithHistory()
    const chat = useChatStore()
    await sendFromUI(w, 'run echo V1')
    expect(chat.streaming).toBe(true)
    hangApproval()

    const before = countHistoryRequests()
    const msgCount = chat.messages.length
    await vi.advanceTimersByTimeAsync(45_000)
    await flushPromises()

    // 未发 reloadLatest（历史请求数不变）、流式态与本地视图原样。
    expect(countHistoryRequests()).toBe(before)
    expect(chat.streaming).toBe(true)
    expect(chat.messages.length).toBe(msgCount)
    w.unmount()
  })

  it('本轮工具事件未 flush（回复未到）→ 同样续期', async () => {
    prepareSession()
    vi.useFakeTimers()
    const w = await mountWithHistory()
    const chat = useChatStore()
    await sendFromUI(w, 'run echo V2')
    // 工具开始未结束（exec 执行中 / 审批中 SDK 不发 Finished 的残留同理）。
    mountedHandler()({
      type: 'push',
      cmd: 'tool_event',
      data: { kind: 'ToolStarted', seq: 9, data: { call_id: 'c1', tool: 'exec', session_id: 's1' } },
    })
    await flushPromises()

    const before = countHistoryRequests()
    await vi.advanceTimersByTimeAsync(45_000)
    await flushPromises()
    expect(countHistoryRequests()).toBe(before)
    expect(chat.streaming).toBe(true)
    // 工具还在 live work 块（未被重灌冲掉）。
    expect(w.find('.wt-block.live').exists()).toBe(true)
    w.unmount()
  })

  it('活跃续期有上限：12 次后走原恢复路径（reloadLatest 发出）', async () => {
    prepareSession()
    vi.useFakeTimers()
    const w = await mountWithHistory()
    await sendFromUI(w, 'run echo V3')
    hangApproval()

    const before = countHistoryRequests()
    // 12 次续期内不发；第 13 个周期（12*45s 之后）走 reloadLatest。
    await vi.advanceTimersByTimeAsync(12 * 45_000)
    expect(countHistoryRequests()).toBe(before)
    await vi.advanceTimersByTimeAsync(45_000)
    await flushPromises()
    expect(countHistoryRequests()).toBe(before + 1)
    expect(lastRequestId().startsWith('watchdog_')).toBe(true)
    w.unmount()
  })

  it('无活跃迹象（纯静默）→ 45s 即走原恢复路径（既有行为不回归）', async () => {
    prepareSession()
    vi.useFakeTimers()
    const w = await mountWithHistory()
    await sendFromUI(w, 'run echo V4')

    const before = countHistoryRequests()
    await vi.advanceTimersByTimeAsync(45_000)
    await flushPromises()
    expect(countHistoryRequests()).toBe(before + 1)
    w.unmount()
  })
})

describe('修复②：「回复已落」语义化 landed 判定（替代窗口错位计数比较）', () => {
  it('磁盘窗口 assistant 计数偏大（旧轮次在前）但尾部是本轮 user → 未落：不重灌、狗重试', async () => {
    prepareSession()
    vi.useFakeTimers()
    const w = await mountWithHistory()
    const chat = useChatStore()
    await sendFromUI(w, 'run echo V5')

    await vi.advanceTimersByTimeAsync(45_000)
    await flushPromises()
    // reloadLatest 已发出（无活跃迹象），requestId 是 watchdog_ 前缀。
    expect(lastRequestId().startsWith('watchdog_')).toBe(true)

    // 磁盘 50 条：大量旧轮次 assistant（计数远超发送时视图——原计数式
    // 判定在此误判「已落」立即重灌），尾部是本轮 user 行、无回复。
    const disk: any[] = []
    for (let i = 0; i < 12; i++) disk.push({ role: 'user', content: `old q ${i}` })
    for (let i = 0; i < 12; i++) disk.push({ role: 'assistant', content: `old a ${i}` })
    disk.push({ role: 'user', content: 'run echo V5' })
    feedHistory(lastRequestId(), disk)
    await flushPromises()

    // 未落 → 视图保持（不被 50 条重灌）、streaming 保持、狗重试（再推进
    // 45s 又发一次 reloadLatest）。
    expect(chat.messages.length).toBeLessThan(disk.length)
    expect(chat.messages[chat.messages.length - 1].content).toBe('run echo V5')
    expect(chat.streaming).toBe(true)
    await vi.advanceTimersByTimeAsync(45_000)
    await flushPromises()
    expect(countHistoryRequests()).toBeGreaterThanOrEqual(2)
    w.unmount()
  })

  it('本轮 user 行之后磁盘有 assistant → 已落：重灌生效、streaming 关、回复可见', async () => {
    prepareSession()
    vi.useFakeTimers()
    const w = await mountWithHistory()
    const chat = useChatStore()
    await sendFromUI(w, 'run echo V6')

    await vi.advanceTimersByTimeAsync(45_000)
    await flushPromises()

    feedHistory(lastRequestId(), [
      { role: 'user', content: 'q1' },
      { role: 'assistant', content: 'a1' },
      { role: 'user', content: 'run echo V6' },
      { role: 'assistant', content: 'V6 输出完成' },
    ])
    await flushPromises()

    // 已落 → replaceMessages（回复出现在视图）、streaming 关、狗清。
    expect(chat.streaming).toBe(false)
    expect(w.text()).toContain('V6 输出完成')
    expect(chat.messages[chat.messages.length - 1].role).toBe('assistant')
    w.unmount()
  })
})

describe('修复③：重灌后从环回放挂回工具卡', () => {
  it('landed 重灌 → chat.sync 回放的 tool 条目挂到最后 assistant 消息', async () => {
    prepareSession()
    requestMock.mockImplementation((_mod: string, cmd: string) => {
      if (cmd === 'inbox_status') return Promise.resolve({ available: true, busy: true })
      if (cmd === 'sync') {
        return Promise.resolve({
          events: [
            { seq: 1, role: 'user', content: 'q1', ts: 'x' },
            toolEvent(2, 'ToolStarted', { call_id: 'c1', tool: 'exec', args_preview: 'echo V7' }),
            toolEvent(3, 'ToolFinished', { call_id: 'c1', ok: true, duration_ms: 80 }),
            { seq: 4, role: 'assistant', content: 'V7 done', ts: 'x' },
          ],
        })
      }
      return Promise.resolve({})
    })
    vi.useFakeTimers()
    const w = await mountWithHistory()
    const chat = useChatStore()
    await sendFromUI(w, 'run echo V7')

    await vi.advanceTimersByTimeAsync(45_000)
    await flushPromises()
    feedHistory(lastRequestId(), [
      { role: 'user', content: 'run echo V7' },
      { role: 'assistant', content: 'V7 done' },
    ])
    await flushPromises()

    // 重灌后回放挂 work 块：最后的 assistant 消息带 work 时间线。
    const last = chat.messages[chat.messages.length - 1]
    expect(last.role).toBe('assistant')
    expect(last.work?.rounds.flatMap(r => r.tools)).toHaveLength(1)
    expect(last.work?.rounds.flatMap(r => r.tools)[0].args_preview).toContain('echo V7')
    const block = w.find('.message .wt-block')
    expect(block.exists()).toBe(true)
    // 落地块默认折叠——点开验证 args_preview。
    await block.find('.wt-header').trigger('click')
    expect(w.find('.message .wt-block .wt-body').text()).toContain('echo V7')
    w.unmount()
  })
})

describe('BUG-20261120-006：重灌恢复行号锚 + 分页游标同步', () => {
  it('landed 重灌后整批按 oldest_index 推导 rowIndex，后续 live 行继续编号', async () => {
    prepareSession()
    vi.useFakeTimers()
    const w = await mountWithHistory()
    const chat = useChatStore()
    await sendFromUI(w, 'run echo A1')

    await vi.advanceTimersByTimeAsync(45_000)
    await flushPromises()
    // 磁盘窗口 4 行，oldest_index=42（raw 行号坐标——分页契约，非 0 起）。
    mountedHandler()({
      type: 'message',
      module: 'chat',
      cmd: 'history_response',
      data: {
        request_id: lastRequestId(),
        messages: [
          { role: 'user', content: 'q1' },
          { role: 'assistant', content: 'a1' },
          { role: 'user', content: 'run echo A1' },
          { role: 'assistant', content: 'A1 输出完成' },
        ],
        has_more: true,
        oldest_index: 42,
      },
    })
    await flushPromises()

    // 行号锚恢复：整批 oldest+j 编号（修复前全 undefined → 回退/重生成/
    // 删除入口消失、fork 点击报「找不到本轮提问行」）。
    expect(chat.messages.map((m: any) => m.rowIndex)).toEqual([42, 43, 44, 45])
    // nextRowIndex 重推成功：后续 live 行继续编号（46）。
    mountedHandler()({
      type: 'message',
      module: 'chat',
      cmd: 'receive',
      data: { session_id: 's1', role: 'assistant', content: '跟进回复', timestamp: new Date().toISOString() },
    })
    await flushPromises()
    expect(chat.messages[chat.messages.length - 1].rowIndex).toBe(46)
    w.unmount()
  })

  it('重灌后滚到顶翻页用重灌游标（before=响应 oldest_index），不取视图内旧行', async () => {
    prepareSession()
    vi.useFakeTimers()
    // 首拉喂 has_more:true + oldest_index:20（>20 行会话形态：游标 20）。
    const w = mount(ChatPanel)
    await flushPromises()
    mountedHandler()({
      type: 'message',
      module: 'chat',
      cmd: 'history_response',
      data: {
        request_id: lastRequestId(),
        messages: [
          { role: 'user', content: 'q1' },
          { role: 'assistant', content: 'a1' },
        ],
        has_more: true,
        oldest_index: 20,
      },
    })
    await flushPromises()
    const chat = useChatStore()
    expect(chat.oldestIndex).toBe(20)
    await sendFromUI(w, 'run echo A2')

    await vi.advanceTimersByTimeAsync(45_000)
    await flushPromises()
    // watchdog 重灌窗口 50 行，oldest_index=4（视图真覆盖起点）。
    mountedHandler()({
      type: 'message',
      module: 'chat',
      cmd: 'history_response',
      data: {
        request_id: lastRequestId(),
        messages: [
          { role: 'user', content: 'q1' },
          { role: 'assistant', content: 'a1' },
          { role: 'user', content: 'run echo A2' },
          { role: 'assistant', content: 'A2 输出完成' },
        ],
        has_more: true,
        oldest_index: 4,
      },
    })
    await flushPromises()

    // 滚到顶触发翻页：before 必须是重灌游标 4（视图外旧行）而非陈旧的
    // 20（视图内——修复前 prependHistory 无条件前插 = 整批重复）。
    const before = vi.mocked(sendHistoryRequest).mock.calls.length
    const el = w.find('.chat-messages').element as HTMLElement
    el.scrollTop = 0
    await w.find('.chat-messages').trigger('scroll')
    await flushPromises()
    expect(vi.mocked(sendHistoryRequest).mock.calls.length).toBe(before + 1)
    const pageCall = vi.mocked(sendHistoryRequest).mock.calls[before]
    expect(pageCall[2]).toBe(4)
    expect(chat.oldestIndex).toBe(4)
    w.unmount()
  })
})

describe('W5 放弃臂 → 缺陷 C 修复改写（2026-11-23）：不杀 busy 诚实降级', () => {
  it('3 次重载均「未落」→ 放弃：busy 保留、inflight 保留、watchdog 停、视图不重灌、补拉 work_current', async () => {
    prepareSession()
    vi.useFakeTimers()
    const w = await mountWithHistory()
    const chat = useChatStore()
    await sendFromUI(w, 'run echo W5')

    // 3 个 watchdog 周期（MAX_WATCHDOG_ATTEMPTS=3），每次都喂「未落」历史
    // （尾部是本轮 user、其后无 assistant）。
    for (let round = 1; round <= 3; round++) {
      await vi.advanceTimersByTimeAsync(45_000)
      await flushPromises()
      expect(lastRequestId().startsWith('watchdog_')).toBe(true)
      const disk = [
        { role: 'user', content: 'q1' },
        { role: 'assistant', content: 'a1' },
        { role: 'user', content: 'run echo W5' },
      ]
      feedHistory(lastRequestId(), disk)
      await flushPromises()
      if (round < 3) {
        expect(chat.streaming, `第 ${round} 轮未耗尽仍在等`).toBe(true)
      }
    }

    // 放弃臂新契约（缺陷 C 修复：attempts 烧穿 ≠ turn 已死——纯 LLM 长窗
    // pendingWork=null 续期臂失明是常态触发路径，旧 dropSessionState 把活
    // turn 打成假空闲）：busy 与 B2 在飞登记原样保留（迟到回复照常经
    // assistant 帧收尾；真死 turn 用户手动停止——停止按钮仍在），视图
    // 保持本地态（磁盘确实没有新回复，不假重灌）。
    expect(chat.streaming).toBe(true)
    expect(chat.inflightTurns['s1']).toBeDefined()
    expect(chat.messages[chat.messages.length - 1].content).toBe('run echo W5')
    expect(chat.messages).toHaveLength(3)
    // GIVEUP 补拉 work_current（活 turn 过程视图回填；本测试无快照，
    // res?.work == null 反向闸自然 no-op——请求发出即可）。
    expect(requestMock).toHaveBeenCalledWith('chat', 'work_current', { session_id: 's1' })
    // watchdog 已停：再走两个周期不产生新 watchdog_ 请求（id 不变）。
    const idAfterGiveup = lastRequestId()
    await vi.advanceTimersByTimeAsync(90_000)
    await flushPromises()
    expect(lastRequestId()).toBe(idAfterGiveup)
    w.unmount()
  })

  it('stopGeneration 遇 cancelled=0（后端无在跑轮次）也复位 streaming + inflight，不加系统行', async () => {
    prepareSession()
    vi.useFakeTimers()
    const w = await mountWithHistory()
    const chat = useChatStore()
    await sendFromUI(w, 'run echo W5b')
    expect(chat.streaming).toBe(true)

    // 后端确定性回答「没有可取消的轮次」（修复前：streaming 永真）。
    requestMock.mockImplementation((_mod: string, cmd: string) => {
      if (cmd === 'cancel') return Promise.resolve({ cancelled: 0 })
      if (cmd === 'inbox_status') return Promise.resolve({ available: true, busy: true })
      return Promise.resolve({})
    })
    await w.find('button.btn-stop').trigger('click')
    await flushPromises()

    expect(chat.streaming).toBe(false)
    expect(chat.inflightTurns['s1']).toBeUndefined()
    // cancelled=0 不加「已停止生成」——后端确认无在跑轮次，加行是假话。
    expect(chat.messages.some((m) => m.content === '已停止生成')).toBe(false)
    w.unmount()
  })

  it('stopGeneration 遇 cancelled>0 走诚实过渡态，后端「已取消」帧收尾（2026-10-31 F2）', async () => {
    // 旧形态（已退役）：受理即假完成（复位 + 加「已停止生成」系统行），
    // 而后端还要跑到检查点——UI 停了、回复还在蹦，割裂。新形态：受理后
    // busy/inflight 保持（诚实反映「还在跑到检查点」），收尾交给后端
    // 「已取消」assistant 帧沿既有清账路径完成。
    prepareSession()
    vi.useFakeTimers()
    const w = await mountWithHistory()
    const chat = useChatStore()
    await sendFromUI(w, 'run echo W5c')

    requestMock.mockImplementation((_mod: string, cmd: string) => {
      if (cmd === 'cancel') return Promise.resolve({ cancelled: 1 })
      if (cmd === 'inbox_status') return Promise.resolve({ available: true, busy: true })
      return Promise.resolve({})
    })
    await w.find('button.btn-stop').trigger('click')
    await flushPromises()

    // 受理成功 ≠ 完成：保持过渡态（检查点语义：工具跑完才停）。
    expect(chat.streaming).toBe(true)
    expect(chat.inflightTurns['s1']).toBeDefined()
    expect(chat.messages.some((m) => m.content === '已停止生成')).toBe(false)
    // 按钮置灰防重复点击。
    expect(w.find('button.btn-stop').attributes('disabled')).toBeDefined()

    // 后端取消命中 →「已取消」assistant 帧回面板 → 既有清账路径收尾。
    mountedHandler()({
      type: 'message',
      module: 'chat',
      cmd: 'receive',
      data: { session_id: 's1', role: 'assistant', content: '已取消', timestamp: new Date().toISOString() },
    })
    await flushPromises()
    expect(chat.streaming).toBe(false)
    expect(chat.inflightTurns['s1']).toBeUndefined()
    expect(chat.messages.some((m) => m.content === '已取消')).toBe(true)
    w.unmount()
  })
})

/** tool 条目（chat_event_log kind="tool" 形态）。 */
function toolEvent(seq: number, kind: string, data: any) {
  return { seq, kind: 'tool', role: '', content: '', tool: { kind, data } }
}

// ---------------------------------------------------------------------------
// 缺陷家族修复回归钉（2026-11-23 插队时间线消失家族审计，A②③/B/D）
// ---------------------------------------------------------------------------

describe('缺陷 B：busy 中回声 dedupe 不排空活时间线缓冲（2026-11-23）', () => {
  it('busy 中插队回声帧命中乐观行 → dedupe 臂保留 pendingWork（修复前此处排空=瞬间消失）', async () => {
    prepareSession()
    vi.useFakeTimers()
    const w = await mountWithHistory()
    const chat = useChatStore()
    await sendFromUI(w, 'run echo B1')
    expect(chat.streaming).toBe(true)
    // 种活时间线缓冲（等价真实链路里 LLM 已产出中间轮 + 工具卡在跑）。
    chat.ingestWorkRoundText('正在分析任务…')
    chat.ingestWorkToolStart('c-b1', 'sleep', '30')
    expect(chat.pendingWork).not.toBeNull()

    // busy 中插队：后端入站咽喉点无条件回推 user 回声帧（先于 busy 裁决）
    // → 与乐观回显行同文同 role → dedupe 臂。修复后 busy 态不排空。
    mountedHandler()({
      type: 'message',
      module: 'chat',
      cmd: 'receive',
      data: { session_id: 's1', role: 'user', content: 'run echo B1', seq: 99, timestamp: new Date().toISOString() },
    })
    await flushPromises()

    expect(chat.pendingWork).not.toBeNull()
    expect(chat.pendingWork!.rounds.some(r => r.tools.some(t => t.call_id === 'c-b1'))).toBe(true)
    expect(chat.streaming).toBe(true)
    w.unmount()
  })

  it('收尾态重复帧仍排空（watchdog 恢复后迟到帧的既有语义不回归）+ 终局回复挂载照常', async () => {
    prepareSession()
    vi.useFakeTimers()
    const w = await mountWithHistory()
    const chat = useChatStore()
    await sendFromUI(w, 'run echo B2')
    chat.ingestWorkToolStart('c-b2', 'sleep', '30')

    // 终局回复 → assistant 分支 flush 挂载（缓冲收走，正对照）。
    mountedHandler()({
      type: 'message',
      module: 'chat',
      cmd: 'receive',
      data: { session_id: 's1', role: 'assistant', content: '完成', timestamp: new Date().toISOString() },
    })
    await flushPromises()
    expect(chat.streaming).toBe(false)
    expect(chat.pendingWork).toBeNull()
    const replyRow = chat.messages[chat.messages.length - 1]
    expect(replyRow.content).toBe('完成')
    expect(replyRow.work?.rounds.some(t => t.tools.some(x => x.call_id === 'c-b2'))).toBe(true)

    // 收尾态（streaming=false）再种缓冲 + 迟到重复 assistant 帧 → dedupe
    // 臂照常排空（R263 语义保留）。
    chat.ingestWorkToolStart('c-b2-late', 'sleep', '5')
    expect(chat.pendingWork).not.toBeNull()
    mountedHandler()({
      type: 'message',
      module: 'chat',
      cmd: 'receive',
      data: { session_id: 's1', role: 'assistant', content: '完成', seq: 555, timestamp: new Date().toISOString() },
    })
    await flushPromises()
    expect(chat.pendingWork).toBeNull()
    w.unmount()
  })
})

describe('缺陷 A：busy 注入不重置 watchdog 锚 + landed 扫描锚必须在场（2026-11-23）', () => {
  it('busy 中插队发送不换锚：锚保持首轮原文（在盘）→ 磁盘无回复判未落 → 不重灌续查（修复前换锚=锚缺→撞旧 assistant 误重灌）', async () => {
    prepareSession()
    vi.useFakeTimers()
    const w = await mountWithHistory()
    const chat = useChatStore()
    await sendFromUI(w, 'run echo A1')
    // busy 中插队（走真实 sendMessage 路由：busy 注入合法）。
    const ta = w.find('textarea')
    await ta.setValue('!紧急插队')
    await ta.trigger('keydown', { key: 'Enter', ctrlKey: true })
    await flushPromises()

    // 45s watchdog 触发（纯静默无活跃迹象——pendingWork=null）。
    await vi.advanceTimersByTimeAsync(45_000)
    await flushPromises()
    expect(lastRequestId().startsWith('watchdog_')).toBe(true)

    // 磁盘：首轮锚行在、无本轮回复（插队原文也永不落盘）。修复后锚保持
    // 'run echo A1' → 扫描命中锚行 → 未落 → 不重灌、狗重试。
    feedHistory(lastRequestId(), [
      { role: 'user', content: 'q1' },
      { role: 'assistant', content: 'a1' },
      { role: 'user', content: 'run echo A1' },
    ])
    await flushPromises()
    expect(chat.messages).toHaveLength(3)
    expect(chat.messages[chat.messages.length - 1].content).toBe('run echo A1')
    expect(chat.streaming).toBe(true)
    await vi.advanceTimersByTimeAsync(45_000)
    await flushPromises()
    expect(countHistoryRequests()).toBeGreaterThanOrEqual(2)
    w.unmount()
  })

  it('锚行整个缺席磁盘窗口（截断/压缩角）→ 保守判未落，绝不拿旧 assistant 冒充本轮回复', async () => {
    prepareSession()
    vi.useFakeTimers()
    const w = await mountWithHistory()
    const chat = useChatStore()
    await sendFromUI(w, 'run echo A2')

    await vi.advanceTimersByTimeAsync(45_000)
    await flushPromises()
    expect(lastRequestId().startsWith('watchdog_')).toBe(true)
    // 磁盘窗口里锚行缺失（老轮 assistant 在尾部上方）——修复前 landed
    // 无锚守卫 → 误判已落 → WIPE。修复后保守未落。
    feedHistory(lastRequestId(), [
      { role: 'user', content: 'q1' },
      { role: 'assistant', content: 'a1' },
      { role: 'user', content: '与锚无关的行' },
    ])
    await flushPromises()
    expect(chat.messages).toHaveLength(3)
    expect(chat.streaming).toBe(true)
    await vi.advanceTimersByTimeAsync(45_000)
    await flushPromises()
    expect(countHistoryRequests()).toBeGreaterThanOrEqual(2)
    w.unmount()
  })
})

describe('缺陷 D：busy 中 resync（subagent_reply mid-turn）保留活时间线不杀 busy（2026-11-23）', () => {
  it('busy 中 subagent_reply push → resync replace 走 keepLiveState：回复行进视图、pendingWork/busy 原样', async () => {
    prepareSession()
    vi.useFakeTimers()
    const w = await mountWithHistory()
    const chat = useChatStore()
    await sendFromUI(w, 'run echo D1')
    expect(chat.streaming).toBe(true)
    chat.ingestWorkToolStart('c-d1', 'sleep', '30')
    expect(chat.pendingWork).not.toBeNull()

    // fire-and-forget 委派的子代理回复落盘 → 定向 push → resyncFromLog。
    mountedHandler()({
      type: 'push',
      cmd: 'subagent_reply',
      data: { session_id: 's1' },
    })
    await flushPromises()
    expect(lastRequestId().startsWith('resync_')).toBe(true)

    // 磁盘快照：含子代理回复行（无本轮终局回复）。
    feedHistory(lastRequestId(), [
      { role: 'user', content: 'q1' },
      { role: 'assistant', content: 'a1' },
      { role: 'user', content: 'run echo D1' },
      { role: 'assistant', content: '子代理回复' },
    ])
    await flushPromises()

    // 行照常替换（子代理回复行进视图）；活时间线与 busy 原样保留
    // （修复前：整表 replace 清缓冲 = 瞬间消失 + setBusy(false) 杀 busy）。
    expect(chat.messages).toHaveLength(4)
    expect(chat.messages[3].content).toBe('子代理回复')
    expect(chat.pendingWork).not.toBeNull()
    expect(chat.pendingWork!.rounds.some(r => r.tools.some(t => t.call_id === 'c-d1'))).toBe(true)
    expect(chat.streaming).toBe(true)
    w.unmount()
  })

  it('空闲态 resync（rewind 典型）行为不变：replace + busy 收尾语义保留', async () => {
    prepareSession()
    vi.useFakeTimers()
    const w = await mountWithHistory()
    const chat = useChatStore()
    // 空闲态 push（无在跑轮）。
    mountedHandler()({
      type: 'push',
      cmd: 'subagent_reply',
      data: { session_id: 's1' },
    })
    await flushPromises()
    feedHistory(lastRequestId(), [
      { role: 'user', content: 'q1' },
      { role: 'assistant', content: 'a1' },
      { role: 'assistant', content: '子代理回复' },
    ])
    await flushPromises()
    expect(chat.messages).toHaveLength(3)
    expect(chat.streaming).toBe(false)
    w.unmount()
  })
})
