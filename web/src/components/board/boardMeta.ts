// 看板领域常量（W2 P3）：IssueListView / IssueDetailModal / BoardKanban 共享。
// 后端仍是唯一真相源——状态机镜像 crates/nemesis-board/src/state_machine.rs，
// 徽标/标签镜像 crates/nemesis-board/src/models.rs（IssueStatus/Priority）。

export const STATUSES = [
  { key: 'backlog', label: 'Backlog' },
  { key: 'todo', label: 'Todo' },
  { key: 'in_progress', label: '进行中' },
  { key: 'in_review', label: '评审中' },
  { key: 'done', label: '已完成' },
  { key: 'blocked', label: '受阻' },
  { key: 'cancelled', label: '已取消' },
] as const

export const STATUS_LABEL: Record<string, string> = Object.fromEntries(
  STATUSES.map((s) => [s.key, s.label]),
)

export const STATUS_BADGE: Record<string, string> = {
  backlog: 'badge-neutral',
  todo: 'badge-info',
  in_progress: 'badge-info',
  in_review: 'badge-warning',
  done: 'badge-success',
  blocked: 'badge-error',
  cancelled: 'badge-neutral',
}

// DAG 全景图节点配色（2026-10-15 需求③）：badge 语义的图形化投影——
// 色相沿用 STATUS_BADGE 同族 CSS 变量（双主题自动适配），同色相内用
// 空心/实心与图标消歧（todo 空心 vs in_progress 实心；backlog 灰空心
// vs cancelled 灰+✕）。图标冗余兼顾色觉障碍（compact 模式色是唯一编码，
// 图标必须保留）。
export interface StatusNodeStyle {
  fill: string
  fillOpacity: number
  stroke: string
  strokeWidth: number
  /** 节点内图标（色觉冗余编码）。 */
  icon: string
}

export const STATUS_NODE: Record<string, StatusNodeStyle> = {
  backlog: { fill: 'var(--surface)', fillOpacity: 0.45, stroke: 'var(--text-muted)', strokeWidth: 1, icon: '' },
  todo: { fill: 'var(--info)', fillOpacity: 0.12, stroke: 'var(--info)', strokeWidth: 1.2, icon: '' },
  in_progress: { fill: 'var(--info)', fillOpacity: 0.4, stroke: 'var(--info)', strokeWidth: 2.2, icon: '▶' },
  in_review: { fill: 'var(--warning)', fillOpacity: 0.32, stroke: 'var(--warning)', strokeWidth: 1.8, icon: '👁' },
  done: { fill: 'var(--success)', fillOpacity: 0.35, stroke: 'var(--success)', strokeWidth: 1.8, icon: '✓' },
  blocked: { fill: 'var(--error)', fillOpacity: 0.35, stroke: 'var(--error)', strokeWidth: 2.2, icon: '⛔' },
  cancelled: { fill: 'var(--surface)', fillOpacity: 0.6, stroke: 'var(--text-muted)', strokeWidth: 1, icon: '✕' },
}

/** 未知状态兜底（后端新增状态先于前端发布时不炸渲染）。 */
export const STATUS_NODE_FALLBACK: StatusNodeStyle = {
  fill: 'var(--surface)',
  fillOpacity: 0.5,
  stroke: 'var(--text-muted)',
  strokeWidth: 1,
  icon: '?',
}

export function statusNodeStyle(s: string): StatusNodeStyle {
  return STATUS_NODE[s] || STATUS_NODE_FALLBACK
}

export const PRIORITY_LABEL: Record<number, string> = {
  0: '低',
  1: '中',
  2: '高',
  3: '紧急',
}

export const PRIORITY_BADGE: Record<number, string> = {
  0: 'badge-neutral',
  1: 'badge-info',
  2: 'badge-warning',
  3: 'badge-error',
}

// 镜像 state_machine::can_transition（终态无出边；自转移不允许）。
export const TRANSITIONS: Record<string, string[]> = {
  backlog: ['todo', 'in_progress', 'done', 'blocked', 'cancelled'],
  todo: ['in_progress', 'done', 'blocked', 'cancelled'],
  in_progress: ['in_review', 'done', 'blocked', 'cancelled'],
  in_review: ['in_progress', 'done', 'blocked', 'cancelled'],
  blocked: ['todo', 'in_progress', 'cancelled'],
  done: [],
  cancelled: [],
}

export function statusLabel(s: string): string {
  return STATUS_LABEL[s] || s
}

/** P1 外循环（2026-10-25）：内置来源族（不显示外部来源徽标）；集外类型
 *  = flow 模板声明 origin_type 的外部 connector 来源，看板卡片亮 🔗 徽标。
 *  与后端回写订阅面同口径——flow 模板 on_transition 按 origin_type 订阅，
 *  内置类型永不匹配模板 = 天然不回写（模板绑定即过滤，无宿主端清单；
 *  前端只是展示侧镜像）。 */
const INTERNAL_ORIGINS = new Set(['autopilot', 'cron', 'channel', 'cli', 'board', 'manual'])

export function isInternalOrigin(t: string): boolean {
  return INTERNAL_ORIGINS.has(t)
}

/** 压测⑥（2026-09-30 三设备）语义消歧：in_progress =「已派发至 worker」，
 *  worker 内部串行队列排队中的单子同样显示 in_progress（非 CPU 执行中）——
 *  徽标悬浮提示说明真实语义，空串 = 无提示。 */
export function statusTitle(s: string): string {
  return s === 'in_progress' ? '已派发至 worker；worker 串行队列排队中的任务同样显示此状态（非立即执行）' : ''
}

export function fmtTime(sec: number | null | undefined): string {
  if (!sec) return '—'
  try {
    return new Date(sec * 1000).toLocaleString()
  } catch {
    return '—'
  }
}

export interface IssueRow {
  id: number
  number: string
  title: string
  description: string
  status: string
  priority: number
  assignee: string | null
  assignee_id: string | null
  creator: { kind: string; id: string }
  project_id: number | null
  due_date: number | null
  position: number
  acceptance_criteria: string | null
  origin: { origin_type: string; origin_id: string } | null
  created_at: number
  updated_at: number
  /** C2（goal P1）：环节（待派发/执行中/待重派/验收中/已停车/已完成…）——
   *  后端 issue.list 附带（issue_stage 单一真相源推导）。 */
  stage?: string
  /** A2b（看板项目档案 goal P1）：取消单清理后的永久收起标记——
   *  issue.list 恒排除 hidden 单，此字段只在详情/CLI 全量面可见。 */
  hidden?: boolean
  /** 拆解运行期可观测（2026-10-11 Part 1）：planner 在途投影——
   *  issue.list 行附 {elapsed_secs, attempt, phase}（phase：
   *  planning=planner 运行中 / dispatching=auto_confirm 发车派发中），
   *  不在途无此键。旧后端无 phase 时视为 planning。 */
  planning?: { elapsed_secs: number; attempt: number; phase?: 'planning' | 'dispatching' }
  /** 依赖上游投影（2026-10-15）：每上游 {id, number, status}——列表行
   *  「🔒 等 NB-x」徽标 + DAG 全景图边数据源。旧后端无此键（可选）。 */
  depends_on?: Array<{ id: number; number: string; status: string }>
  /** 父单 id（拆解关系）：DAG 分层下界（子 ≥ 父层+1，根需求独占首行）+
   *  拆解边渲染。issue.list 走 Issue 全量序列化自带，无需后端投影。 */
  parent_issue_id?: number | null
  comments?: any[]
  activity?: any[]
  subscribers?: any[]
  /** 决策可见性投影（2026-10-15 批，issue.list 附带）：
   *  needs_user = 停车强信号（活档案 + in_review/in_progress 状态闸）——
   *  「✋ 待你决策」徽标与详情页决策面板数据源；
   *  plan_ready = 拆解完成等确认发车（PLAN_CACHE 快照）——「📋 计划待确认」；
   *  attention = 执行档案停滞弱信号（in_progress + 停摆哨兵评论）——只提示。 */
  needs_user?: {
    stop_kind: string
    stop_kind_label: string
    suggested_action: string
    hint: string
    stopped_at: string | null
  } | null
  plan_ready?: { plan_id: string; subs_count: number; created_age_secs: number } | null
  attention?: boolean
}

// ---- 决策可见性 helper（镜像 crates/nemesis-board/src/handoff.rs
// stop_kind_label 兜底；正常路径后端已投影 label，此处只补旧后端缺字段）----

/** 停车类型 → 人话标签（与后端 stop_kind_label 同表；未知兜底「停车待处理」）。 */
export const STOP_KIND_LABEL: Record<string, string> = {
  unsure_streak: '连续不确定',
  fail_streak: '连续失败熔断',
  budget: '预算耗尽',
  retry_exhausted: '重试耗尽',
  worker_loop: 'worker 循环异常',
  capability_fail: '能力不足',
  infra: '基础设施故障',
  defect_gate: '缺陷闸拦截',
  parent_unresolved: '父单收口未决',
  project_unresolved: '项目收口未决',
  no_match: '无匹配节点',
  plan_exhausted: '拆解重试耗尽',
  llm_unavailable: 'LLM 不可用',
  // BUG-20261029-170 回填（与后端 stop_kind_label 同步——EscalateHuman
  // verdict 分型与必需检查契约闸的生产值，此前落兜底语义丢失）。
  unsure: '单轮评审不确定',
  redispatch_budget: '重派预算耗尽',
  required_checks: '必需检查未满足',
}

export function stopKindLabel(stopKind: string | undefined | null): string {
  return (stopKind && STOP_KIND_LABEL[stopKind]) || '停车待处理'
}

/** suggested_action → 人话（决策面板按钮组预设依据）。 */
export const SUGGESTED_ACTION_LABEL: Record<string, string> = {
  manual_check: '人工核查后裁决',
  retry: '重试/复评',
  redispatch: '重新派发',
}

export function suggestedActionLabel(a: string | undefined | null): string {
  return (a && SUGGESTED_ACTION_LABEL[a]) || '人工核查后裁决'
}

/** needs_user 行内徽标文案：「✋ 待你决策」+ 停车类型（title 悬浮用）。 */
export function needsUserBadge(nu: IssueRow['needs_user']): { text: string; title: string } {
  const label = nu?.stop_kind_label || stopKindLabel(nu?.stop_kind)
  return {
    text: '✋ 待你决策',
    title: `停车待裁决：${label}${nu?.hint ? ' — ' + nu.hint : ''}`,
  }
}

// ---------------------------------------------------------------------------
// 人工触点（看板人工触点体系 P1-P4；镜像 crates/nemesis-board/src/models.rs
// TouchpointKind/TouchpointStatus serde 序列化 + touchpoint.rs
// default_action_for）。列表归看板（轻卡/过滤片），页面归工作台
// （WorkbenchView /workbench/:kind/:id——后端 workbench.get/respond）。
// ---------------------------------------------------------------------------

/** 触点承载行（board.touchpoint.list 行形态：touchpoint 全量 serde + 承载单投影）。 */
export interface TouchpointRow {
  touchpoint: {
    id: number
    issue_id: number
    project_id: number | null
    kind: 'spec' | 'prototype' | string
    status:
      | 'pending'
      | 'confirmed'
      | 'timeout_defaulted'
      | 'vetoed_local'
      | 'vetoed_spec'
      | 'cancelled'
      | 'parked_sync'
      | string
    deadline_ms: number
    payload: string
    human_input: string | null
    veto_rounds: number
  }
  issue_number: string
  issue_title: string
  issue_status: string
  overdue: boolean
}

/** kind → 人话徽标（与后端 kind_label 同表）。 */
export const TOUCHPOINT_KIND_LABEL: Record<string, string> = {
  spec: '🎯 规格确认',
  prototype: '🧪 原型评审',
}

export function touchpointKindLabel(kind: string): string {
  return TOUCHPOINT_KIND_LABEL[kind] || '🎯 触点'
}

/** 触点状态 → 人话（未知兜底原文——后端新增状态先于前端发布时不炸渲染）。 */
export const TOUCHPOINT_STATUS_LABEL: Record<string, string> = {
  pending: '待处理',
  confirmed: '已确认',
  timeout_defaulted: '已按默认继续',
  vetoed_local: '否决 · 返工中',
  vetoed_spec: '否决 · 待改规格',
  cancelled: '已取消',
  parked_sync: '挂起',
}

export function touchpointStatusLabel(s: string): string {
  return TOUCHPOINT_STATUS_LABEL[s] || s
}

/** 超时默认动作 → 人话（镜像 touchpoint.rs default_action_for 值域）。 */
export const TOUCHPOINT_DEFAULT_ACTION_LABEL: Record<string, string> = {
  proceed_with_draft: '按草案继续',
  proceed_current_version: '按当前版继续',
}

export function touchpointDefaultActionLabel(a: string): string {
  return TOUCHPOINT_DEFAULT_ACTION_LABEL[a] || a
}

/**
 * 截止倒计时（触点卡/工作台头部共用）：deadline_ms 相对 now 的剩余。
 * 返回 {text, urgent, overdue}：>1h 显「Xh Ym」，≤1h 显「Ym」并 urgent
 * （橙），过期显「已超时」。deadline_ms<=0 = 永不超时（无人值守直通卡
 * 等无倒计时形态），显示「—」。
 */
export function touchpointCountdown(
  deadlineMs: number,
  nowMs: number = Date.now(),
): { text: string; urgent: boolean; overdue: boolean } {
  if (!deadlineMs || deadlineMs <= 0) return { text: '—', urgent: false, overdue: false }
  const remain = deadlineMs - nowMs
  if (remain <= 0) return { text: '已超时', urgent: true, overdue: true }
  const mins = Math.floor(remain / 60000)
  if (mins < 60) return { text: `⏱${mins}m`, urgent: true, overdue: false }
  const h = Math.floor(mins / 60)
  const m = mins % 60
  return { text: `⏱${h}h${m ? m + 'm' : ''}`, urgent: false, overdue: false }
}

/**
 * 触点卡一句话摘要（卡面**限死**：徽标+标题+倒计时+摘要+父单号，绝不渲染
 * 规格正文）。prototype 有显式 summary 字段直接用；spec 从载荷元数据合成
 * （假设/验收条数——是计数不是内容，不泄正文）。
 */
export function touchpointCardSummary(row: TouchpointRow): string {
  let p: any = null
  try {
    p = JSON.parse(row.touchpoint.payload || '{}')
  } catch {
    return ''
  }
  if (row.touchpoint.kind === 'prototype') {
    return typeof p?.summary === 'string' && p.summary ? p.summary : ''
  }
  const na = Array.isArray(p?.assumptions) ? p.assumptions.length : null
  const nc = Array.isArray(p?.acceptance_checks) ? p.acceptance_checks.length : null
  if (na == null && nc == null) return ''
  const parts: string[] = []
  if (na != null) parts.push(`假设 ${na} 条`)
  if (nc != null) parts.push(`验收 ${nc} 条`)
  return parts.join(' · ')
}

/** 触点卡/工作台统一跳转路径（route param kind 与后端 kind_of strip_prefix 对齐）。 */
export function workbenchPath(row: TouchpointRow): string {
  return `/workbench/touchpoint-${row.touchpoint.kind}/${row.touchpoint.id}`
}

/** 里程碑状态点（ProjectPanel 纯投影：latest 触点状态 → 状态点语义）。 */
export function milestoneDot(status: string): { glyph: string; label: string; cls: string } {
  switch (status) {
    case 'pending':
      return { glyph: '●', label: '进行中', cls: 'ms-active' }
    case 'confirmed':
      return { glyph: '✓', label: '通过', cls: 'ms-pass' }
    case 'timeout_defaulted':
      return { glyph: '↷', label: '超时默认', cls: 'ms-timeout' }
    case 'vetoed_local':
    case 'vetoed_spec':
      return { glyph: '✗', label: '否决返工中', cls: 'ms-veto' }
    case 'parked_sync':
      return { glyph: '⏸', label: '挂起', cls: 'ms-parked' }
    case 'cancelled':
      return { glyph: '✕', label: '已取消', cls: 'ms-cancelled' }
    default:
      return { glyph: '○', label: status || '未知', cls: 'ms-unknown' }
  }
}
