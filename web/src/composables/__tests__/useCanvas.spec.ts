import { describe, it, expect, vi, beforeEach } from 'vitest'

// useCanvas（P30 WS14）组合式单测：
// - canvas.open 按 (session_id, index) 幂等 upsert（重复帧覆盖不叠加）；
// - 会话间隔离；每会话容量上限（超出丢最旧）；
// - closeCanvas 本地移除 + canvas.close 回执（fire-and-forget）；
// - injectCanvasCsp 三形态（head / html 无 head / 片段）。

const sseHandlers = new Map<string, (data?: unknown) => void>()
const wsapiRequest = vi.fn()

vi.mock('../../composables/useSSE', () => ({
  on: vi.fn((type: string, handler: (data?: unknown) => void) => {
    sseHandlers.set(type, handler)
  }),
  off: vi.fn(),
}))

vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: wsapiRequest }),
}))

import {
  useCanvas,
  injectCanvasCsp,
  _resetCanvasForTest,
  MAX_CANVASES_PER_SESSION,
  CANVAS_CSP,
} from '../../composables/useCanvas'

function fireCanvas(data: Record<string, unknown>) {
  sseHandlers.get('canvas.open')!(data)
}

beforeEach(() => {
  _resetCanvasForTest()
  sseHandlers.clear()
  wsapiRequest.mockReset().mockResolvedValue({ closed: true })
})

describe('useCanvas upsert', () => {
  it('canvas.open 入列并按 index 升序', () => {
    const { canvasesBySession, initCanvas } = useCanvas()
    initCanvas()
    fireCanvas({ session_id: 's1', html: '<p>a</p>', index: 1 })
    fireCanvas({ session_id: 's1', html: '<p>b</p>', index: 0 })
    expect(canvasesBySession['s1'].map(c => c.index)).toEqual([0, 1])
    expect(canvasesBySession['s1'][0].html).toBe('<p>b</p>')
  })

  it('同 (session_id, index) 重复帧幂等覆盖不叠加', () => {
    const { canvasesBySession, initCanvas } = useCanvas()
    initCanvas()
    fireCanvas({ session_id: 's1', html: '<p>old</p>', index: 0 })
    fireCanvas({ session_id: 's1', html: '<p>new</p>', index: 0 })
    expect(canvasesBySession['s1'].length).toBe(1)
    expect(canvasesBySession['s1'][0].html).toBe('<p>new</p>')
  })

  it('会话间隔离', () => {
    const { canvasesBySession, initCanvas } = useCanvas()
    initCanvas()
    fireCanvas({ session_id: 's1', html: '<p>a</p>', index: 0 })
    fireCanvas({ session_id: 's2', html: '<p>b</p>', index: 0 })
    expect(canvasesBySession['s1'].length).toBe(1)
    expect(canvasesBySession['s2'].length).toBe(1)
    expect(canvasesBySession['s1'][0].html).toBe('<p>a</p>')
  })

  it('超过容量上限丢最旧', () => {
    const { canvasesBySession, initCanvas } = useCanvas()
    initCanvas()
    for (let i = 0; i < MAX_CANVASES_PER_SESSION + 2; i++) {
      fireCanvas({ session_id: 's1', html: `<p>${i}</p>`, index: i })
    }
    const list = canvasesBySession['s1']
    expect(list.length).toBe(MAX_CANVASES_PER_SESSION)
    expect(list[0].index, '最旧两块被挤掉').toBe(2)
    expect(list[list.length - 1].index).toBe(MAX_CANVASES_PER_SESSION + 1)
  })

  it('载荷缺字段静默忽略', () => {
    const { canvasesBySession, initCanvas } = useCanvas()
    initCanvas()
    fireCanvas({ session_id: 's1' })
    fireCanvas({ html: '<p>x</p>', index: 0 })
    expect(canvasesBySession['s1']).toBeUndefined()
  })
})

describe('useCanvas close', () => {
  it('本地移除 + canvas.close 回执', async () => {
    const { canvasesBySession, initCanvas, closeCanvas } = useCanvas()
    initCanvas()
    fireCanvas({ session_id: 's1', html: '<p>a</p>', index: 0 })
    fireCanvas({ session_id: 's1', html: '<p>b</p>', index: 1 })
    closeCanvas('s1', 0)
    expect(canvasesBySession['s1'].map(c => c.index)).toEqual([1])
    expect(wsapiRequest).toHaveBeenCalledWith('canvas', 'close', { session_id: 's1' })
  })

  it('回执失败不回滚本地关闭', async () => {
    wsapiRequest.mockRejectedValue('ws down')
    const { canvasesBySession, initCanvas, closeCanvas } = useCanvas()
    initCanvas()
    fireCanvas({ session_id: 's1', html: '<p>a</p>', index: 0 })
    closeCanvas('s1', 0)
    // 等 microtask 排空 fire-and-forget 的 catch。
    await Promise.resolve()
    expect(canvasesBySession['s1'].length).toBe(0)
  })
})

describe('injectCanvasCsp', () => {
  it('完整文档（有 head）→ meta 插在 head 开头', () => {
    const out = injectCanvasCsp('<html><head><title>t</title></head><body></body></html>')
    const cspAt = out.indexOf('Content-Security-Policy')
    expect(cspAt).toBeGreaterThan(-1)
    expect(out.indexOf('<title>'), 'meta 在既有 head 内容之前').toBeGreaterThan(cspAt)
    expect(out).toContain(CANVAS_CSP)
  })

  it('有 html 无 head → 补 head 装 meta', () => {
    const out = injectCanvasCsp('<html><body><p>x</p></body></html>')
    expect(out).toContain('<head><meta http-equiv="Content-Security-Policy"')
    expect(out).toContain('<body><p>x</p>')
  })

  it('片段 → 包完整文档骨架', () => {
    const out = injectCanvasCsp('<p>x</p>')
    expect(out.startsWith('<!DOCTYPE html>')).toBe(true)
    expect(out).toContain('<head><meta http-equiv="Content-Security-Policy"')
    expect(out.endsWith('<p>x</p></body></html>')).toBe(true)
  })

  it('伪 <head> 字样（前导注释内）不误导注入位（DOMParser 锚定）', () => {
    // 旧正则实现按「全文首个 <head> 形态匹配」注入——这段 HTML 会被插进
    // 注释里，CSP 对整篇失效（2026-09-26 挂账高优 Canvas-#1 的绕过机理）。
    const tricky =
      '<!-- <head> fake -->\n<html><head><script>alert(1)</script></head><body></body></html>'
    const out = injectCanvasCsp(tricky)
    const metaPos = out.indexOf('Content-Security-Policy')
    expect(metaPos).toBeGreaterThan(-1)
    expect(out.indexOf('<script>'), 'meta 必须先于模型脚本').toBeGreaterThan(metaPos)
    expect(out).toContain(CANVAS_CSP)
  })

  it('无 head 无 html 的裸片段 → 照常包骨架（DOMParser 主路径）', () => {
    const out = injectCanvasCsp('<div>x</div>')
    expect(out.startsWith('<!DOCTYPE html>')).toBe(true)
    expect(out).toContain(CANVAS_CSP)
    expect(out).toContain('<div>x</div>')
  })
})
