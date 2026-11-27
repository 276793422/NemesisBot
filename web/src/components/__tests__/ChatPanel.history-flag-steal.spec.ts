import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount, flushPromises } from '@vue/test-utils'

// BUG-20261127-019 回归钉：handleHistoryResponse 的两个分支旗标
// （pendingResync / pendingWatchdogReload）此前对所有过围栏响应无差别消费
// ——围栏只按 request_id 匹配在飞登记，跨 kind 的并行响应匹配的是自己的
// 登记，照样能冒领旗标走别人的分支。
//
// 可达性：服务端 WSAPI 每消息 tokio::spawn 并发派发（websocket_handler.rs
// dispatch），响应顺序不保证与请求一致；翻页读与 rewind 的 session jsonl
// 重写同会话锁争用即可让翻页响应晚于后发的 resync/watchdog 响应落地。
//
// 冒领后果（修复前形态，本 spec 反向钉死）：
// - 冒领 pendingResync：最旧翻页窗口整表 replace 视图（读位丢失+窗口闪跳
//   +分页游标错位），真 resync 响应反落合并路径（'ancient' 残留尾部）；
// - 冒领 pendingWatchdogReload：翻页旧窗走 landed 判定，窗口内恰含同文
//   user 行及其旧回复（重发场景）时 landed+anchor 双真 → mid-turn
//   replaceMessages + setBusy(false) + clearWatchdog——活 turn 视图被打乱、
//   busy 指示被杀、恢复检查被取消，真 watchdog 响应反落合并路径。
//
// 修复语义（本 spec 钉死）：旗标消费按 inflight.kind 门控——跨 kind 响应
// 按自身 kind 走翻页前插路径，旗标留给真响应；双 resync 并发孤儿兑现
// （BUG-20261127-011）同为 resync kind，语义不变。

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

import { onMessage, sendHistoryRequest } from '../../composables/useWebSocket'
import ChatPanel from '../ChatPanel.vue'
import { useChatStore } from '../../stores/chat'
import { useSessionStore } from '../../stores/session'

beforeEach(() => {
  setActivePinia(createPinia())
  requestMock.mockReset()
  requestMock.mockResolvedValue({})
  vi.mocked(sendHistoryRequest).mockClear()
  vi.mocked(onMessage).mockClear()
})

afterEach(() => {
  vi.useRealTimers()
})

function handler(): (msg: any) => void {
  return vi.mocked(onMessage).mock.calls.at(-1)![0] as any
}

function feedHistory(
  requestId: string,
  messages: any[],
  opts?: { oldestIndex?: number; hasMore?: boolean },
) {
  handler()({
    type: 'message',
    module: 'chat',
    cmd: 'history_response',
    data: {
      request_id: requestId,
      session_id: 's1',
      has_more: opts?.hasMore ?? false,
      oldest_index: opts?.oldestIndex ?? 0,
      last_seq: 100,
      messages,
    },
  })
}

function lastRequestId(): string {
  const calls = vi.mocked(sendHistoryRequest).mock.calls
  return calls[calls.length - 1][0] as string
}

/** 挂载 + 首拉全量历史（has_more=true → oldestIndex 非空 = 可翻页）。 */
async function mountWithPaginationBase() {
  const s = useSessionStore()
  s.sessions = [
    { id: 's1', channel: 'web', startTime: '', lastTime: '', messageCount: 0, firstMessage: 'A', model: '' },
  ] as any
  s.currentId = 's1'
  const w = mount(ChatPanel)
  await flushPromises()
  feedHistory(
    lastRequestId(),
    [
      { role: 'user', content: 'q1' },
      { role: 'assistant', content: 'a1' },
    ],
    { oldestIndex: 20, hasMore: true },
  )
  await flushPromises()
  expect(useChatStore().messages.map((m: any) => m.content)).toEqual(['q1', 'a1'])
  return w
}

/** 滚到顶触发翻页请求（在飞窗口 = 旗标置位竞态的第一要素）。 */
async function triggerPagination(w: any): Promise<string> {
  const before = vi.mocked(sendHistoryRequest).mock.calls.length
  const el = w.find('.chat-messages').element as HTMLElement
  el.scrollTop = 0
  await w.find('.chat-messages').trigger('scroll')
  await flushPromises()
  expect(vi.mocked(sendHistoryRequest).mock.calls.length).toBe(before + 1)
  return lastRequestId()
}

/** 「⋯」菜单经 Teleport to body → 须 document.body 找菜单项（regen
 *  spec 同款辅助）。 */
async function clickMsgMenuItem(w: any, role: 'user' | 'assistant', itemText: string) {
  const row = w.findAll('.message.' + role).at(-1)
  expect(row, role + ' 消息行应存在').toBeTruthy()
  await row!.find('.msg-menu-trigger').trigger('click')
  await flushPromises()
  const item = Array.from(document.querySelectorAll('.msg-menu-item')).find(b =>
    b.textContent?.includes(itemText),
  )
  expect(item, '「⋯」菜单应含「' + itemText + '」项').toBeTruthy()
  ;(item as HTMLElement).dispatchEvent(new MouseEvent('click', { bubbles: true }))
  await flushPromises()
}

describe('翻页响应不冒领 pendingResync（BUG-20261127-019）', () => {
  it('翻页响应在 resync 旗标置位后到达：走翻页前插路径，旗标留给真 resync 响应', async () => {
    const w = await mountWithPaginationBase()
    const chat = useChatStore()

    // 翻页请求在飞。
    const ridPage = await triggerPagination(w)
    expect(ridPage.startsWith('hist_')).toBe(true)

    // 翻页在飞窗口内触发 rewind → resyncFromLog（pendingResync=true，
    // resync 请求后发——真实乱序窗口：翻页响应此刻仍在途）。
    await clickMsgMenuItem(w, 'user', '撤销此问')
    const ridRes = lastRequestId()
    expect(ridRes.startsWith('resync_')).toBe(true)

    // 翻页响应先到（乱序竞态的达成形态）。
    feedHistory(ridPage, [{ role: 'user', content: 'ancient' }], { oldestIndex: 0, hasMore: true })
    await flushPromises()

    // 修复断言①：翻页响应按自身 kind 走前插——不得冒领旗标拿翻页窗口
    // 整表 replace（修复前：视图只剩 ['ancient']，q1/a1 被 replace 冲掉）。
    expect(chat.messages.map((m: any) => m.content)).toEqual(['ancient', 'q1', 'a1'])

    // 真 resync 响应随后到达：消费旗标走 replace 分支。
    feedHistory(
      ridRes,
      [
        { role: 'user', content: 'q1' },
        { role: 'assistant', content: 'a1' },
        { role: 'assistant', content: 'root-note' },
      ],
      { oldestIndex: 0, hasMore: false },
    )
    await flushPromises()

    // 修复断言②：resync replace 生效（旗标未被翻页响应提前消费——修复前
    // 此响应落合并路径零认领前置，'ancient' 残留视图尾部）。
    expect(chat.messages.map((m: any) => m.content)).toEqual(['q1', 'a1', 'root-note'])
    w.unmount()
  })
})

describe('翻页响应不冒领 pendingWatchdogReload（BUG-20261127-019）', () => {
  it('翻页响应在 watchdog 旗标置位后到达：走翻页前插，busy/视图不被旧窗 replace', async () => {
    vi.useFakeTimers()
    const w = await mountWithPaginationBase()
    const chat = useChatStore()

    // 真实发送武装 watchdog（锚文本 'go' 记入 watchdogSentContent）。
    const ta = w.find('textarea')
    await ta.setValue('go')
    await ta.trigger('keydown', { key: 'Enter', ctrlKey: true })
    await flushPromises()
    expect(chat.streaming, '发送后进入流式态').toBe(true)

    // 45s watchdog 到点：无审批挂起/工具未收尾迹象 → 恢复路径 reloadLatest
    // （pendingWatchdogReload=true，watchdog 请求在飞）。
    // ⚠ 用例顺序说明：真实竞态是「翻页响应在旗标置位后落地」（翻页请求可
    // 先于/后于 watchdog 到点发出，冒领只看响应落地时机）。这里让 watchdog
    // 先到点、再发起翻页并立即喂响应——同一冒领窗口的等价达成形态。不可
    // 先翻页再推进 45s：fake timers 会把翻页请求自己的 10s 安全超时一并
    // 引爆（在飞登记被兜底清掉 → 围栏丢弃），那是夹具伪影不是产品行为。
    await vi.advanceTimersByTimeAsync(45_000)
    await flushPromises()
    const ridW = lastRequestId()
    expect(ridW.startsWith('watchdog_')).toBe(true)

    // watchdog 恢复窗口内滚到顶触发翻页（滚动处理器无流式闸）→ 翻页请求
    // 在飞，其响应将在旗标已置位的窗口内落地。
    const ridPage = await triggerPagination(w)

    // 翻页响应落地：旧窗恰含同文 user 行及其旧回复（重发场景）——修复前
    // 冒领旗标走 landed 判定：landed+anchor 双真 → mid-turn replace +
    // setBusy(false) + clearWatchdog（busy 被杀、恢复检查取消）。
    feedHistory(
      ridPage,
      [
        { role: 'user', content: 'earlier' },
        { role: 'user', content: 'go' },
        { role: 'assistant', content: 'old reply to go' },
      ],
      { oldestIndex: 0, hasMore: true },
    )
    await flushPromises()

    // 修复断言①：流式态保持（busy 未被冒领分支杀掉）；翻页批次按自身
    // kind 前插（live 乐观行不被 replace 冲掉）。
    expect(chat.streaming, '翻页响应不得冒领 watchdog 分支杀 busy').toBe(true)
    const contents = chat.messages.map((m: any) => m.content)
    expect(contents).toContain('earlier')
    expect(contents).toContain('go')

    // 真 watchdog 响应到达：消费旗标走恢复分支（磁盘真相：回复已落 →
    // replace 重灌——设计语义）。
    feedHistory(
      ridW,
      [
        { role: 'user', content: 'q1' },
        { role: 'assistant', content: 'a1' },
        { role: 'user', content: 'go' },
        { role: 'assistant', content: 'fresh reply' },
      ],
      { oldestIndex: 0, hasMore: false },
    )
    await flushPromises()

    // 修复断言②：恢复重灌生效（旗标未被翻页响应提前消费——修复前此响应
    // 落合并路径零认领前置，翻页旧窗行残留视图尾部）。
    expect(chat.messages.map((m: any) => m.content)).toEqual([
      'q1',
      'a1',
      'go',
      'fresh reply',
    ])
    w.unmount()
  })
})
