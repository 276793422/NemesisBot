import { describe, it, expect, vi, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount, flushPromises } from '@vue/test-utils'

// P5（2026-09-21）回归钉：项目 chip（⟦项目名⟧）的数据链——
// 此前 fetchProjects 全工程唯一调用点在 SessionSidebar.onMounted，而侧栏
// 默认收起（v-if 不挂载）→ 注册表恒空 → projectNameOf 恒 null → 空项目
// 会话的欢迎块 chip 永不显示。修复后 ChatView.onMounted 即拉注册表；
// 本 spec 模拟「注册表有数据 + 当前会话归属项目 + 空历史」→ chip 渲染。
// 注：chip 只存在于欢迎气泡（历史非空时欢迎块不渲染，属既有设计）。

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

vi.mock('../../composables/useSSE', () => ({
  on: vi.fn(),
  off: vi.fn(),
}))

import { onMessage } from '../../composables/useWebSocket'
import ChatPanel from '../ChatPanel.vue'
import { useSessionStore } from '../../stores/session'

// api.listProjects 由 ChatView 调用；ChatPanel 挂载不触发——这里直接预置
// store 注册表（等价于 fetchProjects 已完成的形态）。

beforeEach(() => {
  setActivePinia(createPinia())
  requestMock.mockReset()
  requestMock.mockResolvedValue({})
  vi.mocked(onMessage).mockClear()
})

describe('P5：项目 chip 数据链（注册表 → projectNameOf → 欢迎块）', () => {
  it('空历史 + 项目会话 → 欢迎块 chip 显示项目名', async () => {
    const sessionStore = useSessionStore()
    sessionStore.projects = [{ id: 'p-demo', name: '演示项目', path: 'D:/demo' }] as any
    sessionStore.sessions = [
      {
        id: 's1',
        channel: 'web',
        startTime: '',
        lastTime: '',
        messageCount: 0,
        firstMessage: '',
        model: '',
        projectId: 'p-demo',
      },
    ] as any
    sessionStore.currentId = 's1'

    const w = mount(ChatPanel)
    await flushPromises()

    // 历史为空（feed 不发生）→ 欢迎块渲染 + chip 联结出项目名。
    expect(w.find('.project-chip').exists()).toBe(true)
    expect(w.find('.project-chip').text()).toContain('演示项目')
    w.unmount()
  })

  it('注册表无此 pid（已移除/未知）→ chip 不显示（不误导）', async () => {
    const sessionStore = useSessionStore()
    sessionStore.projects = [] as any
    sessionStore.sessions = [
      {
        id: 's1',
        channel: 'web',
        startTime: '',
        lastTime: '',
        messageCount: 0,
        firstMessage: '',
        model: '',
        projectId: 'p-gone',
      },
    ] as any
    sessionStore.currentId = 's1'

    const w = mount(ChatPanel)
    await flushPromises()
    expect(w.find('.project-chip').exists()).toBe(false)
    // 欢迎语本体仍在。
    expect(w.text()).toContain('NemesisBot')
    w.unmount()
  })
})
