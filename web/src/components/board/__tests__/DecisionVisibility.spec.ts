import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { useToast } from '../../../composables/useToast'

// 决策可见性批（2026-10-15 F1/F2/F3）前端 spec：
// - boardMeta 助手：stopKindLabel / suggestedActionLabel / needsUserBadge；
// - IssueListView：三徽标渲染 + ✋ 待决策筛选 chip（计数/过滤行）；
// - BoardKanban：needs_user 徽标 + is-decision 卡样式钩子；
// - BoardDagView：决策 chip（计数）+ ✋ 节点图标穿透；
// - IssueDetailModal：F2 决策面板——needs_user 原因卡 + 按 suggested_action
//   预设按钮组 + 附言先评论后动作 + 重派目标闸 + plan_ready 变体 + 无决策不渲染。
// 后端投影真相源：crates/nemesis-web/src/handlers/board.rs（issue.list/issue.decision）。

const requestMock = vi.fn()
vi.mock('../../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
  initWSAPI: vi.fn(),
}))

const sseHandlers = new Map<string, (data?: unknown) => void>()
vi.mock('../../../composables/useSSE', () => ({
  on: vi.fn((type: string, handler: (data?: unknown) => void) => {
    sseHandlers.set(type, handler)
  }),
  off: vi.fn((type: string) => {
    sseHandlers.delete(type)
  }),
}))

import {
  stopKindLabel,
  suggestedActionLabel,
  needsUserBadge,
} from '../boardMeta'
import BoardKanban from '../BoardKanban.vue'
import BoardDagView from '../BoardDagView.vue'
import IssueDetailModal from '../IssueDetailModal.vue'
import IssueListView from '../../../views/IssueListView.vue'

function issue(over: Record<string, unknown> = {}) {
  return {
    id: 1,
    number: 'NB-1',
    title: '任务一',
    description: '',
    status: 'backlog',
    priority: 1,
    assignee: null,
    assignee_id: null,
    creator: { kind: 'admin', id: 'admin' },
    project_id: null,
    due_date: null,
    position: 1,
    acceptance_criteria: null,
    origin: null,
    created_at: 1700000000,
    updated_at: 1700000000,
    ...over,
  }
}

const NU = {
  stop_kind: 'fail_streak',
  stop_kind_label: '连续失败熔断',
  suggested_action: 'manual_check',
  hint: '评审连续未过，需人工核查交付',
  stopped_at: '2026-10-15T07:38:00Z',
}

const PLAN_READY = { plan_id: 'plan-dv-1', subs_count: 3, created_age_secs: 30 }

beforeEach(() => {
  sseHandlers.clear()
  requestMock.mockReset()
  useToast().toasts.splice(0)
  requestMock.mockImplementation((_m: string, cmd: string) => {
    if (cmd === 'issue.list') return Promise.resolve({ issues: [], total: 0 })
    if (cmd === 'project.list') return Promise.resolve({ projects: [] })
    if (cmd === 'nodes.list') return Promise.resolve({ nodes: [] })
    if (cmd === 'issue.get') return Promise.resolve({ issue: issue() })
    if (cmd === 'attachment.list') return Promise.resolve({ attachments: [] })
    if (cmd === 'plan.list') return Promise.resolve({ plans: [] })
    return Promise.resolve({})
  })
})

describe('boardMeta 决策助手', () => {
  it('stopKindLabel：已知映射 + 未知/空回退「停车待处理」', () => {
    expect(stopKindLabel('fail_streak')).toBe('连续失败熔断')
    expect(stopKindLabel('plan_exhausted')).toBe('拆解重试耗尽')
    expect(stopKindLabel('no_match')).toBe('无匹配节点')
    // BUG-20261029-170 回填三类（EscalateHuman verdict 分型 + 必需检查闸）：
    expect(stopKindLabel('unsure')).toBe('单轮评审不确定')
    expect(stopKindLabel('redispatch_budget')).toBe('重派预算耗尽')
    expect(stopKindLabel('required_checks')).toBe('必需检查未满足')
    expect(stopKindLabel('unknown_kind')).toBe('停车待处理')
    expect(stopKindLabel('')).toBe('停车待处理')
    expect(stopKindLabel(null)).toBe('停车待处理')
  })

  it('suggestedActionLabel：三档已知 + 未知回退人工核查', () => {
    expect(suggestedActionLabel('manual_check')).toBe('人工核查后裁决')
    expect(suggestedActionLabel('retry')).toBe('重试/复评')
    expect(suggestedActionLabel('redispatch')).toBe('重新派发')
    expect(suggestedActionLabel('other')).toBe('人工核查后裁决')
    expect(suggestedActionLabel(null)).toBe('人工核查后裁决')
  })

  it('needsUserBadge：文本 + 提示含 stop_kind 与 hint', () => {
    const b = needsUserBadge(NU as any)
    expect(b.text).toContain('待你决策')
    expect(b.title).toContain('连续失败熔断')
    expect(b.title).toContain('评审连续未过')
    const noHint = needsUserBadge({ ...NU, hint: '' } as any)
    expect(noHint.title).not.toContain('undefined')
  })
})

describe('IssueListView 决策可见性（F1）', () => {
  async function mountList(issues: any[]) {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'issue.list') return Promise.resolve({ issues, total: issues.length })
      if (cmd === 'stats') return Promise.resolve({ by_status: {} })
      return Promise.resolve({})
    })
    const w = mount(IssueListView)
    await flushPromises()
    return w
  }

  it('三徽标按投影渲染：needs_user / plan_ready / attention', async () => {
    const w = await mountList([
      issue({ id: 1, needs_user: NU, status: 'in_review' }),
      issue({ id: 2, number: 'NB-2', plan_ready: PLAN_READY, status: 'backlog' }),
      issue({ id: 3, number: 'NB-3', attention: true, status: 'in_progress' }),
    ])
    expect(w.find('[data-test="needs-user-badge"]').exists()).toBe(true)
    expect(w.find('[data-test="plan-ready-badge"]').exists()).toBe(true)
    expect(w.find('[data-test="attention-badge"]').exists()).toBe(true)
    expect(w.text()).toContain('📋 计划待确认')
    expect(w.text()).toContain('⚠ 停滞')
  })

  it('✋ 待决策 chip：计数 = needs_user 行数；点击过滤只留待决策行', async () => {
    const w = await mountList([
      issue({ id: 1, needs_user: NU, status: 'in_review' }),
      issue({ id: 2, number: 'NB-2' }),
      issue({ id: 3, number: 'NB-3', needs_user: { ...NU, stop_kind: 'dispatch_no_capacity', stop_kind_label: '无可用节点' }, status: 'in_progress' }),
    ])
    const chip = w.find('[data-test="filter-decision"]')
    expect(chip.text()).toContain('待决策 2')
    await chip.trigger('click')
    await flushPromises()
    const rows = w.findAll('tbody tr')
    expect(rows.length).toBe(2)
    expect(rows.map((r) => r.text()).join('')).toContain('NB-1')
    expect(rows.map((r) => r.text()).join('')).not.toContain('NB-2')
  })

  it('零待决策：chip 禁用（未激活时）', async () => {
    const w = await mountList([issue()])
    const chip = w.find('[data-test="filter-decision"]')
    expect((chip.element as HTMLButtonElement).disabled).toBe(true)
  })
})

describe('BoardKanban 决策可见性（F1）', () => {
  it('needs_user 卡渲染徽标 + is-decision 描边钩子', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'issue.list')
        return Promise.resolve({
          issues: [issue({ status: 'in_review', needs_user: NU })],
          total: 1,
        })
      return Promise.resolve({})
    })
    const w = mount(BoardKanban)
    await flushPromises()
    const card = w.find('.kanban-card.is-decision')
    expect(card.exists()).toBe(true)
    expect(card.find('[data-test="needs-user-badge"]').exists()).toBe(true)
    expect(card.text()).toContain('✋ 待你决策')
  })
})

describe('BoardDagView 决策可见性（F1）', () => {
  const PROJECT = { id: 1, name: 'P', status: 'in_progress', created_at: 0 }
  async function mountDag(issues: any[]) {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'project.list') return Promise.resolve({ projects: [PROJECT] })
      if (cmd === 'issue.list') return Promise.resolve({ issues, total: issues.length })
      if (cmd === 'issue.get') return Promise.resolve({ issue: issues[0] })
      if (cmd === 'nodes.list') return Promise.resolve({ nodes: [] })
      return Promise.resolve({})
    })
    const w = mount(BoardDagView)
    await flushPromises()
    return w
  }

  it('决策 chip 计数 + ✋ 图标穿透到节点', async () => {
    const w = await mountDag([
      issue({ id: 1, status: 'in_review', needs_user: NU, depends_on: [] }),
      issue({ id: 2, plan_ready: PLAN_READY, depends_on: [{ id: 1, number: 'NB-1', status: 'in_review' }] }),
    ])
    const chip = w.find('[data-test="dag-decision-chip"]')
    expect(chip.exists()).toBe(true)
    expect(chip.text()).toContain('1')
    const svg = w.find('.dag-svg')
    expect(svg.text()).toContain('✋')
    expect(svg.text()).toContain('📋')
  })
})

describe('IssueDetailModal 决策面板（F2）', () => {
  function mountWithDecision(decisionResp: any, issueOver: Record<string, unknown> = {}) {
    requestMock.mockImplementation((_m: string, cmd: string, data: any) => {
      if (cmd === 'issue.get') return Promise.resolve({ issue: issue({ status: 'in_review', comments: [], activity: [], ...issueOver }) })
      if (cmd === 'issue.decision') return Promise.resolve(decisionResp)
      if (cmd === 'attachment.list') return Promise.resolve({ attachments: [] })
      if (cmd === 'nodes.list') return Promise.resolve({ nodes: [] })
      if (cmd === 'plan.list') return Promise.resolve({ plans: [] })
      return Promise.resolve({})
    })
    return mount(IssueDetailModal, { props: { issueId: 1 } })
  }

  it('无决策投影（旧后端/无停车）→ 不渲染决策面板', async () => {
    const w = mountWithDecision({})
    await flushPromises()
    expect(w.find('[data-test="decision-panel"]').exists()).toBe(false)
  })

  it('needs_user：原因卡（label+hint+建议处置）+ manual_check 预设四按钮', async () => {
    const w = mountWithDecision({
      issue: 'NB-1',
      issue_id: 1,
      status: 'in_review',
      needs_user: NU,
      plan_ready: null,
      attention: false,
      handoff: null,
      online_nodes: [],
    })
    await flushPromises()
    const panel = w.find('[data-test="decision-panel"]')
    expect(panel.exists()).toBe(true)
    expect(panel.text()).toContain('✋ 需要你决策')
    expect(panel.text()).toContain('连续失败熔断')
    expect(panel.text()).toContain('评审连续未过，需人工核查交付')
    expect(panel.text()).toContain('人工核查后裁决')
    // manual_check 预设：通过验收（主）/ 重跑验收 / 重派（needsTarget）/ 取消
    expect(panel.find('[data-test="decision-act-accept"]').exists()).toBe(true)
    expect(panel.find('[data-test="decision-act-retrigger"]').exists()).toBe(true)
    expect(panel.find('[data-test="decision-act-redispatch"]').exists()).toBe(true)
    expect(panel.find('[data-test="decision-act-cancel"]').exists()).toBe(true)
  })

  it('附言先行：非空附言 → comment.add 先于 issue.status（done 裁决）', async () => {
    const w = mountWithDecision({
      needs_user: NU,
      issue_id: 1,
      status: 'in_review',
      plan_ready: null,
      attention: false,
      handoff: null,
      online_nodes: [],
    })
    await flushPromises()
    await w.find('[data-test="decision-note"]').setValue('交付可用，通过')
    await w.find('[data-test="decision-act-accept"]').trigger('click')
    await flushPromises()
    const commentIdx = requestMock.mock.calls.findIndex((c) => c[1] === 'comment.add')
    const statusIdx = requestMock.mock.calls.findIndex((c) => c[1] === 'issue.status')
    expect(commentIdx).toBeGreaterThanOrEqual(0)
    expect(statusIdx).toBeGreaterThan(commentIdx)
    expect(requestMock.mock.calls[commentIdx][2]).toEqual({ issue_id: 1, content: '交付可用，通过' })
    expect(requestMock.mock.calls[statusIdx][2]).toEqual({ id: 1, status: 'done' })
  })

  it('重派闸：未选目标 → warn 且不发 issue.dispatch；选目标 → 派发', async () => {
    const w = mountWithDecision({
      needs_user: { ...NU, suggested_action: 'redispatch' },
      issue_id: 1,
      status: 'in_review',
      plan_ready: null,
      attention: false,
      handoff: null,
      online_nodes: [{ name: 'node-b', role: 'worker', professions: ['dev'], tier: 'big' }],
    })
    await flushPromises()
    await w.find('[data-test="decision-act-redispatch-main"]').trigger('click')
    await flushPromises()
    expect(requestMock.mock.calls.some((c) => c[1] === 'issue.dispatch')).toBe(false)
    expect(useToast().toasts.some((t) => t.type === 'warn')).toBe(true)

    // online_nodes 进下拉；选中后重派成功
    const opt = w.findAll('[data-test="decision-target"] option').find((o) => (o.element as HTMLOptionElement).value === 'node-b')
    expect(opt).toBeTruthy()
    await w.find('[data-test="decision-target"]').setValue('node-b')
    await w.find('[data-test="decision-act-redispatch-main"]').trigger('click')
    await flushPromises()
    const call = requestMock.mock.calls.find((c) => c[1] === 'issue.dispatch')!
    expect(call[2]).toEqual({ id: 1, target: 'node-b' })
  })

  it('重派下拉去重：在线组非空只渲染在线组（2 worker 不显示 4 项）', async () => {
    // 实录根修（2026-10-17）：在线组（issue.decision）与注册表组
    // （nodes.list）此前无条件拼接，同一在线节点渲染两次。两组同装
    // 才钉得住旧 bug——nodes.list 返回同两台 worker（旧代码出 4 项）。
    requestMock.mockImplementation((_m: string, cmd: string, data: any) => {
      if (cmd === 'issue.get') return Promise.resolve({ issue: issue({ status: 'in_review', comments: [], activity: [] }) })
      if (cmd === 'issue.decision')
        return Promise.resolve({
          needs_user: { ...NU, suggested_action: 'redispatch' },
          issue_id: 1,
          status: 'in_review',
          plan_ready: null,
          attention: false,
          handoff: null,
          online_nodes: [
            { name: 'worker-win', role: 'worker', professions: [], tier: 'big' },
            { name: 'worker-linux', role: 'worker', professions: [], tier: 'normal' },
          ],
        })
      if (cmd === 'attachment.list') return Promise.resolve({ attachments: [] })
      if (cmd === 'nodes.list')
        return Promise.resolve({
          nodes: [
            { id: 'n-win', name: 'worker-win', role: 'worker', online: true },
            { id: 'n-linux', name: 'worker-linux', role: 'worker', online: true },
          ],
        })
      if (cmd === 'plan.list') return Promise.resolve({ plans: [] })
      return Promise.resolve({})
    })
    const w = mount(IssueDetailModal, { props: { issueId: 1 } })
    await flushPromises()
    const opts = w
      .findAll('[data-test="decision-target"] option')
      .map((o) => (o.element as HTMLOptionElement).value)
      .filter(Boolean)
    expect(opts).toEqual(['worker-win', 'worker-linux'])
  })

  it('重派下拉回退：在线组为空才用注册表 worker（coordinator 不进目标域）', async () => {
    requestMock.mockImplementation((_m: string, cmd: string, data: any) => {
      if (cmd === 'issue.get') return Promise.resolve({ issue: issue({ status: 'in_review', comments: [], activity: [] }) })
      if (cmd === 'issue.decision')
        return Promise.resolve({
          needs_user: { ...NU, suggested_action: 'redispatch' },
          issue_id: 1,
          status: 'in_review',
          plan_ready: null,
          attention: false,
          handoff: null,
          online_nodes: [],
        })
      if (cmd === 'attachment.list') return Promise.resolve({ attachments: [] })
      if (cmd === 'nodes.list')
        return Promise.resolve({
          nodes: [
            { id: 'n-master', name: 'master', role: 'coordinator', online: true },
            { id: 'n-win', name: 'worker-win', role: 'worker', online: true },
            { id: 'n-linux', name: 'worker-linux', role: 'worker', online: false },
          ],
        })
      if (cmd === 'plan.list') return Promise.resolve({ plans: [] })
      return Promise.resolve({})
    })
    const w = mount(IssueDetailModal, { props: { issueId: 1 } })
    await flushPromises()
    const opts = w
      .findAll('[data-test="decision-target"] option')
      .map((o) => (o.element as HTMLOptionElement).value)
      .filter(Boolean)
    // coordinator 排除；离线 worker 保留（带离线标注，诚实可见）。
    expect(opts).toEqual(['n-win', 'n-linux'])
  })

  it('更多上下文：展开显示判决链（verdict_chain）', async () => {
    const w = mountWithDecision({
      needs_user: NU,
      issue_id: 1,
      status: 'in_review',
      plan_ready: null,
      attention: false,
      handoff: {
        verdict_chain: [
          // 后端 timeline 聚合写的 at 是 unix 秒（Rust 真相源）——夹具必须
          // 镜像线上 wire 形态；ISO 串夹具会掩盖 Number 换算错误。
          { at: 1792049904, verdict: 'FAIL', decision: '锚点未命中' },
        ],
        attempted: [{ action: 'review' }],
        resume_entry: 'review.retrigger NB-1',
      },
      online_nodes: [],
    })
    await flushPromises()
    expect(w.find('[data-test="decision-context"]').exists()).toBe(false)
    await w.find('[data-test="decision-more"]').trigger('click')
    const ctx = w.find('[data-test="decision-context"]')
    expect(ctx.exists()).toBe(true)
    expect(ctx.text()).toContain('FAIL')
    expect(ctx.text()).toContain('锚点未命中')
    expect(ctx.text()).toContain('review.retrigger NB-1')
    // unix 秒正确换算（1792049904 = 2026 年）：时间列不再是兜底「—」
    expect(ctx.text()).toContain('2026')
  })

  it('plan_ready 变体：确认发车 → issue.plan {plan_id, confirm: true}', async () => {
    // confirmPlan 消费 recoverPlan 恢复的 planId —— plan.list 必须回 ready 计划
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'issue.get') return Promise.resolve({ issue: issue({ status: 'backlog', comments: [], activity: [] }) })
      if (cmd === 'issue.decision')
        return Promise.resolve({
          needs_user: null,
          issue_id: 1,
          status: 'backlog',
          plan_ready: PLAN_READY,
          attention: false,
          handoff: null,
          online_nodes: [],
        })
      if (cmd === 'plan.list')
        return Promise.resolve({
          plans: [{ plan_id: 'plan-dv-1', status: 'ready', subs: [{ title: '子一', depends_on: [] }] }],
        })
      if (cmd === 'attachment.list') return Promise.resolve({ attachments: [] })
      if (cmd === 'nodes.list') return Promise.resolve({ nodes: [] })
      return Promise.resolve({ created: [{ number: 'NB-2' }], dispatched: 1, deferred: [] })
    })
    const w = mount(IssueDetailModal, { props: { issueId: 1 } })
    await flushPromises()
    const panel = w.find('[data-test="decision-panel"]')
    expect(panel.exists()).toBe(true)
    expect(panel.text()).toContain('计划待确认')
    expect(panel.text()).toContain('3 个子单')
    await panel.find('[data-test="decision-plan-confirm"]').trigger('click')
    await flushPromises()
    const call = requestMock.mock.calls.find((c) => c[1] === 'issue.plan' && c[2]?.confirm)!
    expect(call[2]).toMatchObject({ id: 1, plan_id: 'plan-dv-1', confirm: true })
  })

  it('attention 投影 → 元信息区 ⚠ 停滞徽标', async () => {
    const w = mountWithDecision({
      needs_user: null,
      issue_id: 1,
      status: 'in_progress',
      plan_ready: null,
      attention: true,
      handoff: null,
      online_nodes: [],
    }, { status: 'in_progress' })
    await flushPromises()
    expect(w.find('[data-test="detail-attention-badge"]').exists()).toBe(true)
  })

  it('in_progress 停车单：retrigger 禁用（后端只收 in_review）+ 提示出路；accept 照常可用', async () => {
    // in_progress 停车（派发超时/worker 异常类）交付未到，重跑验收必然被
    // 后端拒——按钮禁用并把出路讲清楚，而不是让预设主操作点下去吃报错。
    const w = mountWithDecision({
      needs_user: NU,
      issue_id: 1,
      status: 'in_progress',
      plan_ready: null,
      attention: false,
      handoff: null,
      online_nodes: [],
    }, { status: 'in_progress' })
    await flushPromises()
    const retrig = w.find('[data-test="decision-act-retrigger"]')
    expect(retrig.exists()).toBe(true)
    expect((retrig.element as HTMLButtonElement).disabled).toBe(true)
    expect(retrig.attributes('title')).toContain('in_review')
    expect(((w.find('[data-test="decision-act-accept"]').element) as HTMLButtonElement).disabled).toBe(false)
  })

  it('放弃计划：面板立即收起（不等服务端投影刷新，plan_ready 仍在响应里）', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'issue.get') return Promise.resolve({ issue: issue({ status: 'backlog', comments: [], activity: [] }) })
      if (cmd === 'issue.decision')
        return Promise.resolve({
          needs_user: null, issue_id: 1, status: 'backlog',
          plan_ready: PLAN_READY, attention: false, handoff: null, online_nodes: [],
        })
      if (cmd === 'plan.list')
        return Promise.resolve({ plans: [{ plan_id: 'plan-dv-1', status: 'ready', subs: [{ title: '子一', depends_on: [] }] }] })
      if (cmd === 'attachment.list') return Promise.resolve({ attachments: [] })
      if (cmd === 'nodes.list') return Promise.resolve({ nodes: [] })
      return Promise.resolve({})
    })
    const w = mount(IssueDetailModal, { props: { issueId: 1 } })
    await flushPromises()
    expect(w.find('[data-test="decision-panel"]').exists()).toBe(true)
    await w.find('[data-test="decision-plan-dismiss"]').trigger('click')
    await flushPromises()
    // decision 响应未变（plan_ready 投影仍在），但本地 planId 已清 → 面板收起，
    // 不留「确认发车」死按钮
    expect(w.find('[data-test="decision-panel"]').exists()).toBe(false)
  })

  it('打回部分失败（create 成功 + dispatch 失败）→ warn 指明返工单号，不谎报「打回失败」', async () => {
    const child = issue({ id: 58, number: 'NB-58', title: '导出缺列', status: 'done', parent_issue_id: 1, project_id: 7, priority: 2 })
    requestMock.mockImplementation((_m: string, cmd: string, data: any) => {
      if (cmd === 'issue.get') return Promise.resolve({ issue: issue({ status: 'in_review', comments: [], activity: [] }) })
      if (cmd === 'issue.decision')
        return Promise.resolve({
          issue: 'NB-1', issue_id: 1, status: 'in_review',
          needs_user: {
            stop_kind: 'parent_unresolved', stop_kind_label: '父单收口未决',
            suggested_action: 'manual_check', hint: '父单收口验收 UNSURE', stopped_at: null,
          },
          plan_ready: null, attention: false, handoff: null,
          online_nodes: [{ name: 'node-b', role: 'worker', professions: ['dev'], tier: 'big' }],
        })
      if (cmd === 'issue.list') return Promise.resolve({ issues: [child], total: 1 })
      if (cmd === 'issue.create') return Promise.resolve({ created: true, issue: { id: 97, number: 'NB-97' } })
      if (cmd === 'issue.dispatch')
        return Promise.reject(new Error('该 issue 已有进行中的派发'))
      if (cmd === 'attachment.list') return Promise.resolve({ attachments: [] })
      if (cmd === 'nodes.list') return Promise.resolve({ nodes: [] })
      if (cmd === 'plan.list') return Promise.resolve({ plans: [] })
      return Promise.resolve({})
    })
    const w = mount(IssueDetailModal, { props: { issueId: 1 } })
    await flushPromises()
    await w.find('[data-test="decision-act-sendback"]').trigger('click')
    const card = w.find('[data-test="decision-sendback"]')
    await card.find('[data-test="sendback-child-select"]').setValue('58')
    await card.find('[data-test="sendback-feedback"]').setValue('导出少一列')
    await card.find('[data-test="sendback-target"]').setValue('node-b')
    await card.find('[data-test="sendback-run"]').trigger('click')
    await flushPromises()
    // 诚实化：指明已建出的返工单号 + 勿重复打回，而不是笼统「打回失败」
    const warns = useToast().toasts.filter((t) => t.type === 'warn')
    expect(warns.length).toBe(1)
    expect(warns[0].message).toContain('NB-97')
    expect(warns[0].message).toContain('请勿重复打回')
    // 表单复位：再次点击走前置闸（childPick 已清 → warn 拦下），不会建出第二张返工单
    await card.find('[data-test="sendback-run"]').trigger('click')
    await flushPromises()
    expect(requestMock.mock.calls.filter((c) => c[1] === 'issue.create').length).toBe(1)
  })
})

describe('IssueDetailModal 父单中间档（2026-10-15 用户反馈：复评死循环出路）', () => {
  const PARENT_NU = {
    stop_kind: 'parent_unresolved',
    stop_kind_label: '父单收口未决',
    suggested_action: 'manual_check',
    hint: '父单收口验收 UNSURE：核查各子单结论后人工定 done 或重开子单',
    stopped_at: '2026-10-15T14:45:49Z',
  }

  function mountParentish(issueOver: Record<string, unknown> = {}, children: any[] = []) {
    requestMock.mockImplementation((_m: string, cmd: string, data: any) => {
      if (cmd === 'issue.get') return Promise.resolve({ issue: issue({ status: 'in_review', comments: [], activity: [], ...issueOver }) })
      if (cmd === 'issue.decision')
        return Promise.resolve({
          issue: 'NB-1', issue_id: 1, status: 'in_review',
          needs_user: PARENT_NU, plan_ready: null, attention: false, handoff: null,
          online_nodes: [{ name: 'node-b', role: 'worker', professions: ['dev'], tier: 'big' }],
        })
      if (cmd === 'issue.list') return Promise.resolve({ issues: children, total: children.length })
      if (cmd === 'attachment.list') return Promise.resolve({ attachments: [] })
      if (cmd === 'nodes.list') return Promise.resolve({ nodes: [] })
      if (cmd === 'plan.list') return Promise.resolve({ plans: [] })
      return Promise.resolve({})
    })
    return mount(IssueDetailModal, { props: { issueId: 1 } })
  }

  it('父单面板五按钮：AI 复评收口（主）+ 补验收标准 + 打回子单 + 通过 + 取消', async () => {
    const w = mountParentish()
    await flushPromises()
    const panel = w.find('[data-test="decision-panel"]')
    expect(panel.text()).toContain('AI 复评收口')
    for (const k of ['retrigger', 'edit-ac', 'sendback', 'accept', 'cancel'])
      expect(panel.find(`[data-test="decision-act-${k}"]`).exists()).toBe(true)
  })

  it('补验收标准：预填现状 → issue.update 先于 review.retrigger；空标准 warn 拒发', async () => {
    const w = mountParentish({ acceptance_criteria: '旧基线（缺失）' })
    await flushPromises()
    await w.find('[data-test="decision-act-edit-ac"]').trigger('click')
    const editor = w.find('[data-test="decision-ac-editor"]')
    expect(editor.exists()).toBe(true)
    expect((editor.find('[data-test="decision-ac-input"]').element as HTMLTextAreaElement).value).toBe('旧基线（缺失）')
    // 空标准：warn 且不发
    await editor.find('[data-test="decision-ac-input"]').setValue('   ')
    await editor.find('[data-test="decision-ac-save-retrigger"]').trigger('click')
    await flushPromises()
    expect(requestMock.mock.calls.some((c) => c[1] === 'issue.update')).toBe(false)
    expect(useToast().toasts.some((t) => t.type === 'warn')).toBe(true)
    // 有效标准：update → retrigger 顺序
    await editor.find('[data-test="decision-ac-input"]').setValue('AC-01 地块放置可用')
    await editor.find('[data-test="decision-ac-save-retrigger"]').trigger('click')
    await flushPromises()
    const updIdx = requestMock.mock.calls.findIndex((c) => c[1] === 'issue.update')
    const retIdx = requestMock.mock.calls.findIndex((c) => c[1] === 'review.retrigger')
    expect(updIdx).toBeGreaterThanOrEqual(0)
    expect(retIdx).toBeGreaterThan(updIdx)
    expect(requestMock.mock.calls[updIdx][2]).toEqual({ id: 1, acceptance_criteria: 'AC-01 地块放置可用' })
    expect(requestMock.mock.calls[retIdx][2]).toEqual({ number: 'NB-1' })
  })

  it('打回已完成子单 → 返工单承接：issue.create（挂父单）→ issue.dispatch 新单', async () => {
    const child = issue({ id: 55, number: 'NB-55', title: '渲染循环卡顿', status: 'done', parent_issue_id: 1, project_id: 7, priority: 2 })
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'issue.get') return Promise.resolve({ issue: issue({ status: 'in_review', comments: [], activity: [] }) })
      if (cmd === 'issue.decision')
        return Promise.resolve({
          issue: 'NB-1', issue_id: 1, status: 'in_review',
          needs_user: PARENT_NU, plan_ready: null, attention: false, handoff: null,
          online_nodes: [{ name: 'node-b', role: 'worker', professions: ['dev'], tier: 'big' }],
        })
      if (cmd === 'issue.list') return Promise.resolve({ issues: [child], total: 1 })
      if (cmd === 'attachment.list') return Promise.resolve({ attachments: [] })
      if (cmd === 'nodes.list') return Promise.resolve({ nodes: [] })
      if (cmd === 'plan.list') return Promise.resolve({ plans: [] })
      if (cmd === 'issue.create') return Promise.resolve({ created: true, issue: { id: 99, number: 'NB-99' } })
      return Promise.resolve({})
    })
    const w = mount(IssueDetailModal, { props: { issueId: 1 } })
    await flushPromises()
    await w.find('[data-test="decision-act-sendback"]').trigger('click')
    const card = w.find('[data-test="decision-sendback"]')
    expect(card.exists()).toBe(true)
    // 完成度一行进原因卡（done 子单计入已完成）
    expect(w.find('[data-test="decision-panel"]').text()).toContain('1/1 已完成')
    // 闸：未选子单/意见/节点 → warn 不发
    await card.find('[data-test="sendback-run"]').trigger('click')
    await flushPromises()
    expect(requestMock.mock.calls.some((c) => c[1] === 'issue.create')).toBe(false)
    // 全填 → create + dispatch（新单 id）
    await card.find('[data-test="sendback-child-select"]').setValue('55')
    await card.find('[data-test="sendback-feedback"]').setValue('碰撞检测数组越界，回归用例补上')
    await card.find('[data-test="sendback-target"]').setValue('node-b')
    await card.find('[data-test="sendback-run"]').trigger('click')
    await flushPromises()
    const createCall = requestMock.mock.calls.find((c) => c[1] === 'issue.create')!
    // 全角冒号前缀（与后端 REMEDIATION_TITLE_PREFIX 对齐，见 IssueDetailModal.vue :738 注释）
    expect(createCall[2]).toMatchObject({ title: '返工：渲染循环卡顿', parent_issue_id: 1, project_id: 7, priority: 2 })
    expect(String(createCall[2].description)).toContain('人工打回 NB-55')
    const dispCall = requestMock.mock.calls.find((c) => c[1] === 'issue.dispatch')!
    expect(dispCall[2]).toEqual({ id: 99, target: 'node-b' })
    expect(requestMock.mock.calls.some((c) => c[1] === 'issue.status')).toBe(false)
  })

  it('打回未完成子单 → 评论打回 + 重派原单（in_review 无需状态转移）', async () => {
    const child = issue({ id: 56, number: 'NB-56', title: '存档损坏', status: 'in_review', parent_issue_id: 1, project_id: 7, priority: 1 })
    const w = mountParentish({}, [child])
    await flushPromises()
    await w.find('[data-test="decision-act-sendback"]').trigger('click')
    const card = w.find('[data-test="decision-sendback"]')
    await card.find('[data-test="sendback-child-select"]').setValue('56')
    await card.find('[data-test="sendback-feedback"]').setValue('存档读回后丢字段')
    await card.find('[data-test="sendback-target"]').setValue('node-b')
    await card.find('[data-test="sendback-run"]').trigger('click')
    await flushPromises()
    const cmtCall = requestMock.mock.calls.find((c) => c[1] === 'comment.add')!
    expect(cmtCall[2].issue_id).toBe(56)
    expect(String(cmtCall[2].content)).toContain('【人工打回】')
    const dispCall = requestMock.mock.calls.find((c) => c[1] === 'issue.dispatch')!
    expect(dispCall[2]).toEqual({ id: 56, target: 'node-b' })
    // in_review 不在 dispatch 闸外，无需状态转移
    expect(requestMock.mock.calls.some((c) => c[1] === 'issue.status')).toBe(false)
    expect(requestMock.mock.calls.some((c) => c[1] === 'issue.create')).toBe(false)
  })

  it('返工单 project_id 回落父单：子单无项目时不从项目视图消失', async () => {
    const child = issue({ id: 57, number: 'NB-57', title: '路径查询超时', status: 'done', parent_issue_id: 1, project_id: null, priority: 1 })
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'issue.get') return Promise.resolve({ issue: issue({ status: 'in_review', comments: [], activity: [], project_id: 7 }) })
      if (cmd === 'issue.decision')
        return Promise.resolve({
          issue: 'NB-1', issue_id: 1, status: 'in_review',
          needs_user: PARENT_NU, plan_ready: null, attention: false, handoff: null,
          online_nodes: [{ name: 'node-b', role: 'worker', professions: ['dev'], tier: 'big' }],
        })
      if (cmd === 'issue.list') return Promise.resolve({ issues: [child], total: 1 })
      if (cmd === 'attachment.list') return Promise.resolve({ attachments: [] })
      if (cmd === 'nodes.list') return Promise.resolve({ nodes: [] })
      if (cmd === 'plan.list') return Promise.resolve({ plans: [] })
      if (cmd === 'issue.create') return Promise.resolve({ created: true, issue: { id: 98, number: 'NB-98' } })
      return Promise.resolve({})
    })
    const w = mount(IssueDetailModal, { props: { issueId: 1 } })
    await flushPromises()
    await w.find('[data-test="decision-act-sendback"]').trigger('click')
    const card = w.find('[data-test="decision-sendback"]')
    await card.find('[data-test="sendback-child-select"]').setValue('57')
    await card.find('[data-test="sendback-feedback"]').setValue('超时无降级')
    await card.find('[data-test="sendback-target"]').setValue('node-b')
    await card.find('[data-test="sendback-run"]').trigger('click')
    await flushPromises()
    const createCall = requestMock.mock.calls.find((c) => c[1] === 'issue.create')!
    // 子单 project_id null → 回落父单 7（build_new_issue 不做父继承，前端兜）
    expect(createCall[2].project_id).toBe(7)
  })

  it('✨ AI 起草：issue.draft_ac 草稿预填输入框，不直接落库', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'issue.get') return Promise.resolve({ issue: issue({ status: 'in_review', comments: [], activity: [] }) })
      if (cmd === 'issue.decision')
        return Promise.resolve({
          issue: 'NB-1', issue_id: 1, status: 'in_review',
          needs_user: PARENT_NU, plan_ready: null, attention: false, handoff: null,
          online_nodes: [{ name: 'node-b', role: 'worker', professions: ['dev'], tier: 'big' }],
        })
      if (cmd === 'issue.draft_ac') return Promise.resolve({ issue: 'NB-1', draft: '- AC-01 可客观核验\n- AC-02 无未闭环缺陷' })
      if (cmd === 'issue.list') return Promise.resolve({ issues: [], total: 0 })
      if (cmd === 'attachment.list') return Promise.resolve({ attachments: [] })
      if (cmd === 'nodes.list') return Promise.resolve({ nodes: [] })
      if (cmd === 'plan.list') return Promise.resolve({ plans: [] })
      return Promise.resolve({})
    })
    const w = mount(IssueDetailModal, { props: { issueId: 1 } })
    await flushPromises()
    await w.find('[data-test="decision-act-edit-ac"]').trigger('click')
    const input = w.find('[data-test="decision-ac-input"]')
    expect((input.element as HTMLTextAreaElement).value).toBe('')
    await w.find('[data-test="decision-ac-draft"]').trigger('click')
    await flushPromises()
    const draftCall = requestMock.mock.calls.find((c) => c[1] === 'issue.draft_ac')!
    expect(draftCall[2]).toEqual({ id: 1 })
    // 显式长超时（2026-10-16 用户实锤：默认 30s 在 LLM 慢轮次客户端先拒收，
    // 服务端起草成功但草稿被丢）——第 4 参必须是 180000。
    expect(draftCall[3]).toBe(180000)
    expect((input.element as HTMLTextAreaElement).value).toBe('- AC-01 可客观核验\n- AC-02 无未闭环缺陷')
    // 草稿未落库：没有 issue.update（保存决定权在用户，走保存并复评）
    expect(requestMock.mock.calls.some((c) => c[1] === 'issue.update')).toBe(false)
    expect(useToast().toasts.some((t) => t.type === 'success' && t.message.includes('已生成草稿'))).toBe(true)
    // 按钮态复位（draftingAc 归零，可再次点击）
    expect((w.find('[data-test="decision-ac-draft"]').element as HTMLButtonElement).disabled).toBe(false)
  })

  it('✨ AI 起草空结果：warn 不覆盖输入框', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'issue.get') return Promise.resolve({ issue: issue({ status: 'in_review', comments: [], activity: [] }) })
      if (cmd === 'issue.decision')
        return Promise.resolve({
          issue: 'NB-1', issue_id: 1, status: 'in_review',
          needs_user: PARENT_NU, plan_ready: null, attention: false, handoff: null,
          online_nodes: [],
        })
      if (cmd === 'issue.draft_ac') return Promise.resolve({ issue: 'NB-1', draft: '' })
      if (cmd === 'issue.list') return Promise.resolve({ issues: [], total: 0 })
      if (cmd === 'attachment.list') return Promise.resolve({ attachments: [] })
      if (cmd === 'nodes.list') return Promise.resolve({ nodes: [] })
      if (cmd === 'plan.list') return Promise.resolve({ plans: [] })
      return Promise.resolve({})
    })
    const w = mount(IssueDetailModal, { props: { issueId: 1 } })
    await flushPromises()
    await w.find('[data-test="decision-act-edit-ac"]').trigger('click')
    // 输入框留空（空框走起草不触发覆盖确认——确认臂单独一测）
    const input = w.find('[data-test="decision-ac-input"]')
    await w.find('[data-test="decision-ac-draft"]').trigger('click')
    await flushPromises()
    expect(useToast().toasts.some((t) => t.type === 'warn' && t.message.includes('空结果'))).toBe(true)
    expect((input.element as HTMLTextAreaElement).value).toBe('')
  })

  it('✨ AI 起草覆盖确认：有手输内容时 confirm 拒绝则不发请求', async () => {
    const confirmSpy = vi.spyOn(window, 'confirm').mockReturnValue(false)
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'issue.get') return Promise.resolve({ issue: issue({ status: 'in_review', comments: [], activity: [] }) })
      if (cmd === 'issue.decision')
        return Promise.resolve({
          issue: 'NB-1', issue_id: 1, status: 'in_review',
          needs_user: PARENT_NU, plan_ready: null, attention: false, handoff: null,
          online_nodes: [],
        })
      if (cmd === 'issue.draft_ac') return Promise.resolve({ issue: 'NB-1', draft: '草稿' })
      if (cmd === 'issue.list') return Promise.resolve({ issues: [], total: 0 })
      if (cmd === 'attachment.list') return Promise.resolve({ attachments: [] })
      if (cmd === 'nodes.list') return Promise.resolve({ nodes: [] })
      if (cmd === 'plan.list') return Promise.resolve({ plans: [] })
      return Promise.resolve({})
    })
    const w = mount(IssueDetailModal, { props: { issueId: 1 } })
    await flushPromises()
    await w.find('[data-test="decision-act-edit-ac"]').trigger('click')
    await w.find('[data-test="decision-ac-input"]').setValue('手输基线')
    await w.find('[data-test="decision-ac-draft"]').trigger('click')
    await flushPromises()
    expect(confirmSpy).toHaveBeenCalled()
    expect(requestMock.mock.calls.some((c) => c[1] === 'issue.draft_ac')).toBe(false)
    // 输入框保持手输内容
    expect((w.find('[data-test="decision-ac-input"]').element as HTMLTextAreaElement).value).toBe('手输基线')
    confirmSpy.mockRestore()
  })
})
