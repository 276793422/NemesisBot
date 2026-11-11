import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { useToast } from '../../../composables/useToast'

// IssueDetailModal 未覆盖面补充 spec（增量：本目录此前没有专属 spec，
// 既有 76% 覆盖来自 BoardKanban/BoardPanels 的间接挂载）。
// 覆盖目标（与源码函数一一对应）：
// - 详情/附件/决策/子单/节点加载的失败与过期响应守卫分支；
// - 时间线页签（loadTimeline 守卫/失败/空态/加载中）；
// - 评论线程（回复/@提及/ctype 徽标兜底/提交失败）；
// - 操作层：状态转移/优先级/指派/派发/撤回派发/评审重触发/任务档案；
// - 决策面板全部动作（通过/复评/重派/取消/重新拆解）+ 附言 flush 失败中止；
// - 父单特化中间档（补验收标准含 AI 起草 / 打回子单含 done 建返工单与
//   部分失败诚实化）；
// - plan_ready（recoverPlan 恢复三态 + confirmPlan）与 SSE 三事件；
// - 附件上传（onFilePick 守卫/超限/base64）与下载。

const requestMock = vi.fn()

vi.mock('../../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
  initWSAPI: vi.fn(),
}))

// useBoardActors 是模块级单例（loaded 标志跨测试持久，先跑的测试会把
// nodes 映射定格在空表）——直接 mock 掉，name 解析由测试可控。
vi.mock('../../../composables/useBoardActors', () => ({
  useBoardActors: () => ({
    ensureNodes: vi.fn().mockResolvedValue(undefined),
    displayActor: (kind: string, id: string) => `${kind}/${id}`,
    actorName: (id: string) => (id === 'w1' ? 'Alice' : null),
  }),
}))

// SSE mock：handler 注册表暴露为 __handlers，测试内可派发 board.plan_ready /
// board.plan_failed / board.plan_started / board-changed（useBoardChanged 同源）。
vi.mock('../../../composables/useSSE', () => {
  const handlers: Record<string, Array<(d: any) => void>> = {}
  return {
    on: (ev: string, h: (d: any) => void) => {
      ;(handlers[ev] = handlers[ev] || []).push(h)
    },
    off: (ev: string, h?: (d: any) => void) => {
      if (!h) {
        delete handlers[ev]
        return
      }
      handlers[ev] = (handlers[ev] || []).filter((x) => x !== h)
    },
    __handlers: handlers,
  }
})

import IssueDetailModal from '../IssueDetailModal.vue'
import * as sseModule from '../../../composables/useSSE'

function sseDispatch(ev: string, data: any) {
  const hs = ((sseModule as any).__handlers[ev] || []) as Array<(d: any) => void>
  hs.forEach((h) => h(data))
}

function makeIssue(over: Record<string, unknown> = {}) {
  return {
    id: 1,
    number: 'NB-1',
    title: '标题',
    description: '',
    status: 'todo',
    priority: 1,
    assignee: null,
    assignee_id: null,
    creator: { kind: 'manager', id: 'board' },
    due_date: null,
    position: 0,
    acceptance_criteria: null,
    origin: null,
    created_at: 1700000000,
    updated_at: 1700000001,
    comments: [],
    activity: [],
    subscribers: [],
    ...over,
  }
}

function decision(over: Record<string, unknown> = {}) {
  return {
    issue: 'NB-1',
    issue_id: 1,
    status: 'todo',
    needs_user: null,
    plan_ready: null,
    attention: false,
    handoff: null,
    online_nodes: [],
    ...over,
  }
}

const NU_RETRY = {
  stop_kind: 'unsure_streak',
  stop_kind_label: '连续不确定',
  suggested_action: 'retry',
  hint: '连续 3 轮 UNSURE',
  stopped_at: '2026-10-15T10:00:00Z',
}

const NU_PARENT = {
  stop_kind: 'parent_unresolved',
  stop_kind_label: '父单收口未决',
  suggested_action: 'manual_check',
  hint: '等待子单收口',
  stopped_at: null,
}

const NU_EXHAUSTED = {
  stop_kind: 'plan_exhausted',
  stop_kind_label: '拆解重试耗尽',
  suggested_action: 'retry',
  hint: '拆解三连败',
  stopped_at: null,
}

const NODE_W1 = { id: 'w1', name: 'Node1', role: 'worker', online: true }

const CHILDREN = [
  { id: 11, number: 'NB-11', title: '子单A', status: 'done', priority: 1, project_id: null, parent_issue_id: 1 },
  { id: 12, number: 'NB-12', title: '子单B', status: 'blocked', priority: 2, project_id: null, parent_issue_id: 1 },
]

const SUB = { title: '子A', description: '描述A', required_profession: 'ops', required_role: 'r1', required_tags: ['t1'], acceptance_criteria: 'ac1', depends_on: [1] }

/** 默认 request 分发（按 cmd）。issueOver 注入 issue 字段变体，over 注入
 *  各命令响应变体（decision/attachments/nodes/plans/issues/events）。 */
function defaultImpl(cmd: string, issueOver: Record<string, unknown> = {}, over: Record<string, any> = {}) {
  switch (cmd) {
    case 'issue.get':
      return Promise.resolve({ issue: makeIssue(issueOver) })
    case 'issue.decision':
      return Promise.resolve(over.decision !== undefined ? over.decision : null)
    case 'attachment.list':
      return Promise.resolve({ attachments: over.attachments || [] })
    case 'nodes.list':
      return Promise.resolve({ nodes: over.nodes || [] })
    case 'plan.list':
      return Promise.resolve({ plans: over.plans || [] })
    case 'issue.list':
      return Promise.resolve({ issues: over.issues || [] })
    case 'issue.timeline':
      return Promise.resolve({ events: over.events || [] })
    default:
      return Promise.resolve(over.__default !== undefined ? over.__default : {})
  }
}

function useImpl(issueOver: Record<string, unknown> = {}, over: Record<string, any> = {}) {
  requestMock.mockImplementation((_m: string, cmd: string) => defaultImpl(cmd, issueOver, over))
}

/** 带决策面板的 impl：overrides[cmd] 覆盖特定命令（值=Promise 或 (p)=>Promise）。 */
function useDecisionImpl(
  nu: Record<string, unknown> | null,
  issueOver: Record<string, unknown> = {},
  overrides: Record<string, any> = {},
  decisionOver: Record<string, unknown> = {},
) {
  requestMock.mockImplementation((_m: string, cmd: string, p?: any) => {
    const o = overrides[cmd]
    if (o !== undefined) return typeof o === 'function' ? o(p) : o
    return defaultImpl(cmd, issueOver, {
      decision: decision({ needs_user: nu, ...decisionOver }),
      issues: CHILDREN,
      nodes: [NODE_W1],
    })
  })
}

async function mountModal(issueId: number | null = 1, issueOver: Record<string, unknown> = {}, over: Record<string, any> = {}) {
  useImpl(issueOver, over)
  const w = mount(IssueDetailModal, { props: { issueId } })
  await flushPromises()
  return w
}

function callsFor(cmd: string) {
  return requestMock.mock.calls.filter((c) => c[1] === cmd)
}

beforeEach(() => {
  requestMock.mockReset()
  useToast().toasts.splice(0)
  vi.stubGlobal('confirm', vi.fn(() => true))
})

afterEach(() => {
  vi.unstubAllGlobals()
  vi.useRealTimers()
  // 下载测试对 URL 静态方法的手工桩在此复位。
  const u = URL as unknown as Record<string, unknown>
  if (u.__origCreateObjectURL !== undefined) {
    URL.createObjectURL = u.__origCreateObjectURL as typeof URL.createObjectURL
    URL.revokeObjectURL = u.__origRevokeObjectURL as typeof URL.revokeObjectURL
    delete u.__origCreateObjectURL
    delete u.__origRevokeObjectURL
  }
})

function stubObjectURL() {
  const u = URL as unknown as Record<string, unknown>
  u.__origCreateObjectURL = URL.createObjectURL
  u.__origRevokeObjectURL = URL.revokeObjectURL
  ;(URL as any).createObjectURL = vi.fn(() => 'blob:mock-url')
  ;(URL as any).revokeObjectURL = vi.fn()
}

// ---------------------------------------------------------------------------
// 详情加载 / 过期守卫 / 关闭 / board-changed 静默刷新
// ---------------------------------------------------------------------------
describe('IssueDetailModal 详情加载与守卫', () => {
  it('挂载即拉取详情并渲染元信息；origin/required_profession 有值时显示', async () => {
    const w = await mountModal(1, {
      description: '正文描述',
      required_profession: 'ops',
      origin: { origin_type: 'autopilot', origin_id: 'job-9' },
    })
    expect(callsFor('issue.get')).toEqual([['board', 'issue.get', { id: 1 }]])
    expect(w.text()).toContain('NB-1')
    expect(w.text()).toContain('标题')
    expect(w.text()).toContain('未指派')
    expect(w.text()).toContain('创建者：manager/board')
    expect(w.text()).toContain('来源：autopilot/job-9')
    expect(w.text()).toContain('🛠 ops')
    expect(w.text()).toContain('正文描述')
    w.unmount()
  })

  it('loadDetail 失败（非静默）：error toast + emit close，弹层不渲染', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) =>
      cmd === 'issue.get' ? Promise.reject(new Error('boom')) : Promise.resolve({}),
    )
    const w = mount(IssueDetailModal, { props: { issueId: 1 } })
    await flushPromises()
    expect(useToast().toasts.some((t) => t.type === 'error' && t.message.includes('加载详情失败'))).toBe(true)
    expect(w.emitted('close')).toBeTruthy()
    expect(w.find('.modal-backdrop').exists()).toBe(false)
    w.unmount()
  })

  it('issue.get 返回无 issue 键：detail 置 null，弹层收起', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) =>
      cmd === 'issue.get' ? Promise.resolve({}) : defaultImpl(cmd),
    )
    const w = mount(IssueDetailModal, { props: { issueId: 1 } })
    await flushPromises()
    expect(w.find('.modal-backdrop').exists()).toBe(false)
    w.unmount()
  })

  it('过期响应守卫：换单后旧单成功响应与失败都不污染新单视图', async () => {
    const defs: Array<{ res: (v: any) => void; rej: (e: any) => void }> = []
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'issue.get') {
        return new Promise((res, rej) => {
          defs.push({ res, rej })
        })
      }
      return defaultImpl(cmd)
    })
    const w = mount(IssueDetailModal, { props: { issueId: 1 } })
    await flushPromises()
    expect(defs.length).toBe(1)

    // 切到 2：第二个请求也挂起
    await w.setProps({ issueId: 2 })
    await flushPromises()
    expect(defs.length).toBe(2)

    // 旧单（1）失败响应晚到 → 静默吞，不 toast 不 close
    const closedBefore = w.emitted('close')?.length ?? 0
    defs[0].rej(new Error('stale boom'))
    await flushPromises()
    expect(useToast().toasts.some((t) => t.message.includes('加载详情失败'))).toBe(false)
    expect(w.emitted('close')?.length ?? 0).toBe(closedBefore)

    // 当前单（2）正常返回
    defs[1].res({ issue: makeIssue({ id: 2, number: 'NB-2' }) })
    await flushPromises()
    expect(w.text()).toContain('NB-2')

    // 切 1（挂起）→ 再切 3（挂起）→ 旧 1 的成功响应晚到 → 不得写 3 的视图
    await w.setProps({ issueId: 1 })
    await flushPromises()
    await w.setProps({ issueId: 3 })
    await flushPromises()
    expect(defs.length).toBe(4)
    defs[2].res({ issue: makeIssue({ id: 1, number: 'NB-OLD' }) })
    defs[3].res({ issue: makeIssue({ id: 3, number: 'NB-3' }) })
    await flushPromises()
    expect(w.text()).toContain('NB-3')
    expect(w.text()).not.toContain('NB-OLD')
    w.unmount()
  })

  it('关闭（issueId→null）：弹层收起；关闭态下各 load 守卫直接返回', async () => {
    const w = await mountModal(1)
    await w.setProps({ issueId: null })
    await flushPromises()
    expect(w.find('.modal-backdrop').exists()).toBe(false)

    const vm = w.vm as any
    const gets = callsFor('issue.get').length
    await vm.loadDetail()
    await vm.loadAttachments()
    await vm.loadDecision()
    await vm.loadChildren()
    await vm.recoverPlan()
    vm.switchTab('timeline')
    expect(callsFor('issue.get').length).toBe(gets)
    expect(callsFor('attachment.list').length).toBe(1) // 仍为挂载时那次
    w.unmount()
  })

  it('board-changed（200ms 防抖）：打开且有详情时静默换新；关闭后不再刷', async () => {
    vi.useFakeTimers()
    const w = await mountModal(1)
    const before = callsFor('issue.get').length
    sseDispatch('board-changed', {})
    vi.advanceTimersByTime(200)
    await flushPromises()
    expect(callsFor('issue.get').length).toBe(before + 1)
    expect(callsFor('issue.decision').length).toBe(2)
    expect(callsFor('attachment.list').length).toBe(2)

    // 关闭后 board-changed 不再触发加载
    await w.setProps({ issueId: null })
    await flushPromises()
    const after = callsFor('issue.get').length
    sseDispatch('board-changed', {})
    vi.advanceTimersByTime(200)
    await flushPromises()
    expect(callsFor('issue.get').length).toBe(after)
    w.unmount()
  })
})

// ---------------------------------------------------------------------------
// 时间线页签
// ---------------------------------------------------------------------------
describe('IssueDetailModal 时间线页签（U-C）', () => {
  const EVENTS = [
    { ts: 1700000000, source: 'activity', kind: 'status_change', actor: 'agent/Alex', summary: '摘要1', detail: 'todo→in_review' },
    { ts: 1700000001, source: 'comment', kind: 'discussion', actor: '', summary: '评论内容', detail: '评论内容' },
    { ts: 1700000002, source: 'dispatch', kind: 'dispatch', actor: 'worker/w9', summary: '派发到 x', detail: 'd'.repeat(121) },
    { ts: 0, source: 'unknown-src', kind: 'k', actor: 'a', summary: '档案行' },
  ]

  async function openTimeline(w: any) {
    await w.findAll('button').find((b: any) => b.text().includes('时间线'))!.trigger('click')
    await flushPromises()
  }

  it('时间线渲染：activity 显详情、comment/dispatch 徽标带 kind、>120 截断、未知源兜底、ts0 显 —', async () => {
    const w = await mountModal(1, {}, { events: EVENTS })
    await openTimeline(w)
    expect(callsFor('issue.timeline')).toEqual([['board', 'issue.timeline', { id: 1 }]])
    const rows = w.findAll('.timeline-row')
    expect(rows.length).toBe(4)
    // activity：正文取 detail（比 summary 有信息量），无独立 detail span
    expect(rows[0].text()).toContain('todo→in_review')
    expect(rows[0].find('.activity-details').exists()).toBe(false)
    // comment 徽标：评论·discussion；detail === summary → 不渲染 detail span
    expect(rows[1].text()).toContain('评论·discussion')
    expect(rows[1].find('.activity-details').exists()).toBe(false)
    // dispatch 徽标：派发·dispatch；detail 截断 120 + …
    expect(rows[2].text()).toContain('派发·dispatch')
    expect(rows[2].find('.activity-details').text()).toContain('…')
    // 未知 source：徽标兜底原文；ts 0 → fmtTime '—'
    expect(rows[3].text()).toContain('unknown-src')
    expect(rows[3].text()).toContain('—')
    w.unmount()
  })

  it('空事件与请求失败：诚实空态「暂无记录」（catch 分支）', async () => {
    const w = await mountModal(1)
    await openTimeline(w)
    expect(w.text()).toContain('暂无记录')

    // 失败臂：回详情页签再进时间线
    requestMock.mockImplementation((_m: string, cmd: string) =>
      cmd === 'issue.timeline' ? Promise.reject(new Error('net')) : defaultImpl(cmd),
    )
    await w.findAll('button').find((b: any) => b.text() === '详情')!.trigger('click')
    await openTimeline(w)
    expect(w.text()).toContain('暂无记录')
    w.unmount()
  })

  it('加载中守卫：在飞时重复切页签只发一次请求', async () => {
    let resolveT!: (v: any) => void
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'issue.timeline') {
        return new Promise((res) => {
          resolveT = res
        })
      }
      return defaultImpl(cmd)
    })
    const w = mount(IssueDetailModal, { props: { issueId: 1 } })
    await flushPromises()
    const tabBtn = () => w.findAll('button').find((b: any) => b.text().includes('时间线'))!
    await tabBtn().trigger('click')
    expect(w.text()).toContain('加载中…')
    await tabBtn().trigger('click') // timelineLoading=true → 守卫拦下
    expect(callsFor('issue.timeline').length).toBe(1)
    resolveT({ events: EVENTS })
    await flushPromises()
    expect(w.findAll('.timeline-row').length).toBe(4)
    // 回详情页签
    await w.findAll('button').find((b: any) => b.text() === '详情')!.trigger('click')
    expect(w.text()).toContain('指派：')
    w.unmount()
  })
})

// ---------------------------------------------------------------------------
// 评论线程 / 回复 / @提及
// ---------------------------------------------------------------------------
describe('IssueDetailModal 评论线程与 @提及', () => {
  const COMMENTS = [
    { id: 1, author: { kind: 'agent', id: 'Alex' }, content: '顶层评论', parent_id: null, ctype: 'comment', created_at: 1700000000 },
    { id: 2, author: { kind: 'worker', id: 'w2' }, content: '交付回复', parent_id: 1, ctype: 'delivery', created_at: 1700000001 },
    { id: 3, author: { kind: 'agent', id: 'x' }, content: '提问', parent_id: null, ctype: 'question', created_at: 1700000002 },
    { id: 4, author: { kind: 'agent', id: 'y' }, content: '怪类型', parent_id: null, ctype: 'ctype-x', created_at: 1700000003 },
    { id: 5, author: { kind: 'agent', id: 'z' }, content: '孤儿回复', parent_id: 999, ctype: 'comment', created_at: 1700000004 },
  ]

  it('线程渲染：顶层过滤、回复嵌套、ctype 徽标与兜底、delivery 报告体', async () => {
    const w = await mountModal(1, { comments: COMMENTS })
    expect(w.text()).toContain('评论（3）') // 顶层 = id1/3/4；id5 的父 999 不在 → 不渲染
    const items = w.findAll('.comment-item') // 嵌套 .reply-item 同名类 → 文档序含嵌套
    expect(items.length).toBe(4) // 3 顶层 + 1 嵌套回复
    expect(items[0].text()).toContain('agent/Alex')
    const reply = items[0].find('.reply-item')
    expect(reply.exists()).toBe(true)
    expect(reply.text()).toContain('worker/w2')
    expect(reply.text()).toContain('交付')
    expect(reply.find('.badge').classes()).toContain('badge-info')
    expect(reply.find('.comment-body-report').exists()).toBe(true)
    expect(items[2].text()).toContain('提问')
    expect(items[2].find('.badge').classes()).toContain('badge-warning')
    expect(items[3].text()).toContain('ctype-x') // 未知 ctype 徽标/文案兜底
    expect(items[3].find('.badge').classes()).toContain('badge-neutral')
    w.unmount()
  })

  it('startReply：@可读名前缀；重复回复不加双前缀；取消回复剥离前缀', async () => {
    const w = await mountModal(1, { comments: [COMMENTS[0]] })
    const ta = () => w.find('textarea.form-textarea')
    const replyBtn = () => w.findAll('.comment-item button').find((b: any) => b.text() === '回复')!

    await replyBtn().trigger('click')
    expect(w.find('.replying-hint').text()).toContain('回复 agent/Alex')
    expect((ta().element as HTMLTextAreaElement).value).toBe('@Alex ')

    // 重复点回复：已带 token 前缀 → 不重复
    await replyBtn().trigger('click')
    expect((ta().element as HTMLTextAreaElement).value).toBe('@Alex ')

    // 取消：前缀剥离
    await w.find('.replying-hint button').trigger('click')
    expect(w.find('.replying-hint').exists()).toBe(false)
    expect((ta().element as HTMLTextAreaElement).value).toBe('')
    w.unmount()
  })

  it('@提及候选：订阅者去重 + 指派 worker 并入；插入时空串与尾空白两种拼法', async () => {
    const w = await mountModal(1, {
      assignee: 'worker',
      assignee_id: 'w1',
      subscribers: [{ subscriber: { kind: 'worker', id: 'w1' } }, { kind: 'worker', id: 'w9' }],
    })
    const btns = w.findAll('.mention-row button')
    expect(btns.map((b: any) => b.text())).toEqual(['@w1', '@w9'])
    const ta = () => w.find('textarea.form-textarea')

    await btns.find((b: any) => b.text() === '@w9')!.trigger('click')
    expect((ta().element as HTMLTextAreaElement).value).toBe('@w9 ')
    // 尾部已有空白 → 直接拼接
    await btns.find((b: any) => b.text() === '@w1')!.trigger('click')
    expect((ta().element as HTMLTextAreaElement).value).toBe('@w9 @w1 ')
    // 尾部非空白 → 先补空格
    await ta().setValue('abc')
    await btns.find((b: any) => b.text() === '@w1')!.trigger('click')
    expect((ta().element as HTMLTextAreaElement).value).toBe('abc @w1 ')
    // 指派 label：actorName 命中（mock：w1→Alice）
    expect(w.text()).toContain('worker: Alice')
    w.unmount()
  })

  it('submitComment：成功清空 + 刷新 + changed；回复带 parent_id；失败 toast', async () => {
    const w = await mountModal(1, { comments: [COMMENTS[0]] })
    const ta = () => w.find('textarea.form-textarea')
    await ta().setValue('反馈内容')
    await w.findAll('button').find((b: any) => b.text() === '发表评论')!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('board', 'comment.add', { issue_id: 1, content: '反馈内容' })
    expect((ta().element as HTMLTextAreaElement).value).toBe('')
    expect(w.emitted('changed')).toBeTruthy()
    expect(callsFor('issue.get').length).toBe(2)

    // 回复路径：parent_id 带上（提交后 loadDetail 是最后调用，改用过滤断言）
    await w.findAll('.comment-item button').find((b: any) => b.text() === '回复')!.trigger('click')
    await ta().setValue('回复正文')
    await w.findAll('button').find((b: any) => b.text() === '发表评论')!.trigger('click')
    await flushPromises()
    const cadds = callsFor('comment.add')
    expect(cadds.length).toBe(2)
    expect(cadds[1]).toEqual(['board', 'comment.add', { issue_id: 1, content: '回复正文', parent_id: 1 }])

    // 失败臂
    requestMock.mockImplementation((_m: string, cmd: string) =>
      cmd === 'comment.add' ? Promise.reject(new Error('net')) : defaultImpl(cmd),
    )
    await ta().setValue('x')
    await w.findAll('button').find((b: any) => b.text() === '发表评论')!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some((t) => t.type === 'error' && t.message.includes('评论失败'))).toBe(true)
    w.unmount()
  })
})

// ---------------------------------------------------------------------------
// 操作层：状态转移 / 优先级 / 指派 / 派发 / 撤回 / 评审重触发 / 档案
// ---------------------------------------------------------------------------
describe('IssueDetailModal 操作层', () => {
  it('状态转移：todo 出 4 个出边按钮；成功 toast + 刷新 + changed；失败 toast', async () => {
    const w = await mountModal(1)
    const btn = () => w.findAll('button').find((b: any) => b.text() === '→ 进行中')!
    expect(btn().exists()).toBe(true)
    await btn().trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('board', 'issue.status', { id: 1, status: 'in_progress' })
    expect(useToast().toasts.some((t) => t.message.includes('状态已改为「进行中」'))).toBe(true)
    expect(w.emitted('changed')).toBeTruthy()

    requestMock.mockImplementation((_m: string, cmd: string) =>
      cmd === 'issue.status' ? Promise.reject(new Error('net')) : defaultImpl(cmd),
    )
    await btn().trigger('click')
    await flushPromises()
    expect(useToast().toasts.some((t) => t.message.includes('状态转移失败'))).toBe(true)
    w.unmount()
  })

  it('终态（done）：无出边按钮 → 「终态 issue 不可再转移」；未知优先级兜底徽标', async () => {
    const w = await mountModal(1, { status: 'done', priority: 9 })
    expect(w.text()).toContain('终态 issue 不可再转移')
    expect(w.text()).toContain('P9')
    w.unmount()
  })

  it('updatePriority：成功 toast + 刷新；失败 toast', async () => {
    const w = await mountModal(1)
    const btn = () => w.findAll('button').find((b: any) => b.text() === 'P2 高')!
    await btn().trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('board', 'issue.update', { id: 1, priority: 2 })
    expect(useToast().toasts.some((t) => t.message === '优先级已更新')).toBe(true)
    expect(w.emitted('changed')).toBeTruthy()

    requestMock.mockImplementation((_m: string, cmd: string) =>
      cmd === 'issue.update' ? Promise.reject(new Error('net')) : defaultImpl(cmd),
    )
    await btn().trigger('click')
    await flushPromises()
    expect(useToast().toasts.some((t) => t.message.includes('更新失败'))).toBe(true)
    w.unmount()
  })

  it('assign：worker 手输 id（trim）/ manager_self→local / 清空；失败 toast', async () => {
    const w = await mountModal(1)
    const typeSelect = () => w.findAll('select.form-select').find((s: any) => s.text().includes('worker 节点'))!
    // worker 手输分支（workerNodes 空）
    await typeSelect().setValue('worker')
    const input = w.find('input.form-input')
    expect(input.exists()).toBe(true)
    await input.setValue(' w9 ')
    await w.findAll('button').find((b: any) => b.text() === '指派')!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('board', 'issue.assign', { id: 1, assignee_type: 'worker', assignee_id: 'w9' })
    expect(useToast().toasts.some((t) => t.message === '已指派')).toBe(true)

    // manager_self → 'local'
    await typeSelect().setValue('manager_self')
    await w.findAll('button').find((b: any) => b.text() === '指派')!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('board', 'issue.assign', { id: 1, assignee_type: 'manager_self', assignee_id: 'local' })

    // 失败臂
    requestMock.mockImplementation((_m: string, cmd: string) =>
      cmd === 'issue.assign' ? Promise.reject(new Error('net')) : defaultImpl(cmd),
    )
    await typeSelect().setValue('manager_self')
    await w.findAll('button').find((b: any) => b.text() === '指派')!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some((t) => t.message.includes('指派失败'))).toBe(true)
    w.unmount()

    // 清空
    const w2 = await mountModal(1, { assignee: 'worker', assignee_id: 'w1' })
    expect(w2.text()).toContain('worker: Alice')
    await w2.findAll('button').find((b: any) => b.text() === '清空')!.trigger('click')
    await flushPromises()
    const assigns = callsFor('issue.assign')
    expect(assigns[assigns.length - 1]).toEqual(['board', 'issue.assign', { id: 1 }])
    expect(useToast().toasts.some((t) => t.message === '已清空指派')).toBe(true)
    w2.unmount()
  })

  it('assign：workerNodes 非空时用下拉选 worker（worker-id select 分支）', async () => {
    const w = await mountModal(1, {}, {
      nodes: [{ id: 'w1', name: 'Node1', role: 'worker', online: true }],
    })
    const typeSelect = () => w.findAll('select.form-select').find((s: any) => s.text().includes('worker 节点'))!
    await typeSelect().setValue('worker')
    // workerNodes 非空 → 下拉（而非手输 input）
    const idSelect = w.findAll('select.form-select').find((s: any) => s.text().includes('选择 worker'))
    expect(idSelect!.exists()).toBe(true)
    await idSelect!.setValue('w1')
    await w.findAll('button').find((b: any) => b.text() === '指派')!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('board', 'issue.assign', { id: 1, assignee_type: 'worker', assignee_id: 'w1' })
    w.unmount()
  })

  it('dispatchIssue：目标 select 渲染 worker 过滤（role!=worker 不进）；成功/失败/空目标守卫', async () => {
    const w = await mountModal(1, {}, {
      nodes: [
        { id: 'w1', name: 'Node1', role: 'worker', online: true },
        { id: 'c1', name: 'Coord', role: 'coordinator', online: true },
      ],
    })
    const sel = () => w.findAll('select.form-select').find((s: any) => s.text().includes('Node1'))!
    expect(sel().exists()).toBe(true)
    expect(sel().text()).not.toContain('Coord')
    // 空目标守卫（按钮 disabled，直接 vm 调用打守卫行）
    const dispatched = callsFor('issue.dispatch').length
    await (w.vm as any).dispatchIssue()
    expect(callsFor('issue.dispatch').length).toBe(dispatched)

    requestMock.mockImplementation((_m: string, cmd: string) =>
      cmd === 'issue.dispatch' ? Promise.resolve({ task_id: 'abcdefgh12345678' }) : defaultImpl(cmd, {}, {
        nodes: [{ id: 'w1', name: 'Node1', role: 'worker', online: true }],
      }),
    )
    await sel().setValue('w1')
    await w.findAll('button').find((b: any) => b.text() === '派发 →')!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('board', 'issue.dispatch', { id: 1, target: 'w1' })
    expect(useToast().toasts.some((t) => t.message.includes('已派发（task abcdefgh'))).toBe(true)
    expect((sel().element as HTMLSelectElement).value).toBe('') // 派发后复位

    requestMock.mockImplementation((_m: string, cmd: string) =>
      cmd === 'issue.dispatch' ? Promise.reject(new Error('net')) : defaultImpl(cmd, {}, {
        nodes: [{ id: 'w1', name: 'Node1', role: 'worker', online: true }],
      }),
    )
    await sel().setValue('w1')
    await w.findAll('button').find((b: any) => b.text() === '派发 →')!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some((t) => t.message.includes('派发失败'))).toBe(true)
    w.unmount()
  })

  it('撤回在途派发（B14）：active_dispatch 行显示按钮；成功/失败', async () => {
    // 首次挂载需要 active_dispatch 投影
    requestMock.mockImplementation((_m: string, cmd: string) =>
      cmd === 'issue.cancel_dispatch'
        ? Promise.resolve({ task_id: 'task99887766aabb' })
        : defaultImpl(cmd, { active_dispatch: { task_id: 'task99887766aabb', worker_id: 'w1', state: 'running', dispatched_at: 1 } }),
    )
    const w = mount(IssueDetailModal, { props: { issueId: 1 } })
    await flushPromises()
    const btn = () => w.findAll('button').find((b: any) => b.text().includes('撤回派发'))!
    expect(btn().exists()).toBe(true)
    await btn().trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('board', 'issue.cancel_dispatch', { id: 1 })
    expect(useToast().toasts.some((t) => t.message.includes('派发已撤回（task task9988'))).toBe(true)

    requestMock.mockImplementation((_m: string, cmd: string) =>
      cmd === 'issue.cancel_dispatch'
        ? Promise.reject(new Error('net'))
        : defaultImpl(cmd, { active_dispatch: { task_id: 'task99887766aabb', worker_id: 'w1', state: 'running', dispatched_at: 1 } }),
    )
    await btn().trigger('click')
    await flushPromises()
    expect(useToast().toasts.some((t) => t.message.includes('撤回失败'))).toBe(true)
    w.unmount()
  })

  it('retriggerReview（in_review 专属区块）：成功/失败', async () => {
    const w = await mountModal(1, { status: 'in_review' })
    expect(w.text()).toContain('评审重触发')
    await w.findAll('button').find((b: any) => b.text().includes('重触发评审'))!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('board', 'review.retrigger', { number: 'NB-1' })
    expect(useToast().toasts.some((t) => t.message.includes('评审已重触发（NB-1）'))).toBe(true)

    requestMock.mockImplementation((_m: string, cmd: string) =>
      cmd === 'review.retrigger' ? Promise.reject(new Error('net')) : defaultImpl(cmd, { status: 'in_review' }),
    )
    await w.findAll('button').find((b: any) => b.text().includes('重触发评审'))!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some((t) => t.message.includes('重触发失败'))).toBe(true)
    w.unmount()
  })

  it('exportDossier：无注记 success / 有注记 info / 失败 error', async () => {
    const w = await mountModal(1)
    const btn = () => w.findAll('button').find((b: any) => b.text().includes('导出档案'))!
    // 无注记 → success toast
    requestMock.mockImplementation((_m: string, cmd: string) =>
      cmd === 'dossier.export' ? Promise.resolve({ root: '/tmp/dossier', notes: [] }) : defaultImpl(cmd),
    )
    await btn().trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('board', 'dossier.export', { number: 'NB-1' })
    expect(useToast().toasts.some((t) => t.type === 'success' && t.message.includes('档案已导出：/tmp/dossier'))).toBe(true)

    requestMock.mockImplementation((_m: string, cmd: string) =>
      cmd === 'dossier.export' ? Promise.resolve({ root: '/x', notes: ['n1', 'n2'] }) : defaultImpl(cmd),
    )
    await btn().trigger('click')
    await flushPromises()
    expect(useToast().toasts.some((t) => t.type === 'info' && t.message.includes('2 条缺口注记'))).toBe(true)

    requestMock.mockImplementation((_m: string, cmd: string) =>
      cmd === 'dossier.export' ? Promise.reject(new Error('net')) : defaultImpl(cmd),
    )
    await btn().trigger('click')
    await flushPromises()
    expect(useToast().toasts.some((t) => t.message.includes('档案导出失败'))).toBe(true)
    w.unmount()
  })
})

// ---------------------------------------------------------------------------
// 附件：上传守卫 / base64 编码 / 大小格式 / 下载
// ---------------------------------------------------------------------------
describe('IssueDetailModal 附件', () => {
  it('fmtSize 三档渲染（B / KB / MB）', async () => {
    const w = await mountModal(1, {}, {
      attachments: [
        { id: 1, issue_id: 1, filename: 'small.bin', storage_path: 'p', size: 500, uploaded_by: { kind: 'manager', id: 'board' }, created_at: 1700000000 },
        { id: 2, issue_id: 1, filename: 'mid.bin', storage_path: 'p', size: 2048, uploaded_by: { kind: 'manager', id: 'board' }, created_at: 1700000001 },
        { id: 3, issue_id: 1, filename: 'big.bin', storage_path: 'p', size: 3145728, uploaded_by: { kind: 'manager', id: 'board' }, created_at: 1700000002 },
      ],
    })
    const rows = w.findAll('.attachment-item')
    expect(rows.length).toBe(3)
    expect(rows[0].text()).toContain('500 B')
    expect(rows[1].text()).toContain('2.0 KB')
    expect(rows[2].text()).toContain('3.0 MB')
    w.unmount()
  })

  function pickFile(w: any, file: unknown) {
    const input = w.find('input[type="file"]')
    Object.defineProperty(input.element, 'files', { value: file ? [file] : [], configurable: true })
    return input.trigger('change')
  }

  it('onFilePick → 上传 base64；成功后刷新附件列表', async () => {
    const w = await mountModal(1)
    const file = {
      name: 'a.txt',
      size: 2,
      arrayBuffer: async () => new TextEncoder().encode('hi').buffer,
    }
    await pickFile(w, file)
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('board', 'attachment.add', { issue_id: 1, filename: 'a.txt', content: 'aGk=' })
    expect(useToast().toasts.some((t) => t.message.includes('已上传 a.txt'))).toBe(true)
    expect(callsFor('attachment.list').length).toBe(2)
    w.unmount()
  })

  it('onFilePick 守卫：无文件不动作；超 8MB 报错（fmtSize 上限文案）', async () => {
    const w = await mountModal(1)
    await pickFile(w, null)
    await flushPromises()
    expect(callsFor('attachment.add').length).toBe(0)

    await pickFile(w, { name: 'big.bin', size: 8 * 1024 * 1024 + 1, arrayBuffer: async () => new ArrayBuffer(1) })
    await flushPromises()
    expect(useToast().toasts.some((t) => t.message.includes('附件过大（上限 8.0 MB）'))).toBe(true)
    expect(callsFor('attachment.add').length).toBe(0)
    w.unmount()
  })

  it('上传失败：error toast', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) =>
      cmd === 'attachment.add' ? Promise.reject(new Error('net')) : defaultImpl(cmd),
    )
    const w = mount(IssueDetailModal, { props: { issueId: 1 } })
    await flushPromises()
    await pickFile(w, { name: 'a.txt', size: 2, arrayBuffer: async () => new ArrayBuffer(1) })
    await flushPromises()
    expect(useToast().toasts.some((t) => t.message.includes('上传失败'))).toBe(true)
    w.unmount()
  })

  it('downloadAttachment：attachment.get → Blob 链接点击 + 回收；失败 toast', async () => {
    stubObjectURL()
    const w = await mountModal(1, {}, {
      attachments: [{ id: 7, issue_id: 1, filename: 'dl.txt', storage_path: 'p', size: 2, uploaded_by: { kind: 'manager', id: 'board' }, created_at: 1700000000 }],
    })
    const createSpy = vi.spyOn(document, 'createElement')
    await w.findAll('.attachment-item button').find((b: any) => b.text() === '下载')!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('board', 'attachment.get', { id: 7 })
    expect(URL.createObjectURL).toHaveBeenCalledTimes(1)
    expect(URL.revokeObjectURL).toHaveBeenCalledWith('blob:mock-url')
    const anchors = createSpy.mock.results.map((r) => r.value as any).filter((el) => el?.tagName === 'A')
    expect(anchors.some((a) => a.download === 'dl.txt')).toBe(true)
    createSpy.mockRestore()

    requestMock.mockImplementation((_m: string, cmd: string) =>
      cmd === 'attachment.get' ? Promise.reject(new Error('net')) : defaultImpl(cmd),
    )
    await w.findAll('.attachment-item button').find((b: any) => b.text() === '下载')!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some((t) => t.message.includes('下载失败'))).toBe(true)
    w.unmount()
  })
})

// ---------------------------------------------------------------------------
// 决策面板（F2）：needs_user 各档动作 + 附言 flush + 重派目标单一数据源
// ---------------------------------------------------------------------------
describe('IssueDetailModal 决策面板（needs_user）', () => {
  it('面板渲染：停车徽标/建议处置/停车时间；retry 档非 in_review → 重试按钮禁用并给出路提示', async () => {
    const w = await mountModal(1, {}, { decision: decision({ needs_user: NU_RETRY }) })
    expect(w.find('[data-test="decision-panel"]').exists()).toBe(true)
    expect(w.text()).toContain('需要你决策')
    expect(w.text()).toContain('连续不确定')
    expect(w.text()).toContain('建议处置：重试/复评')
    expect(w.text()).toContain('停车于')
    const act = w.find('[data-test="decision-act-retrigger"]')
    expect(act.exists()).toBe(true)
    expect((act.element as HTMLButtonElement).disabled).toBe(true)
    expect(act.attributes('title')).toContain('当前单不在 in_review')
    // redispatch 按钮带目标选择行
    expect(w.find('[data-test="decision-target"]').exists()).toBe(true)
    w.unmount()
  })

  it('actRerunReview：in_review 时成功 toast / 失败 toast', async () => {
    useDecisionImpl(NU_RETRY, { status: 'in_review' })
    const w = mount(IssueDetailModal, { props: { issueId: 1 } })
    await flushPromises()
    const btn = () => w.find('[data-test="decision-act-retrigger"]')
    expect((btn().element as HTMLButtonElement).disabled).toBe(false)
    await btn().trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('board', 'review.retrigger', { number: 'NB-1' })
    expect(useToast().toasts.some((t) => t.message.includes('评审已重触发（NB-1）'))).toBe(true)
    expect(w.emitted('changed')).toBeTruthy()

    useDecisionImpl(NU_RETRY, { status: 'in_review' }, {
      'review.retrigger': () => Promise.reject(new Error('net')),
    })
    await btn().trigger('click')
    await flushPromises()
    expect(useToast().toasts.some((t) => t.type === 'error' && t.message.includes('重触发失败'))).toBe(true)
    w.unmount()
  })

  it('actAccept：issue.status→done + toast + 刷新；失败 toast', async () => {
    useDecisionImpl(NU_RETRY)
    const w = mount(IssueDetailModal, { props: { issueId: 1 } })
    await flushPromises()
    await w.find('[data-test="decision-act-accept"]').trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('board', 'issue.status', { id: 1, status: 'done' })
    expect(useToast().toasts.some((t) => t.message.includes('已裁决通过：NB-1 → done'))).toBe(true)
    expect(w.emitted('changed')).toBeTruthy()

    useDecisionImpl(NU_RETRY, {}, { 'issue.status': () => Promise.reject(new Error('net')) })
    await w.find('[data-test="decision-act-accept"]').trigger('click')
    await flushPromises()
    expect(useToast().toasts.some((t) => t.type === 'error' && t.message.includes('裁决失败'))).toBe(true)
    w.unmount()
  })

  it('actCancel：issue.cancel + toast；失败 toast', async () => {
    useDecisionImpl(NU_RETRY)
    const w = mount(IssueDetailModal, { props: { issueId: 1 } })
    await flushPromises()
    await w.find('[data-test="decision-act-cancel"]').trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('board', 'issue.cancel', { id: 1 })
    expect(useToast().toasts.some((t) => t.message.includes('已取消：NB-1'))).toBe(true)

    useDecisionImpl(NU_RETRY, {}, { 'issue.cancel': () => Promise.reject(new Error('net')) })
    await w.find('[data-test="decision-act-cancel"]').trigger('click')
    await flushPromises()
    expect(useToast().toasts.some((t) => t.type === 'error' && t.message.includes('取消失败'))).toBe(true)
    w.unmount()
  })

  it('附言（decisionNote）：先 comment.add 落痕再动作；flush 失败中止动作', async () => {
    useDecisionImpl(NU_RETRY)
    const w = mount(IssueDetailModal, { props: { issueId: 1 } })
    await flushPromises()
    await w.find('[data-test="decision-note"]').setValue('裁决理由')
    await w.find('[data-test="decision-act-accept"]').trigger('click')
    await flushPromises()
    const calls = requestMock.mock.calls
    const noteIdx = calls.findIndex((c) => c[1] === 'comment.add')
    const statusIdx = calls.findIndex((c) => c[1] === 'issue.status')
    expect(noteIdx).toBeGreaterThan(-1)
    expect(statusIdx).toBeGreaterThan(noteIdx)
    expect(calls[noteIdx]).toEqual(['board', 'comment.add', { issue_id: 1, content: '裁决理由' }])
    // 附言清空
    expect((w.find('[data-test="decision-note"]').element as HTMLTextAreaElement).value).toBe('')

    // flush 失败：动作不执行
    const statusBefore = callsFor('issue.status').length
    useDecisionImpl(NU_RETRY, {}, { 'comment.add': () => Promise.reject(new Error('net')) })
    await w.find('[data-test="decision-note"]').setValue('第二条')
    await w.find('[data-test="decision-act-accept"]').trigger('click')
    await flushPromises()
    expect(useToast().toasts.some((t) => t.message.includes('附言评论失败，动作未执行'))).toBe(true)
    expect(callsFor('issue.status').length).toBe(statusBefore)
    w.unmount()
  })

  it('redispatch 档：主操作=重新派发；无目标 warn；成功 toast；失败 toast', async () => {
    useDecisionImpl({ ...NU_RETRY, suggested_action: 'redispatch' })
    const w = mount(IssueDetailModal, { props: { issueId: 1 } })
    await flushPromises()
    expect(w.find('[data-test="decision-act-redispatch-main"]').exists()).toBe(true)
    // 无目标 → warn
    await w.find('[data-test="decision-act-redispatch-main"]').trigger('click')
    await flushPromises()
    expect(useToast().toasts.some((t) => t.type === 'warn' && t.message.includes('请先在下方选择重派节点'))).toBe(true)
    expect(callsFor('issue.dispatch').length).toBe(0)

    useDecisionImpl({ ...NU_RETRY, suggested_action: 'redispatch' }, {}, {
      'issue.dispatch': Promise.resolve({ task_id: 'task11112233' }),
    })
    await w.find('[data-test="decision-target"]').setValue('w1')
    await w.find('[data-test="decision-act-redispatch-main"]').trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('board', 'issue.dispatch', { id: 1, target: 'w1' })
    expect(useToast().toasts.some((t) => t.type === 'success' && t.message.includes('已重派（task task1111'))).toBe(true)

    useDecisionImpl({ ...NU_RETRY, suggested_action: 'redispatch' }, {}, {
      'issue.dispatch': () => Promise.reject(new Error('net')),
    })
    await w.find('[data-test="decision-target"]').setValue('w1')
    await w.find('[data-test="decision-act-redispatch-main"]').trigger('click')
    await flushPromises()
    expect(useToast().toasts.some((t) => t.type === 'error' && t.message.includes('重派失败'))).toBe(true)
    w.unmount()
  })

  it('manual_check 档：通过验收 + 重跑验收 + 重派 + 取消 四按钮', async () => {
    useDecisionImpl({ ...NU_RETRY, suggested_action: 'manual_check' }, { status: 'in_review' })
    const w = mount(IssueDetailModal, { props: { issueId: 1 } })
    await flushPromises()
    const keys = w.findAll('[data-test="decision-actions"] button').map((b: any) => b.attributes('data-test'))
    expect(keys).toEqual(['decision-act-accept', 'decision-act-retrigger', 'decision-act-redispatch', 'decision-act-cancel'])
    // in_review → 重跑验收可点（走 actRerunReview）
    await w.find('[data-test="decision-act-retrigger"]').trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('board', 'review.retrigger', { number: 'NB-1' })
    w.unmount()
  })

  it('重派目标单一数据源：online_nodes 在线组优先（tier 标注）；空/非 worker → 回退注册表', async () => {
    // 在线组（空 name 与 coordinator 被过滤）
    const w1 = await mountModal(1, {}, {
      decision: decision({
        needs_user: NU_RETRY,
        online_nodes: [
          { name: 'NodeOn', role: 'worker', tier: 'T2' },
          { name: '', role: 'worker' },
          { name: 'Coord', role: 'coordinator' },
        ],
      }),
      nodes: [{ id: 'w0', name: 'RegNode', role: 'worker', online: false }],
    })
    expect(w1.find('[data-test="decision-target"]').text()).toContain('NodeOn（在线 · T2）')
    expect(w1.find('[data-test="decision-target"]').text()).not.toContain('RegNode')
    w1.unmount()

    // 在线组被过滤清空 → 回退注册表（离线标注）
    const w2 = await mountModal(1, {}, {
      decision: decision({ needs_user: NU_RETRY, online_nodes: [{ name: '', role: 'worker' }, { name: 'Coord', role: 'coordinator' }] }),
      nodes: [{ id: 'w0', name: 'RegNode', role: 'worker', online: false }],
    })
    expect(w2.find('[data-test="decision-target"]').text()).toContain('RegNode（离线）')
    w2.unmount()
  })

  it('issue.decision 拉取失败：面板诚实降级（无决策块）；空响应同样', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) =>
      cmd === 'issue.decision' ? Promise.reject(new Error('legacy')) : defaultImpl(cmd),
    )
    const w = mount(IssueDetailModal, { props: { issueId: 1 } })
    await flushPromises()
    expect(w.find('[data-test="decision-panel"]').exists()).toBe(false)
    // 空 key（旧后端）→ null
    await (w.vm as any).loadDecision()
    expect(w.find('[data-test="decision-panel"]').exists()).toBe(false)
    w.unmount()
  })
})

// ---------------------------------------------------------------------------
// 决策面板：更多上下文（handoff 投影）
// ---------------------------------------------------------------------------
describe('IssueDetailModal 更多上下文', () => {
  const HANDOFF = {
    environment: { last_worker: 'w1' },
    verdict_chain: [
      { at: 1700000000, verdict: 'PASS', decision: '第一条判决' },
      { at: null, verdict: 'FAIL' },
    ],
    attempted: ['纯文本尝试', { op: '对象尝试' }],
    resume_entry: 'NB-0/resume',
  }

  it('展开渲染判决链/已尝试/恢复入口/最后执行；再点收起', async () => {
    const w = await mountModal(1, {}, { decision: decision({ needs_user: NU_RETRY, handoff: HANDOFF }) })
    expect(w.text()).toContain('最后执行：w1')
    expect(w.find('[data-test="decision-context"]').exists()).toBe(false)
    await w.find('[data-test="decision-more"]').trigger('click')
    const ctx = w.find('[data-test="decision-context"]')
    expect(ctx.exists()).toBe(true)
    expect(ctx.text()).toContain('判决链（2）')
    expect(ctx.text()).toContain('第一条判决')
    expect(ctx.text()).toContain('FAIL')
    expect(ctx.text()).toContain('已尝试')
    expect(ctx.text()).toContain('纯文本尝试')
    expect(ctx.text()).toContain('{"op":"对象尝试"}')
    expect(ctx.text()).toContain('恢复入口')
    // 再点收起
    await w.find('[data-test="decision-more"]').trigger('click')
    expect(w.find('[data-test="decision-context"]').exists()).toBe(false)
    w.unmount()
  })

  it('handoff 字段非数组/非对象：诚实空渲染（防御分支）', async () => {
    const w = await mountModal(1, {}, {
      decision: decision({ needs_user: NU_RETRY, handoff: { environment: 'not-obj', verdict_chain: 'nope', attempted: null, resume_entry: null } }),
    })
    await w.find('[data-test="decision-more"]').trigger('click')
    const ctx = w.find('[data-test="decision-context"]')
    expect(ctx.text()).not.toContain('判决链')
    expect(ctx.text()).not.toContain('已尝试')
    expect(ctx.text()).not.toContain('恢复入口')
    expect(w.text()).not.toContain('最后执行：')
    w.unmount()
  })
})

// ---------------------------------------------------------------------------
// 父单/项目特化中间档：补验收标准（含 AI 起草）与打回子单
// ---------------------------------------------------------------------------
describe('IssueDetailModal 父单特化中间档', () => {
  function useParentImpl(overrides: Record<string, any> = {}, issueOver: Record<string, unknown> = {}, decisionOver: Record<string, unknown> = {}) {
    useDecisionImpl(NU_PARENT, issueOver, overrides, decisionOver)
  }

  it('parentish：加载子单清单 + 完成度 + 子卡入口按钮（复评按钮禁用于非 in_review）', async () => {
    useParentImpl()
    const w = mount(IssueDetailModal, { props: { issueId: 1 } })
    await flushPromises()
    expect(callsFor('issue.list').length).toBe(1) // isParentishStop → loadChildren
    expect(w.text()).toContain('子单 1/2 已完成')
    expect(w.find('[data-test="decision-act-edit-ac"]').exists()).toBe(true)
    expect(w.find('[data-test="decision-act-sendback"]').exists()).toBe(true)
    expect((w.find('[data-test="decision-act-retrigger"]').element as HTMLButtonElement).disabled).toBe(true)
    w.unmount()
  })

  it('补验收标准：子卡开合、预填现状、保存并复评成功链；仅保存不触发复评', async () => {
    useParentImpl({}, { acceptance_criteria: '旧基线' })
    const w = mount(IssueDetailModal, { props: { issueId: 1 } })
    await flushPromises()
    await w.find('[data-test="decision-act-edit-ac"]').trigger('click')
    expect(w.find('[data-test="decision-ac-editor"]').exists()).toBe(true)
    expect((w.find('[data-test="decision-ac-input"]').element as HTMLTextAreaElement).value).toBe('旧基线')

    await w.find('[data-test="decision-ac-input"]').setValue('新基线')
    await w.find('[data-test="decision-ac-save-retrigger"]').trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('board', 'issue.update', { id: 1, acceptance_criteria: '新基线' })
    expect(requestMock).toHaveBeenCalledWith('board', 'review.retrigger', { number: 'NB-1' })
    expect(useToast().toasts.some((t) => t.message === '验收标准已保存')).toBe(true)
    expect(w.find('[data-test="decision-ac-editor"]').exists()).toBe(false)
    expect(w.emitted('changed')).toBeTruthy()

    // 仅保存：不复评
    await w.find('[data-test="decision-act-edit-ac"]').trigger('click')
    const retriggers = callsFor('review.retrigger').length
    await w.findAll('[data-test="decision-ac-editor"] button').find((b: any) => b.text() === '仅保存')!.trigger('click')
    await flushPromises()
    expect(callsFor('review.retrigger').length).toBe(retriggers)
    w.unmount()
  })

  it('保存验收标准：空值 warn；保存失败中止（不复评）；复评触发失败 warn 但保存成功', async () => {
    useParentImpl()
    const w = mount(IssueDetailModal, { props: { issueId: 1 } })
    await flushPromises()

    // 空 → warn
    await w.find('[data-test="decision-act-edit-ac"]').trigger('click')
    await w.find('[data-test="decision-ac-save-retrigger"]').trigger('click')
    await flushPromises()
    expect(useToast().toasts.some((t) => t.type === 'warn' && t.message.includes('验收标准不能为空'))).toBe(true)
    expect(callsFor('issue.update').length).toBe(0)

    // 保存失败 → error + 不复评
    useParentImpl({ 'issue.update': () => Promise.reject(new Error('net')) })
    await w.find('[data-test="decision-ac-input"]').setValue('标准')
    await w.find('[data-test="decision-ac-save-retrigger"]').trigger('click')
    await flushPromises()
    expect(useToast().toasts.some((t) => t.message.includes('保存验收标准失败'))).toBe(true)
    expect(callsFor('review.retrigger').length).toBe(0)
    expect(w.find('[data-test="decision-ac-editor"]').exists()).toBe(true) // 未收起

    // 保存成功但复评失败 → warn 提示可稍后复评
    useParentImpl({ 'review.retrigger': () => Promise.reject(new Error('busy')) })
    await w.find('[data-test="decision-ac-save-retrigger"]').trigger('click')
    await flushPromises()
    expect(useToast().toasts.some((t) => t.type === 'warn' && t.message.includes('复评触发失败'))).toBe(true)
    expect(useToast().toasts.some((t) => t.message === '验收标准已保存')).toBe(true)
    w.unmount()
  })

  it('AI 起草：confirm 拒绝拦截；成功回填草稿；空草稿 warn；失败 error', async () => {
    useParentImpl({}, { acceptance_criteria: '手输基线' })
    const w = mount(IssueDetailModal, { props: { issueId: 1 } })
    await flushPromises()
    await w.find('[data-test="decision-act-edit-ac"]').trigger('click')
    const draftBtn = () => w.find('[data-test="decision-ac-draft"]')

    // confirm 拒绝（输入框非空才会走 confirm）
    vi.stubGlobal('confirm', vi.fn(() => false))
    await draftBtn().trigger('click')
    await flushPromises()
    expect(callsFor('issue.draft_ac').length).toBe(0)

    // confirm 通过 → 草稿回填（trim）
    vi.stubGlobal('confirm', vi.fn(() => true))
    useParentImpl({ 'issue.draft_ac': Promise.resolve({ draft: '  AI 生成标准  ' }) })
    await draftBtn().trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('board', 'issue.draft_ac', { id: 1 }, 180000)
    expect((w.find('[data-test="decision-ac-input"]').element as HTMLTextAreaElement).value).toBe('AI 生成标准')
    expect(useToast().toasts.some((t) => t.message.includes('已生成草稿'))).toBe(true)

    // 空草稿 → warn
    useParentImpl({ 'issue.draft_ac': Promise.resolve({ draft: '  ' }) })
    await draftBtn().trigger('click')
    await flushPromises()
    expect(useToast().toasts.some((t) => t.type === 'warn' && t.message.includes('AI 未能生成草稿'))).toBe(true)

    // 失败 → error
    useParentImpl({ 'issue.draft_ac': () => Promise.reject(new Error('slow')) })
    await draftBtn().trigger('click')
    await flushPromises()
    expect(useToast().toasts.some((t) => t.type === 'error' && t.message.includes('AI 起草失败'))).toBe(true)
    w.unmount()
  })

  it('AI 起草过期守卫：换单后旧失败不落新单（无误导 toast）', async () => {
    let draftRej!: (e: any) => void
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'issue.draft_ac') {
        return new Promise((_res, rej) => {
          draftRej = rej
        })
      }
      return defaultImpl(cmd, {}, {
        decision: decision({ needs_user: NU_PARENT }),
        issues: CHILDREN,
        nodes: [NODE_W1],
      })
    })
    const w = mount(IssueDetailModal, { props: { issueId: 1 } })
    await flushPromises()
    await w.find('[data-test="decision-act-edit-ac"]').trigger('click')
    await w.find('[data-test="decision-ac-draft"]').trigger('click')
    // 换单 → 旧失败响应晚到
    useDecisionImpl(NU_PARENT, { id: 2, number: 'NB-2' })
    await w.setProps({ issueId: 2 })
    await flushPromises()
    draftRej(new Error('stale'))
    await flushPromises()
    expect(useToast().toasts.some((t) => t.message.includes('AI 起草失败'))).toBe(false)
    w.unmount()
  })

  it('打回子单：三段守卫（未选子单/未填意见/未选节点）', async () => {
    useParentImpl()
    const w = mount(IssueDetailModal, { props: { issueId: 1 } })
    await flushPromises()
    await w.find('[data-test="decision-act-sendback"]').trigger('click')
    expect(w.find('[data-test="decision-sendback"]').exists()).toBe(true)

    await w.find('[data-test="sendback-run"]').trigger('click')
    await flushPromises()
    expect(useToast().toasts.some((t) => t.type === 'warn' && t.message.includes('请先选择要打回的子单'))).toBe(true)

    await w.find('[data-test="sendback-child-select"]').setValue(12)
    await w.find('[data-test="sendback-run"]').trigger('click')
    await flushPromises()
    expect(useToast().toasts.some((t) => t.type === 'warn' && t.message.includes('请填写返工意见'))).toBe(true)

    await w.find('[data-test="sendback-feedback"]').setValue('重做')
    await w.find('[data-test="sendback-run"]').trigger('click')
    await flushPromises()
    expect(useToast().toasts.some((t) => t.type === 'warn' && t.message.includes('请先选择打回到的节点'))).toBe(true)
    expect(callsFor('comment.add').length).toBe(0)
    w.unmount()
  })

  it('打回未完成子单：评论留痕 + dispatch；blocked 先解阻再重派', async () => {
    useParentImpl()
    const w = mount(IssueDetailModal, { props: { issueId: 1 } })
    await flushPromises()
    await w.find('[data-test="decision-act-sendback"]').trigger('click')
    await w.find('[data-test="sendback-child-select"]').setValue(12)
    await w.find('[data-test="sendback-feedback"]').setValue('返工意见')
    await w.find('[data-test="sendback-target"]').setValue('w1')
    await w.find('[data-test="sendback-run"]').trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('board', 'comment.add', { issue_id: 12, content: '【人工打回】返工意见' })
    expect(requestMock).toHaveBeenCalledWith('board', 'issue.status', { id: 12, status: 'in_progress' })
    expect(requestMock).toHaveBeenCalledWith('board', 'issue.dispatch', { id: 12, target: 'w1' })
    expect(useToast().toasts.some((t) => t.message.includes('已打回 NB-12 并重派到 w1'))).toBe(true)
    // 成功后表单复位：子卡整体收起（decisionExtra 清空）
    expect(w.find('[data-test="decision-sendback"]').exists()).toBe(false)
    w.unmount()
  })

  it('打回已完成子单：建「返工：」新单 + 派发', async () => {
    useParentImpl({
      'issue.create': Promise.resolve({ issue: { id: 99, number: 'NB-9' } }),
      'issue.dispatch': Promise.resolve({ task_id: 'task9988aabb' }),
    })
    const w = mount(IssueDetailModal, { props: { issueId: 1 } })
    await flushPromises()
    await w.find('[data-test="decision-act-sendback"]').trigger('click')
    await w.find('[data-test="sendback-child-select"]').setValue(11)
    await w.find('[data-test="sendback-feedback"]').setValue('重做')
    await w.find('[data-test="sendback-target"]').setValue('w1')
    await w.find('[data-test="sendback-run"]').trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('board', 'issue.create', {
      title: '返工：子单A',
      description: '人工打回 NB-11（子单A）：\n\n重做',
      priority: 1,
      parent_issue_id: 1,
      project_id: undefined,
    })
    expect(requestMock).toHaveBeenCalledWith('board', 'issue.dispatch', { id: 99, target: 'w1' })
    expect(useToast().toasts.some((t) => t.message.includes('已建返工单 NB-9 并派发到 w1'))).toBe(true)
    w.unmount()
  })

  it('打回部分失败诚实化：create 后派发失败 → warn 不重复打回 + 表单复位', async () => {
    useParentImpl({
      'issue.create': Promise.resolve({ issue: { id: 99, number: 'NB-9' } }),
      'issue.dispatch': () => Promise.reject(new Error('no worker')),
    })
    const w = mount(IssueDetailModal, { props: { issueId: 1 } })
    await flushPromises()
    await w.find('[data-test="decision-act-sendback"]').trigger('click')
    await w.find('[data-test="sendback-child-select"]').setValue(11)
    await w.find('[data-test="sendback-feedback"]').setValue('重做')
    await w.find('[data-test="sendback-target"]').setValue('w1')
    await w.find('[data-test="sendback-run"]').trigger('click')
    await flushPromises()
    expect(useToast().toasts.some((t) => t.type === 'warn' && t.message.includes('返工单 NB-9 已创建，但派发失败'))).toBe(true)
    expect(useToast().toasts.some((t) => t.message.includes('请勿重复打回'))).toBe(true)
    // 部分失败复位：意见清空（子卡保留看刷新后的列表）
    expect((w.find('[data-test="sendback-feedback"]').element as HTMLTextAreaElement).value).toBe('')
    w.unmount()
  })

  it('打回部分失败诚实化：评论已留痕而派发失败 → warn 指路手动派发', async () => {
    useParentImpl({ 'issue.dispatch': () => Promise.reject(new Error('no worker')) })
    const w = mount(IssueDetailModal, { props: { issueId: 1 } })
    await flushPromises()
    await w.find('[data-test="decision-act-sendback"]').trigger('click')
    await w.find('[data-test="sendback-child-select"]').setValue(12)
    await w.find('[data-test="sendback-feedback"]').setValue('重做')
    await w.find('[data-test="sendback-target"]').setValue('w1')
    await w.find('[data-test="sendback-run"]').trigger('click')
    await flushPromises()
    expect(useToast().toasts.some((t) => t.type === 'warn' && t.message.includes('打回评论已留痕，但派发失败'))).toBe(true)
    w.unmount()
  })

  it('打回失败（评论即失败）：error toast，不误报部分完成', async () => {
    useParentImpl({ 'comment.add': () => Promise.reject(new Error('net')) })
    const w = mount(IssueDetailModal, { props: { issueId: 1 } })
    await flushPromises()
    await w.find('[data-test="decision-act-sendback"]').trigger('click')
    await w.find('[data-test="sendback-child-select"]').setValue(12)
    await w.find('[data-test="sendback-feedback"]').setValue('重做')
    await w.find('[data-test="sendback-target"]').setValue('w1')
    await w.find('[data-test="sendback-run"]').trigger('click')
    await flushPromises()
    expect(useToast().toasts.some((t) => t.type === 'error' && t.message.includes('打回失败'))).toBe(true)
    w.unmount()
  })

  it('create 未返回新单：诚实报错（createdNumber 未置位 → error 而非 warn）', async () => {
    useParentImpl({ 'issue.create': Promise.resolve({}) })
    const w = mount(IssueDetailModal, { props: { issueId: 1 } })
    await flushPromises()
    await w.find('[data-test="decision-act-sendback"]').trigger('click')
    await w.find('[data-test="sendback-child-select"]').setValue(11)
    await w.find('[data-test="sendback-feedback"]').setValue('重做')
    await w.find('[data-test="sendback-target"]').setValue('w1')
    await w.find('[data-test="sendback-run"]').trigger('click')
    await flushPromises()
    expect(useToast().toasts.some((t) => t.type === 'error' && t.message.includes('后端未返回新单'))).toBe(true)
    w.unmount()
  })
})

// ---------------------------------------------------------------------------
// plan_exhausted 档（actReplan）+ AI 拆解两段式 + SSE 三事件
// ---------------------------------------------------------------------------
describe('IssueDetailModal AI 拆解与 SSE', () => {
  it('plan_exhausted：主操作=重新拆解（actReplan → startPlan → planning）', async () => {
    useDecisionImpl(NU_EXHAUSTED, {}, { 'issue.plan': Promise.resolve({ status: 'planning', plan_id: 'p7' }) })
    const w = mount(IssueDetailModal, { props: { issueId: 1 } })
    await flushPromises()
    expect(w.find('[data-test="decision-act-replan"]').exists()).toBe(true)
    await w.find('[data-test="decision-act-replan"]').trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('board', 'issue.plan', { id: 1 })
    expect(w.text()).toContain('拆解中…')
    w.unmount()
  })

  it('startPlan（✨ AI 拆解）：成功进 planning；响应异常 toast；失败 toast', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) =>
      cmd === 'issue.plan' ? Promise.resolve({ status: 'planning', plan_id: 'p5' }) : defaultImpl(cmd),
    )
    const w = mount(IssueDetailModal, { props: { issueId: 1 } })
    await flushPromises()
    // 成功
    await w.findAll('button').find((b: any) => b.text().includes('AI 拆解'))!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('board', 'issue.plan', { id: 1 })
    expect(w.text()).toContain('⏳ 拆解中…')
    // 取消等待 → idle
    await w.findAll('button').find((b: any) => b.text() === '取消等待')!.trigger('click')
    expect(w.text()).toContain('✨ AI 拆解')

    // 响应异常（无 plan_id）
    requestMock.mockImplementation((_m: string, cmd: string) =>
      cmd === 'issue.plan' ? Promise.resolve({ status: 'planning' }) : defaultImpl(cmd),
    )
    await w.findAll('button').find((b: any) => b.text().includes('AI 拆解'))!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some((t) => t.message.includes('拆解请求响应异常'))).toBe(true)

    // 失败
    requestMock.mockImplementation((_m: string, cmd: string) =>
      cmd === 'issue.plan' ? Promise.reject(new Error('net')) : defaultImpl(cmd),
    )
    await w.findAll('button').find((b: any) => b.text().includes('AI 拆解'))!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some((t) => t.message.includes('AI 拆解失败'))).toBe(true)
    w.unmount()
  })

  it('SSE board.plan_ready：ready 预览 / auto_confirm→发车中 / 空 subs→failed / 异单忽略', async () => {
    const w = await mountModal(1)
    sseDispatch('board.plan_ready', { issue_id: 999, plan_id: 'px', subs: [SUB] })
    expect(w.text()).toContain('✨ AI 拆解') // 异单不落状态

    sseDispatch('board.plan_ready', { issue_id: 1, plan_id: 'p6', subs: [SUB] })
    await flushPromises()
    expect(w.text()).toContain('子A')
    expect(w.text()).toContain('🛠 ops')
    expect(w.text()).toContain('角色:r1')
    expect(w.text()).toContain('#t1')
    expect(w.text()).toContain('描述A')
    expect(w.text()).toContain('验收：ac1')
    expect(w.text()).toContain('确认拆解（1 个子任务）')

    // auto_confirm → dispatching
    sseDispatch('board.plan_ready', { issue_id: 1, plan_id: 'p6', subs: [SUB], auto_confirm: true })
    await flushPromises()
    expect(w.text()).toContain('自动发车派发中')
    await w.findAll('button').find((b: any) => b.text() === '收起')!.trigger('click')
    expect(w.text()).toContain('✨ AI 拆解')

    // 空 subs → failed + planError
    sseDispatch('board.plan_ready', { issue_id: 1, plan_id: 'p6', subs: [] })
    await flushPromises()
    expect(w.text()).toContain('拆解结果为空')
    expect(w.text()).toContain('⛔ 拆解失败')
    // 失败态 → 重试按钮走 startPlan
    await w.findAll('button').find((b: any) => b.text() === '重试')!.trigger('click')
    await flushPromises()
    w.unmount()
  })

  it('SSE board.plan_failed / board.plan_started：状态与守卫', async () => {
    const w = await mountModal(1)
    sseDispatch('board.plan_failed', { issue_id: 999, error: '异单' })
    await flushPromises()
    expect(w.text()).not.toContain('⛔ 拆解失败')
    sseDispatch('board.plan_failed', { issue_id: 1, error: 'planner 崩了' })
    await flushPromises()
    expect(w.text()).toContain('⛔ 拆解失败：planner 崩了')
    // 关闭错误态
    await w.findAll('button').find((b: any) => b.text() === '关闭')!.trigger('click')
    expect(w.text()).toContain('✨ AI 拆解')

    // plan_started：idle → planning；已非 idle 忽略
    sseDispatch('board.plan_started', { issue_id: 1, plan_id: 'p8' })
    await flushPromises()
    expect(w.text()).toContain('拆解中…')
    sseDispatch('board.plan_started', { issue_id: 1, plan_id: 'p8' })
    await flushPromises()
    expect(w.text()).toContain('拆解中…')
    // 缺 error 字段 → 兜底文案
    sseDispatch('board.plan_failed', { issue_id: 1 })
    await flushPromises()
    expect(w.text()).toContain('未知错误')
    w.unmount()
  })

  it('recoverPlan：plan.list ready → 恢复预览 + 决策面板 plan_ready 徽标 + 确认发车', async () => {
    useImpl({}, {
      decision: decision({ plan_ready: { plan_id: 'p1', subs_count: 2, created_age_secs: 60 } }),
      plans: [{ status: 'ready', plan_id: 'p1', subs: [SUB] }],
      __default: { created: [{}, {}], dispatched: 1, deferred: [{}] },
    })
    const w = mount(IssueDetailModal, { props: { issueId: 1 } })
    await flushPromises()
    expect(w.text()).toContain('📋 计划待确认')
    expect(w.text()).toContain('2 个子单')
    expect(w.text()).toContain('♻ 已恢复上次未确认的拆解计划')
    expect(w.text()).toContain('子A')
    expect(w.find('[data-test="decision-plan-confirm"]').exists()).toBe(true)
    await w.find('[data-test="decision-plan-confirm"]').trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('board', 'issue.plan', { id: 1, plan_id: 'p1', confirm: true })
    expect(useToast().toasts.some((t) => t.message.includes('已创建 2 个子任务：派出 1，暂缓 1'))).toBe(true)
    expect(w.find('[data-test="decision-plan-confirm"]').exists()).toBe(false) // dismissPlan
    expect(w.emitted('changed')).toBeTruthy()
    w.unmount()
  })

  it('recoverPlan 其余三态：running(planning/dispatching) / ready 空 subs → failed / 查询失败 → idle', async () => {
    const w2 = await mountModal(1, {}, { plans: [{ status: 'running', plan_id: 'p2', phase: 'planning' }] })
    expect(w2.text()).toContain('拆解中…')
    w2.unmount()

    const w3 = await mountModal(1, {}, { plans: [{ status: 'running', plan_id: 'p2', phase: 'dispatching' }] })
    expect(w3.text()).toContain('自动发车派发中')
    w3.unmount()

    const w4 = await mountModal(1, {}, { plans: [{ status: 'ready', plan_id: 'p3', subs: [] }] })
    expect(w4.text()).toContain('拆解结果为空')
    w4.unmount()

    requestMock.mockImplementation((_m: string, cmd: string) =>
      cmd === 'plan.list' ? Promise.reject(new Error('net')) : defaultImpl(cmd),
    )
    const w5 = mount(IssueDetailModal, { props: { issueId: 1 } })
    await flushPromises()
    expect(w5.text()).toContain('✨ AI 拆解')
    w5.unmount()
  })

  it('confirmPlan 守卫与失败：planId 空诚实提示；请求失败 toast', async () => {
    useImpl({}, {
      decision: decision({ plan_ready: { plan_id: 'p1', subs_count: 1, created_age_secs: 0 } }),
      plans: [{ status: 'ready', plan_id: 'p1', subs: [SUB] }],
    })
    const w = mount(IssueDetailModal, { props: { issueId: 1 } })
    await flushPromises()
    await w.find('[data-test="decision-plan-dismiss"]').trigger('click') // planId 清空
    await (w.vm as any).confirmPlan()
    await flushPromises()
    expect(useToast().toasts.some((t) => t.type === 'warn' && t.message.includes('该计划已确认或已放弃'))).toBe(true)

    // 失败臂：重新恢复计划再确认
    useImpl({}, {
      decision: decision({ plan_ready: { plan_id: 'p1', subs_count: 1, created_age_secs: 0 } }),
      plans: [{ status: 'ready', plan_id: 'p1', subs: [SUB] }],
    })
    await (w.vm as any).recoverPlan()
    await flushPromises()
    requestMock.mockImplementation((_m: string, cmd: string) =>
      cmd === 'issue.plan' ? Promise.reject(new Error('net')) : defaultImpl(cmd),
    )
    await (w.vm as any).confirmPlan()
    await flushPromises()
    expect(useToast().toasts.some((t) => t.type === 'error' && t.message.includes('确认拆解失败'))).toBe(true)
    w.unmount()
  })
})

// ---------------------------------------------------------------------------
// F5 锚点下放检查 + 上游依赖投影
// ---------------------------------------------------------------------------
describe('IssueDetailModal 锚点与上游依赖', () => {
  it('AC 含 [CHECK] re: → 黄条提示；拆解预览无子单继承 → 第二道黄条', async () => {
    const w = await mountModal(1, { acceptance_criteria: '第一行\n[CHECK] re: 交付链接' }, {
      plans: [{ status: 'ready', plan_id: 'p1', subs: [{ ...SUB, acceptance_criteria: '无锚点' }] }],
    })
    expect(w.text()).toContain('⚠ 含 [CHECK] re: 锚点')
    expect(w.text()).toContain('锚点下放检查')
    w.unmount()

    // 子单继承锚点 → 无黄条
    const w2 = await mountModal(1, { acceptance_criteria: '[CHECK] re: x' }, {
      plans: [{ status: 'ready', plan_id: 'p1', subs: [{ ...SUB, acceptance_criteria: '[CHECK] re: y' }] }],
    })
    expect(w2.text()).not.toContain('锚点下放检查')
    w2.unmount()

    // AC 不含锚点 → 两条黄条都不出
    const w3 = await mountModal(1, { acceptance_criteria: '普通标准' }, {
      plans: [{ status: 'ready', plan_id: 'p1', subs: [SUB] }],
    })
    expect(w3.text()).not.toContain('[CHECK] re: 锚点')
    w3.unmount()
  })

  it('上游依赖投影：done ✓ / cancelled ⚠ / 未完成 🔒 + statusLabel', async () => {
    const w = await mountModal(1, {
      depends_on: [
        { id: 2, number: 'NB-2', status: 'done' },
        { id: 3, number: 'NB-3', status: 'cancelled' },
        { id: 4, number: 'NB-4', status: 'in_progress' },
      ],
    })
    expect(w.text()).toContain('上游依赖（3）')
    expect(w.text()).toContain('✓ NB-2')
    expect(w.text()).toContain('⚠ NB-3 已取消')
    expect(w.text()).toContain('🔒 等 NB-4（进行中）')
    w.unmount()
  })

  it('节点/附件/子单加载失败：静默兜底空（三条 catch 分支）', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'nodes.list') return Promise.reject(new Error('net'))
      if (cmd === 'attachment.list') return Promise.reject(new Error('net'))
      if (cmd === 'issue.list') return Promise.reject(new Error('net'))
      return defaultImpl(cmd)
    })
    const w = mount(IssueDetailModal, { props: { issueId: 1 } })
    await flushPromises()
    expect(w.text()).toContain('暂无附件')
    expect(callsFor('nodes.list').length).toBeGreaterThan(0)
    // parentish 决策触发 loadChildren 的失败臂
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'issue.list') return Promise.reject(new Error('net'))
      if (cmd === 'nodes.list') return Promise.reject(new Error('net'))
      if (cmd === 'attachment.list') return Promise.reject(new Error('net'))
      return defaultImpl(cmd)
    })
    await (w.vm as any).loadDecision()
    // 手动触发决策驱动的子单加载（decision 仍为 null → 不触发；直接 vm 调用打 catch）
    await (w.vm as any).loadChildren()
    expect(w.text()).toContain('暂无附件') // 无异常抛出
    w.unmount()
  })
})

// ---------------------------------------------------------------------------
// 杂项兜底：busy 守卫 / assigneeLabel manager_self / actorName 命中
// ---------------------------------------------------------------------------
describe('IssueDetailModal 杂项兜底分支', () => {
  it('busy=true 时各动作守卫直接返回；detail 空时动作守卫', async () => {
    const w = await mountModal(1)
    const vm = w.vm as any
    vm.busy = true
    const gets = callsFor('issue.get').length
    await vm.actAccept()
    await vm.actRerunReview()
    await vm.actRedispatch()
    await vm.actCancel()
    await vm.transition('done')
    await vm.assign(false)
    await vm.submitComment()
    await vm.cancelDispatchIssue()
    await vm.dispatchIssue()
    await vm.retriggerReview()
    await vm.exportDossier()
    expect(callsFor('issue.get').length).toBe(gets)
    vm.busy = false

    // 关闭后 detail null → 守卫 !detail
    await w.setProps({ issueId: null })
    await flushPromises()
    await vm.actAccept()
    await vm.actReplan()
    await vm.confirmPlan()
    await vm.updatePriority(2)
    expect(callsFor('issue.status').length).toBe(0)
    w.unmount()
  })

  it('assigneeLabel：manager_self 特显 / actorName 命中 / 未知 id 回退原 id', async () => {
    const w1 = await mountModal(1, { assignee: 'manager_self', assignee_id: 'local' })
    expect(w1.text()).toContain('指派：manager（本机）')
    w1.unmount()

    // actorName 命中（useBoardActors mock：w1 → Alice）
    const w2 = await mountModal(1, { assignee: 'worker', assignee_id: 'w1' })
    expect(w2.text()).toContain('worker: Alice')
    w2.unmount()

    // 未知 id：actorName null → 回退原 id（空串兜底不渲染 "null"）
    const w3 = await mountModal(1, { assignee: 'worker', assignee_id: 'node-xyz' })
    expect(w3.text()).toContain('worker: node-xyz')
    w3.unmount()
  })
})
