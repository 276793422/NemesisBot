import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount, flushPromises } from '@vue/test-utils'

// 切页恢复（2026-09-21 切标签页丢消息修复）回归钉：
// 后端 user 行已在 turn 开始时落盘，assistant 行 turn 末才落——AI 处理中
// 切走路由再切回，loadHistory 拉到的历史尾部是「悬空 user 行」。契约：
// - 悬空 user 行（非本地发送态）→ pendingTurn 占位（typing-indicator）+
//   定时重拉（L2 chat.sync 增量，append 语义不重复插行），assistant 行
//   落盘后自动出现并停轮；
// - busy=false（agent.inbox_status）连续 2 次重拉仍无回复 → 诚实停轮
//   （轮次已死，悬空 user 行保留展示，不编造回复）；
// - 本地发送态（streaming）不接管（占位由 streaming/watchdog 负责）。

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

import { onMessage, sendHistoryRequest } from '../../composables/useWebSocket'
import ChatPanel from '../ChatPanel.vue'
import { useChatStore } from '../../stores/chat'
import { useSessionStore } from '../../stores/session'

beforeEach(() => {
  setActivePinia(createPinia())
  requestMock.mockReset()
  // 默认：inbox busy（轮询持续）、sync 无新事件；个别用例覆盖。
  requestMock.mockImplementation((_mod: string, cmd: string) => {
    if (cmd === 'inbox_status') return Promise.resolve({ available: true, busy: true })
    return Promise.resolve({})
  })
  vi.mocked(sendHistoryRequest).mockClear()
  vi.mocked(onMessage).mockClear()
})

afterEach(() => {
  vi.useRealTimers()
})

function mountedHandler(): (msg: any) => void {
  return vi.mocked(onMessage).mock.calls[0][0] as any
}

/** 用挂载时登记的 request_id 喂一次 history_response 帧（驱动应用回调）。
 *  帧必须带 type/module 字段——handleWSMessage 的模块路由门（type ===
 *  'message' && module === 活动模块）之内才有 history_response 分支。 */
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

/** 挂载前的公共准备：有活跃会话（syncMissedChat 的 sid 前置）。 */
function prepareSession() {
  const sessionStore = useSessionStore()
  sessionStore.sessions = [
    { id: 's1', channel: 'web', startTime: '', lastTime: '', messageCount: 0, firstMessage: 'A', model: '' },
  ] as any
  sessionStore.currentId = 's1'
}

/** 统计 requestMock 收到的 chat.sync 调用次数（含 primeSeqBaseline 的
 *  基线拉取——断言「轮询是否发生」时用 feed 前后差值，基线那次计入基数）。 */
function countSyncCalls(): number {
  return requestMock.mock.calls.filter((c: any[]) => c[1] === 'sync').length
}

/** 挂载 + 喂首个历史响应，返回 sync 调用基数（含 prime 基线那次）。 */
async function mountAndFeed(messages: any[]): Promise<number> {
  const w = mount(ChatPanel)
  await flushPromises()
  feedHistory(lastRequestId(), messages)
  await flushPromises()
  return countSyncCalls()
}

describe('切页恢复：悬空 user 行的「处理中」占位与轮询', () => {
  it('历史尾部悬空 user → 占位出现 + 4s 轮询（chat.sync 增量）', async () => {
    prepareSession()
    vi.useFakeTimers()
    const w = mount(ChatPanel)
    await flushPromises()
    expect(sendHistoryRequest).toHaveBeenCalledTimes(1)

    // 模拟切回时刻：AI 仍在处理，历史只有 user 行。
    const syncBase = countSyncCalls()
    feedHistory(lastRequestId(), [{ role: 'user', content: 'help me build' }])
    await flushPromises()

    // 占位（typing-indicator）出现，user 行在列表里。
    expect(w.find('.typing-indicator').exists()).toBe(true)
    expect(w.text()).toContain('help me build')

    // 轮询：4s 后自动重拉（chat.sync 增量通道，不是 prepend 翻页）。
    await vi.advanceTimersByTimeAsync(4000)
    expect(countSyncCalls()).toBeGreaterThan(syncBase)
    w.unmount()
  })

  it('assistant 行落盘（sync 增量返回）→ 占位消失 + 轮询停止', async () => {
    prepareSession()
    // sync 第 1 次（primeSeqBaseline 基线）返回空；之后（轮询 tick）返回
    // assistant 事件（AI 完成，行已落盘）。
    let syncN = 0
    requestMock.mockImplementation((_mod: string, cmd: string) => {
      if (cmd === 'inbox_status') return Promise.resolve({ available: true, busy: true })
      if (cmd === 'sync') {
        syncN += 1
        if (syncN === 1) return Promise.resolve({})
        return Promise.resolve({
          events: [{ role: 'assistant', content: 'done!', seq: 5, ts: '2026-09-21T00:00:00Z' }],
        })
      }
      return Promise.resolve({})
    })
    vi.useFakeTimers()
    const w = mount(ChatPanel)
    await flushPromises()

    feedHistory(lastRequestId(), [{ role: 'user', content: 'help me build' }])
    await flushPromises()
    expect(w.find('.typing-indicator').exists()).toBe(true)

    // 下一个轮询 tick：sync 增量补到 assistant 行 → 占位消失。
    await vi.advanceTimersByTimeAsync(4000)
    await flushPromises()
    expect(w.find('.typing-indicator').exists()).toBe(false)
    expect(w.text()).toContain('done!')

    // 轮询已停：再推进两个周期 sync 不再被调用。
    const syncCalls = countSyncCalls()
    await vi.advanceTimersByTimeAsync(8000)
    expect(countSyncCalls()).toBe(syncCalls)
    w.unmount()
  })

  it('历史尾部是 assistant（正常完整历史）→ 不置占位不轮询', async () => {
    prepareSession()
    vi.useFakeTimers()
    const w = mount(ChatPanel)
    await flushPromises()

    feedHistory(lastRequestId(), [
      { role: 'user', content: 'q' },
      { role: 'assistant', content: 'a' },
    ])
    await flushPromises()
    expect(w.find('.typing-indicator').exists()).toBe(false)

    // 基线在 feed 后取（primeSeqBaseline 的基线 sync 在 feed 时同步触发）。
    const syncBase = countSyncCalls()
    await vi.advanceTimersByTimeAsync(12000)
    expect(countSyncCalls()).toBe(syncBase)
    w.unmount()
  })

  it('busy=false 连续 2 次重拉仍无回复 → 诚实停轮（悬空 user 行保留展示）', async () => {
    prepareSession()
    // 轮次已死（agent 重启/异常）：inbox busy=false。
    requestMock.mockImplementation((_mod: string, cmd: string) => {
      if (cmd === 'inbox_status') return Promise.resolve({ available: true, busy: false })
      return Promise.resolve({})
    })
    vi.useFakeTimers()
    const w = mount(ChatPanel)
    await flushPromises()

    const syncBase = countSyncCalls()
    feedHistory(lastRequestId(), [{ role: 'user', content: 'lost turn' }])
    await flushPromises()
    expect(w.find('.typing-indicator').exists()).toBe(true)

    // 第 1 次轮询：busy=false，死轮计数 1，继续。
    await vi.advanceTimersByTimeAsync(4000)
    await flushPromises()
    expect(w.find('.typing-indicator').exists()).toBe(true)
    // 第 2 次：计数 2 达上限 → 停轮，占位消失，悬空 user 行保留。
    await vi.advanceTimersByTimeAsync(4000)
    await flushPromises()
    expect(w.find('.typing-indicator').exists()).toBe(false)
    expect(w.text()).toContain('lost turn')
    // 不再轮询。
    const afterStop = countSyncCalls()
    await vi.advanceTimersByTimeAsync(8000)
    expect(countSyncCalls()).toBe(afterStop)
    expect(afterStop).toBeGreaterThan(syncBase)
    w.unmount()
  })

  it('本地发送态（streaming）不接管：悬空 user 响应不触发轮询', async () => {
    prepareSession()
    vi.useFakeTimers()
    const w = mount(ChatPanel)
    await flushPromises()

    // 用户刚本地发送：streaming=true（占位由 streaming 渲染 + watchdog 兜底）。
    const chat = useChatStore()
    chat.streaming = true

    feedHistory(lastRequestId(), [{ role: 'user', content: 'local send' }])
    await flushPromises()
    // 占位存在（streaming 驱动），但轮询未启动——streaming 结束路径不归本机制。
    expect(w.find('.typing-indicator').exists()).toBe(true)

    // 基线在 feed 后取（primeSeqBaseline 的基线 sync 在 feed 时同步触发）。
    const syncBase = countSyncCalls()
    await vi.advanceTimersByTimeAsync(12000)
    // 轮询未启动：无新增 chat.sync（detect 拒绝在 streaming 态启动）。
    expect(countSyncCalls()).toBe(syncBase)
    w.unmount()
  })
})
