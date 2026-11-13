// 回归钉（2026-11-13）：autoLogin 认证成功必须落盘 localStorage。
//
// 事故：提示词查看器（prompt-viewer 独立页）只读 localStorage 取凭据，
// 而注入链认证（托盘 plugin-ui __DASHBOARD_TOKEN__ / URL token / 桌面
// runtime 事件 → auth.autoLogin）只写内存——同源新 JS 上下文读到空值，
// 恒报「未检测到登录凭据（nemesisbot_auth_token）」。同类坑第二例是
// SSE 恒 401（2026-10-18，见 useSSE.authToken.spec.ts）。
// 修复：认证成功的凭据投影收口在 stores/auth.ts——任何成功路径都写，
// 失败/登出都清；login(remember=false) 是唯一显式豁免。
//
// 钉三个面：autoLogin 成功落盘；autoLogin 失败清残留；登出清盘。

import { describe, it, expect, vi, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'

vi.mock('../../composables/useWebSocket', () => ({
  // testConnection：token 以 'ok' 开头即认证成功，否则失败
  testConnection: vi.fn(async (t: string) => t.startsWith('ok')),
  connect: vi.fn(),
  disconnect: vi.fn(),
}))
vi.mock('../../composables/useSSE', () => ({
  connectEvents: vi.fn(),
  disconnectEvents: vi.fn(),
}))

beforeEach(() => {
  setActivePinia(createPinia())
  localStorage.clear()
})

describe('auth store 凭据落盘投影（autoLogin 落盘回归钉）', () => {
  it('autoLogin 成功 → localStorage 写入凭据（prompt-viewer 等独立页可读）', async () => {
    const { useAuthStore } = await import('../auth')
    const auth = useAuthStore()
    const ok = await auth.autoLogin('ok-token-xyz')
    expect(ok).toBe(true)
    expect(auth.authenticated).toBe(true)
    expect(localStorage.getItem('nemesisbot_auth_token')).toBe('ok-token-xyz')
  })

  it('autoLogin 失败 → 清掉 localStorage 残留（旧行为保持）', async () => {
    localStorage.setItem('nemesisbot_auth_token', 'stale-token')
    const { useAuthStore } = await import('../auth')
    const auth = useAuthStore()
    const ok = await auth.autoLogin('bad-token')
    expect(ok).toBe(false)
    expect(auth.authenticated).toBe(false)
    expect(localStorage.getItem('nemesisbot_auth_token')).toBeNull()
  })

  it('logout → 清盘（旧行为保持）', async () => {
    const { useAuthStore } = await import('../auth')
    const auth = useAuthStore()
    await auth.autoLogin('ok-token-abc')
    expect(localStorage.getItem('nemesisbot_auth_token')).toBe('ok-token-abc')
    auth.logout()
    expect(localStorage.getItem('nemesisbot_auth_token')).toBeNull()
    expect(auth.authenticated).toBe(false)
  })

  it('autoLogin 覆盖旧值 → token 轮换后 localStorage 不留过期凭据', async () => {
    const { useAuthStore } = await import('../auth')
    const auth = useAuthStore()
    await auth.autoLogin('ok-old-token')
    await auth.autoLogin('ok-new-token')
    expect(localStorage.getItem('nemesisbot_auth_token')).toBe('ok-new-token')
  })
})
