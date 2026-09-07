import { describe, it, expect, vi, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount, flushPromises } from '@vue/test-utils'

// E2（2026-09-05）：ChatPanel 并发回执徽章——识别 AgentLoop 的
// ⏳ 排队 / ⚡ 插话回执（具体前缀匹配，不认裸 emoji——/compact 忙时
// 忽略回执也是 ⏳ 开头，语义却是"已忽略"）；user 消息永不带徽章。

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

import ChatPanel from '../ChatPanel.vue'
import { useChatStore } from '../../stores/chat'
import { useSessionStore } from '../../stores/session'

beforeEach(() => {
  setActivePinia(createPinia())
  useSessionStore().currentId = 's1'
  requestMock.mockReset()
  requestMock.mockResolvedValue({})
})

async function mountWithMessages(msgs: Array<{ role: string; content: string; model?: string }>) {
  const store = useChatStore()
  for (const m of msgs) {
    store.messages.push({ role: m.role, content: m.content, timestamp: 't1' } as any)
  }
  const w = mount(ChatPanel)
  await flushPromises()
  return w
}

function badgesOf(w: ReturnType<typeof mount>) {
  return w.findAll('.concurrency-badge').map(b => b.text())
}

describe('ChatPanel 并发回执徽章（E2）', () => {
  it('⏳ 排队回执 → 「已排队」徽章', async () => {
    const w = await mountWithMessages([
      { role: 'assistant', content: '⏳ 当前正在处理上一条消息。你的消息已排队，将在本轮结束后继续处理。' },
    ])
    expect(badgesOf(w)).toEqual(['已排队'])
    expect(w.find('.concurrency-badge').classes()).toContain('badge-info')
    w.unmount()
  })

  it('⚡ 插话回执 → 「插话」徽章', async () => {
    const w = await mountWithMessages([
      { role: 'assistant', content: '⚡ 已接收为紧急插话（消息以 ! 开头），将在 AI 的下一步思考前注入。非紧急消息请去掉 ! 前缀排队等待。' },
    ])
    expect(badgesOf(w)).toEqual(['插话'])
    expect(w.find('.concurrency-badge').classes()).toContain('badge-warning')
    w.unmount()
  })

  it('⏳ 排队满回执 → 「排队满」徽章', async () => {
    const w = await mountWithMessages([
      { role: 'assistant', content: '⏳ 排队已满，消息未能接收。请等当前任务完成后再发。' },
    ])
    expect(badgesOf(w)).toEqual(['排队满'])
    expect(w.find('.concurrency-badge').classes()).toContain('badge-error')
    w.unmount()
  })

  it('忙时 /compact 忽略回执（⏳ 开头但非排队）与普通回复 → 无徽章', async () => {
    const w = await mountWithMessages([
      { role: 'assistant', content: '⏳ 会话正在处理消息，本次 /compact /clear 已忽略，请稍后再试' },
      { role: 'assistant', content: '普通的回答内容' },
    ])
    expect(badgesOf(w)).toEqual([])
    w.unmount()
  })

  it('user 消息即使以 ⏳/⚡ 开头也不带徽章（role 守卫）', async () => {
    const w = await mountWithMessages([
      { role: 'user', content: '⏳ 当前正在处理上一条消息。你的消息已排队，将在本轮结束后继续处理。' },
      { role: 'user', content: '⚡ 已接收为紧急插话' },
    ])
    expect(badgesOf(w)).toEqual([])
    w.unmount()
  })

  it('混合流：排队回执 + 正常回答，只有回执带徽章', async () => {
    const w = await mountWithMessages([
      { role: 'assistant', content: '⏳ 当前正在处理上一条消息。你的消息已排队，将在本轮结束后继续处理。' },
      { role: 'assistant', content: '排队轮的正式回答', model: 'test/model-1' },
    ])
    expect(badgesOf(w)).toEqual(['已排队'])
    // 正文渲染不受徽章影响
    expect(w.text()).toContain('排队轮的正式回答')
    w.unmount()
  })
})
