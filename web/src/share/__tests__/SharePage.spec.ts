import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'

// L4（2026-09-07）：SharePage —— 独立只读分享页。fetch 打桩钉三态：
// 成功渲染白名单字段 / 404 诚实报错 / 缺 token 提示。

import SharePage from '../SharePage.vue'

const fetchMock = vi.fn()

beforeEach(() => {
  fetchMock.mockReset()
  vi.stubGlobal('fetch', fetchMock)
  // jsdom 无 URLSearchParams(location.search) 问题，但要用 ?t= 注入就得换地址
  window.history.replaceState({}, '', '/share/?t=tokabc')
})

afterEach(() => {
  vi.unstubAllGlobals()
  window.history.replaceState({}, '', '/')
})

describe('SharePage', () => {
  it('成功：fetch /api/share/{t} 并渲染 role/content/model/时间/图片占位', async () => {
    fetchMock.mockResolvedValue({
      ok: true,
      json: async () => ({
        title: '我的会话',
        created_at: '2026-09-07T02:00:00Z',
        view: 'live',
        messages: [
          { role: 'user', content: '你好', timestamp: '2026-09-07T02:00:01Z', image_count: 2 },
          { role: 'assistant', content: '你好呀', timestamp: '2026-09-07T02:00:05Z', model: 'test/testai-1.1' },
        ],
      }),
    })
    const w = mount(SharePage)
    await flushPromises()

    expect(fetchMock).toHaveBeenCalledWith('/api/share/tokabc')
    expect(w.find('.share-title').text()).toBe('我的会话')
    const msgs = w.findAll('.msg')
    expect(msgs.length).toBe(2)
    expect(msgs[0].classes()).toContain('user')
    expect(msgs[0].find('.msg-content').text()).toBe('你好')
    // 图片只显示数量占位（白名单契约的前端面）
    expect(msgs[0].find('.msg-images').text()).toContain('2 张图片')
    expect(msgs[1].classes()).toContain('assistant')
    expect(msgs[1].find('.msg-model').text()).toBe('test/testai-1.1')
    // 页面标注 live 语义
    expect(w.text()).toContain('实时只读视图')
    w.unmount()
  })

  it('404：显示后端诚实错误（分享不存在或已撤销）', async () => {
    fetchMock.mockResolvedValue({
      ok: false,
      status: 404,
      json: async () => ({ error: '分享不存在或已撤销' }),
    })
    const w = mount(SharePage)
    await flushPromises()
    expect(w.find('.share-error').text()).toBe('分享不存在或已撤销')
    w.unmount()
  })

  it('缺 token：不发 fetch，直接提示链接缺少令牌', async () => {
    window.history.replaceState({}, '', '/share/')
    const w = mount(SharePage)
    await flushPromises()
    expect(fetchMock).not.toHaveBeenCalled()
    expect(w.find('.share-error').text()).toContain('缺少分享令牌')
    w.unmount()
  })

  it('网络异常：诚实降级为「网络错误」', async () => {
    fetchMock.mockRejectedValue(new TypeError('fail'))
    const w = mount(SharePage)
    await flushPromises()
    expect(w.find('.share-error').text()).toContain('网络错误')
    w.unmount()
  })
})
