import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'

// 提示词内容可视化（2026-11-13）：独立查看器页（prompt-viewer MPA 入口）。
// 自持最小 WS 客户端（chat.prompt_segment 取数），本套件以假 WebSocket
// 钉住：URL 参数校验、无 token 错误卡（不白屏）、取数→渲染、错误响应、
// 刷新重建连接、部分折叠切换。

class FakeWebSocket {
  static instances: FakeWebSocket[] = []
  static CONNECTING = 0
  static OPEN = 1
  static CLOSING = 2
  static CLOSED = 3
  url: string
  sent: string[] = []
  closed = false
  onopen: (() => void) | null = null
  onmessage: ((ev: { data: string }) => void) | null = null
  onerror: (() => void) | null = null
  onclose: (() => void) | null = null
  constructor(url: string) {
    this.url = url
    FakeWebSocket.instances.push(this)
  }
  send(data: string) {
    this.sent.push(data)
  }
  close() {
    this.closed = true
  }
  // —— 测试侧驱动 ——
  simulateOpen() {
    this.onopen?.()
  }
  simulateMessage(obj: unknown) {
    this.onmessage?.({ data: JSON.stringify(obj) })
  }
  simulateError() {
    this.onerror?.()
  }
}

vi.stubGlobal('WebSocket', FakeWebSocket as unknown as typeof WebSocket)

import PromptViewerPage from '../PromptViewerPage.vue'

const SAMPLE_RESULT = {
  segment: 'system',
  title: '系统提示词',
  parts: [
    { label: '系统提示词', kind: 'markdown', content: '# Identity\n你是测试助手。', tokens: 40 },
  ],
  total_tokens: 40,
  generated_at: '2026-11-13T12:00:00Z',
}

function mountWithQuery(query: string) {
  // 相对 query 形态（replaceState 按当前 origin 解析——jsdom 默认 origin
  // 带 :3000 端口，跨端口绝对 URL 会 SecurityError）。
  window.history.replaceState(null, '', query || '/')
  return mount(PromptViewerPage)
}

function respondOk(sock: FakeWebSocket, result: unknown) {
  const frame = JSON.parse(sock.sent[0]) as { reqId: string }
  sock.simulateMessage({ type: 'response', reqId: frame.reqId, data: { prompt_segment: result } })
}

beforeEach(() => {
  FakeWebSocket.instances = []
  localStorage.setItem('nemesisbot_auth_token', 'tok-viewer')
})

afterEach(() => {
  vi.restoreAllMocks()
})

describe('PromptViewerPage（段全文查看器）', () => {
  it('缺参数（无 session/segment）→ 错误卡，不建连接', () => {
    const w = mountWithQuery('')
    expect(w.find('.pv-error-card').exists()).toBe(true)
    expect(w.text()).toContain('缺少参数')
    expect(FakeWebSocket.instances.length).toBe(0)
    w.unmount()
  })

  it('未知段 key → 错误卡列出合法值清单', () => {
    const w = mountWithQuery('?session=s1&segment=bogus')
    expect(w.find('.pv-error-card').exists()).toBe(true)
    expect(w.text()).toContain('未知段 key')
    expect(w.text()).toContain('system / tools / mcp / skill / memory / summary / history')
    expect(FakeWebSocket.instances.length).toBe(0)
    w.unmount()
  })

  it('无登录 token → 错误卡引导先登录 Dashboard，不白屏', () => {
    localStorage.removeItem('nemesisbot_auth_token')
    const w = mountWithQuery('?session=s1&segment=system')
    expect(w.find('.pv-error-card').exists()).toBe(true)
    expect(w.text()).toContain('未检测到登录凭据')
    expect(FakeWebSocket.instances.length).toBe(0)
    w.unmount()
  })

  it('happy path：连接带 token；请求帧 chat.prompt_segment；渲染 parts', async () => {
    const w = mountWithQuery('?session=s1&segment=system')
    expect(FakeWebSocket.instances.length).toBe(1)
    const sock = FakeWebSocket.instances[0]
    expect(sock.url).toContain('token=tok-viewer')

    sock.simulateOpen()
    expect(sock.sent.length).toBe(1)
    const frame = JSON.parse(sock.sent[0]) as { type: string; module: string; cmd: string; data: Record<string, string> }
    expect(frame.type).toBe('request')
    expect(frame.module).toBe('chat')
    expect(frame.cmd).toBe('prompt_segment')
    expect(frame.data).toEqual({ session_id: 's1', segment: 'system' })

    respondOk(sock, SAMPLE_RESULT)
    await flushPromises()
    expect(w.find('.pv-error-card').exists()).toBe(false)
    expect(w.find('.pv-title').text()).toBe('系统提示词')
    expect(w.text()).toContain('~40 tok')
    const part = w.find('.pv-part')
    expect(part.exists()).toBe(true)
    expect(part.find('.pv-part-label').text()).toBe('系统提示词')
    // markdown 段走消毒渲染闸（markdown-body 容器 + 内容进 DOM）
    expect(part.find('.markdown-body').exists()).toBe(true)
    expect(part.find('.markdown-body').element.innerHTML).toContain('你是测试助手')
    // 成功后连接被清理（组件单请求即弃）
    expect(sock.closed).toBe(true)
    w.unmount()
  })

  it('错误响应 → 错误卡展示后端错误文本', async () => {
    const w = mountWithQuery('?session=s1&segment=system')
    const sock = FakeWebSocket.instances[0]
    sock.simulateOpen()
    const frame = JSON.parse(sock.sent[0]) as { reqId: string }
    sock.simulateMessage({ type: 'response', reqId: frame.reqId, error: 'unknown segment: x' })
    await flushPromises()
    expect(w.find('.pv-error-card').exists()).toBe(true)
    expect(w.text()).toContain('unknown segment: x')
    w.unmount()
  })

  it('WS 连接失败（onerror）→ 错误卡；刷新按钮重建连接', async () => {
    const w = mountWithQuery('?session=s1&segment=tools')
    const sock = FakeWebSocket.instances[0]
    sock.simulateOpen()
    sock.simulateError()
    await flushPromises()
    expect(w.find('.pv-error-card').exists()).toBe(true)
    expect(w.text()).toContain('WebSocket 连接失败')

    // 刷新：重建连接、清错误态
    await w.find('.pv-refresh').trigger('click')
    expect(FakeWebSocket.instances.length).toBe(2)
    const sock2 = FakeWebSocket.instances[1]
    sock2.simulateOpen()
    const frame2 = JSON.parse(sock2.sent[0]) as { data: Record<string, string> }
    expect(frame2.data.segment).toBe('tools')
    respondOk(sock2, { ...SAMPLE_RESULT, segment: 'tools', title: '工具定义' })
    await flushPromises()
    expect(w.find('.pv-error-card').exists()).toBe(false)
    expect(w.find('.pv-title').text()).toBe('工具定义')
    w.unmount()
  })

  it('历史段渲染：多轮 parts + tool_calls 旁注 + 图片计数；头部点击可折叠', async () => {
    const w = mountWithQuery('?session=s1&segment=history')
    const sock = FakeWebSocket.instances[0]
    sock.simulateOpen()
    respondOk(sock, {
      segment: 'history',
      title: '会话历史',
      parts: [
        { label: 'user #2', kind: 'markdown', content: '帮我看看日志', tokens: 12, image_refs_count: 1 },
        {
          label: 'assistant #3',
          kind: 'markdown',
          content: '',
          tokens: 5,
          tool_calls: [{ name: 'exec', arguments: '{"cmd":"ls"}' }],
        },
      ],
      total_tokens: 17,
      generated_at: '2026-11-13T12:00:00Z',
    })
    await flushPromises()
    const parts = w.findAll('.pv-part')
    expect(parts.length).toBe(2)
    expect(w.text()).toContain('🖼 ×1')
    expect(w.text()).toContain('tool_calls ×1')
    // tool_calls 参数以高亮 json 呈现
    expect(parts[1].find('.pv-toolcall-name').text()).toBe('exec')
    expect(parts[1].find('.pv-toolcall .hljs').element.innerHTML).toContain('cmd')

    // 头部点击折叠 → body v-show 隐藏；再点恢复（直接断言 style 属性——
    // isVisible 在本 jsdom/test-utils 组合下对 v-show 不可靠，实测恒 true）
    await parts[0].find('.pv-part-head').trigger('click')
    await flushPromises()
    expect(parts[0].find('.pv-part-body').attributes('style')).toContain('display: none')
    await parts[0].find('.pv-part-head').trigger('click')
    await flushPromises()
    expect(parts[0].find('.pv-part-body').attributes('style') ?? '').not.toContain('display: none')
    w.unmount()
  })

  it('json 段（tools）：hljs 高亮直渲', async () => {
    const w = mountWithQuery('?session=s1&segment=tools')
    const sock = FakeWebSocket.instances[0]
    sock.simulateOpen()
    respondOk(sock, {
      segment: 'tools',
      title: '工具定义',
      parts: [{ label: '工具定义', kind: 'json', content: '[{"name":"exec"}]', tokens: 9 }],
      total_tokens: 9,
      generated_at: '2026-11-13T12:00:00Z',
    })
    await flushPromises()
    const part = w.find('.pv-part')
    expect(part.find('.pv-code .hljs').exists()).toBe(true)
    expect(part.find('.pv-code .hljs').element.innerHTML).toContain('exec')
    w.unmount()
  })
})
