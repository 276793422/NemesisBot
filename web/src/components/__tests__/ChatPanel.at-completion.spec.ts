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

  // BUG-20261028-134：token 删除须撤销在途 debounce——旧行为只清 atItems
  // 不清定时器，150ms 后请求照发、回包把已关闭的菜单重新弹开。
  it('debounce 到期前 token 已删除 → 不发请求不出幽灵菜单', async () => {
    const wrapper = await mountPanel()
    const chat = useChatStore()

    chat.input = '看 @src'
    await vi.advanceTimersByTimeAsync(100) // debounce 未到
    chat.input = '看 ' // token 消失
    await nextTick()
    await vi.advanceTimersByTimeAsync(300)
    await flushPromises()

    expect(fsCalls()).toHaveLength(0)
    expect(wrapper.find('.slash-menu').exists()).toBe(false)
    wrapper.unmount()
  })

  it('在途回包到达前 token 已删除 → 回包不弹幽灵菜单', async () => {
    let resolveReq!: (v: { paths: string[]; truncated: boolean }) => void
    requestMock.mockReturnValue(new Promise(resolve => { resolveReq = resolve }))
    const wrapper = await mountPanel()
    const chat = useChatStore()

    chat.input = '看 @src'
    await vi.advanceTimersByTimeAsync(200) // 请求已发出、回包挂起
    expect(fsCalls()).toHaveLength(1)
    chat.input = '看 ' // token 删除
    await nextTick()
    resolveReq({ paths: ['src/main.rs'], truncated: false })
    await flushPromises()

    expect(wrapper.find('.slash-menu').exists()).toBe(false)
    wrapper.unmount()
  })

  it('陈旧失败回包不清掉新 token 刚填的菜单', async () => {
    let rejectReq!: (e: Error) => void
    requestMock.mockReturnValueOnce(new Promise((_res, rej) => { rejectReq = rej }))
    requestMock.mockResolvedValue({ paths: ['new.txt'], truncated: false })
    const wrapper = await mountPanel()
    const chat = useChatStore()

    chat.input = '@ol'
    await vi.advanceTimersByTimeAsync(200) // 第一发已发出（挂起，将失败）
    chat.input = '@old' // token 增长 → 第二发排队
    await vi.advanceTimersByTimeAsync(200)
    await flushPromises() // 第二发成功 → 菜单出现
    expect(wrapper.find('.slash-menu').text()).toContain('new.txt')

    rejectReq(new Error('stale boom')) // 旧请求失败迟到
    await flushPromises()
    expect(wrapper.find('.slash-menu').exists()).toBe(true) // 不被陈旧失败清掉
    wrapper.unmount()
  })
})
