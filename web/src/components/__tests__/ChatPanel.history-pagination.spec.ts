import { describe, it, expect, vi, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount, flushPromises } from '@vue/test-utils'

// W1（2026-09-22 审计修复）：翻页响应误删实时回复。
//
// 根因：A1 清洗（dropAssistantBelowSeq）对翻页响应同样执行——后端翻页响应
// 携带**全会话最新** last_seq（采样不区分 before_index），视图内带 seq 的
// assistant 实时帧（seq ≤ last_seq）被删，而这些帧不在更旧的翻页批次里
// → 直接消失（recomputeNextRowIndex 连带错算 rewind 锚）。原注释「翻页
// 两规则天然不命中」对 A2 成立、对 A1 不成立。
//
// 修复语义（本 spec 钉死）：
// - 翻页请求（发请求时 oldestIndex 非空 ⟺ 在翻页）的响应跳过 A1/A2 清洗
//   ——翻页批次严格更旧，与尾部实时帧零交集；
// - 全量请求（首拉）响应照常清洗——A1 切会话竞态语义不回退。

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

import { sendHistoryRequest, onMessage } from '../../composables/useWebSocket'
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

/** mount 并取回组件注册的 WS 下行帧 handler。 */
async function mountAndCapture() {
  const sessionStore = useSessionStore()
  sessionStore.sessions = [
    { id: 's1', channel: 'web', startTime: '', lastTime: '', messageCount: 0, firstMessage: 'A', model: '' },
  ] as any
  sessionStore.currentId = 's1'

  const wrapper = mount(ChatPanel)
  await flushPromises()
  const handler = vi.mocked(onMessage).mock.calls.at(-1)![0] as (frame: any) => void
  return { wrapper, sessionStore, handler }
}

function historyFrame(opts: {
  rid: string
  lastSeq?: number
  oldestIndex: number
  hasMore: boolean
  count?: number
}) {
  const count = opts.count ?? 1
  return {
    type: 'message',
    module: 'chat',
    cmd: 'history',
    data: {
      request_id: opts.rid,
      session_id: 's1',
      has_more: opts.hasMore,
      oldest_index: opts.oldestIndex,
      total_count: count,
      last_seq: opts.lastSeq,
      messages: Array.from({ length: count }, (_, i) => ({
        role: 'user',
        content: `msg-${opts.rid}-${i}`,
        timestamp: '2026-09-22T10:00:00Z',
      })),
    },
  }
}

/** 往视图推一条带 seq 的 assistant 实时帧（模拟「回复先到、历史后到」
 *  竞态窗口里的实时渲染，或全量加载后新到的实时回复）。 */
function pushLiveReply(chat: ReturnType<typeof useChatStore>, seq: number, content = 'live reply') {
  chat.messages.push({
    role: 'assistant',
    content,
    timestamp: '2026-09-22T10:01:00Z',
    seq,
  } as any)
}

describe('翻页响应不删尾部实时帧（W1）', () => {
  it('翻页批次落地后，视图内带 seq 的 assistant 实时帧保留', async () => {
    const { wrapper, handler } = (await mountAndCapture()) as any
    const chat = useChatStore()

    // 首拉（全量）：oldest_index=20、has_more → oldestIndex 非空 = 可翻页。
    const rid1 = vi.mocked(sendHistoryRequest).mock.calls[0][0] as string
    handler(historyFrame({ rid: rid1, lastSeq: 50, oldestIndex: 20, hasMore: true }))
    await flushPromises()
    expect(chat.messages).toHaveLength(1)

    // 全量加载后新到的实时回复（带 seq ≤ 最新 last_seq）。
    pushLiveReply(chat, 41)
    expect(chat.messages.map((m: any) => m.content)).toContain('live reply')

    // 滚到顶触发翻页请求。
    const el = wrapper.find('.chat-messages').element as HTMLElement
    el.scrollTop = 0
    await wrapper.find('.chat-messages').trigger('scroll')
    await flushPromises()
    expect(vi.mocked(sendHistoryRequest).mock.calls.length).toBe(2)
    const rid2 = vi.mocked(sendHistoryRequest).mock.calls[1][0] as string

    // 翻页响应（更旧批次，last_seq 仍是全会话最新的 50）。
    handler(historyFrame({ rid: rid2, lastSeq: 50, oldestIndex: 0, hasMore: false }))
    await flushPromises()

    // 修复点：实时帧不被误删；翻页批次正常前插（1 + 1 实时 + 1 = 3）。
    const contents = chat.messages.map((m: any) => m.content)
    expect(contents).toContain('live reply')
    expect(contents).toContain(`msg-${rid2}-0`)
    expect(chat.messages).toHaveLength(3)
    wrapper.unmount()
  })

  it('全量响应照常执行 A1 清洗（切会话竞态语义不回退）', async () => {
    const { wrapper, handler } = (await mountAndCapture()) as any
    const chat = useChatStore()

    // 实时帧先入列（切会话 reset 清空视图的竞态窗口）。
    pushLiveReply(chat, 41)

    // 首拉（全量，oldestIndex 为 null → paginated=false）响应到达：
    // last_seq=50 ≥ 实时帧 seq 41 ⟹ 历史已含该回复 → A1 精确剔除。
    const rid = vi.mocked(sendHistoryRequest).mock.calls[0][0] as string
    handler(historyFrame({ rid, lastSeq: 50, oldestIndex: 0, hasMore: false }))
    await flushPromises()

    const contents = chat.messages.map((m: any) => m.content)
    expect(contents).not.toContain('live reply')
    expect(contents).toContain(`msg-${rid}-0`)
    wrapper.unmount()
  })

  it('首拉请求的 paginated 钉子为 false（oldestIndex 初始 null）', async () => {
    // 依据链：oldestIndex 初始 null（chat.ts）→ 发请求时非 null ⟺ 翻页。
    // 本用例钉「首拉（重置态）不带翻页标记」的判定输入。
    const { wrapper, handler } = (await mountAndCapture()) as any
    const chat = useChatStore()
    expect(chat.oldestIndex).toBeNull()

    const rid = vi.mocked(sendHistoryRequest).mock.calls[0][0] as string
    handler(historyFrame({ rid, lastSeq: 50, oldestIndex: 0, hasMore: false }))
    await flushPromises()

    // 首拉语义仍在：响应后 oldestIndex 回写。
    expect(chat.oldestIndex).toBe(0)
    wrapper.unmount()
  })
})
