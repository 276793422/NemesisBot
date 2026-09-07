import { describe, it, expect, vi, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount, flushPromises } from '@vue/test-utils'

// F1（devtool-upgrade 阶段 4）：ChatPanel plan/build 模式徽标——
// chat.get_mode 进会对齐、chat.set_mode 点击切换、ModeChanged push 实时刷新。

const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
}))

/** 捕获 onMessage 注册的处理函数（推 ModeChanged 帧用）。 */
let wsHandler: ((data: any) => void) | null = null

vi.mock('../../composables/useWebSocket', async () => {
  const { ref } = await import('vue')
  return {
    connect: vi.fn(),
    send: vi.fn(),
    sendHistoryRequest: vi.fn(),
    onMessage: vi.fn((fn: (data: any) => void) => { wsHandler = fn }),
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
  wsHandler = null
})

async function mountPanel() {
  const w = mount(ChatPanel)
  await flushPromises()
  return w
}

function modeBtn(w: ReturnType<typeof mount>) {
  return w.find('.mode-btn')
}

describe('ChatPanel 模式徽标（F1）', () => {
  it('默认 build 徽标；挂载时 chat.get_mode 对齐真实模式', async () => {
    requestMock.mockImplementation((_mod: string, cmd: string) => {
      if (cmd === 'get_mode') return Promise.resolve({ mode: 'build', session_id: 's1' })
      return Promise.resolve({})
    })
    const w = await mountPanel()
    expect(modeBtn(w).exists()).toBe(true)
    expect(modeBtn(w).text()).toContain('构建')
    // 挂载即拉取 get_mode
    expect(requestMock).toHaveBeenCalledWith('chat', 'get_mode', { session_id: 's1' })
    w.unmount()
  })

  it('get_mode 返回 plan → 徽标显示计划 + plan 常驻条出现', async () => {
    requestMock.mockImplementation((_mod: string, cmd: string) => {
      if (cmd === 'get_mode') return Promise.resolve({ mode: 'plan' })
      return Promise.resolve({})
    })
    const w = await mountPanel()
    await flushPromises()
    expect(modeBtn(w).text()).toContain('计划')
    expect(modeBtn(w).classes()).toContain('mode-plan')
    expect(w.find('.plan-strip').exists()).toBe(true)
    w.unmount()
  })

  it('点击徽标 → chat.set_mode plan → 徽标翻转', async () => {
    const w = await mountPanel()
    requestMock.mockImplementation((_mod: string, cmd: string, _data: any) => {
      if (cmd === 'set_mode') return Promise.resolve({ mode: 'plan' })
      return Promise.resolve({})
    })
    await modeBtn(w).trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('chat', 'set_mode', { session_id: 's1', mode: 'plan' })
    expect(w.text()).toContain('计划')
    w.unmount()
  })

  it('set_mode 失败 → 徽标不翻转', async () => {
    const w = await mountPanel()
    requestMock.mockImplementation((_mod: string, cmd: string) => {
      if (cmd === 'set_mode') return Promise.reject(new Error('unknown mode'))
      return Promise.resolve({})
    })
    await modeBtn(w).trigger('click')
    await flushPromises()
    expect(modeBtn(w).text()).toContain('构建')
    w.unmount()
  })

  it('ModeChanged push（kind=ModeChanged, chat_id 匹配当前会话）→ 徽标实时刷新', async () => {
    const w = await mountPanel()
    expect(wsHandler).not.toBeNull()
    wsHandler!({
      type: 'push',
      cmd: 'tool_event',
      data: { kind: 'ModeChanged', data: { session_key: 'agent:main:session:s1', chat_id: 'web:s1', mode: 'plan' } },
    })
    await flushPromises()
    expect(modeBtn(w).text()).toContain('计划')
    expect(w.find('.plan-strip').exists()).toBe(true)
    w.unmount()
  })

  it('ModeChanged push chat_id 不匹配 → 忽略', async () => {
    const w = await mountPanel()
    wsHandler!({
      type: 'push',
      cmd: 'tool_event',
      data: { kind: 'ModeChanged', data: { session_key: 'x', chat_id: 'web:other', mode: 'plan' } },
    })
    await flushPromises()
    expect(modeBtn(w).text()).toContain('构建')
    w.unmount()
  })

  it('非默认 chat 模块（workflow_chat）不渲染徽标', async () => {
    const w = mount(ChatPanel, { props: { module: 'workflow_chat' } })
    await flushPromises()
    expect(modeBtn(w).exists()).toBe(false)
    expect(w.find('.plan-strip').exists()).toBe(false)
    w.unmount()
  })
})

describe('chat store agentMode（F1）', () => {
  it('setAgentMode 更新；reset 回落 build', () => {
    const store = useChatStore()
    expect(store.agentMode).toBe('build')
    store.setAgentMode('plan')
    expect(store.agentMode).toBe('plan')
    store.reset()
    expect(store.agentMode).toBe('build')
  })
})
