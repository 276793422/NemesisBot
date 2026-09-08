import { describe, it, expect, vi, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount, flushPromises } from '@vue/test-utils'

// 路由卸载重挂载补偿回归钉（2026-09-08 用户报告的 UI bug）：
// 离开聊天页（切到定时/Skills 等路由再回来）期间 ChatPanel 被卸载
// （AppLayout 的 router-view 无 KeepAlive），messageHandler 随之移除——
// 卸载窗口内晚到的 receive 帧无人接收而丢失（后端 session_log 照常落盘）。
// Pinia store 是全局单例、historyLoaded 残留 true，重挂载若跳过重载，
// 视图停留在旧数据直到手动 F5。修后：重挂载即 reset + 游标归零 +
// loadHistory 全量重拉（与 onSSEResync 同链）。

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

import { sendHistoryRequest } from '../../composables/useWebSocket'
import ChatPanel from '../ChatPanel.vue'
import { useChatStore } from '../../stores/chat'
import { useSessionStore } from '../../stores/session'

beforeEach(() => {
  setActivePinia(createPinia())
  requestMock.mockReset()
  requestMock.mockResolvedValue({})
  vi.mocked(sendHistoryRequest).mockClear()
})

describe('路由卸载重挂载 → 强制全量重拉（丢帧窗口闭合）', () => {
  it('store 残留 historyLoaded=true 的重挂载：reset 清旧视图 + history_request 重发（session_id 路由正确）', async () => {
    const sessionStore = useSessionStore()
    sessionStore.sessions = [
      { id: 's1', channel: 'web', startTime: '', lastTime: '', messageCount: 2, firstMessage: '项目会话', model: '' },
    ] as any
    sessionStore.currentId = 's1'

    // 模拟「上一挂载周期」残留：Pinia 全局单例跨路由存活，卸载期间晚到
    // 的回复帧丢了、但 historyLoaded 仍是 true。
    const chat = useChatStore()
    chat.historyLoaded = true
    chat.messages = [{ role: 'user', content: '旧消息（重拉后必须消失）' }] as any

    const wrapper = mount(ChatPanel)
    await flushPromises()

    // reset 生效：旧视图清空（等 loadHistory 响应回来重建）。
    expect(chat.messages).toHaveLength(0)
    // history_request 重发，且 moduleData 带当前会话 id（后端按会话路由）。
    expect(sendHistoryRequest).toHaveBeenCalledTimes(1)
    const call = vi.mocked(sendHistoryRequest).mock.calls[0]
    expect(call[call.length - 1]).toMatchObject({ moduleData: { session_id: 's1' } })

    wrapper.unmount()
  })

  it('首次挂载（historyLoaded=false）仍走原首拉路径，不重复 reset', async () => {
    const sessionStore = useSessionStore()
    sessionStore.sessions = [
      { id: 's1', channel: 'web', startTime: '', lastTime: '', messageCount: 0, firstMessage: 'A', model: '' },
    ] as any
    sessionStore.currentId = 's1'

    const wrapper = mount(ChatPanel)
    await flushPromises()

    expect(sendHistoryRequest).toHaveBeenCalledTimes(1)
    const chat = useChatStore()
    // 首拉路径不清 messages（本就为空），只置加载态。
    expect(chat.messages).toHaveLength(0)
    wrapper.unmount()
  })
})
