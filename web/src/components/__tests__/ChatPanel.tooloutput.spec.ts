import { describe, it, expect, vi, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount, flushPromises } from '@vue/test-utils'

// 2026-11-08（子进程实时输出与完成唤醒）：ChatPanel tool_event 帧 →
// ToolOutput 分支 → store running 条目 live_tail 端到端。会话过滤沿用
// 既有 tool_event 通道（chat_id web: 前缀 legacy 豁免 / session_id 精确）。

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

function wsHandler(): (frame: any) => void {
  const calls = vi.mocked(onMessage).mock.calls
  expect(calls.length, 'onMessage must have been called on mount').toBeGreaterThan(0)
  return calls[calls.length - 1][0] as (frame: any) => void
}

function frame(kind: string, data: Record<string, unknown>): any {
  return { type: 'push', cmd: 'tool_event', data: { kind, data: { chat_id: 'web:s1', ...data } } }
}

beforeEach(() => {
  setActivePinia(createPinia())
  requestMock.mockReset()
  requestMock.mockResolvedValue({})
  vi.mocked(onMessage).mockClear()
})

describe('ChatPanel ToolOutput 帧 → store live_tail (2026-11-08)', () => {
  it('Started → ToolOutput → running 条目携带尾窗；Finished 清除', async () => {
    const wrapper = await mount(ChatPanel)
    await flushPromises()
    const h = wsHandler()
    const store = useChatStore()

    h(frame('ToolStarted', { call_id: 'c1', tool: 'exec', args_preview: 'ping' }))
    h(frame('ToolOutput', { call_id: '', tool: 'exec', tail: 'PING 1.2.3.4\n', total_bytes: 13 }))
    await flushPromises()
    const t = store.pendingWork?.rounds.at(-1)?.tools[0]
    expect(t?.running).toBe(true)
    expect(t?.live_tail).toBe('PING 1.2.3.4\n')
    expect(t?.live_total_bytes).toBe(13)

    h(
      frame('ToolFinished', {
        call_id: 'c1',
        tool: 'exec',
        duration_ms: 4200,
        ok: true,
        result_preview: 'ping statistics ---',
      }),
    )
    await flushPromises()
    const done = store.pendingWork?.rounds.at(-1)?.tools[0]
    expect(done?.live_tail).toBeUndefined()
    expect(done?.result_preview).toBe('ping statistics ---')
    wrapper.unmount()
  })

  it('空 tail 帧诚实忽略（不覆盖既有尾窗为空）', async () => {
    const wrapper = await mount(ChatPanel)
    await flushPromises()
    const h = wsHandler()
    const store = useChatStore()

    h(frame('ToolStarted', { call_id: 'c1', tool: 'exec' }))
    h(frame('ToolOutput', { call_id: '', tool: 'exec', tail: 'real\n', total_bytes: 5 }))
    h(frame('ToolOutput', { call_id: '', tool: 'exec', tail: '', total_bytes: 5 }))
    await flushPromises()
    expect(store.pendingWork?.rounds.at(-1)?.tools[0].live_tail).toBe('real\n')
    wrapper.unmount()
  })
})
