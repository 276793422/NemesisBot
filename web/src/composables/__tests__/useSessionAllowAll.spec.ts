import { describe, it, expect, vi, beforeEach } from 'vitest'

// useSessionAllowAll（2026-10-24 会话级全部放行，Feature A 前端单例）：
// - set：WSAPI `security.session_allow`，生效值以服务端响应为准（响应带
//   set 后快照 enabled/since/count）；
// - refresh：WSAPI `security.session_allow_status`，暂时性失败静默（开关
//   保持旧态——安全缺省 = 不放行）；「未装配」错误 → available=false；
// - `injected:false` 响应 → available=false（security 未启用实例）；
// - 分桶按 session_id 隔离（多会话互不串扰）。
// useWSAPI / useToast 打桩（useEditorMode.spec 同款）。

const wsapiRequest = vi.fn()

vi.mock('../useWSAPI', () => ({
  useWSAPI: () => ({ request: wsapiRequest }),
}))

const toasts: Array<{ msg: string; type: string }> = []
vi.mock('../useToast', () => ({
  useToast: () => ({
    toasts: [],
    info: (m: string) => toasts.push({ msg: m, type: 'info' }),
    success: (m: string) => toasts.push({ msg: m, type: 'success' }),
    warn: (m: string) => toasts.push({ msg: m, type: 'warn' }),
    error: (m: string) => toasts.push({ msg: m, type: 'error' }),
    remove: vi.fn(),
  }),
}))

import {
  useSessionAllowAll,
  allowAllBySession,
  sessionAllowAvailable,
  _resetSessionAllowForTest,
} from '../useSessionAllowAll'

beforeEach(() => {
  _resetSessionAllowForTest()
  wsapiRequest.mockReset()
  toasts.length = 0
})

describe('useSessionAllowAll', () => {
  it('set 成功：以服务端响应为准写入分桶（enabled/since/count）', async () => {
    wsapiRequest.mockResolvedValue({
      injected: true,
      session_id: 's1',
      enabled: true,
      since: '2026-10-24T10:00:00Z',
      count: 3,
    })
    const { setSessionAllow } = useSessionAllowAll()
    await setSessionAllow('s1', true)
    expect(wsapiRequest).toHaveBeenCalledWith('security', 'session_allow', {
      session_id: 's1',
      enabled: true,
    })
    expect(allowAllBySession.value.s1).toEqual({
      enabled: true,
      since: '2026-10-24T10:00:00Z',
      count: 3,
    })
  })

  it('set 失败：错误 toast，分桶不写（安全缺省 = 不放行）', async () => {
    wsapiRequest.mockRejectedValue('session not found')
    const { setSessionAllow } = useSessionAllowAll()
    await setSessionAllow('s1', true)
    expect(allowAllBySession.value.s1).toBeUndefined()
    expect(toasts.some(t => t.type === 'error')).toBe(true)
  })

  it('refresh 暂时性失败静默：无 toast，旧态保留', async () => {
    allowAllBySession.value = { s1: { enabled: true, since: null, count: 1 } }
    wsapiRequest.mockRejectedValue('ws busy')
    const { refreshSessionAllow } = useSessionAllowAll()
    await refreshSessionAllow('s1')
    expect(toasts.some(t => t.type === 'error')).toBe(false)
    expect(allowAllBySession.value.s1.enabled).toBe(true)
  })

  it('injected:false（security 未装配）→ available=false，按钮诚实禁用的依据', async () => {
    wsapiRequest.mockResolvedValue({ injected: false })
    const { refreshSessionAllow } = useSessionAllowAll()
    await refreshSessionAllow('s1')
    expect(sessionAllowAvailable.value).toBe(false)
    expect(allowAllBySession.value.s1).toBeUndefined()
  })

  it('「未装配」错误文案同样降级 available=false', async () => {
    wsapiRequest.mockRejectedValue('security.session_allow 未装配')
    const { refreshSessionAllow } = useSessionAllowAll()
    await refreshSessionAllow('s1')
    expect(sessionAllowAvailable.value).toBe(false)
  })

  it('分桶按会话隔离：切会话互不串扰', async () => {
    wsapiRequest.mockImplementation((_m: string, _c: string, data: any) =>
      Promise.resolve({ injected: true, session_id: data.session_id, enabled: data.enabled, since: null, count: 0 })
    )
    const { setSessionAllow } = useSessionAllowAll()
    await setSessionAllow('s1', true)
    await setSessionAllow('s2', false)
    expect(allowAllBySession.value.s1.enabled).toBe(true)
    expect(allowAllBySession.value.s2.enabled).toBe(false)
  })

  it('空 sessionId 的调用直接短路（未选会话态）', async () => {
    const { setSessionAllow, refreshSessionAllow } = useSessionAllowAll()
    await setSessionAllow(null, true)
    await refreshSessionAllow(undefined)
    expect(wsapiRequest).not.toHaveBeenCalled()
  })
})
