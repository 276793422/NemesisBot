<script setup lang="ts">
import { ref, computed, watch, onUnmounted } from 'vue'
import { useWSAPI } from '../../composables/useWSAPI'
import { useToast } from '../../composables/useToast'
import { useBoardChanged } from '../../composables/useBoardChanged'
import { useBoardActors } from '../../composables/useBoardActors'
import { on, off } from '../../composables/useSSE'
import {
  PRIORITY_BADGE,
  PRIORITY_LABEL,
  STATUS_BADGE,
  TRANSITIONS,
  fmtTime,
  statusLabel,
  statusTitle,
  stopKindLabel,
  suggestedActionLabel,
} from './boardMeta'

// Issue 详情弹窗（W2 P3）：从 IssueListView 抽出为共享组件——看板（BoardKanban）
// 与列表（IssueListView）点击卡片/行都打开它。P3 增强：
// - 评论线程（parent_id 一层嵌套，回复 @ 作者）；
// - @提及辅助（订阅者/指派对象一键插入，后端 extract_mentions 产生 mentioned 通知）；
// - 附件（上传 base64 → attachment.add，下载 attachment.get → Blob）。
// 后端真相源：crates/nemesis-web/src/handlers/board.rs。

const { request } = useWSAPI()
const toast = useToast()

const props = defineProps<{
  issueId: number | null
}>()

const emit = defineEmits<{
  (e: 'close'): void
  (e: 'changed'): void
}>()

const MAX_ATTACHMENT_BYTES = 8 * 1024 * 1024 // 与后端 MAX_ATTACHMENT_BYTES 对齐

interface Actor {
  kind: string
  id: string
}

interface Issue {
  id: number
  number: string
  title: string
  description: string
  status: string
  priority: number
  assignee: string | null
  assignee_id: string | null
  creator: Actor
  due_date: number | null
  position: number
  /** 所属项目（issue_to_view 序列化 Issue 携带）：返工单 project_id 回落用。 */
  project_id?: number | null
  acceptance_criteria: string | null
  required_profession?: string
  origin: { origin_type: string; origin_id: string } | null
  created_at: number
  updated_at: number
  /** 依赖上游投影（2026-10-15 需求①）：issue.get 附带。旧后端无此键。 */
  depends_on?: Array<{ id: number; number: string; status: string }>
  /** 在途派发投影（B14）：issue.get 附带，撤回派发按钮的显示依据；无在途不插键。 */
  active_dispatch?: { task_id: string; worker_id: string; state: string; dispatched_at: number }
  comments?: CommentRow[]
  activity?: ActivityRow[]
  subscribers?: any[]
}

interface CommentRow {
  id: number
  author: Actor
  content: string
  parent_id: number | null
  ctype: string
  created_at: number
}

interface ActivityRow {
  id: number
  actor: Actor
  action: string
  details: string | null
  created_at: number
}

interface AttachmentRow {
  id: number
  issue_id: number
  filename: string
  storage_path: string
  size: number
  uploaded_by: Actor
  created_at: number
}

const detail = ref<Issue | null>(null)
const attachments = ref<AttachmentRow[]>([])
const busy = ref(false)

// 上游依赖投影（2026-10-15 需求①）：详情页「上游依赖」区块数据源。
const detailDeps = computed(() => detail.value?.depends_on ?? [])

// U-C（2026-10-12 §9.3）：单子全链路时间线页签——活动/评论/派发/任务结果
// 统一投影（后端单一真相源 nemesis-board::timeline，与 dossier timeline.md
// 同源聚合）。默认「详情」页保持原布局；时间线按需加载（每次激活重查）。
type DetailTab = 'detail' | 'timeline'
const activeTab = ref<DetailTab>('detail')

interface TimelineEvent {
  ts: number
  source: string
  kind: string
  actor: string
  summary: string
  detail?: string
}
const timelineEvents = ref<TimelineEvent[]>([])
const timelineLoading = ref(false)

const SOURCE_BADGE: Record<string, string> = {
  activity: 'badge-neutral',
  comment: 'badge-info',
  dispatch: 'badge-warning',
  archive: 'badge-neutral',
  cluster_task: 'badge-info',
  continuation: 'badge-warning',
}
const SOURCE_LABEL: Record<string, string> = {
  activity: '活动',
  comment: '评论',
  dispatch: '派发',
  archive: '档案',
  cluster_task: '任务',
  continuation: '在途',
}
// 活动行：action 词进了徽标（·kind），正文优先展示 details（如 todo→in_review
// / auto_decide 的 verdict JSON）——比重复 action 词更有信息量。
function timelineSummary(e: TimelineEvent): string {
  if (e.source === 'activity') return e.detail || e.kind
  return e.summary || e.kind
}
function timelineDetail(e: TimelineEvent): string {
  if (e.source === 'activity') return ''
  if (!e.detail || e.detail === e.summary) return ''
  return e.detail.length > 120 ? e.detail.slice(0, 120) + '…' : e.detail
}

async function loadTimeline() {
  if (!props.issueId || timelineLoading.value) return
  timelineLoading.value = true
  try {
    const r = await request('board', 'issue.timeline', { id: props.issueId })
    timelineEvents.value = (r?.events || []) as TimelineEvent[]
  } catch {
    timelineEvents.value = []
  } finally {
    timelineLoading.value = false
  }
}

function switchTab(t: DetailTab) {
  activeTab.value = t
  if (t === 'timeline') loadTimeline()
}

const newComment = ref('')
const replyTo = ref<CommentRow | null>(null)
// @提及辅助的候选集（订阅者 + 当前指派对象；插入后端能识别的 @id 记号）。
const mentionCandidates = computed<Actor[]>(() => {
  if (!detail.value) return []
  const seen = new Set<string>()
  const out: Actor[] = []
  for (const s of detail.value.subscribers || []) {
    const a: Actor = s.subscriber || s
    const key = `${a.kind}/${a.id}`
    if (!seen.has(key)) {
      seen.add(key)
      out.push(a)
    }
  }
  if (detail.value.assignee === 'worker' && detail.value.assignee_id) {
    const key = `worker/${detail.value.assignee_id}`
    if (!seen.has(key)) out.push({ kind: 'worker', id: detail.value.assignee_id })
  }
  return out
})

// 集群 worker 节点（指派/派发下拉；集群不可用时空列表 → 回退手输）。
interface ClusterNode {
  id: string
  name: string
  role: string
  online: boolean
}
const workerNodes = ref<ClusterNode[]>([])

// H1（goal P1）：可读设备名——评论作者/回复提示显示 agent/Alex，未知回退短 id。
const { ensureNodes: ensureActorNodes, displayActor, actorName } = useBoardActors()
function authorLabel(a: Actor): string {
  return displayActor(a.kind, a.id)
}
// H4：回复 @ token 用可读名（裁决器 name/id 均识别）；未知回退原 id。
function replyToken(a: Actor): string {
  return `@${actorName(a.id) ?? a.id} `
}

const detailAssignType = ref('')
const detailAssignId = ref('')
const dispatchTarget = ref('')

const canDispatch = computed(
  () =>
    !!detail.value &&
    ['backlog', 'todo', 'in_progress', 'in_review'].includes(detail.value.status),
)

function fmtSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`
}

function assigneeLabel(issue: Issue): string {
  if (!issue.assignee) return '未指派'
  // 指派目标可读名（H1 单一解析层；未知 id 回退原始 id）。
  if (issue.assignee === 'manager_self') return 'manager（本机）'
  // assignee_id 可空（Issue 接口 string | null）：actorName 入参与回退值都
  // 归一为 string（vue-tsc TS2345；id 为 null 时回退空串，不再渲染 "null"）。
  return `worker: ${actorName(issue.assignee_id ?? '') ?? issue.assignee_id ?? ''}`
}

function allowedTargets(status: string): string[] {
  return TRANSITIONS[status] || []
}

// 评论线程（一层）：顶层评论 + 各自的回复。
const topLevelComments = computed<CommentRow[]>(() =>
  (detail.value?.comments || []).filter((c) => !c.parent_id),
)

function repliesOf(parentId: number): CommentRow[] {
  return (detail.value?.comments || []).filter((c) => c.parent_id === parentId)
}

// ctype 彩色徽标（M3 批次 E）：delivery（worker 交付汇报）蓝 / question 橙 /
// discussion 紫（badge-info 复用）/ status_change、system 机械行灰。
const CTYPE_BADGE: Record<string, string> = {
  delivery: 'badge-info',
  question: 'badge-warning',
  discussion: 'badge-info',
  status_change: 'badge-neutral',
  system: 'badge-neutral',
}

const CTYPE_LABEL: Record<string, string> = {
  delivery: '交付',
  question: '提问',
  discussion: '讨论',
  status_change: '状态',
  system: '系统',
}

function ctypeBadge(c: CommentRow): string {
  return CTYPE_BADGE[c.ctype] || 'badge-neutral'
}

function ctypeLabel(c: CommentRow): string {
  return CTYPE_LABEL[c.ctype] || c.ctype
}

async function loadWorkerNodes() {
  ensureActorNodes().catch(() => {})
  try {
    const r = await request('cluster', 'nodes.list', {})
    workerNodes.value = (r?.nodes || []).filter((n: any) => n.role === 'worker')
  } catch {
    workerNodes.value = []
  }
}

async function loadDetail(silent = false) {
  const id = props.issueId
  if (id == null) return
  try {
    const r = await request('board', 'issue.get', { id })
    // 过期响应守卫：await 期间用户可能已换单/关闭，旧单数据不许污染新单视图。
    if (props.issueId !== id) return
    detail.value = r?.issue || null
    if (!silent) {
      // 仅显式加载重置指派下拉/派发目标；后台静默换新不动用户正在编辑的表单态。
      detailAssignType.value = ''
      detailAssignId.value = ''
      dispatchTarget.value =
        detail.value?.assignee === 'worker' ? detail.value?.assignee_id || '' : ''
    }
  } catch (e: any) {
    if (props.issueId !== id) return
    if (!silent) {
      toast.error('加载详情失败: ' + e)
      emit('close')
    }
  }
}

async function loadAttachments() {
  const id = props.issueId
  if (id == null) return
  try {
    const r = await request('board', 'attachment.list', { issue_id: id })
    if (props.issueId !== id) return
    attachments.value = r?.attachments || []
  } catch {
    if (props.issueId !== id) return
    attachments.value = []
  }
}

// --- 决策面板（2026-10-15 F2：issue.decision 投影 + 预设操作按钮）---
// 后端真相源：handlers/board.rs issue.decision——needs_user（活停车档案 +
// InReview|InProgress 闸）/ plan_ready（PLAN_CACHE TTL 内待确认）/ attention
// （停滞哨兵）/ handoff（needs_user 时回全量档案：verdict_chain/attempted/
// resume_entry/environment）/ online_nodes（在线非本机 worker，重派下拉）。
interface DecisionInfo {
  issue: string
  issue_id: number
  status: string
  needs_user: {
    stop_kind: string
    stop_kind_label: string
    suggested_action: string
    hint: string
    stopped_at: string | null
  } | null
  plan_ready: { plan_id: string; subs_count: number; created_age_secs: number } | null
  attention: boolean
  handoff: any | null
  online_nodes: Array<{ name: string; role: string; professions?: string[]; tier?: string | null }>
}

const decision = ref<DecisionInfo | null>(null)
const decisionNote = ref('') // 可选附言：非空时先落评论留痕再执行动作
const decisionMore = ref(false) // 「更多上下文」折叠
const decisionTarget = ref('') // 重派目标（online_nodes / workerNodes 融合）
// 父单/项目特化中间档（2026-10-15 用户反馈：复评死循环没有出路）：
// decisionExtra 切换子卡（补验收标准 / 打回子单），三态互斥。
const decisionExtra = ref<'' | 'ac' | 'sendback'>('')
const acDraft = ref('') // 验收标准草稿（打开子卡时预填现状）
const draftingAc = ref(false) // AI 起草进行中（独立于 busy：失败不惊动其他动作位）
const childPick = ref<number | null>(null) // 待打回子单 id
const childFeedback = ref('') // 打回意见（必填：这是给 worker 的工作指令）
const childrenList = ref<any[]>([]) // 父单子单行（打回下拉数据源）

async function loadDecision() {
  const id = props.issueId
  if (id == null) {
    decision.value = null
    return
  }
  try {
    decision.value = (await request('board', 'issue.decision', { id })) || null
  } catch {
    decision.value = null // 旧后端无此命令 / 集群未启用 → 无面板（诚实降级）
  }
  if (props.issueId !== id) return // 过期响应守卫：换单后别触发旧单的子单加载
  if (isParentishStop.value) void loadChildren()
}

// 父单/项目停车（parent_unresolved / project_unresolved）→ 面板给中间档。
const isParentishStop = computed(() => {
  const k = decision.value?.needs_user?.stop_kind
  return k === 'parent_unresolved' || k === 'project_unresolved'
})

// 子单清单（打回下拉 + 完成度一行）：issue.list 全量行按 parent_issue_id 过滤。
// 看板规模内全量拉取可接受（与列表/看板页同源同量）。
async function loadChildren() {
  const id = props.issueId
  if (id == null) return
  try {
    const r = await request('board', 'issue.list', {})
    if (props.issueId !== id) return // 过期响应守卫
    const pid = decision.value?.issue_id
    childrenList.value = (r?.issues || []).filter((c: any) => c.parent_issue_id === pid)
  } catch {
    childrenList.value = []
  }
}
const childrenDoneCount = computed(
  () => childrenList.value.filter((c) => c.status === 'done').length,
)

// 面板显示闸：有活停车档案或有待确认计划才出现（纯「停滞」attention 只在
// 三页徽标提示，不顶到详情页决策层——还没有需要用户裁决的事）。plan_ready
// 分支还要求本地 planId 在手（放弃/确认后清空）——只看服务端投影会出现
// 「放弃后确认发车变死按钮」的僵尸态（投影要等下轮刷新才消失）。
const showDecisionPanel = computed(
  () =>
    !!decision.value &&
    (!!decision.value.needs_user || (!!decision.value.plan_ready && !!planId.value)),
)

const nu = computed(() => decision.value?.needs_user ?? null)
// handoff.environment 自由 JSON：防御取 last_worker（老档案可能没有）。
const lastWorker = computed(() => {
  const env = decision.value?.handoff?.environment
  if (!env || typeof env !== 'object') return ''
  return String((env as any).last_worker ?? '')
})

// 「更多上下文」展开内容（handoff 全量档案的决策辅助字段，可缺省）。
const verdictChain = computed<any[]>(() => {
  const c = decision.value?.handoff?.verdict_chain
  return Array.isArray(c) ? c : []
})
const attemptedList = computed<any[]>(() => {
  const a = decision.value?.handoff?.attempted
  return Array.isArray(a) ? a : []
})
const resumeEntry = computed<string>(() => String(decision.value?.handoff?.resume_entry ?? ''))

// 预设按钮组（mockup 2 决策面板）：主操作按 suggested_action 预设、不再
// 让用户想措辞；每档都给「取消」兜底。needsTarget 标记的按钮点击前先在
// 重派下拉选目标。
interface DecisionAction {
  key: string
  label: string
  title: string
  cls: string
  needsTarget?: boolean
  disabled?: boolean
  run: () => void
}
const decisionActions = computed<DecisionAction[]>(() => {
  const n = nu.value
  if (!n) return []
  const out: DecisionAction[] = []
  const isParentish = n.stop_kind === 'parent_unresolved' || n.stop_kind === 'project_unresolved'
  // retrigger 类动作的后端闸：review.retrigger 只收 in_review。in_progress 停车
  // 单（如派发超时/worker 异常）交付都没到，重跑验收必然被拒——按钮禁用并把
  // 出路讲清楚，而不是让预设主操作点下去吃报错。
  const canRetrigger = detail.value?.status === 'in_review'
  const retriggerBlockHint = canRetrigger
    ? ''
    : '——当前单不在 in_review：评审链只在验收态运行。可先在下方操作层转 in_review，或改用重派/通过/取消'
  const redispatchAct: DecisionAction = {
    key: 'redispatch',
    label: '🚚 重派',
    title: '按下方节点选择重新派发（换环境/换节点再试）',
    cls: '',
    needsTarget: true,
    run: actRedispatch,
  }
  const acceptAct: DecisionAction = {
    key: 'accept',
    label: '✅ 通过验收',
    title: '裁决通过：状态转 done（车间的「定案」）',
    cls: 'btn-primary',
    run: actAccept,
  }
  const cancelAct: DecisionAction = {
    key: 'cancel',
    label: '✖ 取消',
    title: '取消该单（级联取消子单）',
    cls: 'btn-danger',
    run: actCancel,
  }
  if (n.suggested_action === 'redispatch') {
    out.push({
      key: 'redispatch-main',
      label: '🚚 重新派发',
      title: '按下方节点选择重新派发',
      cls: 'btn-primary',
      needsTarget: true,
      run: actRedispatch,
    })
    out.push(acceptAct, cancelAct)
  } else if (n.suggested_action === 'retry' && n.stop_kind === 'plan_exhausted') {
    // 拆解三连败停车：主 = 重新拆解（重跑 planner），次 = 直接人工创建子单路径没有，给取消兜底
    out.push({
      key: 'replan',
      label: '🔁 重新拆解',
      title: '重新跑 AI 拆解（planner），生成新的子单计划',
      cls: 'btn-primary',
      run: actReplan,
    })
    out.push(cancelAct)
  } else if (n.suggested_action === 'retry') {
    out.push({
      key: 'retrigger',
      label: '🔁 重试（重跑验收）',
      title: '重入自动验收链：锚点 → 执行档案直读 → LLM 评审' + retriggerBlockHint,
      cls: 'btn-primary',
      disabled: !canRetrigger,
      run: actRerunReview,
    })
    out.push(redispatchAct, acceptAct, cancelAct)
  } else if (isParentish) {
    // 父单/项目收口等待（2026-10-15 用户反馈补中间档）：纯「复评→退回→复评」
    // 是死循环——UNSURE 的根因常是本单没写验收标准（评审无客观基线），补上
    // 即可定案；子单质量不达则打回重做。复评只应在中间档处理完后点。
    out.push({
      key: 'retrigger',
      label: '🔁 AI 复评收口',
      title:
        '重跑自动验收：通过 → 自动收口父单/项目；未过 → 带新证据退回此处（建议先补验收标准/打回子单）' +
        retriggerBlockHint,
      cls: 'btn-primary',
      disabled: !canRetrigger,
      run: actRerunReview,
    })
    out.push({
      key: 'edit-ac',
      label: '📝 补验收标准',
      title: 'UNSURE 常见根因：本单没有验收标准，评审给不出 PASS/FAIL。补上客观基线后「保存并复评」即可定案',
      cls: decisionExtra.value === 'ac' ? 'btn-primary' : '',
      run: toggleAcEditor,
    })
    out.push({
      key: 'sendback',
      label: '🔧 打回子单',
      title: '对具体子单提返工意见：已完成单补一张返工单承接（done 是终态不能重开），未完成单评论打回并重派',
      cls: decisionExtra.value === 'sendback' ? 'btn-primary' : '',
      run: toggleSendback,
    })
    out.push(acceptAct, cancelAct)
  } else {
    // manual_check 及未知档位：人工核查后裁决
    out.push(acceptAct)
    out.push({
      key: 'retrigger',
      label: '🔁 重跑验收',
      title: '重入自动验收链：锚点 → 执行档案直读 → LLM 评审' + retriggerBlockHint,
      cls: '',
      disabled: !canRetrigger,
      run: actRerunReview,
    })
    out.push(redispatchAct, cancelAct)
  }
  return out
})

// 可选附言先行：非空则先 comment.add 落审计，再执行动作（失败则中止动作）。
async function flushDecisionNote(): Promise<boolean> {
  const content = decisionNote.value.trim()
  if (!content || !detail.value) return true
  try {
    await request('board', 'comment.add', { issue_id: detail.value.id, content })
    decisionNote.value = ''
    return true
  } catch (e: any) {
    toast.error('附言评论失败，动作未执行: ' + e)
    return false
  }
}

// 决策动作统一收尾：刷新详情 + 决策投影 + 广播。
async function afterDecisionAction() {
  await loadDetail()
  await loadDecision()
  changed()
}

// busy 在附言 flush 之前就上锁：comment.add 往返窗口里按钮必须已禁用，
// 否则双击会发出两份评论 + 两次动作（复查 #5）。d = 入口处捕获的 detail
// 局部引用：动作进行中用户关弹窗会把 detail 置空（watch），toast 里再读
// 会抛 TypeError 变成误导报错（复查 #3 同族）。
async function actAccept() {
  const d = detail.value
  if (!d || busy.value) return
  busy.value = true
  try {
    if (!(await flushDecisionNote())) return
    await request('board', 'issue.status', { id: d.id, status: 'done' })
    toast.success(`已裁决通过：${d.number} → done`)
    await afterDecisionAction()
  } catch (e: any) {
    toast.error('裁决失败: ' + e)
  } finally {
    busy.value = false
  }
}

async function actRerunReview() {
  const d = detail.value
  if (!d || busy.value) return
  busy.value = true
  try {
    if (!(await flushDecisionNote())) return
    await request('board', 'review.retrigger', { number: d.number })
    toast.success(`评审已重触发（${d.number}）：正在重跑自动验收`)
    await afterDecisionAction()
  } catch (e: any) {
    toast.error('重触发失败: ' + e)
  } finally {
    busy.value = false
  }
}

async function actRedispatch() {
  const d = detail.value
  if (!d || busy.value) return
  busy.value = true
  try {
    if (!decisionTarget.value) {
      toast.warn('请先在下方选择重派节点')
      return
    }
    if (!(await flushDecisionNote())) return
    const r = await request('board', 'issue.dispatch', {
      id: d.id,
      target: decisionTarget.value,
    })
    toast.success(`已重派（task ${(r?.task_id || '').slice(0, 8)}…），等 worker 回报后自动写回`)
    decisionTarget.value = ''
    await afterDecisionAction()
  } catch (e: any) {
    toast.error('重派失败: ' + e)
  } finally {
    busy.value = false
  }
}

async function actCancel() {
  const d = detail.value
  if (!d || busy.value) return
  busy.value = true
  try {
    if (!(await flushDecisionNote())) return
    await request('board', 'issue.cancel', { id: d.id })
    toast.success(`已取消：${d.number}`)
    await afterDecisionAction()
  } catch (e: any) {
    toast.error('取消失败: ' + e)
  } finally {
    busy.value = false
  }
}

// plan_exhausted 的重试 = 重新拆解：直接复用 AI 拆解链（issue.plan 起新
// planner 轮；旧档案停在原地，新计划确认后覆盖停车态）。
async function actReplan() {
  if (!detail.value || busy.value) return
  busy.value = true
  try {
    if (!(await flushDecisionNote())) return
    await startPlan()
    await afterDecisionAction()
  } finally {
    busy.value = false
  }
}

// --- 父单/项目特化中间档（2026-10-15 用户反馈）：补验收标准 / 打回子单 ---

function toggleAcEditor() {
  decisionExtra.value = decisionExtra.value === 'ac' ? '' : 'ac'
  if (decisionExtra.value === 'ac') acDraft.value = detail.value?.acceptance_criteria || ''
}

// AI 起草验收标准（NB-101 AC 缺失根修的人工面）：草稿只预填输入框，用户
// 编辑后走下方保存流——落库决定权在用户，AI 不代写已生效基线。已有手输
// 内容时先确认覆盖（草稿会整框替换）；LLM 往返以秒计，过期响应守卫防
// 换单后旧草稿写进新单输入框。
// 显式 3 分钟超时（2026-10-16 用户实锤 BUG：默认 REQUEST_TIMEOUT=30s 在
// LLM 慢轮次客户端先到点拒收，服务端起草成功但草稿被丢弃——第二次点
// 「碰巧快」的假象即此根因；同步 LLM WSAPI 必须传长超时）。
const DRAFT_AC_TIMEOUT_MS = 180000
async function aiDraftAc() {
  const d = detail.value
  if (!d || busy.value || draftingAc.value) return
  const id = d.id
  if (acDraft.value.trim() && !window.confirm('AI 起草将覆盖当前输入框内容（含未保存的手输），继续？')) return
  draftingAc.value = true
  busy.value = true
  try {
    const r = await request('board', 'issue.draft_ac', { id }, DRAFT_AC_TIMEOUT_MS)
    if (props.issueId !== id) return
    const draft = String(r?.draft || '').trim()
    if (!draft) {
      toast.warn('AI 未能生成草稿（空结果），请手写或稍后重试')
      return
    }
    acDraft.value = draft
    toast.success('已生成草稿——请检查编辑后再保存')
  } catch (e: any) {
    if (props.issueId === id) toast.error('AI 起草失败: ' + e)
  } finally {
    draftingAc.value = false
    busy.value = false
  }
}

function toggleSendback() {
  decisionExtra.value = decisionExtra.value === 'sendback' ? '' : 'sendback'
  if (decisionExtra.value === 'sendback') void loadChildren()
}

// 保存验收标准（唯一判定依据），可选紧接着复评——UNSURE 无基线的治本路径。
// 两段各自报错：保存成功而复评触发失败 ≠ 保存失败（AC 已落库，别吓用户）。
async function saveAcThenRetrigger(retrigger: boolean) {
  const d = detail.value
  if (!d || busy.value) return
  const ac = acDraft.value.trim()
  if (!ac) {
    toast.warn('验收标准不能为空——留空评审永远出 UNSURE')
    return
  }
  busy.value = true
  try {
    try {
      await request('board', 'issue.update', { id: d.id, acceptance_criteria: ac })
    } catch (e: any) {
      toast.error('保存验收标准失败: ' + e)
      return
    }
    toast.success('验收标准已保存')
    if (retrigger) {
      try {
        await request('board', 'review.retrigger', { number: d.number })
        toast.success(`评审已重触发（${d.number}）：按新基线重跑自动验收`)
      } catch (e: any) {
        toast.warn(`验收标准已保存，但复评触发失败（${e}）——可稍后点「AI 复评收口」`)
      }
    }
    decisionExtra.value = ''
    await afterDecisionAction()
  } finally {
    busy.value = false
  }
}

// 打回子单重做。done 是终态（无重开）——已完成单以「返工： …」子单承接意见
// 并派发；未完成单（in_review/in_progress/blocked/backlog/todo）评论打回，
// blocked 先解阻再重派（dispatch 闸不收 blocked）。
// ⚠ 标题前缀必须用全角冒号「返工：」——后端父单返工轮数清点按
// starts_with("返工：") 计（board_review.rs REMEDIATION_TITLE_PREFIX），
// 半角「返工: 」建出的单不进清点，parent_remediation_max 预算失真。
// 部分失败诚实化（复查 #4）：create/评论已落库而 dispatch 失败时不能只报
// 「打回失败」——用户照原样重试会建出第二张返工单/重复留评论。已落库的
// 半程要指明产物与下一步（去列表手动派发），并复位表单防重复提交。
async function sendbackChild() {
  const d = detail.value
  if (!d || busy.value) return
  const child = childrenList.value.find((c) => c.id === childPick.value)
  if (!child) {
    toast.warn('请先选择要打回的子单')
    return
  }
  const fb = childFeedback.value.trim()
  if (!fb) {
    toast.warn('请填写返工意见——这是给 worker 的工作指令')
    return
  }
  if (!decisionTarget.value) {
    toast.warn('请先选择打回到的节点')
    return
  }
  busy.value = true
  let createdNumber = '' // 已建出的返工单号（部分失败提示用）
  let commentLeft = false // 打回评论是否已落库
  try {
    if (child.status === 'done') {
      const created = await request('board', 'issue.create', {
        // Array.from 按 Unicode 码点截断：String.slice 会劈开代理对，产出
        // 孤代理让后端 serde_json 拒收（rune 截断教训的前端同族）。
        title: Array.from('返工：' + child.title).slice(0, 120).join(''),
        description: `人工打回 ${child.number}（${child.title}）：\n\n${fb}`,
        priority: child.priority,
        parent_issue_id: d.id,
        // 后端 build_new_issue 不做父继承：子单无项目时回落父单的，保证
        // 返工单进同一项目视图（否则从项目分组消失）。
        project_id: child.project_id ?? d.project_id ?? undefined,
      })
      const ni = created?.issue
      if (!ni?.id) throw new Error('后端未返回新单（创建失败）')
      createdNumber = ni.number
      await request('board', 'issue.dispatch', { id: ni.id, target: decisionTarget.value })
      toast.success(`已建返工单 ${ni.number} 并派发到 ${decisionTarget.value}；完成后父单复评自动收口`)
    } else {
      await request('board', 'comment.add', { issue_id: child.id, content: `【人工打回】${fb}` })
      commentLeft = true
      if (child.status === 'blocked')
        await request('board', 'issue.status', { id: child.id, status: 'in_progress' })
      await request('board', 'issue.dispatch', { id: child.id, target: decisionTarget.value })
      toast.success(`已打回 ${child.number} 并重派到 ${decisionTarget.value}；完成后父单复评自动收口`)
    }
    childPick.value = null
    childFeedback.value = ''
    decisionExtra.value = ''
    await afterDecisionAction()
  } catch (e: any) {
    if (createdNumber) {
      toast.warn(
        `返工单 ${createdNumber} 已创建，但派发失败：${e}——请勿重复打回（会再建一张返工单）；可到看板列表/详情对新单手动派发`,
      )
    } else if (commentLeft) {
      toast.warn(`打回评论已留痕，但派发失败：${e}——可到看板列表对该单手动派发；重复打回会再留一条评论`)
    } else {
      toast.error('打回失败: ' + e)
    }
    // 半程已落库 = 重试只会重复产件：复位选中与意见（子卡留着看刷新后的列表）。
    if (createdNumber || commentLeft) {
      childPick.value = null
      childFeedback.value = ''
      await afterDecisionAction()
    }
  } finally {
    busy.value = false
  }
}

// plan_ready 决策变体：拆解完成待确认——确认发车 / 重新拆解 / 放弃
// （复用既有 confirmPlan/startPlan/dismissPlan，决策面板只做引导层）。
const planReadyInfo = computed(() => decision.value?.plan_ready ?? null)
// 重派下拉的在线节点源（issue.decision 附带；集群离线时空列表回退 workerNodes）。
const decisionOnlineNodes = computed(
  () => (decision.value?.online_nodes || []).filter((n) => !!n.name),
)
// 重派目标单一数据源（2026-10-17 去重根修）：后端权威投影 online_nodes
// （在线非本机，按 name 寻址）非空时只渲染它；集群离线/投影缺失时才回退
// workerNodes（注册表全部 worker，按 id 寻址，dispatch 端 name/id 均收）。
// 此前两组无条件拼接，同一在线节点渲染两次（实录：2 worker 显示 4 项）；
// role 过滤只留 worker（coordinator 等非执行角色不在重派目标域）。
const redispatchTargetOptions = computed(() => {
  const online = decisionOnlineNodes.value.filter((n) => !n.role || n.role === 'worker')
  if (online.length > 0) {
    return online.map((n) => ({
      value: n.name,
      label: `${n.name}（在线${n.tier ? ' · ' + n.tier : ''}）`,
    }))
  }
  return workerNodes.value.map((n) => ({
    value: n.id,
    label: `${n.name}（${n.online ? '在线' : '离线'}）`,
  }))
})

// --- AI 拆解（Swarm M1）：两段式 —— 一段 issue.plan（异步 planner，
// plan_id 回执 + board.plan_ready/failed SSE 推送），二段 confirm:true
// 落库 + 依赖闸派发波 + 父单联动。后端真相源：handlers/board.rs issue_plan。
// 声明在下方 issueId watch 之前：watch immediate 首跑 dismissPlan。
interface PlannedSub {
  title: string
  description: string
  required_profession?: string
  required_role: string
  required_tags: string[]
  acceptance_criteria: string
  depends_on: number[]
}
type PlanState = 'idle' | 'planning' | 'dispatching' | 'ready' | 'failed'
const planState = ref<PlanState>('idle')
const planId = ref('')
const planSubs = ref<PlannedSub[]>([])
const planError = ref('')
const planBusy = ref(false)

// BUG-①（2026-10-15 NB-1 事故）：重开详情页恢复未确认计划——此前 ready
// 态只能靠拆解完成瞬间的 plan_ready SSE（要求模态恰开着），错过即没有
// 任何确认入口，只能干等 TTL 过期或误触直接派发。board.plan.list 过滤
// 本单：ready → 恢复预览 + 确认按钮；running → planning 态（后续
// plan_ready SSE 仍会落状态）。失败 = 保持 idle（不阻塞详情页打开）。
const planRecovered = ref(false)
async function recoverPlan() {
  const id = props.issueId
  if (id == null) return
  try {
    const r = await request('board', 'plan.list', { issue_id: id })
    if (props.issueId !== id) return // 过期响应守卫：换单后别恢复旧单的计划预览
    const plans: any[] = r?.plans || []
    const ready = plans.find((p) => p.status === 'ready')
    const running = plans.find((p) => p.status === 'running')
    if (ready && Array.isArray(ready.subs)) {
      planId.value = ready.plan_id
      planSubs.value = ready.subs
      planRecovered.value = true
      planState.value = ready.subs.length ? 'ready' : 'failed'
      if (planState.value === 'failed') planError.value = '拆解结果为空'
    } else if (running) {
      planId.value = running.plan_id
      planRecovered.value = true
      planState.value = running.phase === 'dispatching' ? 'dispatching' : 'planning'
    }
  } catch {
    // 无发现面（集群未启用）/查询失败 → 无可恢复，保持 idle
  }
}

function dismissPlan() {
  planState.value = 'idle'
  planId.value = ''
  planSubs.value = []
  planError.value = ''
  planRecovered.value = false
}

// F5 件①前端：AC 含 `[CHECK] re:` 锚点的黄条提示——本单日后被 AI 拆解
// 成父单时，父层没有交付文本可核验该锚点（必须由子单继承）。行判定与后端
// nemesis_board::parse_anchors 同构（行 trim 后 [CHECK] 前缀 + re: 形态）。
const RE_ANCHOR_LINE = /^\s*\[CHECK\]\s*re:/
const hasRegexAnchorLine = (ac: string | null | undefined) =>
  !!(ac || '').split('\n').some((l) => RE_ANCHOR_LINE.test(l))
const acHasRegexAnchor = computed(() => hasRegexAnchorLine(detail.value?.acceptance_criteria))
// 拆解预览（确认发车前）的最后一道人眼闸：父单锚点没有被任何子单继承。
const planAnchorGap = computed(
  () => planState.value === 'ready' && acHasRegexAnchor.value && !planSubs.value.some((s) => hasRegexAnchorLine(s.acceptance_criteria)),
)

async function startPlan() {
  if (!detail.value || planBusy.value) return
  planBusy.value = true
  planError.value = ''
  try {
    const r = await request('board', 'issue.plan', { id: detail.value.id })
    if (r?.status === 'planning' && r?.plan_id) {
      planId.value = r.plan_id
      planState.value = 'planning'
    } else {
      toast.error('拆解请求响应异常')
    }
  } catch (e: any) {
    toast.error('AI 拆解失败: ' + e)
  } finally {
    planBusy.value = false
  }
}

// planner 完成推送：只认当前打开 issue 的回执（异会话晚到不串台）。
function onPlanReady(data: any) {
  if (!detail.value || data?.issue_id !== detail.value.id) return
  planSubs.value = data.subs || []
  planId.value = data.plan_id || planId.value
  planRecovered.value = false // 现场推送落定，恢复态标记失效
  // auto_confirm 链：确认面板无从参与（派发已在途），直接进「发车中」态；
  // 人工确认模式维持 ready 预览等点按钮。
  if (data?.auto_confirm) {
    planState.value = 'dispatching'
    return
  }
  planState.value = planSubs.value.length ? 'ready' : 'failed'
  if (planState.value === 'failed') planError.value = '拆解结果为空'
}

function onPlanFailed(data: any) {
  if (!detail.value || data?.issue_id !== detail.value.id) return
  planError.value = data?.error || '未知错误'
  planState.value = 'failed'
}

// 拆解运行期可观测（2026-10-11「停着」事故）：他处触发的拆解（auto_start /
// autopilot / agent 工具）开始时，模态内同步进 planning 态（此前只有
// plan_ready/failed 会落状态，拆解中 4~7 分钟模态看起来无动于衷）。
function onPlanStarted(data: any) {
  if (!detail.value || data?.issue_id !== detail.value.id) return
  if (planState.value === 'idle') {
    planId.value = data?.plan_id || planId.value
    planState.value = 'planning'
  }
}

on('board.plan_ready', onPlanReady)
on('board.plan_failed', onPlanFailed)
on('board.plan_started', onPlanStarted)
onUnmounted(() => {
  off('board.plan_ready', onPlanReady)
  off('board.plan_failed', onPlanFailed)
  off('board.plan_started', onPlanStarted)
})

async function confirmPlan() {
  if (!detail.value || planBusy.value) return
  if (!planId.value) {
    // 防御闸（正常不达：planId 空时面板已收起）——诚实提示胜过静默无响应。
    toast.warn('该计划已确认或已放弃')
    return
  }
  planBusy.value = true
  try {
    const r = await request('board', 'issue.plan', {
      id: detail.value.id,
      plan_id: planId.value,
      confirm: true,
    })
    const created = r?.created?.length || 0
    const dispatched = r?.dispatched || 0
    const deferred = r?.deferred?.length || 0
    toast.success(
      `已创建 ${created} 个子任务：派出 ${dispatched}，暂缓 ${deferred}（依赖满足后自动补派）`,
    )
    dismissPlan()
    await loadDetail()
    changed()
  } catch (e: any) {
    toast.error('确认拆解失败: ' + e)
  } finally {
    planBusy.value = false
  }
}

watch(
  () => props.issueId,
  (id) => {
    dismissPlan() // 换 issue/关闭 → 上一个拆解预览作废
    activeTab.value = 'detail' // 换单回落详情页；时间线重进重查
    timelineEvents.value = []
    decision.value = null
    decisionNote.value = ''
    decisionMore.value = false
    decisionTarget.value = ''
    decisionExtra.value = ''
    acDraft.value = ''
    childPick.value = null
    childFeedback.value = ''
    childrenList.value = []
    if (id != null) {
      detail.value = null
      newComment.value = ''
      replyTo.value = null
      attachments.value = []
      loadDetail()
      loadDecision()
      loadAttachments()
      loadWorkerNodes()
      recoverPlan() // BUG-①：打开即查未确认计划（ready 恢复确认入口 / running 转 planning）
    } else {
      // 关闭（父组件置空 issueId）→ 清 detail 让根 v-if="detail" 收起弹层。
      detail.value = null
    }
  },
  { immediate: true },
)

// 看板数据变化推送（W2.5 SSE board-changed）：弹窗打开期间外部写入
// （CLI 跨进程 / 其他标签页 / 集群回调 / autopilot）→ 详情静默换新，
// 不闪 loading、失败不打扰。自己的写操作也会触发一次（幂等 GET，无害）。
useBoardChanged(() => {
  if (props.issueId != null && detail.value) {
    loadDetail(true)
    loadDecision()
    loadAttachments()
  }
})

function changed() {
  emit('changed')
}

async function transition(to: string) {
  if (!detail.value || busy.value) return
  busy.value = true
  try {
    await request('board', 'issue.status', { id: detail.value.id, status: to })
    toast.success(`状态已改为「${statusLabel(to)}」`)
    await loadDetail()
    changed()
  } catch (e: any) {
    toast.error('状态转移失败: ' + e)
  } finally {
    busy.value = false
  }
}

async function assign(clear: boolean) {
  if (!detail.value || busy.value) return
  busy.value = true
  try {
    const payload: any = { id: detail.value.id }
    if (!clear) {
      payload.assignee_type = detailAssignType.value
      payload.assignee_id =
        detailAssignType.value === 'manager_self' ? 'local' : detailAssignId.value.trim()
    }
    await request('board', 'issue.assign', payload)
    toast.success(clear ? '已清空指派' : '已指派')
    detailAssignType.value = ''
    detailAssignId.value = ''
    await loadDetail()
    changed()
  } catch (e: any) {
    toast.error('指派失败: ' + e)
  } finally {
    busy.value = false
  }
}

function insertMention(a: Actor) {
  const token = a.kind === 'worker' ? a.id : `${a.id}`
  const cur = newComment.value
  newComment.value = cur && !cur.endsWith(' ') ? `${cur} @${token} ` : `${cur}@${token} `
}

function startReply(c: CommentRow) {
  replyTo.value = c
  // 回复自带 @ 作者前缀（后端 extract_mentions 产生 mentioned 通知）。
  // H4：@ 用可读名（裁决器 name/id 均识别）。
  const token = replyToken(c.author)
  if (!newComment.value.startsWith(token)) {
    newComment.value = token + newComment.value
  }
}

function cancelReply() {
  replyTo.value = null
  newComment.value = newComment.value.replace(/^@\S+\s*/, '')
}

async function submitComment() {
  if (!detail.value || !newComment.value.trim()) return
  busy.value = true
  try {
    const payload: any = {
      issue_id: detail.value.id,
      content: newComment.value.trim(),
    }
    if (replyTo.value) payload.parent_id = replyTo.value.id
    await request('board', 'comment.add', payload)
    newComment.value = ''
    replyTo.value = null
    await loadDetail()
    changed()
  } catch (e: any) {
    toast.error('评论失败: ' + e)
  } finally {
    busy.value = false
  }
}

async function updatePriority(p: number) {
  if (!detail.value) return
  try {
    await request('board', 'issue.update', { id: detail.value.id, priority: p })
    toast.success('优先级已更新')
    await loadDetail()
    changed()
  } catch (e: any) {
    toast.error('更新失败: ' + e)
  }
}

// 撤回在途派发（B14）：只撤派发、单据存活——派错节点换人重派 / worker
// 假死不等超时 sweep 的轻量出口。与「✖ 取消」（连单终杀）不同：issue
// 状态不动，撤后可直接重新派发。
async function cancelDispatchIssue() {
  if (!detail.value || busy.value) return
  busy.value = true
  try {
    const r = await request('board', 'issue.cancel_dispatch', { id: detail.value.id })
    toast.success(`派发已撤回（task ${(r?.task_id || '').slice(0, 8)}…），单据保留可重新派发`)
    await loadDetail()
    changed()
  } catch (e: any) {
    toast.error('撤回失败: ' + e)
  } finally {
    busy.value = false
  }
}

async function dispatchIssue() {
  if (!detail.value || !dispatchTarget.value || busy.value) return
  busy.value = true
  try {
    const r = await request('board', 'issue.dispatch', {
      id: detail.value.id,
      target: dispatchTarget.value,
    })
    toast.success(`已派发（task ${(r?.task_id || '').slice(0, 8)}…），等 worker 回报后自动写回`)
    dispatchTarget.value = ''
    await loadDetail()
    changed()
  } catch (e: any) {
    toast.error('派发失败: ' + e)
  } finally {
    busy.value = false
  }
}

// 评审重触发（review.retrigger，M5-1）：仅 in_review；重入评审链
// （评审会按需重新取证——锚点/执行档案直读/LLM 评审）。
async function retriggerReview() {
  if (!detail.value || busy.value) return
  busy.value = true
  try {
    await request('board', 'review.retrigger', { number: detail.value.number })
    toast.success(`评审已重触发（${detail.value.number}）：正在重跑自动验收`)
    await loadDetail()
    changed()
  } catch (e: any) {
    toast.error('重触发失败: ' + e)
  } finally {
    busy.value = false
  }
}

// 任务档案（2026-09-29 goal）：本单 + 全部后代记录导出为自包含目录。
async function exportDossier() {
  if (!detail.value || busy.value) return
  busy.value = true
  try {
    const r = await request('board', 'dossier.export', { number: detail.value.number })
    const notes = (r?.notes || []) as string[]
    if (notes.length) {
      toast.info(`档案已导出 ${r.root}（${notes.length} 条缺口注记已写入 README）`)
    } else {
      toast.success(`档案已导出：${r.root}`)
    }
  } catch (e: any) {
    toast.error('档案导出失败: ' + e)
  } finally {
    busy.value = false
  }
}

// --- 附件（W2 P3）---
function onFilePick(ev: Event) {
  const input = ev.target as HTMLInputElement
  const file = input.files?.[0]
  input.value = '' // 允许重复选同一文件
  if (!file || !detail.value) return
  if (file.size > MAX_ATTACHMENT_BYTES) {
    toast.error(`附件过大（上限 ${fmtSize(MAX_ATTACHMENT_BYTES)}）`)
    return
  }
  uploadAttachment(file)
}

async function uploadAttachment(file: File) {
  if (!detail.value || busy.value) return
  busy.value = true
  try {
    const buf = await file.arrayBuffer()
    let binary = ''
    const bytes = new Uint8Array(buf)
    for (let i = 0; i < bytes.length; i++) binary += String.fromCharCode(bytes[i])
    const content = btoa(binary)
    await request('board', 'attachment.add', {
      issue_id: detail.value.id,
      filename: file.name,
      content,
    })
    toast.success(`已上传 ${file.name}`)
    await loadAttachments()
  } catch (e: any) {
    toast.error('上传失败: ' + e)
  } finally {
    busy.value = false
  }
}

async function downloadAttachment(a: AttachmentRow) {
  try {
    const r = await request('board', 'attachment.get', { id: a.id })
    const b64: string = r?.content || ''
    const bin = atob(b64)
    const bytes = new Uint8Array(bin.length)
    for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i)
    const url = URL.createObjectURL(new Blob([bytes]))
    const link = document.createElement('a')
    link.href = url
    link.download = a.filename
    link.click()
    URL.revokeObjectURL(url)
  } catch (e: any) {
    toast.error('下载失败: ' + e)
  }
}
</script>

<template>
  <div v-if="detail" class="modal-backdrop" @click.self="emit('close')">
    <div class="modal" style="max-width: 720px;">
      <div class="modal-header" style="display: flex; align-items: center; gap: var(--space-3);">
        <h3><code>{{ detail.number }}</code> {{ detail.title }}</h3>
        <!-- U-C 页签：详情（默认，原布局）/ 时间线（全链路统一投影） -->
        <div class="detail-tabs">
          <button
            class="btn btn-sm"
            :class="{ 'btn-primary': activeTab === 'detail' }"
            @click="switchTab('detail')"
          >详情</button>
          <button
            class="btn btn-sm"
            :class="{ 'btn-primary': activeTab === 'timeline' }"
            title="活动/评论/派发/任务结果统一时间线（与任务档案同源聚合）"
            @click="switchTab('timeline')"
          >🕐 时间线</button>
        </div>
      </div>
      <div class="modal-body" v-if="activeTab === 'timeline'">
        <!-- U-C 时间线页 -->
        <div v-if="timelineLoading" class="muted">加载中…</div>
        <div v-else-if="timelineEvents.length === 0" class="muted" style="padding: var(--space-2) 0;">暂无记录</div>
        <div v-else class="timeline-list">
          <div v-for="(e, i) in timelineEvents" :key="i" class="timeline-row">
            <span class="muted timeline-ts">{{ fmtTime(e.ts) }}</span>
            <span class="badge" :class="SOURCE_BADGE[e.source] || 'badge-neutral'">{{ SOURCE_LABEL[e.source] || e.source }}<template v-if="e.source === 'comment' || e.source === 'dispatch'">·{{ e.kind }}</template></span>
            <span class="timeline-body">
              <strong v-if="e.actor">{{ e.actor }}</strong>
              {{ timelineSummary(e) }}
              <span v-if="timelineDetail(e)" class="muted activity-details" :title="e.detail">{{ timelineDetail(e) }}</span>
            </span>
          </div>
        </div>
      </div>
      <div class="modal-body" v-else>
        <!-- 元信息 -->
        <div class="detail-meta">
          <span class="badge" :class="STATUS_BADGE[detail.status]" :title="statusTitle(detail.status)">{{ statusLabel(detail.status) }}</span>
          <span class="badge" :class="PRIORITY_BADGE[detail.priority] || 'badge-neutral'">P{{ detail.priority }} {{ PRIORITY_LABEL[detail.priority] || '' }}</span>
          <span v-if="detail.required_profession" class="badge badge-info" title="职能派发：仅匹配声明该职能的节点">🛠 {{ detail.required_profession }}</span>
          <span v-if="decision?.attention" class="badge badge-attention" title="执行档案停滞（哨兵）：交付已收但变更集未合入" data-test="detail-attention-badge">⚠ 停滞</span>
          <span class="muted">指派：{{ assigneeLabel(detail) }}</span>
          <span class="muted">创建者：{{ detail.creator.kind }}/{{ detail.creator.id }}</span>
          <span class="muted" v-if="detail.origin">来源：{{ detail.origin.origin_type }}/{{ detail.origin.origin_id }}</span>
          <span class="muted">更新：{{ fmtTime(detail.updated_at) }}</span>
        </div>

        <!-- ═══ 第一层「需要你」（F2 决策面板，2026-10-15 批）：只在真有
             待裁决事项（活停车档案 / 待确认计划）时出现，置顶即视。 ═══ -->
        <div v-if="showDecisionPanel && decision" class="decision-panel" data-test="decision-panel">
          <div class="decision-head">
            <span v-if="nu" class="badge badge-decision">✋ 需要你决策</span>
            <span v-if="nu" class="badge badge-neutral">{{ nu.stop_kind_label || stopKindLabel(nu.stop_kind) }}</span>
            <template v-if="planReadyInfo">
              <span class="badge badge-plan">📋 计划待确认</span>
              <span class="muted">{{ planReadyInfo.subs_count }} 个子单 · 拆解于 {{ fmtTime(planReadyInfo.created_age_secs ? Date.now() / 1000 - planReadyInfo.created_age_secs : 0) }}</span>
            </template>
            <span v-if="nu?.stopped_at" class="muted decision-stopped">停车于 {{ fmtTime(Date.parse(nu.stopped_at) / 1000 || 0) }}</span>
          </div>

          <!-- plan_ready 变体：子单预览在下方「AI 拆解」区块（复用
               recoverPlan/confirmPlan/dismissPlan 既有链），面板做引导层。
               planId 闸：放弃/确认后本地句柄清空，分支立即收起（不等投影刷新）。 -->
          <template v-if="planReadyInfo && planId">
            <div class="reason-card">
              <div class="reason-line">AI 拆解已完成：<strong>{{ planReadyInfo.subs_count }} 个子单</strong>等待确认发车。</div>
              <div class="muted reason-sub">子单计划预览见下方「AI 拆解」区块——确认后自动落库并按依赖派发。</div>
            </div>
            <div class="decision-actions">
              <button class="btn btn-sm btn-primary" :disabled="planBusy" data-test="decision-plan-confirm" @click="confirmPlan">✅ 确认发车</button>
              <button class="btn btn-sm" :disabled="planBusy" title="丢弃当前计划，重新跑 AI 拆解" @click="startPlan">🔁 重新拆解</button>
              <button class="btn btn-sm btn-ghost" :disabled="planBusy" data-test="decision-plan-dismiss" @click="dismissPlan">✖ 放弃</button>
            </div>
          </template>

          <!-- needs_user 变体：停车原因卡 + 预设操作按钮 + 可选附言 + 更多上下文 -->
          <template v-else-if="nu">
            <div class="reason-card">
              <div class="reason-line">{{ nu.hint || '（停车档案未附原因说明，点开「更多上下文」看判决链）' }}</div>
              <div class="muted reason-sub">
                <span>建议处置：{{ suggestedActionLabel(nu.suggested_action) }}</span>
                <span v-if="isParentishStop && childrenList.length"> · 子单 {{ childrenDoneCount }}/{{ childrenList.length }} 已完成</span>
                <span v-if="lastWorker"> · 最后执行：{{ lastWorker }}</span>
              </div>
            </div>

            <!-- 重派目标（有重派动作时展示；在线组优先，空才回退注册表） -->
            <div v-if="decisionActions.some(a => a.needsTarget)" class="decision-target-row">
              <span class="muted">重派到：</span>
              <select v-model="decisionTarget" class="form-select" style="max-width: 280px;" data-test="decision-target">
                <option value="">选择节点…</option>
                <option v-for="n in redispatchTargetOptions" :key="'rt-' + n.value" :value="n.value">{{ n.label }}</option>
              </select>
            </div>

            <div class="decision-actions" data-test="decision-actions">
              <button
                v-for="a in decisionActions"
                :key="a.key"
                class="btn btn-sm"
                :class="a.cls"
                :disabled="busy || a.disabled"
                :title="a.title"
                :data-test="'decision-act-' + a.key"
                @click="a.run"
              >{{ a.label }}</button>
            </div>

            <!-- 父单/项目特化中间档：复评死循环的两条出路（互斥子卡）。 -->
            <div v-if="decisionExtra === 'ac'" class="decision-subcard" data-test="decision-ac-editor">
              <div class="form-label">验收标准（唯一判定依据）——评审据此定 PASS / FAIL</div>
              <textarea
                v-model="acDraft"
                class="form-textarea"
                style="min-height: 72px;"
                placeholder="逐条写清可核验的客观标准。留空 = 评审没有基线，永远只能给 UNSURE"
                data-test="decision-ac-input"
              ></textarea>
              <div class="decision-actions">
                <button class="btn btn-sm" :disabled="busy || draftingAc" data-test="decision-ac-draft" title="让 AI 依据任务描述、子单情况与交付讨论起草验收标准，填入上方输入框供编辑" @click="aiDraftAc">✨ {{ draftingAc ? 'AI 起草中…' : 'AI 起草' }}</button>
                <button class="btn btn-sm btn-primary" :disabled="busy" data-test="decision-ac-save-retrigger" @click="saveAcThenRetrigger(true)">保存并复评</button>
                <button class="btn btn-sm" :disabled="busy" @click="saveAcThenRetrigger(false)">仅保存</button>
                <button class="btn btn-sm btn-ghost" @click="decisionExtra = ''">收起</button>
              </div>
            </div>
            <div v-else-if="decisionExtra === 'sendback'" class="decision-subcard" data-test="decision-sendback">
              <div class="form-label">打回子单重做</div>
              <div class="muted reason-sub">已完成单无法重开（终态）——将以「返工：…」子单承接意见；未完成单评论打回并重派。父单保持待收口，返工完成复评后自动收口。</div>
              <select v-model.number="childPick" class="form-select" style="max-width: 100%;" data-test="sendback-child-select">
                <option :value="null" disabled>选择子单…</option>
                <option v-for="c in childrenList" :key="c.id" :value="c.id">{{ c.number }}（{{ statusLabel(c.status) }}）{{ String(c.title || '').slice(0, 26) }}</option>
              </select>
              <textarea
                v-model="childFeedback"
                class="form-textarea"
                style="min-height: 56px;"
                placeholder="返工意见（必填）：哪里不行、要改成什么样——worker 按此重做"
                data-test="sendback-feedback"
              ></textarea>
              <select v-model="decisionTarget" class="form-select" style="max-width: 280px;" data-test="sendback-target">
                <option value="">打回到节点…</option>
                <!-- 与重派目标同一单一数据源（此前两组节点源无条件拼接：同一
                     在线节点渲染两次、无 role 过滤——redispatch 去重根修的漏改面） -->
                <option v-for="n in redispatchTargetOptions" :key="'sb-' + n.value" :value="n.value">{{ n.label }}</option>
              </select>
              <div class="decision-actions">
                <button class="btn btn-sm btn-primary" :disabled="busy" data-test="sendback-run" @click="sendbackChild">打回并派发</button>
                <button class="btn btn-sm btn-ghost" @click="decisionExtra = ''">收起</button>
              </div>
            </div>

            <textarea
              v-model="decisionNote"
              class="form-textarea decision-note"
              style="min-height: 44px;"
              placeholder="附言（可选）：留下裁决理由，将先作为评论留痕再执行动作"
              data-test="decision-note"
            ></textarea>

            <button class="btn btn-xs btn-ghost decision-more-btn" data-test="decision-more" @click="decisionMore = !decisionMore">
              {{ decisionMore ? '收起上下文 ▴' : '更多上下文 ▾' }}
            </button>
            <div v-if="decisionMore" class="decision-context" data-test="decision-context">
              <div v-if="verdictChain.length">
                <div class="form-label">判决链（{{ verdictChain.length }}）</div>
                <div v-for="(v, i) in verdictChain" :key="i" class="chain-row">
                  <!-- at 是后端 timeline 聚合写的 unix 秒（Rust 真相源），直接
                       Number 换算；Date.parse 会把纯数字串当年份解析成 NaN。 -->
                  <span class="muted">{{ v.at ? fmtTime(Number(v.at)) : `#${i + 1}` }}</span>
                  <span class="badge" :class="v.verdict === 'PASS' ? 'badge-success' : v.verdict === 'FAIL' ? 'badge-error' : 'badge-warning'">{{ v.verdict || '?' }}</span>
                  <span class="chain-details">{{ v.decision || v.details || '' }}</span>
                </div>
              </div>
              <div v-if="attemptedList.length" style="margin-top: var(--space-2);">
                <div class="form-label">已尝试</div>
                <div v-for="(a, i) in attemptedList" :key="i" class="chain-row">
                  <span class="muted">#{{ i + 1 }}</span>
                  <span class="chain-details">{{ typeof a === 'string' ? a : JSON.stringify(a) }}</span>
                </div>
              </div>
              <div v-if="resumeEntry" style="margin-top: var(--space-2);">
                <div class="form-label">恢复入口</div>
                <code class="chain-details">{{ resumeEntry }}</code>
              </div>
            </div>
          </template>
        </div>

        <!-- 上游依赖（2026-10-15 需求①）：✓ = 已满足；🔒 = 未完成（派发闸会等）；
             ⚠ 已取消 = 死等异常残留。点击不了（只读投影），管理依赖走 CLI。 -->
        <div class="form-group" v-if="detailDeps.length">
          <label class="form-label">上游依赖（{{ detailDeps.length }}）</label>
          <div class="assign-row">
            <span
              v-for="d in detailDeps"
              :key="d.id"
              class="badge"
              :class="d.status === 'done' ? 'badge-success' : d.status === 'cancelled' ? 'badge-error' : 'badge-warning'"
              :title="d.status === 'done'
                ? `${d.number} 已完成，依赖已满足`
                : d.status === 'cancelled'
                  ? `${d.number} 已取消：本单将永远无法开工（正常应已被级联取消）`
                  : `${d.number} 未完成（${statusLabel(d.status)}），完成后本单才可派发`"
            >{{ d.status === 'done' ? `✓ ${d.number}` : d.status === 'cancelled' ? `⚠ ${d.number} 已取消` : `🔒 等 ${d.number}（${statusLabel(d.status)}）` }}</span>
          </div>
        </div>

        <!-- ═══ 第二层「你来操作」（F3 三层重排）：状态转移/评审/优先级/
             指派/派发/档案/拆解收拢为一组，与记录层视觉分离。 ═══ -->
        <div class="ops-group">
        <div class="layer-head">⚙ 你来操作</div>

        <!-- 状态转移 -->
        <div class="form-group" v-if="allowedTargets(detail.status).length">
          <label class="form-label">状态转移</label>
          <div class="assign-row">
            <button
              v-for="t in allowedTargets(detail.status)"
              :key="t"
              class="btn btn-sm"
              :disabled="busy"
              @click="transition(t)"
            >→ {{ statusLabel(t) }}</button>
          </div>
        </div>
        <div class="form-group" v-else>
          <p class="muted">终态 issue 不可再转移。</p>
        </div>

        <!-- 评审重触发（review.retrigger）：卡 in_review 的单子人工一键重入验收 -->
        <div class="form-group" v-if="detail.status === 'in_review'">
          <label class="form-label">评审重触发</label>
          <div class="assign-row">
            <button class="btn btn-sm" :disabled="busy" @click="retriggerReview()">🔄 重触发评审</button>
            <span class="muted">按当前材料重跑自动验收（锚点 → 执行档案直读 → LLM 评审）</span>
          </div>
        </div>

        <!-- 优先级 -->
        <div class="form-group">
          <label class="form-label">优先级</label>
          <div class="assign-row">
            <button
              v-for="(_, p) in PRIORITY_LABEL"
              :key="p"
              class="btn btn-sm"
              :class="{ 'btn-primary': Number(p) === detail.priority }"
              :disabled="busy"
              @click="updatePriority(Number(p))"
            >P{{ p }} {{ PRIORITY_LABEL[Number(p)] }}</button>
          </div>
        </div>

        <!-- 指派 -->
        <div class="form-group">
          <label class="form-label">指派</label>
          <div class="assign-row">
            <select v-model="detailAssignType" class="form-select" style="max-width: 180px;">
              <option value="">选择…</option>
              <option value="manager_self">manager（本机）</option>
              <option value="worker">worker 节点</option>
            </select>
            <select
              v-if="detailAssignType === 'worker' && workerNodes.length"
              class="form-select"
              v-model="detailAssignId"
              style="max-width: 260px;"
            >
              <option value="">选择 worker…</option>
              <option v-for="n in workerNodes" :key="n.id" :value="n.id">{{ `${n.name} (${n.id})${n.online ? '' : ' · 离线'}` }}</option>
            </select>
            <input
              v-else-if="detailAssignType === 'worker'"
              class="form-input"
              v-model="detailAssignId"
              placeholder="节点 id"
              style="max-width: 200px;"
            />
            <button class="btn btn-sm" :disabled="busy || !detailAssignType" @click="assign(false)">指派</button>
            <button class="btn btn-sm btn-ghost" :disabled="busy || !detail.assignee" @click="assign(true)">清空</button>
          </div>
        </div>

        <!-- 派发执行（W2 P2 派发链路） -->
        <div class="form-group" v-if="canDispatch">
          <label class="form-label">派发执行</label>
          <div class="assign-row">
            <select
              class="form-select"
              v-model="dispatchTarget"
              style="max-width: 260px;"
            >
              <option value="">选择 worker…</option>
              <option v-for="n in workerNodes" :key="n.id" :value="n.id">{{ `${n.name} (${n.id})${n.online ? '' : ' · 离线'}` }}</option>
            </select>
            <button
              class="btn btn-sm btn-primary"
              :disabled="busy || !dispatchTarget"
              title="把 issue 作为任务派给 worker 执行，结果自动写回评论"
              @click="dispatchIssue"
            >派发 →</button>
            <button
              v-if="detail.active_dispatch"
              class="btn btn-sm btn-danger"
              :disabled="busy"
              :title="`撤回在途派发（worker=${detail.active_dispatch.worker_id}）：只撤派发，单据保留可直接重派；worker 迟到回报会被写回幂等丢弃`"
              @click="cancelDispatchIssue"
            >⏹ 撤回派发</button>
            <span class="muted">派发后 issue 转进行中；worker 回报 → 自动写结果评论并转评审中</span>
          </div>
        </div>

        <!-- 任务档案（2026-09-29 goal：整合本单全部记录为自包含目录） -->
        <div class="form-group">
          <label class="form-label">任务档案</label>
          <div class="assign-row">
            <button
              class="btn btn-sm"
              :disabled="busy"
              title="把本单及全部子单的活动/评论/派发/执行档案/token 用量整合导出到 workspace/logs/dossiers/ 下的自包含目录"
              @click="exportDossier"
            >📁 导出档案</button>
            <span class="muted">整合活动/评论/派发/执行档案/用量为自包含目录（logs/dossiers/）</span>
          </div>
        </div>

        <!-- AI 拆解（Swarm M1：planner 两段式） -->
        <div class="form-group">
          <label class="form-label">AI 拆解</label>
          <div v-if="planState === 'idle'" class="assign-row">
            <button
              class="btn btn-sm"
              :disabled="busy || planBusy"
              title="按标题/描述/验收标准单层拆解为子任务，预览确认后自动落库并按依赖派发"
              @click="startPlan"
            >✨ AI 拆解</button>
            <span class="muted">拆解为子任务 → 预览确认 → 自动派发（依赖满足后自动补派）</span>
          </div>
          <div v-else-if="planState === 'planning'" class="assign-row">
            <span class="muted">⏳ 拆解中…（后台运行，完成后自动展示预览）</span>
            <button class="btn btn-xs btn-ghost" @click="dismissPlan">取消等待</button>
          </div>
          <div v-else-if="planState === 'dispatching'" class="assign-row">
            <span class="muted">🚗 拆解完成，自动发车派发中…（子单已在看板可见，逐个送达 worker）</span>
            <button class="btn btn-xs btn-ghost" @click="dismissPlan">收起</button>
          </div>
          <div v-else-if="planState === 'failed'">
            <p class="plan-error">⛔ 拆解失败：{{ planError }}</p>
            <div class="assign-row">
              <button class="btn btn-sm" @click="startPlan">重试</button>
              <button class="btn btn-sm btn-ghost" @click="dismissPlan">关闭</button>
            </div>
          </div>
          <template v-else>
            <div v-if="planRecovered && planState === 'ready'" class="muted" style="margin-bottom: var(--space-1);">
              ♻ 已恢复上次未确认的拆解计划（预览有效期 10 分钟，过期需重拆）
            </div>
            <div v-if="planAnchorGap" class="anchor-warn-bar">
              ⚠ 锚点下放检查：本单验收标准含 [CHECK] re: 锚点，但下方子单均未继承——确认发车后父单收口时该锚点将无法核验。
            </div>
            <div class="plan-tree">
              <div v-for="(s, i) in planSubs" :key="i" class="plan-node">
                <div class="plan-node-head">
                  <strong>{{ i + 1 }}. {{ s.title }}</strong>
                  <span v-if="s.required_profession" class="badge badge-info" title="该子单指派给声明此职能的节点">🛠 {{ s.required_profession }}</span>
                  <span v-if="s.required_role" class="badge badge-neutral">角色:{{ s.required_role }}</span>
                  <span v-for="t in s.required_tags" :key="t" class="badge badge-neutral">#{{ t }}</span>
                </div>
                <div v-if="s.description" class="muted">{{ s.description }}</div>
                <div v-if="s.acceptance_criteria" class="muted">验收：{{ s.acceptance_criteria }}</div>
                <div v-if="s.depends_on.length" class="muted">
                  依赖：{{ s.depends_on.map(d => `#${d + 1}`).join('、') }}完成后派发
                </div>
              </div>
            </div>
            <div class="assign-row" style="margin-top: var(--space-2);">
              <button class="btn btn-sm btn-primary" :disabled="planBusy" @click="confirmPlan">
                ✓ 确认拆解（{{ planSubs.length }} 个子任务）并派发
              </button>
              <button class="btn btn-sm btn-ghost" :disabled="planBusy" @click="dismissPlan">放弃</button>
            </div>
          </template>
        </div>
        </div><!-- /ops-group -->

        <!-- ═══ 第三层「系统记录」（F3）：描述/验收/评论/附件/活动，只读为主。 ═══ -->
        <div class="records-group">
        <div class="layer-head">📋 系统记录</div>

        <!-- 描述 / 验收 -->
        <div class="form-group" v-if="detail.description">
          <label class="form-label">描述</label>
          <pre class="detail-pre">{{ detail.description }}</pre>
        </div>
        <div class="form-group" v-if="detail.acceptance_criteria">
          <label class="form-label">验收标准</label>
          <div v-if="acHasRegexAnchor" class="anchor-warn-bar">
            ⚠ 含 [CHECK] re: 锚点：本单若后续被 AI 拆解为父单，父层没有交付文本可核验该锚点——拆解时请确保至少一个子单继承它。
          </div>
          <pre class="detail-pre">{{ detail.acceptance_criteria }}</pre>
        </div>

        <!-- 评论（W2 P3：线程 + @提及） -->
        <div class="form-group">
          <label class="form-label">评论（{{ topLevelComments.length }}）</label>
          <div v-if="topLevelComments.length === 0" class="muted" style="padding: var(--space-2) 0;">暂无评论</div>
          <div v-for="c in topLevelComments" :key="c.id" class="comment-item">
            <div class="comment-head">
              <strong>{{ authorLabel(c.author) }}</strong>
              <span v-if="c.ctype !== 'comment'" class="badge" :class="ctypeBadge(c)">{{ ctypeLabel(c) }}</span>
              <span class="muted">{{ fmtTime(c.created_at) }}</span>
              <button v-if="c.ctype === 'comment'" class="btn btn-xs btn-ghost" @click="startReply(c)">回复</button>
            </div>
            <div class="comment-body" :class="{ 'comment-body-report': c.ctype === 'delivery' }">{{ c.content }}</div>
            <!-- 一层回复 -->
            <div v-for="r in repliesOf(c.id)" :key="r.id" class="comment-item reply-item">
              <div class="comment-head">
                <strong>{{ authorLabel(r.author) }}</strong>
                <span v-if="r.ctype !== 'comment'" class="badge" :class="ctypeBadge(r)">{{ ctypeLabel(r) }}</span>
                <span class="muted">{{ fmtTime(r.created_at) }}</span>
                <button v-if="r.ctype === 'comment'" class="btn btn-xs btn-ghost" @click="startReply(r)">回复</button>
              </div>
              <div class="comment-body" :class="{ 'comment-body-report': r.ctype === 'delivery' }">{{ r.content }}</div>
            </div>
          </div>
          <div v-if="replyTo" class="replying-hint">
            回复 {{ authorLabel(replyTo.author) }}
            <button class="btn btn-xs btn-ghost" @click="cancelReply">取消</button>
          </div>
          <div v-if="mentionCandidates.length" class="mention-row">
            <span class="muted">@提及：</span>
            <button
              v-for="a in mentionCandidates"
              :key="`${a.kind}/${a.id}`"
              class="btn btn-xs btn-ghost"
              :title="`插入 @${a.id}`"
              @click="insertMention(a)"
            >@{{ a.id }}</button>
          </div>
          <textarea class="form-textarea" v-model="newComment" style="min-height: 60px; margin-top: var(--space-2);" placeholder="添加评论…（@节点id 可提及）"></textarea>
          <button class="btn btn-sm btn-primary" style="margin-top: var(--space-2);" :disabled="busy || !newComment.trim()" @click="submitComment">发表评论</button>
        </div>

        <!-- 附件（W2 P3） -->
        <div class="form-group">
          <label class="form-label">附件（{{ attachments.length }}）</label>
          <div v-if="attachments.length === 0" class="muted" style="padding: var(--space-2) 0;">暂无附件</div>
          <div v-for="a in attachments" :key="a.id" class="attachment-item">
            <span class="attachment-name">{{ a.filename }}</span>
            <span class="muted">{{ fmtSize(a.size) }} · {{ fmtTime(a.created_at) }}</span>
            <button class="btn btn-xs" @click="downloadAttachment(a)">下载</button>
          </div>
          <label class="btn btn-sm" style="margin-top: var(--space-2); cursor: pointer;">
            上传附件
            <input type="file" style="display: none;" @change="onFilePick" />
          </label>
          <span class="muted" style="margin-left: var(--space-2);">≤ 8 MB</span>
        </div>

        <!-- 活动时间线 -->
        <div class="form-group">
          <label class="form-label">活动</label>
          <div v-if="(detail.activity || []).length === 0" class="muted">暂无活动</div>
          <div v-for="a in (detail.activity as ActivityRow[])" :key="a.id" class="activity-item">
            <span class="muted">{{ fmtTime(a.created_at) }}</span>
            <span><strong>{{ a.actor.kind }}/{{ a.actor.id }}</strong> {{ a.action }}</span>
            <span v-if="a.details" class="muted activity-details">{{ a.details }}</span>
          </div>
        </div>
        </div><!-- /records-group -->
      </div>
      <div class="modal-footer">
        <button class="btn" @click="emit('close')">关闭</button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.muted {
  color: var(--text-muted);
  font-size: var(--text-sm);
}
.assign-row {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: var(--space-2);
}
.detail-meta {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: var(--space-2);
  margin-bottom: var(--space-4);
}
.detail-pre {
  white-space: pre-wrap;
  background: var(--bg-secondary);
  border-radius: var(--radius-md);
  padding: var(--space-3);
  font-size: var(--text-sm);
  max-height: 200px;
  overflow-y: auto;
}
.comment-item {
  border-left: 2px solid var(--border-color, var(--border));
  padding: var(--space-2) var(--space-3);
  margin-bottom: var(--space-2);
}
.reply-item {
  margin-top: var(--space-2);
  margin-bottom: 0;
}
.comment-head {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  font-size: var(--text-sm);
  margin-bottom: var(--space-1);
}
.comment-body {
  font-size: var(--text-sm);
  white-space: pre-wrap;
}
.comment-body-report {
  background: var(--bg-secondary);
  border-radius: var(--radius-sm);
  padding: var(--space-2);
}
.replying-hint {
  font-size: var(--text-sm);
  color: var(--text-muted);
  margin-top: var(--space-2);
  display: flex;
  align-items: center;
  gap: var(--space-2);
}
.mention-row {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: var(--space-1);
  margin-top: var(--space-2);
  font-size: var(--text-sm);
}
.attachment-item {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  font-size: var(--text-sm);
  padding: var(--space-1) 0;
}
.attachment-name {
  font-weight: 500;
}
.activity-item {
  display: flex;
  flex-wrap: wrap;
  gap: var(--space-2);
  font-size: var(--text-sm);
  padding: var(--space-1) 0;
}
.activity-details {
  word-break: break-all;
}
/* U-C 时间线页签 */
.detail-tabs {
  display: flex;
  gap: var(--space-1);
  margin-left: auto;
}
.timeline-list {
  display: flex;
  flex-direction: column;
}
.timeline-row {
  display: flex;
  flex-wrap: wrap;
  align-items: baseline;
  gap: var(--space-2);
  font-size: var(--text-sm);
  padding: var(--space-1) 0;
  border-bottom: 1px solid var(--border-color, var(--border));
}
.timeline-ts {
  white-space: nowrap;
  font-variant-numeric: tabular-nums;
}
.timeline-body {
  flex: 1;
  min-width: 0;
  word-break: break-word;
}
.plan-error {
  color: var(--danger, #d33);
  font-size: var(--text-sm);
  margin: 0 0 var(--space-2);
}
/* F5 件①：锚点下放警示黄条（验收标准块 + 拆解预览共用） */
.anchor-warn-bar {
  background: var(--warning-bg, rgba(255, 193, 7, 0.12));
  border: 1px solid var(--warning-border, rgba(255, 193, 7, 0.45));
  border-radius: var(--radius-sm, 6px);
  color: var(--warning-text, #8a6d00);
  font-size: var(--text-sm);
  padding: var(--space-2) var(--space-3);
  margin-bottom: var(--space-2);
}
.plan-tree {
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
}
.plan-node {
  border-left: 2px solid var(--border-color, var(--border));
  padding: var(--space-1) var(--space-3);
  font-size: var(--text-sm);
}
.plan-node-head {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: var(--space-2);
}
/* ═══ F2 决策面板 + F3 三层重排（2026-10-15 批）═══ */
.decision-panel {
  background: var(--warning-bg, rgba(245, 158, 11, 0.08));
  border: 1px solid var(--warning-border, rgba(245, 158, 11, 0.4));
  border-radius: var(--radius-md);
  padding: var(--space-3);
  margin-bottom: var(--space-4);
}
.decision-head {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: var(--space-2);
  margin-bottom: var(--space-2);
}
.decision-stopped {
  font-size: var(--text-xs);
}
.reason-card {
  background: var(--surface);
  border-radius: var(--radius-sm);
  padding: var(--space-2) var(--space-3);
  margin-bottom: var(--space-2);
}
.reason-line {
  font-size: var(--text-sm);
  font-weight: 500;
  white-space: pre-wrap;
  word-break: break-word;
}
.reason-sub {
  font-size: var(--text-xs);
  margin-top: var(--space-1);
}
.decision-target-row {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  margin-bottom: var(--space-2);
  font-size: var(--text-sm);
}
.decision-actions {
  display: flex;
  flex-wrap: wrap;
  gap: var(--space-2);
  margin-bottom: var(--space-2);
}
/* 父单特化中间档子卡（补验收标准 / 打回子单）：琥珀面板内的表面层。 */
.decision-subcard {
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
  padding: var(--space-2);
  margin-bottom: var(--space-2);
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
}
.decision-note {
  margin-bottom: var(--space-2);
}
.decision-more-btn {
  margin-bottom: var(--space-1);
}
.decision-context {
  background: var(--surface);
  border-radius: var(--radius-sm);
  padding: var(--space-2) var(--space-3);
  font-size: var(--text-xs);
  max-height: 220px;
  overflow-y: auto;
}
.chain-row {
  display: flex;
  align-items: baseline;
  gap: var(--space-2);
  padding: var(--space-1) 0;
  border-bottom: 1px solid var(--border-color, var(--border));
}
.chain-details {
  word-break: break-all;
  white-space: pre-wrap;
}
/* F3 层分组：第二层操作 / 第三层记录，轻量区块头 + 左基线。 */
.ops-group,
.records-group {
  border-top: 1px solid var(--border-color, var(--border));
  padding-top: var(--space-3);
  margin-top: var(--space-2);
}
.layer-head {
  font-size: var(--text-xs);
  font-weight: 700;
  color: var(--text-muted);
  text-transform: uppercase;
  letter-spacing: 0.04em;
  margin-bottom: var(--space-3);
}
</style>
