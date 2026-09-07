import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount, flushPromises } from '@vue/test-utils'
import { nextTick } from 'vue'

// I2（devtool-upgrade 阶段 3）：ChatPanel 的 @文件引用补全 —— 输入尾部
// `@片段` 防抖拉 fs.complete_path、菜单选择原位替换 token；邮箱/非词首
// @ 不触发；失败静默。

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

async function mountPanel() {
  const wrapper = mount(ChatPanel)
  await flushPromises()
  return wrapper
}

/// ChatPanel 生命周期自身会轮询 inbox_status 等 —— 断言只看 fs 模块的调用。
function fsCalls() {
  return requestMock.mock.calls.filter((c: any[]) => c[0] === 'fs')
}

beforeEach(() => {
  setActivePinia(createPinia())
  requestMock.mockReset()
  vi.useFakeTimers()
})

afterEach(() => {
  vi.useRealTimers()
})

describe('ChatPanel @文件引用补全', () => {
  it('尾部 @片段 → 防抖调 fs.complete_path，菜单列出路径', async () => {
    requestMock.mockResolvedValue({ paths: ['src/main.rs', 'src/'], truncated: false })
    const wrapper = await mountPanel()
    const chat = useChatStore()

    chat.input = '看一下 @src'
    await vi.advanceTimersByTimeAsync(200)
    await flushPromises()

    expect(requestMock).toHaveBeenCalledWith('fs', 'complete_path', { prefix: 'src' })
    const menu = wrapper.find('.slash-menu')
    expect(menu.exists()).toBe(true)
    expect(menu.text()).toContain('src/main.rs')
    wrapper.unmount()
  })

  it('邮箱 user@x（非词首 @）→ 不请求不出菜单', async () => {
    const wrapper = await mountPanel()
    const chat = useChatStore()

    chat.input = '联系 user@example.com'
    await vi.advanceTimersByTimeAsync(200)
    await flushPromises()

    expect(fsCalls()).toHaveLength(0)
    expect(wrapper.find('.slash-menu').exists()).toBe(false)
    wrapper.unmount()
  })

  it('选择路径 → @token 原位替换为 `@path `，菜单关闭', async () => {
    requestMock.mockResolvedValue({ paths: ['src/main.rs'], truncated: false })
    const wrapper = await mountPanel()
    const chat = useChatStore()

    chat.input = '看 @ma'
    await vi.advanceTimersByTimeAsync(200)
    await flushPromises()

    const item = wrapper.findAll('.slash-item')[0]
    await item.trigger('mousedown')
    expect(chat.input).toBe('看 @src/main.rs ')
    expect(wrapper.find('.slash-menu').exists()).toBe(false)
    wrapper.unmount()
  })

  it('Enter 选中 → 同样原位替换（keydown 接管）', async () => {
    requestMock.mockResolvedValue({ paths: ['README.md'], truncated: false })
    const wrapper = await mountPanel()
    const chat = useChatStore()

    chat.input = '@READ'
    await vi.advanceTimersByTimeAsync(200)
    await flushPromises()
    expect(wrapper.find('.slash-menu').exists()).toBe(true)

    await wrapper.find('textarea').trigger('keydown', { key: 'Enter' })
    expect(chat.input).toBe('@README.md ')
    wrapper.unmount()
  })

  it('Esc 关闭菜单；后端失败 → 静默无菜单不抛错', async () => {
    requestMock.mockRejectedValue(new Error('boom'))
    const wrapper = await mountPanel()
    const chat = useChatStore()

    chat.input = '@xyz'
    await vi.advanceTimersByTimeAsync(200)
    await flushPromises()
    expect(wrapper.find('.slash-menu').exists()).toBe(false)

    // 失败后继续输入（token 增长）且后端恢复 → 菜单恢复
    requestMock.mockResolvedValue({ paths: ['x.txt'], truncated: false })
    chat.input = '@xyza'
    await vi.advanceTimersByTimeAsync(200)
    await flushPromises()
    expect(wrapper.find('.slash-menu').exists()).toBe(true)
    await wrapper.find('textarea').trigger('keydown', { key: 'Escape' })
    await nextTick()
    expect(wrapper.find('.slash-menu').exists()).toBe(false)
    wrapper.unmount()
  })

  it('@token 已闭合（后跟空白）→ 不出菜单', async () => {
    const wrapper = await mountPanel()
    const chat = useChatStore()

    chat.input = '已引用 @src/main.rs 继续'
    await vi.advanceTimersByTimeAsync(200)
    await flushPromises()

    expect(fsCalls()).toHaveLength(0)
    expect(wrapper.find('.slash-menu').exists()).toBe(false)
    wrapper.unmount()
  })
})
