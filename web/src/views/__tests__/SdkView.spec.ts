import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { useToast } from '../../composables/useToast'

// M6 补测（quality-hardening goal 2026-08-25）：P2-2 二次开发页 ——
// 两个下载按钮的鉴权头/失败提示/互斥禁用/文件名兜底。
// 后端 /api/sdk/* 字节契约由 sdk_route_tests.rs 钉住。

vi.mock('../../stores/auth', () => ({
  useAuthStore: () => ({ token: 'tok-sdk' }),
}))

import SdkView from '../SdkView.vue'

const fetchMock = vi.fn()

beforeEach(() => {
  fetchMock.mockReset()
  vi.stubGlobal('fetch', fetchMock)
  // jsdom 没有 createObjectURL —— 桩掉下载链路
  vi.stubGlobal('URL', Object.assign(URL, {
    createObjectURL: vi.fn(() => 'blob:mock'),
    revokeObjectURL: vi.fn(),
  }))
  useToast().toasts.splice(0)
})

afterEach(() => {
  vi.unstubAllGlobals()
})

function okZip(kind: 'export' | 'pip', filename?: string): Promise<Response> {
  const headers = new Headers()
  headers.set('Content-Type', 'application/zip')
  if (filename) headers.set('Content-Disposition', `attachment; filename="${filename}"`)
  return Promise.resolve({
    ok: true,
    status: 200,
    headers,
    blob: () => Promise.resolve(new Blob(['zip-bytes'])),
  } as unknown as Response)
}

describe('SdkView 下载', () => {
  it('两个入口分别打 /api/sdk/export 与 /api/sdk/pip，带鉴权头', async () => {
    fetchMock.mockReturnValue(okZip('export', 'nemesisbot-sdk-export-1.2.3.zip'))
    const w = mount(SdkView)
    await w.findAll('button').find(b => b.text().includes('导出 SDK 目录'))!.trigger('click')
    await flushPromises()
    let [url, init] = fetchMock.mock.calls[0]
    expect(url).toBe('/api/sdk/export')
    expect(init.headers['X-Auth-Token']).toBe('tok-sdk')

    fetchMock.mockReturnValue(okZip('pip', 'nemesisbot-sdk-pip-1.2.3.zip'))
    await w.findAll('button').find(b => b.text().includes('pip 包'))!.trigger('click')
    await flushPromises()
    ;[url] = fetchMock.mock.calls[1]
    expect(url).toBe('/api/sdk/pip')
    expect(useToast().toasts.some(t => t.type === 'success' && t.message.includes('SDK 已开始下载'))).toBe(true)
  })

  it('下载中两个按钮互斥禁用', async () => {
    let release!: (r: Response) => void
    fetchMock.mockReturnValue(new Promise(r => (release = r)))
    const w = mount(SdkView)
    const btnE = w.findAll('button').find(b => b.text().includes('导出 SDK 目录'))!
    const btnP = w.findAll('button').find(b => b.text().includes('pip 包'))!
    await btnE.trigger('click')
    expect(btnE.attributes('disabled')).toBeDefined()
    expect(btnP.attributes('disabled')).toBeDefined()
    expect(btnE.text()).toContain('下载中')

    release(okZip('export') as unknown as Response)
    await flushPromises()
    expect(btnE.attributes('disabled')).toBeUndefined()
    expect(btnP.attributes('disabled')).toBeUndefined()
  })

  it('HTTP 失败 → 错误 toast，按钮复位', async () => {
    fetchMock.mockReturnValue(
      Promise.resolve({ ok: false, status: 500, headers: new Headers(), blob: () => Promise.resolve(new Blob()) } as unknown as Response),
    )
    const w = mount(SdkView)
    const btn = w.findAll('button').find(b => b.text().includes('pip 包'))!
    await btn.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.type === 'error' && t.message.includes('HTTP 500'))).toBe(true)
    expect(btn.attributes('disabled')).toBeUndefined()
  })

  it('无 Content-Disposition 文件名 → 兜底名（不抛错）', async () => {
    fetchMock.mockReturnValue(okZip('export'))
    const w = mount(SdkView)
    await w.findAll('button').find(b => b.text().includes('导出 SDK 目录'))!.trigger('click')
    await flushPromises()
    // 走到 success 即未抛错（jsdom 下 a.download 赋值兜底名即可）
    expect(useToast().toasts.some(t => t.type === 'success')).toBe(true)
  })
})
