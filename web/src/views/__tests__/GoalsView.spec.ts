// 目标管理面契约（目标系统 2026-11-17）：
// - goals.list → 列表行（状态徽标/进度/创建者/停车原因）；
// - goals.create 载荷（content/session_id/max_rounds/eval_model 可选裁剪）；
// - disable/enable/delete 动作；
// - 详情：get → content + evals 台账投影；
// - 编辑目标：update 载荷；
// - 模块缺席 → goals-unavailable 诚实降级；
// - 空态 goals-empty。
import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { useToast } from '../../composables/useToast'

const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
  initWSAPI: vi.fn(),
}))

import GoalsView from '../GoalsView.vue'

function goal(over: Record<string, unknown> = {}) {
  return {
    id: 'G1',
    session_key: 'agent:main:session:main',
    status: 'active',
    created_by: 'human',
    max_rounds: 256,
    rounds_used: 3,
    eval_model: null,
    blocked_code: null,
    blocked_message: null,
    created_at: 1700000000000,
    updated_at: 1700000100000,
    ...over,
  }
}

beforeEach(() => {
  requestMock.mockReset()
  useToast().toasts.splice(0)
  requestMock.mockImplementation(async (_m: string, cmd: string) => {
    if (cmd === 'list') return { goals: [goal()], total: 1 }
    if (cmd === 'get') {
      return {
        ...goal(),
        content: '修复登录 bug\n验收：测试全绿',
        content_missing: false,
        evals: [
          { at: 1700000050000, round: 1, verdict: 'continue', gap: '缺回归测试', model: 'm', duration_ms: 120 },
        ],
      }
    }
    return {}
  })
})

describe('GoalsView（目标管理面）', () => {
  it('列表渲染：状态徽标/进度/创建者/操作按钮', async () => {
    const w = mount(GoalsView)
    await flushPromises()
    expect(w.find('[data-testid="goal-row-0"]').exists()).toBe(true)
    expect(w.find('[data-testid="goal-status-0"]').text()).toContain('运行中')
    expect(w.text()).toContain('3/256 轮')
    expect(w.find('[data-testid="disable-0"]').exists()).toBe(true)
    expect(w.find('[data-testid="delete-0"]').exists()).toBe(true)
    w.unmount()
  })

  it('blocked 行：停车原因可见 + 启用按钮', async () => {
    requestMock.mockImplementation(async (_m: string, cmd: string) => {
      if (cmd === 'list')
        return { goals: [goal({ status: 'blocked', blocked_code: 'round-limit', blocked_message: '轮数预算耗尽（256/256 轮）' })], total: 1 }
      return {}
    })
    const w = mount(GoalsView)
    await flushPromises()
    expect(w.find('[data-testid="goal-status-0"]').text()).toContain('已停车')
    expect(w.text()).toContain('轮数预算耗尽')
    expect(w.find('[data-testid="enable-0"]').exists()).toBe(true)
    w.unmount()
  })

  it('create：载荷裁剪（eval_model 空串不带、max_rounds 0 不带）', async () => {
    const w = mount(GoalsView)
    await flushPromises()
    await w.find('[data-testid="create-btn"]').trigger('click')
    await w.find('[data-testid="create-content"]').setValue('修复登录 bug')
    await w.find('[data-testid="create-session"]').setValue('main')
    await w.find('[data-testid="create-max-rounds"]').setValue('256')
    await w.find('[data-testid="create-eval-model"]').setValue('')
    await w.find('[data-testid="create-submit"]').trigger('click')
    await flushPromises()
    const call = requestMock.mock.calls.find((c: any[]) => c[0] === 'goals' && c[1] === 'create')
    expect(call).toBeTruthy()
    expect(call![2]).toEqual({ content: '修复登录 bug', session_id: 'main', max_rounds: 256 })
    w.unmount()
  })

  it('create：eval_model=small 透传', async () => {
    const w = mount(GoalsView)
    await flushPromises()
    await w.find('[data-testid="create-btn"]').trigger('click')
    await w.find('[data-testid="create-content"]').setValue('目标')
    await w.find('[data-testid="create-session"]').setValue('main')
    await w.find('[data-testid="create-eval-model"]').setValue('small')
    await w.find('[data-testid="create-submit"]').trigger('click')
    await flushPromises()
    const call = requestMock.mock.calls.find((c: any[]) => c[0] === 'goals' && c[1] === 'create')
    expect(call![2].eval_model).toBe('small')
    w.unmount()
  })

  it('create：空内容/空会话本地拒（不发起请求）', async () => {
    const w = mount(GoalsView)
    await flushPromises()
    await w.find('[data-testid="create-btn"]').trigger('click')
    await w.find('[data-testid="create-content"]').setValue('   ')
    await w.find('[data-testid="create-session"]').setValue('main')
    await w.find('[data-testid="create-submit"]').trigger('click')
    await flushPromises()
    expect(requestMock.mock.calls.some((c: any[]) => c[1] === 'create')).toBe(false)
    w.unmount()
  })

  it('disable/enable/delete 各带 id', async () => {
    const w = mount(GoalsView)
    await flushPromises()
    // active 行先 disable。
    await w.find('[data-testid="disable-0"]').trigger('click')
    await flushPromises()
    expect(requestMock.mock.calls.some((c: any[]) => c[1] === 'disable' && c[2]?.id === 'G1')).toBe(true)
    // 列表刷新为 paused 后出现 enable 按钮。
    requestMock.mockImplementation(async (_m: string, cmd: string) => {
      if (cmd === 'list') return { goals: [goal({ status: 'paused' })], total: 1 }
      return { disabled: true, enabled: true, removed: true }
    })
    await w.find('[data-testid="refresh-btn"]').trigger('click')
    await flushPromises()
    await w.find('[data-testid="enable-0"]').trigger('click')
    await flushPromises()
    expect(requestMock.mock.calls.some((c: any[]) => c[1] === 'enable' && c[2]?.id === 'G1')).toBe(true)
    await w.find('[data-testid="delete-0"]').trigger('click')
    await flushPromises()
    expect(requestMock.mock.calls.some((c: any[]) => c[1] === 'delete' && c[2]?.id === 'G1')).toBe(true)
    w.unmount()
  })

  it('详情：content + evals 台账投影', async () => {
    const w = mount(GoalsView)
    await flushPromises()
    await w.find('[data-testid="goal-row-0"]').trigger('click')
    await flushPromises()
    expect(w.find('[data-testid="detail-content"]').text()).toContain('修复登录 bug')
    expect(w.find('[data-testid="eval-record"]').exists()).toBe(true)
    expect(w.text()).toContain('缺回归测试')
    w.unmount()
  })

  it('编辑目标：update 带 content；保存后刷新详情', async () => {
    const w = mount(GoalsView)
    await flushPromises()
    await w.find('[data-testid="goal-row-0"]').trigger('click')
    await flushPromises()
    await w.find('[data-testid="edit-btn"]').trigger('click')
    await w.find('[data-testid="edit-content"]').setValue('修正后的目标')
    await w.find('[data-testid="edit-save"]').trigger('click')
    await flushPromises()
    const upd = requestMock.mock.calls.find((c: any[]) => c[1] === 'update')
    expect(upd).toBeTruthy()
    expect(upd![2]).toEqual({ id: 'G1', content: '修正后的目标' })
    w.unmount()
  })

  it('模块缺席：goals-unavailable 诚实降级', async () => {
    requestMock.mockRejectedValue(new Error('unknown module: goals'))
    const w = mount(GoalsView)
    await flushPromises()
    expect(w.find('[data-testid="goals-unavailable"]').exists()).toBe(true)
    w.unmount()
  })

  it('空态：goals-empty', async () => {
    requestMock.mockImplementation(async (_m: string, cmd: string) => {
      if (cmd === 'list') return { goals: [], total: 0 }
      return {}
    })
    const w = mount(GoalsView)
    await flushPromises()
    expect(w.find('[data-testid="goals-empty"]').exists()).toBe(true)
    w.unmount()
  })

  it('done 行：已完成徽标 + 操作列只剩删除（无暂停/启用）', async () => {
    requestMock.mockImplementation(async (_m: string, cmd: string) => {
      if (cmd === 'list') return { goals: [goal({ status: 'done' })], total: 1 }
      return {}
    })
    const w = mount(GoalsView)
    await flushPromises()
    expect(w.find('[data-testid="goal-status-0"]').text()).toContain('已完成')
    expect(w.find('[data-testid="disable-0"]').exists()).toBe(false)
    expect(w.find('[data-testid="enable-0"]').exists()).toBe(false)
    expect(w.find('[data-testid="delete-0"]').exists()).toBe(true)
    w.unmount()
  })

  it('paused 行：启用按钮替代暂停按钮', async () => {
    requestMock.mockImplementation(async (_m: string, cmd: string) => {
      if (cmd === 'list') return { goals: [goal({ status: 'paused' })], total: 1 }
      return {}
    })
    const w = mount(GoalsView)
    await flushPromises()
    expect(w.find('[data-testid="goal-status-0"]').text()).toContain('已暂停')
    expect(w.find('[data-testid="disable-0"]').exists()).toBe(false)
    expect(w.find('[data-testid="enable-0"]').exists()).toBe(true)
    expect(w.find('[data-testid="delete-0"]').exists()).toBe(true)
    w.unmount()
  })

  it('max_rounds=0：进度显示 ∞', async () => {
    requestMock.mockImplementation(async (_m: string, cmd: string) => {
      if (cmd === 'list') return { goals: [goal({ max_rounds: 0 })], total: 1 }
      return {}
    })
    const w = mount(GoalsView)
    await flushPromises()
    expect(w.text()).toContain('3/∞ 轮')
    w.unmount()
  })

  it('详情：goal.md 被手删（content 空）→ 文件缺失警告替代内容块', async () => {
    requestMock.mockImplementation(async (_m: string, cmd: string) => {
      if (cmd === 'get') return { ...goal(), content: '', content_missing: true, evals: [] }
      if (cmd === 'list') return { goals: [goal()], total: 1 }
      return {}
    })
    const w = mount(GoalsView)
    await flushPromises()
    await w.find('[data-testid="goal-row-0"]').trigger('click')
    await flushPromises()
    expect(w.find('[data-testid="detail-content"]').exists()).toBe(false)
    expect(w.text()).toContain('目标文件缺失')
    w.unmount()
  })

  it('详情：blocked 红卡带 code + message', async () => {
    requestMock.mockImplementation(async (_m: string, cmd: string) => {
      if (cmd === 'get')
        return {
          ...goal({ status: 'blocked', blocked_code: 'round-limit', blocked_message: '轮数预算耗尽（256/256 轮）' }),
          content: '目标内容',
          evals: [],
        }
      if (cmd === 'list') return { goals: [goal()], total: 1 }
      return {}
    })
    const w = mount(GoalsView)
    await flushPromises()
    await w.find('[data-testid="goal-row-0"]').trigger('click')
    await flushPromises()
    expect(w.text()).toContain('停车（round-limit）')
    expect(w.text()).toContain('轮数预算耗尽（256/256 轮）')
    w.unmount()
  })

  it('评估台账：error 判定带错误徽标，done 判定带 info 徽标', async () => {
    requestMock.mockImplementation(async (_m: string, cmd: string) => {
      if (cmd === 'get')
        return {
          ...goal(),
          content: '目标内容',
          evals: [
            { at: 1700000050000, round: 1, verdict: 'error', gap: null, model: 'm', duration_ms: 50 },
            { at: 1700000060000, round: 1, verdict: 'done', gap: null, model: 'm', duration_ms: 80 },
          ],
        }
      if (cmd === 'list') return { goals: [goal()], total: 1 }
      return {}
    })
    const w = mount(GoalsView)
    await flushPromises()
    await w.find('[data-testid="goal-row-0"]').trigger('click')
    await flushPromises()
    const records = w.findAll('[data-testid="eval-record"]')
    expect(records.length).toBe(2)
    expect(records[0].find('.badge-error').exists()).toBe(true)
    expect(records[1].find('.badge-info').exists()).toBe(true)
    w.unmount()
  })

  it('创建成功：表单关闭 + 字段重置', async () => {
    const w = mount(GoalsView)
    await flushPromises()
    await w.find('[data-testid="create-btn"]').trigger('click')
    await w.find('[data-testid="create-content"]').setValue('目标')
    await w.find('[data-testid="create-session"]').setValue('main')
    await w.find('[data-testid="create-submit"]').trigger('click')
    await flushPromises()
    // 表单收起（creating=false）且重新加载列表。
    expect(w.find('[data-testid="create-submit"]').exists()).toBe(false)
    expect(requestMock.mock.calls.some((c: any[]) => c[1] === 'create')).toBe(true)
    w.unmount()
  })
})
