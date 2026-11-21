import { describe, it, expect, vi, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount, flushPromises } from '@vue/test-utils'

// 插话打断感修复（2026-11-20）回归钉：
// - 忙碌期瞬态回执（⚡ 插话已接收 / ⏳ 排队）note_kind=busy_receipt →
//   灰徽标渲染（role=system 投影），完成语义分支不触发——busy 保持、
//   停止按钮存活、时间线不被冻结到回执上。
// - 终局回复/错误帧到场 → 全部回执徽标扫除（单条 AI 回复消化一切）。
// - 插话注入注记 note_kind=steer_inject（2026-11-21 二次改版：注记进
//   「LLM 返回的过程」）→ 并入 pendingWork 注记轮（text/tools 皆空、
//   steer_note 有值），独立注记行隐藏（占行锚不动）；终局回复到场随
//   work 键挂载，WorkTimelineBlock 过程流内可见插话落点。

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

describe('忙碌回执徽标（busy_receipt）', () => {
  it('⚡ 插话回执帧 → 灰徽标（busy 保持、停止按钮存活、不触发完成语义）', async () => {
    const w = await mount(ChatPanel)
    await flushPromises()
    const chat = useChatStore()
    chat.setBusy('s1', true) // 原 turn 进行中
    receive({
      role: 'system',
      note_kind: 'busy_receipt',
      content: '⚡ 已接收为紧急插话（消息以 ！ 开头），将在本轮思考中一并处理。',
    })
    await flushPromises()
    const badge = w.find('.busy-receipt-note')
    expect(badge.exists()).toBe(true)
    expect(badge.text()).toContain('已接收为紧急插话')
    // 完成语义不触发：busy 保持（停止按钮不消失）。
    expect(chat.isBusy('s1')).toBe(true)
    expect(w.find('button.btn-stop').exists()).toBe(true)
    // 过程行无头像（isProcessRow）。
    expect(w.find('.message.system .message-avatar').exists()).toBe(false)
    w.unmount()
  })

  it('⚡ 插话回执与 ⏳ 排队回执共存（互不摘除，非单例）；同文重复帧按既有防重复语义去重', async () => {
    const w = await mount(ChatPanel)
    await flushPromises()
    const chat = useChatStore()
    chat.setBusy('s1', true)
    receive({
      role: 'system',
      note_kind: 'busy_receipt',
      content: '⚡ 已接收为紧急插话（消息以 ！ 开头），将在本轮思考中一并处理。',
    })
    await flushPromises()
    receive({
      role: 'system',
      note_kind: 'busy_receipt',
      content: '⏳ 当前正在处理上一条消息。你的消息已排队，处理完成后自动继续。',
    })
    await flushPromises()
    // 不同内容回执各自成徽标（排队回执不是单例族——与 rate_limit 的
    // 单例语义不同）；同文重复帧由 receive 分支既有 isDuplicateRecovery
    // 去重（排队反馈一条即足，watchdog 恢复防双渲染同一闸门）。
    const badges = w.findAll('.busy-receipt-note')
    expect(badges).toHaveLength(2)
    expect(badges[0].text()).toContain('已接收为紧急插话')
    expect(badges[1].text()).toContain('已排队')
    expect(chat.isBusy('s1')).toBe(true)
    w.unmount()
  })

  it('终局回复到场 → 全部回执徽标扫除 + busy 释放', async () => {
    const w = await mount(ChatPanel)
    await flushPromises()
    const chat = useChatStore()
    chat.setBusy('s1', true)
    receive({
      role: 'system',
      note_kind: 'busy_receipt',
      content: '⚡ 已接收为紧急插话（消息以 ！ 开头），将在本轮思考中一并处理。',
    })
    receive({
      role: 'system',
      note_kind: 'busy_receipt',
      content: '⏳ 当前正在处理上一条消息。你的消息已排队，处理完成后自动继续。',
    })
    await flushPromises()
    expect(w.findAll('.busy-receipt-note').length).toBe(2)
    receive({ role: 'assistant', content: '插话和原任务都已完成：……' })
    await flushPromises()
    expect(w.find('.busy-receipt-note').exists()).toBe(false)
    expect(chat.isBusy('s1')).toBe(false)
    w.unmount()
  })

  it('错误帧到场 → 回执徽标扫除（错误即终局）', async () => {
    const w = await mount(ChatPanel)
    await flushPromises()
    const chat = useChatStore()
    chat.setBusy('s1', true)
    receive({
      role: 'system',
      note_kind: 'busy_receipt',
      content: '⚡ 已接收为紧急插话（消息以 ！ 开头），将在本轮思考中一并处理。',
    })
    await flushPromises()
    expect(w.find('.busy-receipt-note').exists()).toBe(true)
    h.wsHandler!({
      type: 'message',
      module: 'chat',
      cmd: 'error',
      data: { session_id: 's1', content: 'turn failed' },
    })
    await flushPromises()
    expect(w.find('.busy-receipt-note').exists()).toBe(false)
    expect(chat.isBusy('s1')).toBe(false)
    w.unmount()
  })

  it('WS 协议层 error notify 帧 → 回执徽标扫除（兜底通道同构）', async () => {
    const w = await mount(ChatPanel)
    await flushPromises()
    receive({
      role: 'system',
      note_kind: 'busy_receipt',
      content: '⏳ 当前正在处理上一条消息。你的消息已排队，处理完成后自动继续。',
    })
    await flushPromises()
    expect(w.find('.busy-receipt-note').exists()).toBe(true)
    h.wsHandler!({
      type: 'system',
      module: 'error',
      cmd: 'notify',
      data: { content: 'bad frame', session_id: 's1' },
    })
    await flushPromises()
    expect(w.find('.busy-receipt-note').exists()).toBe(false)
    w.unmount()
  })
})

describe('插话注入注记（steer_inject，2026-11-21 二次改版：进时间线过程流）', () => {
  it('实时注记帧 → 并入 pendingWork 注记轮；独立注记行隐藏（占行锚不动）', async () => {
    const w = await mount(ChatPanel)
    await flushPromises()
    const chat = useChatStore()
    chat.setBusy('s1', true) // turn 进行中（claim 时刻必有活时间线场景）
    receive({
      role: 'system',
      note_kind: 'steer_inject',
      content: '⚡ 插话已注入本轮思考：补充边界情况的处理',
    })
    await flushPromises()
    // 注记并入进行中时间线（注记轮：text/tools 皆空、steer_note 有值）。
    expect(chat.pendingWork?.rounds).toHaveLength(1)
    expect(chat.pendingWork?.rounds[0].steer_note).toContain('补充边界情况的处理')
    expect(chat.pendingWork?.rounds[0].text).toBe('')
    // 独立注记行不再渲染（iteration-1 的 .steer-inject-note 已移除）。
    expect(w.find('.steer-inject-note').exists()).toBe(false)
    // 消息行照常入列（jsonl 占行锚）但整行隐藏（v-show display:none）。
    const hiddenRow = w.findAll('.message.system').find(n => !n.isVisible())
    expect(hiddenRow).toBeTruthy()
    w.unmount()
  })

  it('终局回复到场 → 注记随 work 键挂载，时间线过程流内可见插话落点', async () => {
    const w = await mount(ChatPanel)
    await flushPromises()
    const chat = useChatStore()
    chat.setBusy('s1', true)
    receive({
      role: 'system',
      note_kind: 'steer_inject',
      content: '⚡ 插话已注入本轮思考：补充边界情况的处理',
    })
    await flushPromises()
    receive({ role: 'assistant', content: '已完成（含插话内容）。' })
    await flushPromises()
    expect(chat.isBusy('s1')).toBe(false)
    // work 键挂载到 assistant 行（flushPendingWork 取走缓冲，注记轮在内）。
    const last = chat.messages[chat.messages.length - 1]
    expect(last.work?.rounds.some(r => r.steer_note)).toBe(true)
    // 落地折叠时间线展开后，过程流内可见注记行（⚡ 行自带内容摘要）。
    await w.find('button.wt-header').trigger('click')
    const note = w.find('.wt-steer-note')
    expect(note.exists()).toBe(true)
    expect(note.text()).toContain('插话已注入本轮思考')
    expect(note.text()).toContain('补充边界情况的处理')
    w.unmount()
  })

  it('历史 assistant 行 work 键含注记轮 → 展开时间线可见（刷新后保留）', async () => {
    const w = await mount(ChatPanel)
    await flushPromises()
    await loadHistory(w, [
      { role: 'user', content: '跑个长任务' },
      { role: 'user', content: '[steer] 补充一下边界情况', steer: true },
      { role: 'system', note_kind: 'steer_inject', content: '⚡ 插话已注入本轮思考' },
      {
        role: 'assistant',
        content: '已完成。',
        work: {
          started_at_ms: 1_730_000_000_000,
          ended_at_ms: 1_730_000_003_500,
          rounds: [
            { text: '', tools: [], steer_note: '⚡ 插话已注入本轮思考：补充一下边界情况' },
          ],
        },
      },
    ])
    // 注记 jsonl 行隐藏（不作为独立行露出）。
    expect(w.find('.steer-inject-note').exists()).toBe(false)
    // 时间线展开后注记可见。
    await w.find('button.wt-header').trigger('click')
    expect(w.find('.wt-steer-note').text()).toContain('补充一下边界情况')
    w.unmount()
  })

  it('历史 steer_inject 行隐藏但行号锚不动（占行数学不受改版影响）', async () => {
    const w = await mount(ChatPanel)
    await flushPromises()
    await loadHistory(w, [
      { role: 'user', content: '跑个长任务' },
      { role: 'system', note_kind: 'steer_inject', content: '⚡ 插话已注入本轮思考' },
      { role: 'assistant', content: '已完成。' },
    ])
    expect(w.find('.steer-inject-note').exists()).toBe(false)
    // 注记行仍占 jsonl 行（rowIndex 连续 0..2）——rewind/fork 锚位数学
    // 与改版前完全一致。
    const store = useChatStore()
    const rowIndexes = store.messages
      .filter(m => m.rowIndex !== undefined)
      .map(m => m.rowIndex)
    expect(rowIndexes).toEqual([0, 1, 2])
    w.unmount()
  })
})

describe('chat.sync 重放窗口', () => {
  it('重放 assistant 行到场 → 过期 busy_receipt 扫除；live 注记随 work 键挂载（重放不二次摄取）', async () => {
    const w = await mount(ChatPanel)
    await flushPromises()
    const chatStore = useChatStore()
    // 先实时挂回执 + 注记（注记并入 pendingWork 注记轮，消息行隐藏）。
    receive({
      role: 'system',
      note_kind: 'busy_receipt',
      content: '⚡ 已接收为紧急插话（消息以 ！ 开头），将在本轮思考中一并处理。',
    })
    receive({
      role: 'system',
      note_kind: 'steer_inject',
      content: '⚡ 插话已注入本轮思考：补充边界情况的处理',
    })
    await flushPromises()
    chatStore.historyLoading = false
    chatStore.historyLoaded = true
    requestMock.mockImplementation((_mod: string, cmd: string) => {
      if (cmd === 'sync') {
        return Promise.resolve({
          events: [
            {
              seq: 2,
              role: 'system',
              note_kind: 'busy_receipt',
              content: '⚡ 已接收为紧急插话（消息以 ！ 开头），将在本轮思考中一并处理。',
              ts: '2026-11-20T10:00:03+08:00',
            },
            { seq: 3, role: 'assistant', content: '最终回复。', ts: '2026-11-20T10:00:04+08:00' },
          ],
        })
      }
      return Promise.resolve({})
    })
    // 断线→重连触发 syncMissedChat（historyLoaded 已置位 → 补拉分支）。
    h.wsStatus.value = 'disconnected'
    await flushPromises()
    h.wsStatus.value = 'connected'
    await flushPromises()
    expect(w.find('.busy-receipt-note').exists()).toBe(false)
    // 独立注记行不再露出（隐藏行占位）；live 注记已并入 pendingWork，
    // 重放 assistant 行 flush 挂载 work 键——注记轮随键到场，时间线内可见。
    expect(w.find('.steer-inject-note').exists()).toBe(false)
    const last = chatStore.messages[chatStore.messages.length - 1]
    expect(last.role).toBe('assistant')
    expect(last.work?.rounds.some(r => r.steer_note)).toBe(true)
    await w.find('button.wt-header').trigger('click')
    expect(w.find('.wt-steer-note').text()).toContain('补充边界情况的处理')
    w.unmount()
  })
})

describe('插话专轮收口回应（steer_response，2026-11-21 插话专轮批）', () => {
  it('live 回应帧 → 摄取进注记轮（反查命中）；无新消息行；live 时间线渲染回应行', async () => {
    const w = await mount(ChatPanel)
    await flushPromises()
    const chat = useChatStore()
    chat.setBusy('s1', true)
    receive({
      role: 'system',
      note_kind: 'steer_inject',
      seq: 2,
      content: '⚡ 插话已注入本轮思考：补充边界情况的处理',
    })
    receive({
      role: 'system',
      note_kind: 'steer_response',
      seq: 3,
      content: 'E2E_STEER_ACK：插话已收到，边界说明已纳入后续处理（中间回应，主线继续）。',
    })
    await flushPromises()
    // 回应记在注记轮上（反查最近未回注记轮），不新开轮。
    expect(chat.pendingWork?.rounds).toHaveLength(1)
    expect(chat.pendingWork?.rounds[0].steer_response).toContain('E2E_STEER_ACK')
    // 瞬态帧不入列：消息流只有注记的隐藏占行，回应没有自己的行。
    const systemRows = w.findAll('.message.system')
    expect(systemRows).toHaveLength(1)
    expect(systemRows[0].classes().join(' ')).not.toContain('d-none') // v-show 由 style 控制，此处只验证行数
    // live 时间线（pendingWork 在场默认展开）：回应行渲染在注记行下方。
    const liveResp = w.find('.wt-block.live .wt-steer-response')
    expect(liveResp.exists()).toBe(true)
    expect(liveResp.text()).toContain('E2E_STEER_ACK')
    // busy 保持（回应不是终局——完成语义不触发）。
    expect(chat.isBusy('s1')).toBe(true)
    w.unmount()
  })

  it('终局回复到场 → 回应随 work 键挂载，落地时间线展开可见成对（注记+回应）', async () => {
    const w = await mount(ChatPanel)
    await flushPromises()
    const chat = useChatStore()
    chat.setBusy('s1', true)
    receive({
      role: 'system',
      note_kind: 'steer_inject',
      seq: 2,
      content: '⚡ 插话已注入本轮思考：补充边界情况的处理',
    })
    receive({
      role: 'system',
      note_kind: 'steer_response',
      seq: 3,
      content: 'E2E_STEER_ACK：插话已收到。',
    })
    receive({ role: 'assistant', content: 'E2E_STEER_MAIN_DONE：主线收尾。' })
    await flushPromises()
    expect(chat.isBusy('s1')).toBe(false)
    const last = chat.messages[chat.messages.length - 1]
    expect(last.role).toBe('assistant')
    expect(last.work?.rounds[0].steer_note).toBeTruthy()
    expect(last.work?.rounds[0].steer_response).toContain('E2E_STEER_ACK')
    await w.find('button.wt-header').trigger('click')
    expect(w.find('.wt-steer-note').text()).toContain('补充边界情况的处理')
    expect(w.find('.wt-steer-response').text()).toContain('E2E_STEER_ACK')
    w.unmount()
  })

  it('sync 重放跳过 steer_response（不二次摄取、不成行）；assistant 行照常挂载', async () => {
    const w = await mount(ChatPanel)
    await flushPromises()
    const chat = useChatStore()
    chat.setBusy('s1', true)
    // live 已摄取（游标推到 3）；重放窗口给出 seq 更大的同 kind 帧（sync
    // 在 live 处理前已发起的时序）——若不跳过会反查覆盖成重放文本。
    receive({
      role: 'system',
      note_kind: 'steer_inject',
      seq: 2,
      content: '⚡ 插话已注入本轮思考：补充边界情况的处理',
    })
    receive({
      role: 'system',
      note_kind: 'steer_response',
      seq: 3,
      content: 'E2E_STEER_ACK：live 原文。',
    })
    await flushPromises()
    chat.historyLoading = false
    chat.historyLoaded = true
    requestMock.mockImplementation((_mod: string, cmd: string) => {
      if (cmd === 'sync') {
        return Promise.resolve({
          events: [
            {
              seq: 5,
              role: 'system',
              note_kind: 'steer_response',
              content: 'E2E_STEER_ACK：重放副本（不应摄取）。',
              ts: '2026-11-21T10:00:05+08:00',
            },
            { seq: 6, role: 'assistant', content: '最终回复。', ts: '2026-11-21T10:00:06+08:00' },
          ],
        })
      }
      return Promise.resolve({})
    })
    h.wsStatus.value = 'disconnected'
    await flushPromises()
    h.wsStatus.value = 'connected'
    await flushPromises()
    // 重放副本未摄取——assistant 重放行触发 flushPendingWork，缓冲随键
    // 挂载；若重放被摄取，反查会覆盖成「重放副本」，此断言即红。
    expect(w.findAll('.message.system')).toHaveLength(1)
    // assistant 重放行照常落地 + work 键挂载（注记+回应成对随键）。
    const last = chat.messages[chat.messages.length - 1]
    expect(last.role).toBe('assistant')
    expect(last.work?.rounds[0].steer_response).toContain('live 原文')
    w.unmount()
  })
})
