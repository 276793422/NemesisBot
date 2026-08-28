import { describe, it, expect, vi, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount, flushPromises } from '@vue/test-utils'
import { nextTick } from 'vue'

// U7 inbox visibility (G1): ChatPanel 的排队 chip、steer hint、`!` 插队
// 按钮与 busy 发送解锁 —— 按后端 inbox_status 的 mode 驱动渲染。

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
    removeMessageHandler: vi.fn(),
    wsStatus: ref('connected'),
  }
})

import { send, wsStatus } from '../../composables/useWebSocket'
import type { InboxStatusData } from '../../composables/useInboxStatus'
import ChatPanel from '../ChatPanel.vue'
import { useChatStore } from '../../stores/chat'

function status(over: Partial<InboxStatusData>): InboxStatusData {
  return {
    available: true,
    session_key: 'agent:main:session:s1',
    next_turn: 0,
    next_step: 0,
    capacity: 8,
    busy: false,
    mode: 'reject',
    ...over,
  }
}

async function mountPanel() {
  const wrapper = mount(ChatPanel)
  await flushPromises()
  return wrapper
}

beforeEach(() => {
  setActivePinia(createPinia())
  requestMock.mockReset()
  vi.mocked(send).mockReset()
  wsStatus.value = 'connected'
})

describe('ChatPanel U7 可见性', () => {
  it('steer 模式：工具栏出现「插队」按钮，点击给输入加 ! 前缀', async () => {
    requestMock.mockResolvedValue(status({ mode: 'steer' }))
    const wrapper = await mountPanel()
    const chat = useChatStore()

    const btn = wrapper.find('.steer-btn')
    expect(btn.exists()).toBe(true)

    chat.input = '别删那行'
    await btn.trigger('click')
    expect(chat.input).toBe('! 别删那行')
    // 已有前缀时不重复加
    await btn.trigger('click')
    expect(chat.input).toBe('! 别删那行')
    wrapper.unmount()
  })

  it('steer 模式：busy 中输入 ! 开头 → steer hint 出现', async () => {
    requestMock.mockResolvedValue(status({ mode: 'steer', busy: true }))
    const wrapper = await mountPanel()
    const chat = useChatStore()

    chat.input = '看看进度'
    await nextTick()
    expect(wrapper.find('.steer-hint').exists()).toBe(false)

    chat.input = '! 停一下，别删'
    await nextTick()
    expect(wrapper.find('.steer-hint').exists()).toBe(true)
    expect(wrapper.find('.steer-hint').text()).toContain('插队')
    wrapper.unmount()
  })

  it('steer 模式：busy 中可继续发送（textarea 不禁用、发送按钮在、点按走 send）', async () => {
    requestMock.mockResolvedValue(status({ mode: 'steer', busy: true }))
    const wrapper = await mountPanel()
    const chat = useChatStore()
    chat.streaming = true
    await nextTick()

    const textarea = wrapper.find('textarea')
    expect(textarea.attributes('disabled')).toBeUndefined()

    const sendBtn = wrapper.findAll('button').find(b => b.text() === '发送')
    expect(sendBtn).toBeDefined()

    chat.input = '插队消息'
    await nextTick() // 让 :disabled 绑定刷新，否则 jsdom 对 disabled 按钮不派发 click
    await sendBtn!.trigger('click')
    await flushPromises()
    expect(vi.mocked(send)).toHaveBeenCalledTimes(1)
    expect(vi.mocked(send).mock.calls[0][0]).toBe('插队消息')
    wrapper.unmount()
  })

  it('busy 中有排队消息 → chip 显示条数与插队数；满员追加「队列已满」', async () => {
    requestMock.mockResolvedValue(status({ mode: 'steer', busy: true, next_turn: 2, next_step: 1 }))
    const wrapper = await mountPanel()
    const chat = useChatStore()

    chat.streaming = true
    await flushPromises()
    const chip = wrapper.find('.queue-chip')
    expect(chip.exists()).toBe(true)
    expect(chip.text()).toContain('已排队 3 条')
    expect(chip.text()).toContain('插队 1')
    expect(chip.classes()).not.toContain('full')

    // 满员：换快照后经 streaming=false→true 的 watch（stopPolling+syncInboxMode）重新拉取
    requestMock.mockResolvedValue(status({ mode: 'steer', busy: true, next_turn: 8, capacity: 8 }))
    chat.streaming = false
    await flushPromises()
    chat.streaming = true
    await flushPromises()
    expect(wrapper.find('.queue-chip').classes()).toContain('full')
    expect(wrapper.find('.queue-chip').text()).toContain('队列已满')
    wrapper.unmount()
  })

  it('reject 模式：无插队按钮；busy 中 textarea 禁用且发送按钮消失', async () => {
    requestMock.mockResolvedValue(status({ mode: 'reject' }))
    const wrapper = await mountPanel()
    const chat = useChatStore()

    expect(wrapper.find('.steer-btn').exists()).toBe(false)

    chat.streaming = true
    await nextTick()
    expect(wrapper.find('textarea').attributes('disabled')).toBeDefined()
    expect(wrapper.findAll('button').find(b => b.text() === '发送')).toBeUndefined()
    wrapper.unmount()
  })

  it('reject 模式：空态不显示 chip，不显示 steer hint', async () => {
    requestMock.mockResolvedValue(status({ mode: 'reject', busy: false }))
    const wrapper = await mountPanel()
    const chat = useChatStore()
    chat.input = '!abc'
    chat.streaming = true
    await nextTick()
    expect(wrapper.find('.queue-chip').exists()).toBe(false)
    expect(wrapper.find('.steer-hint').exists()).toBe(false)
    wrapper.unmount()
  })
})
