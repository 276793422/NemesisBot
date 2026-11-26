import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount, flushPromises } from '@vue/test-utils'

// 回退链跨会话守卫回归（resync 10s 安全超时 + 切换围栏的组合缝隙）：
// - BUG：rewindToTurnStart await resyncFromLog() 窗口内切换会话 → resync
//   响应被切换围栏丢弃（inFlightHistory.clear()）→ Promise 由 10s 安全超时
//   放行 → doRegenerate 续体照走 sendValidated(旧会话提问原文)——而
//   sendValidated 全程用活的 effectiveSid，旧会话的提问原文被发进新会话
//   并触发真实 agent 执行。
// - 修复 a：resync 放行后校验 effectiveSid 仍是回退目标会话，不匹配则
//   中止续作（回退本身按显式 sid 已落在旧会话，正确，只裁续作）。
// - 修复 b：会话切换 watcher 与 inFlightHistory.clear() 同点清
//   pendingResync/pendingWatchdogReload——旗标是组件实例级残留，不清则
//   新会话首个历史响应被 resync 分支冒领（setBusy 误清新会话 busy、
//   historyLoaded 不落，后续翻页/重连闸全部错位）。

const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
}))

/** 捕获槽（vi.hoisted——mock 工厂在 import 期执行，普通顶层 let 会 TDZ）。 */
const h = vi.hoisted(() => ({
  wsHandler: null as ((data: any) => void) | null,
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
import { sendHistoryRequest, send as wsSend } from '../../composables/useWebSocket'
import { useChatStore } from '../../stores/chat'
import { useSessionStore } from '../../stores/session'
import { useToast } from '../../composables/useToast'

beforeEach(() => {
  setActivePinia(createPinia())
  requestMock.mockReset()
  requestMock.mockResolvedValue({})
  h.wsHandler = null
  h.wsStatus.value = 'connected'
})

afterEach(() => {
  vi.useRealTimers()
})

function prepareSessions() {
  const s = useSessionStore()
  s.sessions = [
    { id: 's1', channel: 'web', startTime: '', lastTime: '', messageCount: 0, firstMessage: 'A', model: '' },
    // s2 非零计数：2026-11-07 起切换已知零消息会话跳过重拉——本文件测「切
    // 换后首个页响应走 page 分支」，目标行必须非零才有请求可喂。
    { id: 's2', channel: 'web', startTime: '', lastTime: '', messageCount: 3, firstMessage: 'B', model: '' },
  ] as any
  s.currentId = 's1'
}

function feedHistory(requestId: string, messages: any[]) {
  h.wsHandler!({
    type: 'message',
    module: 'chat',
    cmd: 'history_response',
    data: { request_id: requestId, messages, has_more: false, oldest_index: 0 },
  })
}

function lastHistoryRequestId(): string {
  const calls = vi.mocked(sendHistoryRequest).mock.calls
  return calls[calls.length - 1][0] as string
}

async function mountWithS1History() {
  prepareSessions()
  const w = mount(ChatPanel)
  await flushPromises()
  feedHistory(lastHistoryRequestId(), [
    { role: 'user', content: 'q1' },
    { role: 'assistant', content: 'a1' },
  ])
  await flushPromises()
  return w
}

/** 2026-11-12「⋯」菜单迁移：回退按钮收编进三点菜单（Teleport to body →
 *  wrapper.find 找不到，须经 document.body）。流程 = 点开目标行的触发钮 →
 *  点菜单项。 */
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

describe('ChatPanel 回退链跨会话守卫（resync 安全超时缝隙）', () => {
  it('重新生成 await 窗口内切换会话：10s 超时放行后中止重发，不把旧会话提问发进新会话', async () => {
    vi.useFakeTimers()
    const w = await mountWithS1History()
    const chat = useChatStore()
    expect(chat.messages).toHaveLength(2)

    // assistant 行（a1）的「↻ 重新生成」（⋯ 菜单内）→ doRegenerate →
    // rewind 请求落地 → resyncFromLog 挂起（其响应不喂）。
    await clickMsgMenuItem(w, 'assistant', '重新生成')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('sessions', 'rewind_to_message', {
      session_id: 's1',
      message_index: 0,
      mode: 'before',
      anchor_content: 'q1',
    })

    // await 窗口内切换到 s2：切换围栏清掉在飞 resync 登记（响应永不到达）。
    useSessionStore().currentId = 's2'
    await flushPromises()

    // 10s 安全超时放行 resyncFromLog——守卫必须在此中止重发。
    await vi.advanceTimersByTimeAsync(10_000)
    await flushPromises()

    // 主断言对准真实发送通道（sendValidated → useWebSocket send）；副断言
    // 兜 chat.send WSAPI 请求面。
    const wsSendCalls = vi.mocked(wsSend).mock.calls.filter(
      (c: any[]) => c[0] === 'q1',
    )
    expect(wsSendCalls, '旧会话提问原文不得发进新会话').toHaveLength(0)
    const sendCalls = requestMock.mock.calls.filter(
      (c: any[]) => c[0] === 'chat' && c[1] === 'send',
    )
    expect(sendCalls).toHaveLength(0)
    const toasts = useToast().toasts
    expect(
      toasts.some(t => t.message.includes('会话已切换')),
      '应 toast 告知续作已中止',
    ).toBe(true)
    w.unmount()
  })

  it('切换会话清分支旗标：新会话首个页响应走 page 分支（historyLoaded 落位），不被 resync 分支冒领', async () => {
    const w = await mountWithS1History()

    // user 行（q1）的「⏪ 撤销此问」（⋯ 菜单内）→ rewind → resyncFromLog
    // 挂起（不喂响应）→ pendingResync = true 残留。
    await clickMsgMenuItem(w, 'user', '撤销此问')
    await flushPromises()

    // 切到 s2（watcher 清旗标）→ 新会话 loadHistory 在飞 → 喂 s2 页响应。
    useSessionStore().currentId = 's2'
    await flushPromises()
    const chat = useChatStore()
    feedHistory(lastHistoryRequestId(), [{ role: 'user', content: 's2q' }])
    await flushPromises()

    expect(chat.historyLoaded, 's2 页响应必须走 page 分支落 historyLoaded').toBe(true)
    expect(chat.messages.map(m => m.content)).toEqual(['s2q'])
    w.unmount()
  })
})

describe('双 resync 并发：孤儿响应合并路径即时兑现 onDone（BUG-20261127-011）', () => {
  it('subagent_reply 与重新生成的 resync 重叠：第二响应落地即续作重发链，不等 10s 安全超时', async () => {
    vi.useFakeTimers()
    const w = await mountWithS1History()
    // useToast 的 toasts 是模块级单例（非 Pinia 态）——文件内前序用例的
    // toast（如「会话已切换」）在 fake timers 下永不过期，会泄漏进本用例
    // 的负断言。清场后只看本用例新产生的 toast。
    useToast().toasts.splice(0, useToast().toasts.length)

    // 并发源 1：subagent_reply 定向 push → resyncFromLog A（委派回复落盘）。
    h.wsHandler!({ type: 'push', cmd: 'subagent_reply', data: { session_id: 's1' } })
    await flushPromises()
    const ridA = lastHistoryRequestId()
    expect(ridA.startsWith('resync_')).toBe(true)

    // fake timers 冻结 Date.now() → rid = 'resync_' + Date.now() 同毫秒同
    // 值（inFlightHistory 按 request_id 键控会互相覆盖，测试伪影而非产品
    // 行为）。推进模拟时钟 5ms，让 B 拿到独立 rid。
    vi.setSystemTime(Date.now() + 5)

    // 并发源 2：assistant 行「↻ 重新生成」→ rewind + resyncFromLog B。
    // A 已把 pendingResync 旗标置位，B 再置位是 no-op（仍 true）。
    await clickMsgMenuItem(w, 'assistant', '重新生成')
    await flushPromises()
    const ridB = lastHistoryRequestId()
    expect(ridB.startsWith('resync_')).toBe(true)
    expect(ridB).not.toBe(ridA)

    // A 的响应先到：消费旗标走 resync replace 分支（含委派回复行）。
    feedHistory(ridA, [
      { role: 'user', content: 'q1' },
      { role: 'assistant', content: 'a1' },
      { role: 'assistant', content: 'delegate reply' },
    ])
    await flushPromises()

    // B 的响应后到：旗标已被 A 消费 → 落合并路径（孤儿 resync；此时视图
    // 已被 A 的 replace 定形，B 的合并是全认领重建，视图终态与 resync 分支
    // 同构）。修复前：onDone 只在 resync 分支兑现 → rewindToTurnStart 的
    // await 干等 10s 安全超时；修复后：合并路径尾部即时兑现 → 重发链立即
    // 续作（rewind 到 index 0 之前 = 空日志，B 响应 messages 为空合法）。
    feedHistory(ridB, [])
    await flushPromises()

    // 主断言对准真实发送通道：不推进计时器（10s 安全超时不得到场），
    // 重发链必须已经走完 sendValidated('q1')。
    const wsSendCalls = vi.mocked(wsSend).mock.calls.filter(
      (c: any[]) => c[0] === 'q1',
    )
    expect(
      wsSendCalls,
      '重新生成的重发应在孤儿 resync 响应落地后即时续作（不等 10s）',
    ).toHaveLength(1)
    // 会话未切换 → 守卫放行，不得出现「会话已切换」中止 toast。
    expect(useToast().toasts.some(t => t.message.includes('会话已切换'))).toBe(false)
    w.unmount()
  })
})
