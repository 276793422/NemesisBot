import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount, flushPromises } from '@vue/test-utils'

// 四体验与安全问题修复（2026-11-20）回归钉：
// - 批 1（限流回执徽标化）：note_kind=rate_limit 帧渲染折叠倒计时徽标
//   （不打断中间流程、不关 streaming）、单例（新帧摘旧）、assistant 终局
//   回复到场扫除、⏳ 排队收据不扫除（收据 ≠ 限流结束）。
// - 批 2（hook 注入轮点亮停止按钮）：system_notice / cron tick 帧（user
//   帧）到达 → busy=true + inflight 登记（停止按钮亮、切回可恢复），
//   assistant 到场既有完成语义收尾。
// - 批 3（切走切回时间线恢复）：busy 态（streaming）切回 → detectPendingTurn
//   streaming 分支拉 chat.work_current 快照恢复中间流程（修复前直接
//   return，快照恢复通道在 busy 态死掉）。

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

function receive(data: any) {
  h.wsHandler!({
    type: 'message',
    module: 'chat',
    cmd: 'receive',
    data: { session_id: 's1', timestamp: new Date().toISOString(), ...data },
  })
}

async function loadHistory(w: Awaited<ReturnType<typeof mount>>, messages: any[]) {
  const { sendHistoryRequest } = await import('../../composables/useWebSocket')
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
      messages,
    },
  })
  await flushPromises()
}

beforeEach(() => {
  setActivePinia(createPinia())
  useSessionStore().currentId = 's1'
  requestMock.mockReset()
  requestMock.mockImplementation((_mod: string, _cmd: string) => Promise.resolve({}))
  h.wsHandler = null
  h.wsStatus.value = 'connected'
})

afterEach(() => {
  vi.useRealTimers()
})

describe('批 1：限流进度徽标', () => {
  it('note_kind=rate_limit 帧 → 折叠徽标（不关 streaming、不触发完成语义）', async () => {
    const w = await mount(ChatPanel)
    await flushPromises()
    const chat = useChatStore()
    chat.setBusy('s1', true) // 中间流程进行中
    receive({
      role: 'system',
      note_kind: 'rate_limit',
      content: '⏳ 上游限流（gpt-6.2-sol），第 1/10 次重试，等待 5 秒…',
    })
    await flushPromises()
    const note = w.find('.rate-limit-note')
    expect(note.exists()).toBe(true)
    expect(note.text()).toContain('上游限流')
    expect(note.text()).toContain('等待 5 秒')
    // 限流回执不触发完成语义：busy 保持（停止按钮不消失）。
    expect(chat.isBusy('s1')).toBe(true)
    w.unmount()
  })

  it('倒计时：1s tick 本地递减，归零显示重试中', async () => {
    vi.useFakeTimers()
    const w = await mount(ChatPanel)
    await flushPromises()
    receive({
      role: 'system',
      note_kind: 'rate_limit',
      content: '⏳ 上游限流（m1），第 2/10 次重试，等待 10 秒…',
    })
    await flushPromises()
    expect(w.find('.rate-limit-note').text()).toContain('等待 10 秒')
    await vi.advanceTimersByTimeAsync(1000)
    expect(w.find('.rate-limit-note').text()).toContain('等待 9 秒')
    await vi.advanceTimersByTimeAsync(10_000)
    expect(w.find('.rate-limit-note').text()).toContain('重试中')
    w.unmount()
  })

  it('单例：第二条进度帧到达先摘旧徽标（恒一条动态徽标）', async () => {
    const w = await mount(ChatPanel)
    await flushPromises()
    receive({
      role: 'system',
      note_kind: 'rate_limit',
      content: '⏳ 上游限流（m1），第 1/10 次重试，等待 5 秒…',
    })
    await flushPromises()
    receive({
      role: 'system',
      note_kind: 'rate_limit',
      content: '⏳ 上游限流（m1），第 2/10 次重试，等待 10 秒…',
    })
    await flushPromises()
    const notes = w.findAll('.rate-limit-note')
    expect(notes).toHaveLength(1)
    expect(notes[0].text()).toContain('等待 10 秒')
    w.unmount()
  })

  it('assistant 终局回复到场 → 徽标扫除 + busy 释放', async () => {
    const w = await mount(ChatPanel)
    await flushPromises()
    const chat = useChatStore()
    chat.setBusy('s1', true)
    receive({
      role: 'system',
      note_kind: 'rate_limit',
      content: '⏳ 上游限流（m1），第 1/10 次重试，等待 5 秒…',
    })
    await flushPromises()
    expect(w.find('.rate-limit-note').exists()).toBe(true)
    receive({ role: 'assistant', content: '任务完成，已从限流恢复。' })
    await flushPromises()
    expect(w.find('.rate-limit-note').exists()).toBe(false)
    expect(chat.isBusy('s1')).toBe(false)
    w.unmount()
  })

  it('⏳ 排队收据不扫除徽标（收据到场 ≠ 上一条消息的限流结束）', async () => {
    const w = await mount(ChatPanel)
    await flushPromises()
    receive({
      role: 'system',
      note_kind: 'rate_limit',
      content: '⏳ 上游限流（m1），第 1/10 次重试，等待 5 秒…',
    })
    await flushPromises()
    receive({ role: 'assistant', content: '⏳ 当前正在处理上一条消息，本条已排队。' })
    await flushPromises()
    expect(w.find('.rate-limit-note').exists()).toBe(true)
    w.unmount()
  })

  it('WS 协议层 error notify 帧 → busy 关 + 徽标扫除 + inflight 清（横扫补齐）', async () => {
    const w = await mount(ChatPanel)
    await flushPromises()
    const chat = useChatStore()
    chat.setBusy('s1', true)
    chat.markInflightTurn('s1', '注入轮')
    receive({
      role: 'system',
      note_kind: 'rate_limit',
      content: '⏳ 上游限流（m1），第 1/10 次重试，等待 5 秒…',
    })
    await flushPromises()
    expect(w.find('.rate-limit-note').exists()).toBe(true)
    // WS 协议层坏帧兜底（websocket_handler build_error_message）：type=system
    // module=error cmd=notify——busy 关的同时徽标/inflight 对称收走。
    h.wsHandler!({
      type: 'system',
      module: 'error',
      cmd: 'notify',
      data: { content: 'bad frame', session_id: 's1' },
    })
    await flushPromises()
    expect(w.find('.rate-limit-note').exists()).toBe(false)
    expect(chat.isBusy('s1')).toBe(false)
    expect(chat.inflightTurnOf('s1')).toBeUndefined()
    w.unmount()
  })
})

describe('批 2：system_notice 帧点亮停止按钮', () => {
  it('system_notice 帧到达 → busy=true + inflight 登记（停止按钮亮）', async () => {
    const w = await mount(ChatPanel)
    await flushPromises()
    await loadHistory(w, [
      { role: 'user', content: 'q1' },
      { role: 'assistant', content: 'a1' },
    ])
    const chat = useChatStore()
    expect(chat.isBusy('s1')).toBe(false)
    // hook 续行注入：SystemNotice → receive 帧 role=user + system_notice 标记。
    receive({
      role: 'user',
      system_notice: 'system',
      content: 'hook 提示词注入：继续上一轮任务',
    })
    await flushPromises()
    expect(chat.isBusy('s1')).toBe(true)
    expect(chat.inflightTurnOf('s1')).toBeDefined()
    // 停止按钮可见（busy 态 v-if）。
    expect(w.find('button.btn-stop').exists()).toBe(true)
    w.unmount()
  })

  it('cron tick 帧同臂（cron_job_id 标记）', async () => {
    const w = await mount(ChatPanel)
    await flushPromises()
    const chat = useChatStore()
    receive({
      role: 'user',
      cron_job_id: 'job-7',
      content: '每日站会提醒',
    })
    await flushPromises()
    expect(chat.isBusy('s1')).toBe(true)
    expect(chat.inflightTurnOf('s1')).toBeDefined()
    w.unmount()
  })

  it('assistant 到场 → 完成语义收尾（busy 释放 + inflight 清）', async () => {
    const w = await mount(ChatPanel)
    await flushPromises()
    const chat = useChatStore()
    receive({
      role: 'user',
      system_notice: 'system',
      content: 'hook 提示词注入：继续上一轮任务',
    })
    await flushPromises()
    expect(chat.isBusy('s1')).toBe(true)
    receive({ role: 'assistant', content: '续轮任务完成。' })
    await flushPromises()
    expect(chat.isBusy('s1')).toBe(false)
    expect(chat.inflightTurnOf('s1')).toBeUndefined()
    w.unmount()
  })

  it('异会话 system_notice 帧不影响本会话（帧路由围栏）', async () => {
    const w = await mount(ChatPanel)
    await flushPromises()
    const chat = useChatStore()
    receive({
      session_id: 'other-sess',
      role: 'user',
      system_notice: 'system',
      content: '别的会话的注入',
    })
    await flushPromises()
    expect(chat.isBusy('s1')).toBe(false)
    expect(chat.inflightTurnOf('s1')).toBeUndefined()
    w.unmount()
  })

  it('刷新恢复态（尾部悬空 user 占位轮询）→ 停止按钮亮；取消受理 + Done 帧到场按钮收场（第二轮横扫补齐）', async () => {
    const w = await mount(ChatPanel)
    await flushPromises()
    const chat = useChatStore()
    // 刷新恢复形态：尾部悬空 user 行（回复未落盘）→ detectPendingTurn 起
    // 占位轮询——修复前此形态不亮停止按钮，注入轮刷新后只能干等。
    await loadHistory(w, [
      { role: 'user', content: 'q1' },
      { role: 'assistant', content: 'a1' },
      { role: 'user', content: 'hook 注入轮（刷新后恢复）' },
    ])
    await flushPromises()
    expect(chat.isBusy('s1')).toBe(false) // 恢复态不置 busy（零纠缠设计）
    const btn = w.find('button.btn-stop')
    expect(btn.exists()).toBe(true)
    // 取消受理（cancelled>0）→ 保持过渡态；Done assistant 帧到场 →
    // receive 分支 detectPendingTurn 停轮清占位 → 按钮卸载。
    requestMock.mockImplementation((_mod: string, cmd: string) =>
      cmd === 'cancel' ? Promise.resolve({ cancelled: 1 }) : Promise.resolve({}),
    )
    await btn.trigger('click')
    await flushPromises()
    receive({ role: 'assistant', content: '已取消。' })
    await flushPromises()
    expect(w.find('button.btn-stop').exists()).toBe(false)
    w.unmount()
  })
})

describe('批 3：busy 态切回的快照恢复通道', () => {
  it('streaming 态挂载（切走切回形态）→ detectPendingTurn 拉 chat.work_current', async () => {
    const chat = useChatStore()
    chat.setBusy('s1', true) // 切走前发送、busy 表不清 → 切回 streaming 态
    requestMock.mockImplementation((_mod: string, cmd: string) => {
      if (cmd === 'work_current') {
        return Promise.resolve({
          work: { rounds: [{ text: '', tools: [{ tool: 'exec', args_preview: 'echo x', ok: true }] }] },
        })
      }
      if (cmd === 'inbox_status') return Promise.resolve({ available: true, busy: true })
      return Promise.resolve({})
    })
    const w = await mount(ChatPanel)
    await flushPromises()
    // 尾部悬空 user 行 → detectPendingTurn → streaming 分支（占位已由
    // streaming 渲染、不接管轮询）→ 补拉 work_current 快照恢复中间流程。
    await loadHistory(w, [
      { role: 'user', content: 'q1' },
      { role: 'assistant', content: 'a1' },
      { role: 'user', content: '跑个长任务' },
    ])
    await flushPromises()
    const workCalls = requestMock.mock.calls.filter((c) => c[1] === 'work_current')
    expect(workCalls.length).toBeGreaterThanOrEqual(1)
    expect(workCalls[0][2]).toEqual({ session_id: 's1' })
    w.unmount()
  })
})
