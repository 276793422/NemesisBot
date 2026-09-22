import { describe, it, expect, vi, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount, flushPromises } from '@vue/test-utils'

// 集群续行归属（2026-09-23）：worker 节点回复在 assistant 行渲染
// 「节点 X」徽章（与模型徽章并列）。覆盖实时 receive 帧、user 行守卫、
// chat.sync 断线补拉重放三条入路 + 缺席不渲染（历史行为逐字节）。

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

beforeEach(() => {
  setActivePinia(createPinia())
  useSessionStore().currentId = 's1'
  requestMock.mockReset()
  requestMock.mockResolvedValue({})
  h.wsHandler = null
  h.wsStatus.value = 'connected'
})

async function mountPanel() {
  const w = mount(ChatPanel)
  await flushPromises()
  return w
}

/** 实时 receive 帧构造（后端 send_to_session 帧形状）。 */
function receiveFrame(over: Record<string, any> = {}) {
  return {
    type: 'message',
    module: 'chat',
    cmd: 'receive',
    timestamp: '2026-09-23T10:00:00+08:00',
    data: { role: 'assistant', content: '集群回复正文', session_id: 's1', ...over },
  }
}

describe('ChatPanel 节点徽章（集群续行归属）', () => {
  it('receive 帧带 source_node → 节点徽章与模型徽章并列渲染', async () => {
    const w = await mountPanel()
    h.wsHandler!(receiveFrame({
      role: 'assistant',
      content: '集群回复正文',
      model: 'zhipu/glm-4.7',
      source_node: 'node-b',
      seq: 1,
    }))
    await flushPromises()
    const badge = w.find('.model-badge.node-badge')
    expect(badge.exists()).toBe(true)
    expect(badge.text()).toContain('节点 node-b')
    // 模型徽章照常（转述文本由主节点模型生成——各说各的真话）。
    const badges = w.findAll('.model-badge').map(b => b.text())
    expect(badges.some(t => t.includes('zhipu · glm-4.7'))).toBe(true)
    w.unmount()
  })

  it('receive 帧无 source_node → 不渲染节点徽章（缺省逐字节历史行为）', async () => {
    const w = await mountPanel()
    h.wsHandler!(receiveFrame({ role: 'assistant', content: '普通回复', model: 'm1', seq: 1 }))
    await flushPromises()
    expect(w.find('.node-badge').exists()).toBe(false)
    expect(w.text()).toContain('普通回复')
    w.unmount()
  })

  it('user 行带 source_node 也不渲染徽章（模板守卫 role=assistant）', async () => {
    const w = await mountPanel()
    h.wsHandler!(receiveFrame({ role: 'user', content: '用户行', source_node: 'node-b', seq: 1 }))
    await flushPromises()
    expect(w.find('.node-badge').exists()).toBe(false)
    w.unmount()
  })

  it('chat.sync 补拉重放带 source_node → 徽章（断线重连窗口不丢归属）', async () => {
    const w = await mountPanel()
    // 首连 loadHistory 走 sendHistoryRequest 通道（mock 无响应）→
    // historyLoading 悬挂 true 会令 syncMissedChat 早退——手动置齐两态，
    // 随后断开→重连触发 syncMissedChat。
    const chatStore = useChatStore()
    chatStore.historyLoading = false
    chatStore.historyLoaded = true
    requestMock.mockImplementation((_mod: string, cmd: string) => {
      if (cmd === 'sync') {
        return Promise.resolve({
          events: [
            { seq: 1, role: 'assistant', content: '补拉的集群回复', model: 'm1', source_node: 'node-c', ts: '2026-09-23T10:00:00+08:00' },
            { seq: 2, role: 'assistant', content: '补拉的普通回复', model: 'm1', ts: '2026-09-23T10:00:01+08:00' },
          ],
        })
      }
      return Promise.resolve({})
    })
    h.wsStatus.value = 'disconnected'
    await flushPromises()
    h.wsStatus.value = 'connected'
    await flushPromises()
    const badges = w.findAll('.node-badge').map(b => b.text())
    expect(badges).toHaveLength(1)
    expect(badges[0]).toContain('节点 node-c')
    expect(w.text()).toContain('补拉的普通回复')
    w.unmount()
  })
})
