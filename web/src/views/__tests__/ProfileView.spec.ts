// B 线用户习惯面板契约（自进化升级 2026-10-14）：
// - profile.list → 开关态 + 条目表（类别/内容/置信度/状态）；
// - veto/forget/pin 后门动作带 content 载荷；
// - 未开启 → 空态文案诚实标注 + 显示开启按钮。
import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { useToast } from '../../composables/useToast'

const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
  initWSAPI: vi.fn(),
}))

import ProfileView from '../ProfileView.vue'

function habit(over: Record<string, unknown> = {}) {
  return {
    category: 'style',
    content: '用户偏好简洁回复',
    confidence: 0.85,
    hits: 3,
    first_seen: '2026-10-01T00:00:00+08:00',
    last_seen: '2026-10-14T00:00:00+08:00',
    status: 'active',
    pinned: false,
    ...over,
  }
}

beforeEach(() => {
  requestMock.mockReset()
  useToast().toasts.splice(0)
  requestMock.mockImplementation(async (_m: string, cmd: string) => {
    if (cmd === 'list') {
      return {
        enabled: true,
        enabled_at: '2026-10-01T08:00:00+08:00',
        habits: [
          habit(),
          habit({ category: 'correction', content: '先确认理解再动手', status: 'decaying', confidence: 0.72, pinned: true }),
        ],
      }
    }
    return {}
  })
})

describe('ProfileView（B 线习惯面板）', () => {
  it('开启态：条目表渲染类别/内容/状态，展示采集起点', async () => {
    const w = mount(ProfileView)
    await flushPromises()
    expect(w.find('[data-testid="habit-row-0"]').exists()).toBe(true)
    expect(w.text()).toContain('用户偏好简洁回复')
    expect(w.text()).toContain('先确认理解再动手')
    expect(w.find('[data-testid="habit-status-1"]').text()).toContain('衰减中')
    expect(w.find('[data-testid="enabled-at"]').exists()).toBe(true)
    w.unmount()
  })

  it('veto/forget/pin 各带 content 载荷', async () => {
    const w = mount(ProfileView)
    await flushPromises()
    const calls: any[] = []
    requestMock.mockImplementation(async (_m: string, cmd: string, data: any) => {
      calls.push([cmd, data])
      if (cmd === 'veto' || cmd === 'forget' || cmd === 'pin') return { vetoed: true, forgotten: true, updated: true }
      if (cmd === 'list') return { enabled: true, enabled_at: null, habits: [habit()] }
      return {}
    })
    await w.find('[data-testid="veto-0"]').trigger('click')
    await w.find('[data-testid="forget-0"]').trigger('click')
    await w.find('[data-testid="pin-0"]').trigger('click')
    await flushPromises()
    const cmds = calls.map((c) => c[0])
    expect(cmds).toContain('veto')
    expect(cmds).toContain('forget')
    expect(cmds).toContain('pin')
    for (const [cmd, data] of calls.filter((c) => c[0] !== 'list')) {
      expect(data).toEqual({ content: '用户偏好简洁回复' })
      expect(cmds.length).toBeGreaterThanOrEqual(3)
    }
    w.unmount()
  })

  it('未开启：空态诚实标注 + 显示开启按钮；enable 调用后按钮切换', async () => {
    requestMock.mockImplementation(async (_m: string, cmd: string) => {
      if (cmd === 'list') return { enabled: false, enabled_at: null, habits: [] }
      if (cmd === 'enable') return { enabled: true, enabled_at: '2026-10-14T10:00:00+08:00' }
      return {}
    })
    const w = mount(ProfileView)
    await flushPromises()
    expect(w.find('[data-testid="empty-state"]').text()).toContain('画像未开启')
    expect(w.find('[data-testid="enable-btn"]').exists()).toBe(true)
    await w.find('[data-testid="enable-btn"]').trigger('click')
    await flushPromises()
    expect(w.find('[data-testid="disable-btn"]').exists()).toBe(true)
    w.unmount()
  })
})
