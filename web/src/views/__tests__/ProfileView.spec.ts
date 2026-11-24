// B 线用户习惯面板契约（Forge 化重构 2026-11-24 更新；批 D 增补）：
// - profile.list → 开关态 + 条目表（类别/内容/置信度/状态 + steps 步骤
//   清单 + auto 挖掘徽标 + ui_telemetry 开关投影）；
// - 批 D：五维分组（流程仪式/操作仪式/表达粒度/节奏分工/信任画像）+
//   profile.stats 统计卡（todo 完成度/教训分流/条目来源/近期强信号）；
// - veto/forget/pin 后门动作带 content 载荷；
// - telemetry_set 切换 UI 遥测（config user_profile.ui_telemetry 面）；
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
    steps: [],
    evidence: [],
    auto: false,
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
        ui_telemetry: true,
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

  it('结构化条目：steps 编号清单 + auto 徽标 + 扩容类别', async () => {
    const w = mount(ProfileView)
    await flushPromises()
    // 重 mock：一条带 steps 的 workflow 挖掘条目 + 一条 quality_bar。
    requestMock.mockImplementation(async (_m: string, cmd: string) => {
      if (cmd === 'list') {
        return {
          enabled: true,
          enabled_at: '2026-10-01T08:00:00+08:00',
          ui_telemetry: true,
          habits: [
            habit({
              category: 'workflow',
              content: '计划式工作流',
              auto: true,
              source: 'forge-pattern',
              steps: ['先制定计划', '回顾确认', '转成步骤清单', '测试收尾'],
              evidence: ['session-a', 'session-b'],
            }),
            habit({ category: 'quality_bar', content: '交付前必测试' }),
          ],
        }
      }
      return {}
    })
    const w2 = mount(ProfileView)
    await flushPromises()
    expect(w2.text()).toContain('计划式工作流')
    expect(w2.find('[data-testid="habit-step-0-0"]').text()).toContain('先制定计划')
    expect(w2.find('[data-testid="habit-step-0-3"]').text()).toContain('测试收尾')
    expect(w2.find('[data-testid="auto-badge"]').exists()).toBe(true)
    // 来源徽标（批 A 接口二）：source 非空才渲染。
    expect(w2.find('[data-testid="source-badge-0"]').text()).toContain('来自 forge-pattern')
    expect(w2.find('[data-testid="source-badge-1"]').exists()).toBe(false)
    expect(w2.text()).toContain('质量线')
    expect(w2.text()).toContain('session-a、session-b')
    w2.unmount()
    w.unmount()
  })

  it('telemetry_set 切换 UI 遥测开关（带 enabled 布尔载荷）', async () => {
    const w = mount(ProfileView)
    await flushPromises()
    expect(w.find<HTMLInputElement>('[data-testid="telemetry-toggle"]').element.checked).toBe(true)
    const calls: any[] = []
    requestMock.mockImplementation(async (_m: string, cmd: string, data: any) => {
      calls.push([cmd, data])
      if (cmd === 'telemetry_set') return { ui_telemetry: false }
      if (cmd === 'list') return { enabled: true, enabled_at: null, ui_telemetry: false, habits: [] }
      return {}
    })
    await w.find('[data-testid="telemetry-toggle"]').setValue(false)
    await flushPromises()
    expect(calls.some(([cmd, data]) => cmd === 'telemetry_set' && data?.enabled === false)).toBe(true)
    w.unmount()
  })

  it('veto/forget/pin 各带 content 载荷', async () => {
    const w = mount(ProfileView)
    await flushPromises()
    const calls: any[] = []
    requestMock.mockImplementation(async (_m: string, cmd: string, data: any) => {
      calls.push([cmd, data])
      if (cmd === 'veto' || cmd === 'forget' || cmd === 'pin') return { vetoed: true, forgotten: true, updated: true }
      if (cmd === 'list') return { enabled: true, enabled_at: null, ui_telemetry: true, habits: [habit()] }
      return {}
    })
    await w.find('[data-testid="veto-0"]').trigger('click')
    await flushPromises()
    await w.find('[data-testid="forget-0"]').trigger('click')
    await flushPromises()
    await w.find('[data-testid="pin-0"]').trigger('click')
    await flushPromises()
    const cmds = calls.map((c) => c[0])
    expect(cmds).toContain('veto')
    expect(cmds).toContain('forget')
    expect(cmds).toContain('pin')
    for (const [cmd, data] of calls.filter(([cmd]) => ['veto', 'forget', 'pin'].includes(cmd))) {
      expect(data).toEqual({ content: '用户偏好简洁回复' })
      expect(cmds.length).toBeGreaterThanOrEqual(3)
    }
    w.unmount()
  })

  it('未开启：空态诚实标注 + 显示开启按钮；enable 调用后按钮切换', async () => {
    requestMock.mockImplementation(async (_m: string, cmd: string) => {
      if (cmd === 'list') return { enabled: false, enabled_at: null, ui_telemetry: true, habits: [] }
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

  it('批 D：stats 统计卡渲染四组读数；trust 条目落信任画像分组', async () => {
    requestMock.mockImplementation(async (_m: string, cmd: string) => {
      if (cmd === 'list') {
        return {
          enabled: true,
          enabled_at: '2026-10-01T08:00:00+08:00',
          ui_telemetry: true,
          habits: [
            habit({ category: 'workflow', content: '计划式工作流', auto: true }),
            habit({ category: 'trust', content: '审批卡平均 3 秒批准，信任度高', confidence: 0.9 }),
          ],
        }
      }
      if (cmd === 'stats') {
        return {
          enabled: true,
          stats: {
            todos: { snapshot_count: 5, item_count: 12, completed_count: 9, completion_ratio: 0.75, snapshots_with_pending: 1 },
            lessons: { pending: 3, acked: 7 },
            habits_by_category: { workflow: 1, trust: 1 },
            habits_by_source: { auto: 1, manual: 1, external: 0 },
            signals_recent: { approval_cards: 14, estop: 2 },
          },
        }
      }
      return {}
    })
    const w = mount(ProfileView)
    await flushPromises()
    // 统计卡四组读数。
    expect(w.find('[data-testid="stats-card"]').exists()).toBe(true)
    expect(w.find('[data-testid="stats-todos"]').text()).toContain('75%')
    expect(w.find('[data-testid="stats-todos"]').text()).toContain('9/12 项')
    expect(w.find('[data-testid="stats-lessons"]').text()).toContain('待消费 3')
    expect(w.find('[data-testid="stats-lessons"]').text()).toContain('已消费 7')
    expect(w.find('[data-testid="stats-sources"]').text()).toContain('自动 1')
    expect(w.find('[data-testid="stats-signals"]').text()).toContain('审批卡 14')
    expect(w.find('[data-testid="stats-signals"]').text()).toContain('急停 2')
    // 分组头：流程仪式（workflow）与信任画像（trust）各自成组。
    expect(w.text()).toContain('流程仪式')
    expect(w.text()).toContain('信任画像')
    // trust 条目渲染在新类别标签下。
    expect(w.text()).toContain('审批卡平均 3 秒批准，信任度高')
    w.unmount()
  })

  it('批 D：stats 请求失败 → 统计卡隐藏不阻塞条目表', async () => {
    requestMock.mockImplementation(async (_m: string, cmd: string) => {
      if (cmd === 'list') return { enabled: true, enabled_at: null, ui_telemetry: true, habits: [habit()] }
      if (cmd === 'stats') throw new Error('boom')
      return {}
    })
    const w = mount(ProfileView)
    await flushPromises()
    expect(w.find('[data-testid="stats-card"]').exists()).toBe(false)
    expect(w.find('[data-testid="habit-row-0"]').exists()).toBe(true)
    w.unmount()
  })
})
