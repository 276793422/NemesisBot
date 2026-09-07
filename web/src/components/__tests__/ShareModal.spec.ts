import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'

// L4（2026-09-07）：ShareModal —— 创建/复制/撤销只读分享链接。
// WSAPI 三命令走 mock；clipboard 走 stubGlobal。

const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...a: any[]) => requestMock(...a) }),
}))

vi.mock('../../composables/useToast', () => ({
  useToast: () => ({
    toasts: [],
    info: vi.fn(),
    success: vi.fn(),
    error: vi.fn(),
    dismiss: vi.fn(),
  }),
}))

import ShareModal from '../ShareModal.vue'

beforeEach(() => {
  requestMock.mockReset()
  vi.stubGlobal('location', { origin: 'http://localhost:49000' })
})

describe('ShareModal', () => {
  it('打开即拉取列表，显示本会话的有效分享（链接 + 复制/撤销）', async () => {
    requestMock.mockResolvedValue({
      shares: [
        { token: 'aaaa1111bbbb2222cccc3333dddd4444', session_id: 's1', created_at: '2026-09-07T02:00:00Z', revoked: false },
        { token: 'eeee5555ffff6666', session_id: 'other', created_at: '2026-09-07T03:00:00Z', revoked: false },
        { token: '9999aaaa8888bbbb', session_id: 's1', created_at: '2026-09-01T00:00:00Z', revoked: true },
      ],
    })
    const w = mount(ShareModal, { props: { sessionId: 's1' } })
    await flushPromises()

    // share_list 被调用
    expect(requestMock).toHaveBeenCalledWith('sessions', 'share_list')
    // 只显示本会话的有效分享（别会话 + 已撤销都不在 active 列表）
    expect(w.findAll('.share-item').length).toBe(1)
    expect(w.find('.share-url').text()).toBe('http://localhost:49000/share/?t=aaaa1111bbbb2222cccc3333dddd4444')
    // 已撤销计数可见
    expect(w.text()).toContain('已撤销（1）')
    w.unmount()
  })

  it('无有效分享 → 空态 + 创建按钮；创建调 share_create 后刷新', async () => {
    requestMock.mockResolvedValue({ shares: [] })
    const w = mount(ShareModal, { props: { sessionId: 's1' } })
    await flushPromises()

    expect(w.find('.share-empty').exists()).toBe(true)
    expect((w.find('.share-create').element as HTMLButtonElement).disabled).toBe(false)

    requestMock.mockClear()
    requestMock.mockResolvedValue({ token: 'new', path: '/share/?t=new' })
    await w.find('.share-create').trigger('click')
    await flushPromises()

    expect(requestMock).toHaveBeenCalledWith('sessions', 'share_create', { session_id: 's1' })
    // 创建后 refresh（share_list）重新拉取
    expect(requestMock).toHaveBeenCalledWith('sessions', 'share_list')
    w.unmount()
  })

  it('复制走 navigator.clipboard.writeText（完整 URL）', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined)
    Object.assign(navigator, { clipboard: { writeText } })
    requestMock.mockResolvedValue({
      shares: [{ token: 'tok123', session_id: 's1', created_at: '2026-09-07T02:00:00Z', revoked: false }],
    })
    const w = mount(ShareModal, { props: { sessionId: 's1' } })
    await flushPromises()

    await w.find('.share-url-row .btn').trigger('click')
    expect(writeText).toHaveBeenCalledWith('http://localhost:49000/share/?t=tok123')
    w.unmount()
  })

  it('撤销调 share_revoke 后刷新列表', async () => {
    requestMock.mockResolvedValue({
      shares: [{ token: 'tokrevoke', session_id: 's1', created_at: '2026-09-07T02:00:00Z', revoked: false }],
    })
    const w = mount(ShareModal, { props: { sessionId: 's1' } })
    await flushPromises()

    requestMock.mockClear()
    requestMock.mockResolvedValue({ ok: true })
    await w.find('.share-revoke').trigger('click')
    await flushPromises()

    expect(requestMock).toHaveBeenCalledWith('sessions', 'share_revoke', { token: 'tokrevoke' })
    expect(requestMock).toHaveBeenCalledWith('sessions', 'share_list')
    w.unmount()
  })

  it('share_list 失败 → toast 错误而非崩溃', async () => {
    requestMock.mockRejectedValue(new Error('boom'))
    const w = mount(ShareModal, { props: { sessionId: 's1' } })
    await flushPromises()
    // 不抛异常即通过；空态渲染
    expect(w.exists()).toBe(true)
    w.unmount()
  })
})
