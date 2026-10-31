import { describe, it, expect, vi, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount, flushPromises } from '@vue/test-utils'

// BUG-20261031-253：rewind 成功 toast 的文件回滚计数恒显示「写 0 / 删 0」。
// - 根因：AgentLoop rewind/redo 响应把恢复文件清单放嵌套 `restored_files`
//   下（rewind.rs 成功路径 json!），rewindToastSummary 读的是顶层
//   `resp?.written` / `resp?.deleted`——恒 undefined → `?? 0` 恒 0。
//   出生即失配（8e80caa2，2026-09-07 同提交内 BE 嵌套、FE 读顶层）。
// - 修复：FE 改读 `resp?.restored_files?.written?.length`。
// 本 spec 用真实响应形状（嵌套 restored_files）钉住 toast 计数非零。

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
import { sendHistoryRequest } from '../../composables/useWebSocket'
import { useSessionStore } from '../../stores/session'
import { useToast } from '../../composables/useToast'

beforeEach(() => {
  setActivePinia(createPinia())
  requestMock.mockReset()
  // 默认全成功空响应；rewind 用例内覆写。
  requestMock.mockResolvedValue({})
  h.wsHandler = null
  h.wsStatus.value = 'connected'
})

function prepareSessions() {
  const s = useSessionStore()
  s.sessions = [
    { id: 's1', channel: 'web', startTime: '', lastTime: '', messageCount: 0, firstMessage: 'A', model: '' },
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

describe('ChatPanel rewind toast 文件回滚计数（restored_files 嵌套键）', () => {
  it('rewind 成功响应（嵌套 restored_files）→ toast 显示真实写/删计数而非恒 0', async () => {
    prepareSessions()
    const w = mount(ChatPanel)
    await flushPromises()
    feedHistory(lastHistoryRequestId(), [
      { role: 'user', content: 'q1' },
      { role: 'assistant', content: 'a1' },
    ])
    await flushPromises()

    // AgentLoop rewind_from_cut 成功路径的真实响应形状（rewind.rs json!）：
    // 文件清单在嵌套 restored_files 下。
    requestMock.mockImplementation(async (_mod: string, cmd: string) => {
      if (cmd === 'rewind_to_message') {
        return {
          session_key: 's1',
          message_index: 0,
          kept_count: 0,
          removed_count: 2,
          file_restore: 'applied',
          restored_files: { written: ['src/lib.rs'], deleted: ['tmp/old.rs'] },
          redoable: true,
          forced: false,
        }
      }
      return {} // resync / 其他命令默认空响应
    })

    // user 行的「⏪ 撤销此问」→ rewindToTurnStart → rewindToastSummary。
    const undoBtn = w.findAll('.msg-action-btn').find(b => b.text().includes('撤销此问'))
    expect(undoBtn, 'user 行应有撤销此问入口').toBeTruthy()
    await undoBtn!.trigger('click')
    await flushPromises()

    expect(requestMock).toHaveBeenCalledWith('sessions', 'rewind_to_message', {
      session_id: 's1',
      message_index: 0,
      mode: 'before',
    })
    const toasts = useToast().toasts
    expect(
      toasts.some(t => t.message.includes('写 1 / 删 1')),
      `toast 应显示真实计数「写 1 / 删 1」，实际：${toasts.map(t => t.message).join(' | ')}`,
    ).toBe(true)
    w.unmount()
  })
})
