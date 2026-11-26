import { describe, it, expect, vi, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount, flushPromises } from '@vue/test-utils'

// 2026-11-26 锚规范化组件级回归：markdown 链接语法 [label](本地路径) 此前
// 渲染成裸 <a href>（linkifyFilePaths 跳过已有 <a>），点击浏览器按相对 URL
// 导航 → 报错页。修复后与纯文本路径同一真相源（normalizeAnchors 收编
// file-link）。本 spec 用**用户实际案例原文**钉住「渲染形态 + 左键点击
// fs.open 全链路」——单元层（markdown.spec.ts）管产出 HTML，这里管挂在
// ChatPanel 里之后用户点下去发生什么。

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
  requestMock.mockResolvedValue({})
  h.wsHandler = null
  h.wsStatus.value = 'connected'
})

async function mountWithAssistant(content: string) {
  const s = useSessionStore()
  s.sessions = [
    { id: 's1', channel: 'web', startTime: '', lastTime: '', messageCount: 0, firstMessage: 'A', model: '' },
  ] as any
  s.currentId = 's1'
  const w = mount(ChatPanel, { attachTo: document.body })
  await flushPromises()
  const calls = vi.mocked(sendHistoryRequest).mock.calls
  h.wsHandler!({
    type: 'message',
    module: 'chat',
    cmd: 'history_response',
    data: {
      request_id: calls[calls.length - 1][0] as string,
      messages: [{ role: 'assistant', content }],
      has_more: false,
      oldest_index: 0,
    },
  })
  await flushPromises()
  return w
}

describe('ChatPanel 文件链接点击（markdown 链接收编形态）', () => {
  it('用户实际案例：渲染为 file-link（无 href/行号剥离/label 保留），点击 → fs.open 成功 toast', async () => {
    requestMock.mockResolvedValue({ path: 'C:/resolved/x.md', kind: 'file', exists: true })
    const w = await mountWithAssistant(
      '第 320 轮已完成并[归档](.bug_hunter/HUNT/2026_11_26_320_发布标签全仓验证.md:1)',
    )
    const link = w.find('a.file-link')
    expect(link.exists(), 'markdown 链接应被收编为 file-link').toBe(true)
    const el = link.element as HTMLAnchorElement
    expect(el.hasAttribute('href'), '不得残留 href（浏览器导航面必须消灭）').toBe(false)
    expect(el.dataset.path).toBe('.bug_hunter/HUNT/2026_11_26_320_发布标签全仓验证.md')
    expect(link.text()).toContain('归档')
    expect(el.getAttribute('title')).toBe('打开文件')

    link.element.dispatchEvent(
      new MouseEvent('click', { bubbles: true, cancelable: true }),
    )
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('fs', 'open', {
      path: '.bug_hunter/HUNT/2026_11_26_320_发布标签全仓验证.md',
      session_id: 's1',
    })
    const toasts = useToast().toasts
    expect(toasts.some((t: any) => t.type === 'success')).toBe(true)
    w.unmount()
  })

  it('https 链接：target=_blank + rel，点击不走 fs.open（委托只认 file-link）', async () => {
    const w = await mountWithAssistant('参考 [官网](https://example.com) 与文档')
    const a = w.find('a[href="https://example.com"]')
    expect(a.exists()).toBe(true)
    expect(a.attributes('target')).toBe('_blank')
    expect(a.attributes('rel')).toBe('noopener noreferrer')
    a.element.dispatchEvent(
      new MouseEvent('click', { bubbles: true, cancelable: true }),
    )
    await flushPromises()
    // 挂载期本身会发若干 WSAPI 请求（chat.sync 等），只断言无 fs.open。
    const fsOpens = requestMock.mock.calls.filter(
      (c: any[]) => c[0] === 'fs' && c[1] === 'open',
    )
    expect(fsOpens).toHaveLength(0)
    w.unmount()
  })

  it('scheme-less 非路径链接解包：消息区不再有任何带 href 的锚', async () => {
    const w = await mountWithAssistant('[注入](//evil.example/x) 和 [foo](bar)')
    const html = w.html()
    expect(html).not.toContain('href=')
    expect(html).not.toContain('<a')
    expect(w.text()).toContain('注入')
    expect(w.text()).toContain('foo')
    w.unmount()
  })
})
