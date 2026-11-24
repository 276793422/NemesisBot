<script setup lang="ts">
/**
 * SessionResourcesPanel — 会话资源盘点看板（2026-11-17，B3 前端主体）。
 *
 * 渲染当前会话的 16 类资源聚合（后端 WSAPI `session.resources`，Provider
 * 注册制聚合；协议见 crates/nemesis-web/src/handlers/resources.rs）：
 * - 头部：会话标题 / 项目 / 工作目录（identity.summary + system.status）
 *   +「+ 添加项目目录」（ProjectCreateModal）；
 * - 会话工具分节（2026-11-19 拆分迁移 → 九键全迁）：构建/计划 + 插队 +
 *   分享 / 委派 / 思考强度；放行开关分节（Full Access / 外部写删 / 全部
 *   放行）；语音分节（听写 / 语音对话 / 语音播放 / 实时语音）——原
 *   ChatPanel 输入框工具栏九键在有资源窗注入面时不再渲染（standalone/
 *   workflow 挂载面保留原工具栏）；状态真值走各单例（useComposerTools /
 *   useEditorMode / useSessionAllowAll / useVoiceControls / session
 *   store），弹窗（ShareModal/DelegateDialog）渲染仍在 ChatPanel；
 *   头部 ✕ 恒渲染（两形态的收起出口）；
 * - 执行态置顶大卡（仅 status==='ok' 渲染；空闲 = Empty 整卡隐藏）；
 *   运行中卡头 ⏱ 当前轮实时用时徽标（锚 turn_started_ms，2026-11-24）；
 * - 在途区：待审批 / 待提问（只展示清单——审批交互走既有弹窗链路，
 *   看板红线：绝不内联审批按钮）；
 * - 资源卡流：其余类目按 order 排序；空类目收「无资源」徽标行（可展开）；
 *   Unavailable 单独 ⚠ 行（reason 诚实透传）；
 * - CTA 矩阵：durable_loops 空态「+ 设置目标」内联表单（loops.create，
 *   支持 cron / delay_secs 初始排程）；hooks_global→/hooks 跳转；
 *   cron→/tasks 跳转；session_allow 卡内开关（useSessionAllowAll）。
 *
 * B4 活态刷新：
 * - WS push 帧 `tool_event`（TodoPanel 同款会话过滤：session_id 优先 /
 *   chat_id `web:{sid}` 兜底）→ debounce 重拉；
 * - SSE approval/question 四事件（载荷顶层 session_key）→ debounce 重拉；
 * - 乱序丢弃：应用层 revision 比较（SSE handler 拿不到 EventHub seq，
 *   只能比响应里的 revision >= lastApplied）；
 * - 30s 兜底轮询（仅面板展开期间挂表；收起/卸载即停）；
 * - 会话切换：清态 + revision 归零 + 重拉（回包校验还是当前会话，
 *   TodoPanel 同款防晚到回包串台）。
 *
 * 挂载面：ChatView 壳（宽屏右栏）；会话寻址锚定 props.sessionId，
 * 绝不直引全局 currentId（TodoPanel 2026-09-24 串扰修复同款纪律）。
 */
import { computed, onUnmounted, ref, watch } from 'vue'
import { useRouter } from 'vue-router'
import { addMessageHandler, removeMessageHandler, wsStatus } from '../../composables/useWebSocket'
import { useWSAPI } from '../../composables/useWSAPI'
import { on as sseOn, off as sseOff } from '../../composables/useSSE'
import { useSessionAllowAll } from '../../composables/useSessionAllowAll'
// 会话工具分节（2026-11-19 拆分迁移）：分享/委派/思考强度状态真值单例
// （原 ChatPanel 输入框工具栏三键迁入；弹窗渲染仍在 ChatPanel）。
import { useComposerTools } from '../../composables/useComposerTools'
// 九键全迁（2026-11-19）：放行两开关（useEditorMode）/语音四键
// （useVoiceControls）/模式与输入（chat store）/插队显隐（inbox 快照）。
import { useEditorMode } from '../../composables/useEditorMode'
import { useVoiceControls } from '../../composables/useVoiceControls'
import { useInboxStatus } from '../../composables/useInboxStatus'
import { useChatStore } from '../../stores/chat'
import { useToast } from '../../composables/useToast'
import ProjectCreateModal from '../ProjectCreateModal.vue'

const props = defineProps<{
  /** 壳层钉死的会话 id（ChatView → ChatPanel.effectiveSid 同域）；null =
   * 尚未就绪，不拉取不监听渲染。本组件全部取数的唯一会话域。 */
  sessionId?: string | null
  /** overlay 抽屉形态（窄窗口悬浮覆盖右侧）：壳层据此决定是否挂遮罩；
   * 关闭钮恒渲染（两形态都需要收起出路），关闭事件一律上抛壳层。 */
  overlay?: boolean
}>()

const emit = defineEmits<{ (e: 'close'): void }>()

const router = useRouter()
const { request } = useWSAPI()
const { allowAllBySession, sessionAllowAvailable, refreshSessionAllow, setSessionAllow } =
  useSessionAllowAll()
const { sessionEffort, effortMarkFor, effortNoteFor, showShare, showDelegate, setSessionEffort } =
  useComposerTools()

/** 思考强度下拉切换（接值解包；无会话 id 时 select 已禁用不会触发）。 */
function onEffortChange(e: Event) {
  void setSessionEffort(props.sessionId || '', (e.target as HTMLSelectElement).value)
}

// ---- 九键全迁（2026-11-19）：构建/计划 + 插队 + 放行三开关 + 语音四键 ----
// ChatPanel 输入框上方工具栏在有资源窗注入面（ChatView provide）时不再
// 渲染——这些控件的家在本面板。状态真值：agentMode/input 在 chat store
//（与 ChatPanel 同一份 Pinia store——模式徽标/插队前缀两边即时互见），
// 放行两开关在 useEditorMode 单例，全部放行在 useSessionAllowAll 单例，
// 语音在 useVoiceControls 单例（ChatPanel 的 voice strip 同源亮）。

const chatStore = useChatStore()
const toast = useToast()
const { fullAccess, externalWrite, editorAvailable, setEditorAccess } = useEditorMode()
const {
  sttReady,
  ttsReady,
  voiceDictation,
  voiceDialogue,
  voicePlayback,
  voiceRealtime,
  initVoiceState,
  toggleDictation,
  toggleDialogue,
  togglePlayback,
  toggleRealtime,
} = useVoiceControls()

// steer 可用性（插队键显隐）：面板侧独立 useInboxStatus 实例但只刷不轮询
//（模式快照非低延迟关键；挂载/会话切换各刷一次。ChatPanel 自己的实例
// 照常轮询，两边只读同一后端真相，无状态同步问题）。
const {
  steerEnabled: panelSteerEnabled,
  queueEnabled: panelQueueEnabled,
  refresh: refreshInboxMode,
} = useInboxStatus()

/** 构建/计划模式切换（ChatPanel toggleAgentMode 同款：await 回包后更新，
 *  失败 toast 不翻转）。 */
async function toggleAgentMode() {
  const target = chatStore.agentMode === 'plan' ? 'build' : 'plan'
  try {
    const data = await request('chat', 'set_mode', {
      session_id: props.sessionId || '',
      mode: target,
    })
    if (data?.mode === 'plan' || data?.mode === 'build') chatStore.setAgentMode(data.mode)
  } catch (e) {
    toast.error(String((e as Error)?.message ?? e))
  }
}

/** 一键插队：给输入加 `!` 前缀（已有前缀则不动）。输入框本体在 ChatPanel
 *  ——面板只改共享 store；不抢焦点（用户下一步本来要回输入框）。 */
function prefixSteer() {
  if (!/^[!！]/.test(chatStore.input.trimStart())) {
    chatStore.input = '! ' + chatStore.input
  }
}

// ---- 发送方式卡（2026-11-24 三键迁移）：立即/插队/排队三键 ----
// 状态真值 chat store sendMode（与 ChatPanel 发送链 applySendMode 同一份
// ——此处切换即时生效，无请求、纯前端路由偏好；输入框旁小徽标同步互见）。
// 「立即」忙碌时字面降级与 ChatPanel nowLabel/nowTitle 同语义（BUG-1 同源
// 修复口径）；busy 读 chat store busyBySid 实时（非轮询）。

const panelBusy = computed(() => !!props.sessionId && chatStore.isBusy(props.sessionId))
const panelNowLabel = computed(() =>
  panelBusy.value ? (panelSteerEnabled.value ? '立即→插队' : '立即→排队') : '立即',
)
const panelNowTitle = computed(() =>
  !panelBusy.value
    ? '立即发送（默认）：agent 忙碌时自动插队（steer 模式）或排队（queue 模式）'
    : panelSteerEnabled.value
      ? '忙碌中：本次发送将插队注入当前轮'
      : '忙碌中：当前为排队模式（后端未开插队通道），本次发送将排队，本轮结束后自动执行',
)
function setSendMode(mode: 'now' | 'steer' | 'queue') {
  chatStore.sendMode = mode
}

// Full Access 点击语义（ChatPanel 同款）：关 FA 必须随关 ext（服务端联动
// 会把 (false,true) 折回 full=true，前端显式双关才能真关掉）；开 ext 确保 full 开。
async function toggleFullAccess() {
  if (fullAccess.value) {
    await setEditorAccess(false, false)
  } else {
    await setEditorAccess(true, externalWrite.value)
  }
}
async function toggleExternalWrite() {
  // ext 依赖 full：按钮仅在 fullAccess 时可点（disabled），此处仍显式带 full。
  await setEditorAccess(true, !externalWrite.value)
}

/** 当前会话的全部放行态（未查询过 = false）。 */
const sessionAllowActive = computed(
  () => !!props.sessionId && (allowAllBySession.value[props.sessionId]?.enabled ?? false),
)
async function toggleSessionAllow() {
  await setSessionAllow(props.sessionId, !sessionAllowActive.value)
}

// ---- 类型（对应后端 resources.rs 投影）----

interface CategoryEntry {
  label: string
  order: number
  transient: boolean
  status: 'ok' | 'empty' | 'unavailable'
  reason?: string
  total: number
  summary?: Record<string, unknown>
}

interface ResourcesResponse {
  session_id: string
  session_key: string
  revision: number
  categories: Record<string, CategoryEntry>
}

// ---- 状态 ----

const resp = ref<ResourcesResponse | null>(null)
const lastRevision = ref(-1)
const loadError = ref('')
const workdir = ref('')
const showProjectModal = ref(false)
const showEmptyDetail = ref(false)
/** 资源卡流折叠集（卡 id 集合；默认全展开，不持久化）。 */
const collapsedCards = ref(new Set<string>())

const sessionKey = computed(() =>
  props.sessionId ? `agent:main:session:${props.sessionId}` : '',
)

// ---- 类目分区（computed 派生）----

const categories = computed<Record<string, CategoryEntry>>(() => resp.value?.categories ?? {})

/** 执行态大卡：仅 ok 渲染（空闲 = Empty 隐藏整卡）。 */
const execution = computed(() => {
  const c = categories.value['execution']
  return c && c.status === 'ok' ? c : null
})

/** 执行态卡头计时徽标（2026-11-24）：当前轮实时用时——锚
 *  summary.turn.turn_started_ms（后端 TurnTimelineGuard acquire 时刻），
 *  nowTick 1s 走表驱动；running=false 无 turn 数据 → 空串不渲染。
 *  turn 结束整卡转空闲隐藏，计时随卡消失（语义 = 当前任务执行了多久）。 */
const executionElapsed = computed(() => {
  if (execution.value?.summary?.running !== true) return ''
  const turn = execution.value.summary.turn as Record<string, unknown> | undefined
  const start = typeof turn?.turn_started_ms === 'number' ? turn.turn_started_ms : Number.NaN
  return Number.isFinite(start) && start > 0 ? fmtElapsed(start, nowTick.value) : ''
})

/** 在途区：待审批/待提问（ok 且 total>0 才渲染）。 */
const pendingCategories = computed(() =>
  (['approvals_pending', 'questions_pending'] as const)
    .map(id => ({ id, c: categories.value[id] }))
    .filter(x => x.c && x.c.status === 'ok' && x.c.total > 0),
)

/** 等待队列气泡（2026-11-24）：ok 才渲染（无等待 = Empty 自动隐藏，与
 *  执行态同机制）。数据 = summary.steer / follow_up 逐条全文（后端单条
 *  8000 字上限截断）。插队组在上先执行、排队组在下，组内按后端队列序
 *  （High 优先级已在后端插到队首，展示序 = 真实执行序）。 */
interface WqRow {
  icon: string
  kind: string
  content: string
  timestamp: string
  priority: string
}

const waitingQueue = computed(() => {
  const c = categories.value['waiting_queue']
  return c && c.status === 'ok' ? c : null
})

const wqRows = computed<WqRow[]>(() => {
  const s = waitingQueue.value?.summary ?? {}
  const steer = Array.isArray(s.steer) ? (s.steer as Record<string, unknown>[]) : []
  const follow = Array.isArray(s.follow_up) ? (s.follow_up as Record<string, unknown>[]) : []
  const toRow = (e: Record<string, unknown>, icon: string, kind: string): WqRow => ({
    icon,
    kind,
    content: typeof e.content === 'string' ? e.content : '',
    timestamp: typeof e.timestamp === 'string' ? e.timestamp : '',
    priority: typeof e.priority === 'string' ? e.priority : 'normal',
  })
  return [
    ...steer.map(e => toRow(e, '⚡', '插队')),
    ...follow.map(e => toRow(e, '⏳', '排队')),
  ]
})

const wqSteerCount = computed(
  () => wqRows.value.filter(r => r.kind === '插队').length,
)
const wqFollowCount = computed(
  () => wqRows.value.filter(r => r.kind === '排队').length,
)

/** 行预览截断（全文在 hover 弹层；48 字符与既有 goal 行 80 字符口径同型）。 */
function wqPreview(text: string): string {
  return text.length > 48 ? `${text.slice(0, 48)}…` : text
}

/** hover 弹层状态：fixed 定位（逃出 srp-card overflow:hidden 与面板
 *  overflow-y:auto 的裁剪链——absolute 会被双层裁剪）。坐标取 mouseenter
 *  视口点，弹出偏移 + 视口右/下缘钳位。 */
const wqHover = ref<{ content: string; meta: string; x: number; y: number } | null>(null)

/** hover-intent：弹层 spawn 在光标旁 12px，指针移入弹层会触发行
 *  mouseleave——立即隐藏 = 弹层在眼前闪烁。行离开延迟 200ms 再藏，
 *  悬入弹层取消（指针可移进弹层选读全文）；弹层离开即时藏。 */
let wqHideTimer: ReturnType<typeof setTimeout> | null = null

function wqCancelHide(): void {
  if (wqHideTimer) {
    clearTimeout(wqHideTimer)
    wqHideTimer = null
  }
}

function wqScheduleHide(): void {
  wqCancelHide()
  wqHideTimer = setTimeout(() => {
    wqHover.value = null
    wqHideTimer = null
  }, 200)
}

function wqShow(e: WqRow, ev: MouseEvent): void {
  wqCancelHide()
  wqHover.value = {
    content: e.content,
    meta: `${e.kind} · ${e.timestamp || '—'}${e.priority === 'high' ? ' · 优先' : ''}`,
    x: ev.clientX,
    y: ev.clientY,
  }
}

const wqPopStyle = computed(() => {
  const h = wqHover.value
  if (!h) return {}
  return {
    left: `${Math.max(8, Math.min(h.x + 12, window.innerWidth - 380))}px`,
    top: `${Math.max(8, Math.min(h.y + 12, window.innerHeight - 240))}px`,
  }
})

/** 资源卡流：排除执行态/在途区/identity 后的 ok 类目，order 升序。
 *  session_allow 例外：empty（未开启=常态）也渲染——开关是「状态开关」
 *  不是资源清单，只在已开启态可见 = 用户没有从面板开启的入口（死锁）。 */
const PENDING_IDS = new Set(['approvals_pending', 'questions_pending'])
const cardCategories = computed(() =>
  Object.entries(categories.value)
    .filter(([id, c]) =>
      (c.status === 'ok' || id === 'session_allow')
      && id !== 'execution'
      && id !== 'identity'
      && id !== 'waiting_queue'
      && !PENDING_IDS.has(id))
    .filter(([, c]) => !(c.transient && c.total === 0))
    .sort((a, b) => a[1].order - b[1].order),
)

/** CTA 空态卡：cron/hooks_global 空态 = 最需要入口的时刻，单独渲染跳转
 *  小卡（否则只有「+ 设置」过后的 ok 态才有入口，空态反而没路可走）。 */
const ctaEmptyCategories = computed(() =>
  (['cron', 'hooks_global'] as const)
    .map(id => ({ id, c: categories.value[id] }))
    .filter(x => x.c && x.c.status === 'empty'),
)

/** 空类目徽标：empty 态集合（CTA 空态卡已单独渲染的除外；session_allow
 *  走开关卡，不进「无资源」徽标流）。 */
const emptyCategories = computed(() =>
  Object.entries(categories.value)
    .filter(([id, c]) => c.status === 'empty' && id !== 'cron' && id !== 'hooks_global' && id !== 'session_allow' && id !== 'waiting_queue')
    .sort((a, b) => a[1].order - b[1].order),
)

/** Unavailable ⚠ 行。 */
const unavailableCategories = computed(() =>
  Object.entries(categories.value)
    .filter(([, c]) => c.status === 'unavailable')
    .sort((a, b) => a[1].order - b[1].order),
)

// ---- identity 头部字段 ----

const identitySummary = computed<Record<string, unknown>>(() =>
  (categories.value['identity']?.summary ?? {}) as Record<string, unknown>,
)
const sessionTitle = computed(() =>
  typeof identitySummary.value.title === 'string' && identitySummary.value.title
    ? identitySummary.value.title
    : (props.sessionId ?? ''),
)
const projectPath = computed(() =>
  typeof identitySummary.value.project_path === 'string' ? identitySummary.value.project_path : '',
)

// ---- CTA：durable_loops 内联表单 ----

const loopsCard = computed(() => categories.value['durable_loops'])
const loopsSummary = computed<Record<string, unknown>>(() =>
  (loopsCard.value?.summary ?? {}) as Record<string, unknown>,
)
const loopsList = computed<Record<string, unknown>[]>(() => {
  const v = loopsSummary.value.loops
  return Array.isArray(v) ? (v as Record<string, unknown>[]) : []
})
const loopFormOpen = ref(false)
const loopGoal = ref('')
const loopCron = ref('')
const loopDelay = ref('')
const loopBusy = ref(false)
const loopError = ref('')

function loopsHasActive(): boolean {
  const v = loopsSummary.value.active
  return typeof v === 'number' && v > 0
}

async function submitLoop(): Promise<void> {
  const goal = loopGoal.value.trim()
  if (!goal || !props.sessionId) return
  const data: Record<string, unknown> = { goal, session_id: props.sessionId }
  const cron = loopCron.value.trim()
  const delay = Number(loopDelay.value.trim())
  if (cron) data.cron = cron
  else if (Number.isFinite(delay) && delay > 0) data.delay_secs = delay
  loopBusy.value = true
  loopError.value = ''
  try {
    await request('loops', 'create', data)
    loopFormOpen.value = false
    loopGoal.value = ''
    loopCron.value = ''
    loopDelay.value = ''
    void fetchResources()
  } catch (e) {
    loopError.value = String(e)
  } finally {
    loopBusy.value = false
  }
}

// ---- CTA：session_allow 卡内开关 ----

const allowSummary = computed<Record<string, unknown>>(() =>
  (categories.value['session_allow']?.summary ?? {}) as Record<string, unknown>,
)
const allowEnabled = computed(() => allowSummary.value.enabled === true)
const allowBusy = ref(false)

async function toggleAllow(): Promise<void> {
  if (!props.sessionId || allowBusy.value) return
  allowBusy.value = true
  try {
    await setSessionAllow(props.sessionId, !allowEnabled.value)
    void fetchResources()
  } finally {
    allowBusy.value = false
  }
}

// ---- CTA：跳转 ----

function goHooks(): void { void router.push('/hooks') }
function goTasks(): void { void router.push('/tasks') }

// ---- 拉取（B4 乱序丢弃 + 回包会话校验）----

// 竞态修复（2026-11-19）：面板可能在 WS 就绪前挂载——request() 把消息发进
// 未开的 socket 会丢（30s 超时才 reject），面板「加载中…」挂半分钟。修法
// 三件套：① ws 未连上时不发（门），连接建立后由 wsStatus watch 触发首拉；
// ② 空应答/失败走温和重试（400ms 步进、上限 5 次、成功归零——裁剪构建下
// session.resources 恒错也不空转，停在诚实错误行）。
let fetchRetries = 0
const FETCH_RETRY_MAX = 5

function scheduleFetchRetry(): void {
  if (fetchRetries >= FETCH_RETRY_MAX) return
  fetchRetries++
  scheduleRefresh()
}

async function fetchResources(): Promise<void> {
  const sid = props.sessionId
  if (!sid) return
  if (wsStatus.value !== 'connected') return
  try {
    const data = await request('session', 'resources', { session_id: sid })
    if (!data) { scheduleFetchRetry(); return }
    if (props.sessionId !== sid) return
    if (typeof data.revision === 'number' && data.revision < lastRevision.value) return
    if (typeof data.revision === 'number') lastRevision.value = data.revision
    resp.value = data as ResourcesResponse
    loadError.value = ''
    fetchRetries = 0
  } catch (e) {
    // 资源盘点未装配（裁剪构建）/网络断 = 诚实错误行 + 上限内温和重试。
    loadError.value = String(e)
    scheduleFetchRetry()
  }
}

/** 拉工作目录（挂载一次；失败静默——头部只少一行）。 */
async function fetchWorkdir(): Promise<void> {
  try {
    const data = await request('system', 'status')
    if (data && typeof data.workspace === 'string') workdir.value = data.workspace
  } catch { /* 静默 */ }
}

// ---- B4 刷新触发器 ----

let refreshTimer: ReturnType<typeof setTimeout> | null = null
function scheduleRefresh(): void {
  if (refreshTimer) clearTimeout(refreshTimer)
  refreshTimer = setTimeout(() => { refreshTimer = null; void fetchResources() }, 400)
}

/** WS push 帧 → tool_event 全 kind（工具启停/todo/roundtext 等都可能改资源面）。 */
function onWsFrame(frame: unknown): void {
  const f = frame as Record<string, unknown> | null
  if (f?.type !== 'push' || f?.cmd !== 'tool_event') return
  const ev = (f.data ?? {}) as Record<string, unknown>
  const payload = (ev.data ?? {}) as Record<string, unknown>
  // TodoPanel 同款过滤：session_id 优先（web pump 注入），chat_id 兜底。
  if (typeof payload.session_id === 'string' && payload.session_id.length > 0) {
    if (payload.session_id !== props.sessionId) return
  } else if (payload.chat_id !== `web:${props.sessionId}`) {
    return
  }
  scheduleRefresh()
}
addMessageHandler(onWsFrame)

/** SSE approval/question 四事件：载荷顶层 session_key 匹配本会话。 */
function sseMatches(data: unknown): boolean {
  const d = data as Record<string, unknown> | null
  return typeof d?.session_key === 'string' && d.session_key === sessionKey.value && sessionKey.value !== ''
}
function onApprovalReq(d: unknown): void { if (sseMatches(d)) scheduleRefresh() }
function onApprovalRes(d: unknown): void { if (sseMatches(d)) scheduleRefresh() }
function onQuestionAsk(d: unknown): void { if (sseMatches(d)) scheduleRefresh() }
function onQuestionRes(d: unknown): void { if (sseMatches(d)) scheduleRefresh() }
sseOn('approval-requested', onApprovalReq)
sseOn('approval-resolved', onApprovalRes)
sseOn('question-asked', onQuestionAsk)
sseOn('question-resolved', onQuestionRes)

/** revision 乱序丢弃的代际语义（2026-11-19 修「钩子设了面板永远不显示」）：
 *  revision = EventHub 全局 seq，**每个 gateway 进程从 0 重新计数**。
 *  面板开着跨了一次 gateway 重启 → lastRevision 残留旧进程高位 → 重启后
 *  所有新回包 revision 更小 → 被当乱序旧包永久丢弃 → 面板冻结在重启前
 *  的数据（钩子/待审批等一切变化都不可见，直到切会话或刷新页面）。
 *  重连（WS connected）与 SSE resync（L2 缺口提示）都是「可能换了进程
 *  代」的信号——归零高水位再拉，让下一包无条件生效。 */
function resetRevisionGuard(): void {
  lastRevision.value = -1
}
function onSseResync(): void {
  resetRevisionGuard()
  void fetchResources()
}
sseOn('resync', onSseResync)

/** 30s 兜底轮询（组件挂载期生效；壳层收起态 v-if 卸载本组件 = 自动停）。 */
let pollTimer: ReturnType<typeof setInterval> | null = null
pollTimer = setInterval(() => { void fetchResources() }, 30000)

/** C1：running 子代理「实时用时」驱动钟（1s 粒度；卸载即停）。 */
const nowTick = ref(Date.now())
const tickTimer: ReturnType<typeof setInterval> = setInterval(() => { nowTick.value = Date.now() }, 1000)

/** 会话切换：清态 + revision 归零 + 重拉（watch immediate 首拉；工作目录
 *  同拍刷新——跨实例切换后 workspace 可能不同）。九键迁入的对齐项也挂这
 *  里：语音引擎位（单例幂等，ChatPanel WS ready 时也会调）、插队模式
 *  快照、会话全部放行态（容错静默）。 */
watch(() => props.sessionId, (sid) => {
  resp.value = null
  lastRevision.value = -1
  loadError.value = ''
  workdir.value = ''
  collapsedCards.value = new Set()
  if (sid) {
    void fetchResources()
    void fetchWorkdir()
    void initVoiceState()
    void refreshInboxMode(sid)
    void refreshSessionAllow(sid)
  }
}, { immediate: true })

// 连接建立（从非 connected → connected 的转移）→ 首拉/重拉。挂载时已连接
// 的场景由 sessionId watch（immediate）覆盖，这里不重复 immediate（避免
// 挂载双发）；门在 fetch 里，这里只管触发时机。
watch(wsStatus, (val) => {
  if (val === 'connected') {
    resetRevisionGuard()
    void fetchResources()
  }
})

onUnmounted(() => {
  removeMessageHandler(onWsFrame)
  sseOff('approval-requested', onApprovalReq)
  sseOff('approval-resolved', onApprovalRes)
  sseOff('question-asked', onQuestionAsk)
  sseOff('question-resolved', onQuestionRes)
  sseOff('resync', onSseResync)
  if (refreshTimer) clearTimeout(refreshTimer)
  if (pollTimer) clearInterval(pollTimer)
  clearInterval(tickTimer)
})

// ---- 渲染辅助 ----

function toggleCard(id: string): void {
  const next = new Set(collapsedCards.value)
  if (next.has(id)) next.delete(id)
  else next.add(id)
  collapsedCards.value = next
}

function truncPath(p: unknown, n: number): string {
  const s = typeof p === 'string' ? p : ''
  return s.length > n ? `…${s.slice(-n)}` : s
}

function fmtCost(v: unknown): string {
  const n = typeof v === 'number' ? v : Number(v)
  return Number.isFinite(n) ? `$${n.toFixed(4)}` : '—'
}

function fmtNum(v: unknown): string {
  const n = typeof v === 'number' ? v : Number(v)
  return Number.isFinite(n) ? String(Math.max(0, Math.round(n))) : '—'
}

function hookEvents(summary: Record<string, unknown>): string[] {
  const by = summary.by_event
  if (!by || typeof by !== 'object') return []
  return Object.keys(by as Record<string, unknown>)
}

function cronJobs(summary: Record<string, unknown>): Record<string, unknown>[] {
  const v = summary.jobs
  return Array.isArray(v) ? (v as Record<string, unknown>[]) : []
}

function genericRows(entry: CategoryEntry): Record<string, unknown>[] {
  const v = entry.summary?.rows
  return Array.isArray(v) ? (v as Record<string, unknown>[]) : []
}

/** 会话钩子明细行（summary.rows：event/kind/content/matcher/timeout）。 */
function hookRows(summary: Record<string, unknown>): Record<string, unknown>[] {
  return genericRows({ summary } as CategoryEntry)
}

/** 钩子类型中文标（command=命令 / prompt=提示词）。 */
function hookKindLabel(row: Record<string, unknown>): string {
  return row.kind === 'prompt' ? '提示词' : '命令'
}

/** 钩子行悬浮详情（matcher/timeout 后端给了才显示）。 */
function hookRowTitle(row: Record<string, unknown>): string {
  const parts = [`${String(row.event ?? '')}·${hookKindLabel(row)}`]
  if (typeof row.matcher === 'string' && row.matcher) parts.push(`matcher=${row.matcher}`)
  if (typeof row.timeout === 'number') parts.push(`timeout=${row.timeout}s`)
  return parts.join(' / ')
}

/** 头部截断（truncPath 是尾截、留给路径；钩子内容是自然语句用头截）。 */
function truncHead(s: unknown, n: number): string {
  const v = typeof s === 'string' ? s : ''
  return v.length > n ? `${v.slice(0, n)}…` : v
}

/** 审批/提问行的展示文本（pending() 行字段开放集——优先 name/description/
 *  pattern，回退紧凑 JSON）。 */
function pendingRowText(row: Record<string, unknown>): string {
  for (const k of ['description', 'pattern', 'tool', 'question', 'title', 'kind']) {
    const v = row[k]
    if (typeof v === 'string' && v) return v.length > 120 ? `${v.slice(0, 120)}…` : v
  }
  const keys = Object.keys(row).filter(k => k !== 'session_key')
  return keys.map(k => `${k}=${String(row[k]).slice(0, 40)}`).join(' ').slice(0, 160)
}

// ---- C1：subagents 专用渲染（严禁 raw 字段 dump——不走 pendingRowText）----

/** 后端 SubagentStatus 是 snake_case（running/done/failed/cancelled/interrupted）。 */
function subagentIsRunning(row: Record<string, unknown>): boolean {
  return row.status === 'running'
}

function subagentIcon(row: Record<string, unknown>): string {
  if (subagentIsRunning(row)) return '⏳'
  return row.status === 'done' ? '✅' : '❌'
}

function subagentTaskText(row: Record<string, unknown>): string {
  const t = typeof row.task === 'string' ? row.task.trim() : ''
  if (!t) return '（无任务描述）'
  return t.length > 80 ? `${t.slice(0, 80)}…` : t
}

/** 计时格式单一真相源（子代理行 + 执行态卡头共用）：epoch ms 起锚 +
 *  当前时刻 → `5s` / `2m05s` / `1h03m`。 */
function fmtElapsed(startMs: number, now: number): string {
  const secs = Math.max(0, Math.floor((now - startMs) / 1000))
  const m = Math.floor(secs / 60)
  const s = secs % 60
  if (m >= 60) return `${Math.floor(m / 60)}h${String(m % 60).padStart(2, '0')}m`
  return m > 0 ? `${m}m${String(s).padStart(2, '0')}s` : `${s}s`
}

/** running 行实时用时（锚 started_at_ms，nowTick 驱动）。终态行不走这。 */
function subagentElapsed(row: Record<string, unknown>): string {
  const start = typeof row.started_at_ms === 'number' ? row.started_at_ms : Number.NaN
  return Number.isFinite(start) && start > 0 ? fmtElapsed(start, nowTick.value) : ''
}
</script>

<template>
  <div class="srp-root">
    <!-- 头部：identity 常驻区（#7 面板级 ✕ 住 header 行，2026-11-19——
         关闭是面板级动作，不属于任何业务卡；与 ＋项目 同排最右）。 -->
    <div class="srp-header">
      <div class="srp-header-row">
        <span class="srp-title" :title="sessionTitle">{{ sessionTitle }}</span>
        <span class="srp-header-actions">
          <button class="srp-add-project" type="button" title="添加项目目录" @click="showProjectModal = true">＋项目</button>
          <button
            class="srp-tools-close"
            type="button"
            title="收起资源窗"
            @click="emit('close')"
          >✕</button>
        </span>
      </div>
      <div v-if="projectPath" class="srp-sub" :title="projectPath">项目：{{ truncPath(projectPath, 32) }}</div>
      <div v-if="workdir" class="srp-sub" :title="workdir">目录：{{ truncPath(workdir, 36) }}</div>
    </div>

    <!-- 会话工具分节（2026-11-19 拆分迁移）：分享/委派/思考强度三键从
         ChatPanel 输入框工具栏迁入（低频配置键不再占输入近邻）。状态真值
         走 useComposerTools 单例；弹窗（ShareModal/DelegateDialog）渲染仍
         在 ChatPanel——此处只置触发器 ref。思考强度 select 与原 G3 下拉
         同语义（会话级 override，乐观更新失败回滚在单例内）。 -->
    <div class="srp-card srp-tools">
      <div class="srp-card-head">
        <span class="srp-card-title">会话工具</span>
      </div>
      <div class="srp-tools-row">
        <button
          class="srp-tool-btn"
          :class="{ active: chatStore.agentMode === 'plan' }"
          :title="chatStore.agentMode === 'plan'
            ? '计划模式：文件修改类工具已停用（plans/ 目录写入放行）。点击切回构建模式（/build）'
            : '构建模式：全量工具。点击切换到计划模式（/plan），AI 只读代码出计划'"
          @click="toggleAgentMode"
        >{{ chatStore.agentMode === 'plan' ? '📋 计划' : '🛠 构建' }}</button>
        <button
          v-if="panelSteerEnabled"
          class="srp-tool-btn"
          title="一键插队：给输入加 ! 前缀，agent 忙碌时立即送达当前轮"
          @click="prefixSteer"
        >⚡ 插队</button>
        <button
          class="srp-tool-btn"
          title="分享本会话（只读链接，token 即凭据，可随时撤销）"
          :disabled="!props.sessionId"
          @click="showShare = true"
        >🔗 分享</button>
        <button
          class="srp-tool-btn"
          title="委派任务给子代理（选角色 + 工具档位，完成后回复交回本会话）"
          :disabled="!props.sessionId"
          @click="showDelegate = true"
        >🤝 委派</button>
        <select
          class="srp-tool-btn srp-effort-select"
          :value="sessionEffort"
          :disabled="!props.sessionId"
          title="思考强度（会话级，仅本会话生效）：默认=跟随模型配置；off=关闭思考（省 token）；极简/低/中/高/极高/最大=思考预算档位。✓=端点已验证接受，✗=端点拒绝（悬停看原文），?=未探测。重启后回落默认"
          @change="onEffortChange"
        >
          <option value="">思考·默认</option>
          <option value="off">思考·关</option>
          <option :title="effortNoteFor('minimal') || undefined" value="minimal">思考·极简{{ effortMarkFor('minimal') }}</option>
          <option :title="effortNoteFor('low') || undefined" value="low">思考·低{{ effortMarkFor('low') }}</option>
          <option :title="effortNoteFor('medium') || undefined" value="medium">思考·中{{ effortMarkFor('medium') }}</option>
          <option :title="effortNoteFor('high') || undefined" value="high">思考·高{{ effortMarkFor('high') }}</option>
          <option :title="effortNoteFor('xhigh') || undefined" value="xhigh">思考·极高{{ effortMarkFor('xhigh') }}</option>
          <option :title="effortNoteFor('max') || undefined" value="max">思考·最大{{ effortMarkFor('max') }}</option>
        </select>
      </div>
    </div>

    <!-- 放行开关分节（2026-11-19 九键全迁）：Full Access / 外部写删 / 全部
         放行三键从 ChatPanel 工具栏迁入。激活态输入框下方常驻条照常显示
         （editor-strip / session-allow-strip）——安全状态可见性不依赖工具栏。 -->
    <div class="srp-card srp-tools">
      <div class="srp-card-head">
        <span class="srp-card-title">放行开关</span>
      </div>
      <div class="srp-tools-row">
        <button
          class="srp-tool-btn"
          :class="{ active: fullAccess }"
          :disabled="!editorAvailable"
          title="Full Access（运行时开关，重启失效）：项目目录内文件操作全放行；项目目录外读/执行/网络/系统放行；项目外写删仍需审批；自毁硬拦不被绕过"
          @click="toggleFullAccess"
        >⚡ Full Access</button>
        <button
          class="srp-tool-btn"
          :class="{ active: externalWrite }"
          :disabled="!fullAccess"
          title="外部写删放行（依赖 Full Access）：项目目录外的写入/删除也放行。两开关全开 = 真·全放（真沙盒仍兜底）"
          @click="toggleExternalWrite"
        >外部写删</button>
        <button
          class="srp-tool-btn"
          :class="{ active: sessionAllowActive }"
          :disabled="!sessionAllowAvailable || !props.sessionId"
          title="会话全部放行（本会话持久）：本会话所有安全审批自动放行（含高危）；硬拒绝与急停不受影响；长任务无人值守用，用完记得关"
          @click="toggleSessionAllow"
        >🔓 全部放行</button>
      </div>
    </div>

    <!-- 语音分节（2026-11-19 九键全迁）：听写/语音对话/语音播放/实时语音
         四键从 ChatPanel 工具栏迁入；状态真值 useVoiceControls 单例（与
         ChatPanel 共享——激活时输入框下方 voice strip 同步亮）。 -->
    <div class="srp-card srp-tools">
      <div class="srp-card-head">
        <span class="srp-card-title">语音</span>
      </div>
      <div class="srp-tools-row">
        <button
          class="srp-tool-btn"
          :class="{ active: voiceDictation }"
          :disabled="!sttReady"
          :title="sttReady ? '听写：说话内容追加到输入框' : '请先在语音通道页启用 STT 引擎'"
          @click="toggleDictation"
        >🎙 听写</button>
        <button
          class="srp-tool-btn"
          :class="{ active: voiceDialogue }"
          :disabled="!sttReady"
          :title="sttReady ? '语音对话：说话后自动发送给 AI' : '请先在语音通道页启用 STT 引擎'"
          @click="toggleDialogue(props.sessionId || undefined)"
        >🗣 语音对话</button>
        <button
          class="srp-tool-btn"
          :class="{ active: voicePlayback }"
          :disabled="!ttsReady"
          :title="ttsReady ? '语音播放：AI 回复自动朗读' : '请先在语音通道页启用 TTS 引擎'"
          @click="togglePlayback"
        >🔊 语音播放</button>
        <button
          class="srp-tool-btn"
          :class="{ active: voiceRealtime }"
          :disabled="!sttReady || !ttsReady"
          :title="sttReady && ttsReady ? '实时语音：语音对话中后端接力播放回复（TTS）、说话即打断（需 AEC）。流式识别/口播清洗等子开关见语音通道页「实时语音」小节，热生效无需重启' : '请先在语音通道页启用语音识别与语音合成引擎（实时语音依赖 STT + TTS 双引擎）'"
          @click="toggleRealtime"
        >⚡ 实时语音</button>
      </div>
    </div>

    <!-- 发送方式分节（2026-11-24 三键迁移）：立即/插队/排队三键从 ChatPanel
         输入框旁迁入（用户拍板位置：语音卡下、执行态卡上）。状态真值 chat
         store sendMode；ChatPanel 输入框旁选择器在有资源窗注入面时不再渲染
         （standalone chat 页保留原 UI），资源窗收起后非默认方式以只读小徽标
         留守输入框旁。卡片渲染门 queueEnabled（reject 模式 busy 发送直接被拒，
         三键无意义——与 ChatPanel 选择器同门）；「立即」忙碌时字面降级同
         ChatPanel nowLabel/nowTitle（BUG-1 同源口径）。 -->
    <div v-if="panelQueueEnabled" class="srp-card srp-tools">
      <div class="srp-card-head">
        <span class="srp-card-title">发送方式</span>
      </div>
      <div class="srp-tools-row">
        <button
          class="srp-tool-btn"
          :class="{ active: chatStore.sendMode === 'now' }"
          type="button"
          :title="panelNowTitle"
          @click="setSendMode('now')"
        >{{ panelNowLabel }}</button>
        <button
          v-if="panelSteerEnabled"
          class="srp-tool-btn"
          :class="{ active: chatStore.sendMode === 'steer' }"
          type="button"
          title="插队（steer）：发送时自动加 ! 前缀，agent 处理中时立即送达当前轮（工具间隙即中断剩余工具）"
          @click="setSendMode('steer')"
        >⚡插队</button>
        <button
          class="srp-tool-btn"
          :class="{ active: chatStore.sendMode === 'queue' }"
          type="button"
          title="排队（followUp）：发送时剥掉 ! 标记，agent 处理中时在本轮结束后处理"
          @click="setSendMode('queue')"
        >⏭排队</button>
      </div>
    </div>

    <div v-if="loadError" class="srp-error">{{ loadError }}</div>

    <div v-if="!resp && !loadError" class="srp-empty-hint">加载中…</div>

    <template v-if="resp">
      <!-- 执行态置顶大卡 -->
      <div v-if="execution" class="srp-card srp-exec">
        <div class="srp-card-head">
          <span class="srp-dot" :class="{ running: execution.summary?.running === true }" />
          <span class="srp-card-title">{{ execution.label }}</span>
          <!-- ⏱ 当前轮实时用时（running 且 turn_started_ms 有效才渲染；1s 走表）。 -->
          <span v-if="executionElapsed" class="srp-badge srp-exec-elapsed">⏱ {{ executionElapsed }}</span>
          <span v-if="execution.summary?.running !== true" class="srp-badge">空闲</span>
          <span v-if="typeof execution.summary?.steer_pending === 'number' && execution.summary.steer_pending > 0" class="srp-badge">插队 {{ execution.summary.steer_pending }}</span>
          <span v-if="typeof execution.summary?.followup_pending === 'number' && execution.summary.followup_pending > 0" class="srp-badge">追问 {{ execution.summary.followup_pending }}</span>
        </div>
        <div v-if="execution.summary?.running === true" class="srp-exec-tool">
          <template v-if="execution.summary?.turn && typeof execution.summary.turn === 'object'">
            {{ (execution.summary.turn as Record<string, unknown>).current_tool
              ? `执行中：${((execution.summary.turn as Record<string, unknown>).current_tool as Record<string, unknown>).name}`
              : '执行中：思考中' }}
          </template>
        </div>
      </div>

      <!-- 等待队列（2026-11-24）：插队/排队消息逐条可见，行 hover 弹层看
           全文（fixed 定位逃裁剪链）；无等待 = Empty 不渲染，与执行态同机制 -->
      <div v-if="waitingQueue" class="srp-card srp-queue">
        <div class="srp-card-head">
          <span class="srp-card-title">{{ waitingQueue.label }}</span>
          <span v-if="wqSteerCount" class="srp-badge">插队 {{ wqSteerCount }}</span>
          <span v-if="wqFollowCount" class="srp-badge">排队 {{ wqFollowCount }}</span>
        </div>
        <ul class="srp-rows">
          <li
            v-for="(e, i) in wqRows"
            :key="i"
            class="srp-row srp-wq-row"
            @mouseenter="wqShow(e, $event)"
            @mouseleave="wqScheduleHide"
          >
            <span class="srp-key">{{ e.icon }}</span>
            <span class="srp-wq-preview">{{ wqPreview(e.content) }}</span>
          </li>
        </ul>
      </div>

      <!-- 在途区（审批/提问只展示，交互走弹窗） -->
      <div v-for="p in pendingCategories" :key="p.id" class="srp-card srp-pending">
        <div class="srp-card-head">
          <span class="srp-card-title">{{ p.c!.label }}</span>
          <span class="srp-badge warn">{{ p.c!.total }}</span>
        </div>
        <ul class="srp-rows">
          <li v-for="(row, i) in genericRows(p.c!)" :key="i" class="srp-row">{{ pendingRowText(row) }}</li>
          <li v-if="p.c!.summary?.omitted" class="srp-row muted">…还有 {{ p.c!.summary.omitted }} 项</li>
        </ul>
      </div>

      <!-- 资源卡流 -->
      <div
        v-for="[id, c] in cardCategories"
        :key="id"
        class="srp-card"
        :class="{ 'srp-collapsed': collapsedCards.has(id) }"
      >
        <button class="srp-card-head clickable" type="button" @click="toggleCard(id)">
          <span class="srp-card-title">{{ c.label }}</span>
          <span v-if="id === 'subagents'" class="srp-badge">运行中 {{ fmtNum((c.summary ?? {}).running) }} / 历史 {{ fmtNum((c.summary ?? {}).finished_total) }}</span>
          <span v-else class="srp-badge">{{ c.total }}</span>
          <span class="srp-caret">{{ collapsedCards.has(id) ? '▸' : '▾' }}</span>
        </button>
        <div v-if="!collapsedCards.has(id)" class="srp-card-body">
          <!-- durable_loops：loop 行 + 空态 CTA -->
          <template v-if="id === 'durable_loops'">
            <ul v-if="loopsList.length" class="srp-rows">
              <li v-for="(lp, i) in loopsList" :key="i" class="srp-row">
                <span class="srp-key">{{ typeof lp.status === 'string' ? lp.status : '?' }}</span>
                <span>{{ typeof lp.goal === 'string' ? (lp.goal.length > 80 ? `${lp.goal.slice(0, 80)}…` : lp.goal) : '' }}</span>
              </li>
            </ul>
            <button v-if="!loopFormOpen && !loopsHasActive()" class="srp-cta" type="button" @click="loopFormOpen = true">＋ 设置目标</button>
            <div v-if="loopFormOpen" class="srp-form">
              <input v-model="loopGoal" class="srp-input" placeholder="目标（例：盯着 CI 直到全绿）" @keydown.enter="submitLoop">
              <input v-model="loopCron" class="srp-input" placeholder="cron（可选，例：0 9 * * *）">
              <input v-model="loopDelay" class="srp-input" placeholder="延迟秒（可选，例：300）">
              <div v-if="loopError" class="srp-sub err">{{ loopError }}</div>
              <div class="srp-form-row">
                <button class="srp-cta" type="button" :disabled="loopBusy || !loopGoal.trim()" @click="submitLoop">创建</button>
                <button class="srp-cta ghost" type="button" @click="loopFormOpen = false">取消</button>
              </div>
            </div>
          </template>

          <!-- cron：job 行 + 空态 CTA -->
          <template v-else-if="id === 'cron'">
            <ul v-if="cronJobs(c.summary ?? {}).length" class="srp-rows">
              <li v-for="(job, i) in cronJobs(c.summary ?? {})" :key="i" class="srp-row">
                <span class="srp-key">{{ job.enabled === true ? '启用' : '停用' }}</span>
                <span :title="typeof (job.schedule as Record<string, unknown>)?.description === 'string' ? String((job.schedule as Record<string, unknown>).description) : ''">{{ typeof job.name === 'string' ? job.name : '' }}</span>
              </li>
            </ul>
            <button class="srp-cta" type="button" @click="goTasks">前往定时任务设置</button>
          </template>

          <!-- hooks_global：事件计数 + CTA -->
          <template v-else-if="id === 'hooks_global'">
            <div v-if="hookEvents(c.summary ?? {}).length" class="srp-chips">
              <span v-for="ev in hookEvents(c.summary ?? {})" :key="ev" class="srp-chip">{{ ev }}</span>
            </div>
            <button class="srp-cta" type="button" @click="goHooks">前往 Hooks 设置</button>
          </template>

          <!-- session_allow：卡内开关 -->
          <template v-else-if="id === 'session_allow'">
            <label class="srp-switch-row">
              <span>全部放行（本会话审批自动通过）</span>
              <input
                type="checkbox"
                :checked="allowEnabled"
                :disabled="allowBusy"
                @change="toggleAllow"
              >
            </label>
          </template>

          <!-- usage：四指标行 -->
          <template v-else-if="id === 'usage'">
            <div class="srp-metrics">
              <span>请求 {{ fmtNum((c.summary ?? {}).requests) }}</span>
              <span>输入 {{ fmtNum((c.summary ?? {}).input_tokens) }}</span>
              <span>输出 {{ fmtNum((c.summary ?? {}).output_tokens) }}</span>
              <span>费用 {{ fmtCost((c.summary ?? {}).total_cost_usd) }}</span>
            </div>
          </template>

          <!-- hooks_session：计数 + 明细行（2026-11-19「只有个数不知道是啥」
               补齐：每行 事件·类型 + 内容预览，悬浮看全文/matcher/timeout） -->
          <template v-else-if="id === 'hooks_session'">
            <div class="srp-metrics"><span>本会话钩子 {{ c.total }} 个</span></div>
            <ul v-if="hookRows(c.summary ?? {}).length" class="srp-rows">
              <li v-for="(hk, i) in hookRows(c.summary ?? {})" :key="i" class="srp-row">
                <span class="srp-key">{{ String(hk.event ?? '') }}·{{ hookKindLabel(hk) }}</span>
                <span :title="hookRowTitle(hk)">{{ truncHead(hk.content, 48) }}</span>
              </li>
            </ul>
          </template>
          <template v-else-if="id === 'context'">
            <pre class="srp-pre">{{ JSON.stringify(c.summary, null, 1) }}</pre>
          </template>

          <!-- todos：三态计数 + 行 -->
          <template v-else-if="id === 'todos'">
            <div class="srp-metrics">
              <span>待办 {{ fmtNum((c.summary ?? {}).pending) }}</span>
              <span>进行 {{ fmtNum((c.summary ?? {}).in_progress) }}</span>
              <span>完成 {{ fmtNum((c.summary ?? {}).completed) }}</span>
            </div>
            <ul v-if="genericRows(c).length" class="srp-rows">
              <li v-for="(t, i) in genericRows(c)" :key="i" class="srp-row">
                <span class="srp-key">{{ t.status }}</span>
                <span>{{ t.content }}</span>
              </li>
            </ul>
          </template>

          <!-- subagents：专用渲染（C1：状态点 + ✅/❌ + summary/error + running
               实时用时；脱离 pendingRowText stringify——raw dump 禁令） -->
          <template v-else-if="id === 'subagents'">
            <ul v-if="genericRows(c).length" class="srp-rows">
              <li v-for="(row, i) in genericRows(c)" :key="i" class="srp-row subagent-row">
                <span class="srp-dot" :class="{ running: subagentIsRunning(row) }" />
                <span class="subagent-icon">{{ subagentIcon(row) }}</span>
                <span class="subagent-main">
                  <span class="subagent-task">{{ subagentTaskText(row) }}</span>
                  <span v-if="subagentIsRunning(row)" class="subagent-meta live">运行中 {{ subagentElapsed(row) }}</span>
                  <span v-else-if="typeof row.summary === 'string' && row.summary" class="subagent-meta ok">{{ row.summary }}</span>
                  <span v-else-if="typeof row.error === 'string' && row.error" class="subagent-meta err">{{ row.error }}</span>
                </span>
              </li>
              <li v-if="c.summary?.omitted" class="srp-row muted">…还有 {{ c.summary.omitted }} 条历史</li>
            </ul>
            <div v-else class="srp-metrics"><span>暂无子代理</span></div>
          </template>

          <!-- 通用：rows 行渲染（file_changes/checkpoint/background…） -->
          <template v-else>
            <ul v-if="genericRows(c).length" class="srp-rows">
              <li v-for="(row, i) in genericRows(c)" :key="i" class="srp-row">
                <template v-if="typeof row.path === 'string'">
                  <span class="srp-key">{{ typeof row.kind === 'string' ? row.kind : 'file' }}</span>
                  <span :title="row.path">{{ truncPath(row.path, 44) }}</span>
                </template>
                <template v-else-if="typeof row.goal === 'string'">
                  <span>{{ row.goal }}</span>
                </template>
                <template v-else>
                  <span>{{ pendingRowText(row) }}</span>
                </template>
              </li>
              <li v-if="c.summary?.omitted" class="srp-row muted">…还有 {{ c.summary.omitted }} 项</li>
            </ul>
          </template>
        </div>
      </div>

      <!-- CTA 空态小卡（cron/hooks_global 空态入口） -->
      <div v-for="x in ctaEmptyCategories" :key="`cta-${x.id}`" class="srp-card">
        <div class="srp-card-head">
          <span class="srp-card-title">{{ x.c!.label }}</span>
          <span class="srp-badge">0</span>
        </div>
        <div class="srp-card-body">
          <button class="srp-cta" type="button" @click="x.id === 'cron' ? goTasks() : goHooks()">前往{{ x.id === 'cron' ? '定时任务' : 'Hooks' }}设置</button>
        </div>
      </div>

      <!-- 空类目徽标行（可展开） -->
      <button
        v-if="emptyCategories.length"
        class="srp-empty-row"
        type="button"
        @click="showEmptyDetail = !showEmptyDetail"
      >
        无资源 {{ emptyCategories.length }} 项 <span class="srp-caret">{{ showEmptyDetail ? '▾' : '▸' }}</span>
      </button>
      <div v-if="showEmptyDetail" class="srp-chips srp-empty-detail">
        <span v-for="[id, c] in emptyCategories" :key="id" class="srp-chip">{{ c.label }}</span>
      </div>

      <!-- Unavailable ⚠ 行 -->
      <div v-for="[id, c] in unavailableCategories" :key="id" class="srp-unavail">⚠ {{ c.label }}：{{ c.reason ?? '不可用' }}</div>
    </template>

    <!-- 等待队列 hover 全文弹层（fixed 定位，挂根级逃出卡片/面板裁剪链；
         悬入取消行离开的延迟隐藏——指针可移进弹层选读全文） -->
    <div
      v-if="wqHover"
      class="srp-wq-pop"
      :style="wqPopStyle"
      @mouseenter="wqCancelHide"
      @mouseleave="wqHover = null"
    >
      <div class="srp-wq-pop-text">{{ wqHover.content }}</div>
      <div class="srp-wq-pop-meta">{{ wqHover.meta }}</div>
    </div>

    <ProjectCreateModal
      v-if="showProjectModal"
      @close="showProjectModal = false"
      @created="showProjectModal = false; void fetchResources()"
    />
  </div>
</template>

<style scoped>
.srp-root {
  display: flex;
  flex-direction: column;
  gap: 8px;
  height: 100%;
  min-height: 0;
  overflow-y: auto;
  padding: 10px 12px;
  background: var(--bg-secondary);
  font-size: 12px;
  color: var(--text);
}

/* #8 滚动条修复（2026-11-19）：子卡默认 flex-shrink:1 会被压扁到
   scrollHeight==clientHeight，overflow-y:auto 永不触发。全部直接子元素
   禁收缩 → 内容自然溢出 → 滚动条出现。 */
.srp-root > * {
  flex-shrink: 0;
}

/* 头部 */
.srp-header {
  border-bottom: 1px solid var(--border);
  padding-bottom: 8px;
}
.srp-header-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
}
.srp-title {
  font-weight: 600;
  color: var(--text-primary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.srp-header-actions {
  display: flex;
  align-items: center;
  gap: 6px;
  flex-shrink: 0;
}
.srp-add-project {
  flex-shrink: 0;
  border: 1px solid var(--border);
  background: var(--bg-elev);
  color: var(--text-muted);
  border-radius: 6px;
  padding: 2px 8px;
  cursor: pointer;
  font-size: 11px;
}
.srp-add-project:hover { color: var(--accent); border-color: var(--accent); }
.srp-sub {
  margin-top: 4px;
  color: var(--text-muted);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.srp-sub.err { color: var(--color-error, #e5484d); }

/* 卡 */
.srp-card {
  border: 1px solid var(--border);
  border-radius: 8px;
  background: var(--bg-elev);
  overflow: hidden;
}
.srp-card-head {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 6px 10px;
  width: 100%;
  border: none;
  background: transparent;
  color: var(--text);
  text-align: left;
}
.srp-card-head.clickable { cursor: pointer; }
.srp-card-head.clickable:hover { background: var(--bg-secondary); }
.srp-card-title { font-weight: 600; }
.srp-caret { margin-left: auto; color: var(--text-muted); font-size: 10px; }
.srp-card-body { padding: 0 10px 8px; }

/* 执行态大卡 */
.srp-exec { border-color: var(--accent); }
/* ⏱ 当前轮计时徽标（2026-11-24）：accent 走表；tabular-nums 防每秒
   跳动时徽标宽度抖动。 */
.srp-exec-elapsed {
  color: var(--accent);
  border-color: var(--accent);
  font-variant-numeric: tabular-nums;
}
.srp-dot {
  width: 8px; height: 8px; border-radius: 50%;
  background: var(--text-muted);
  flex-shrink: 0;
}
.srp-dot.running { background: var(--accent); animation: srp-pulse 1.2s infinite; }
@keyframes srp-pulse { 50% { opacity: 0.4; } }
.srp-exec-tool { padding: 0 10px 8px; color: var(--text-muted); }

/* 等待队列（2026-11-24）：行 hover 全文弹层。行本体单行截断（flex 内
   preview span 自截断），弹层 fixed 定位挂根级（卡片 overflow:hidden 与
   面板 overflow-y:auto 会裁 absolute 子元素）。 */
.srp-wq-row { cursor: default; }
.srp-wq-preview {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
}
.srp-wq-pop {
  position: fixed;
  z-index: 500;
  width: 340px;
  max-height: 220px;
  overflow: auto;
  padding: 8px 10px;
  background: var(--bg-elev);
  border: 1px solid var(--border);
  border-radius: 8px;
  box-shadow: 0 4px 16px rgb(0 0 0 / 25%);
}
.srp-wq-pop-text {
  white-space: pre-wrap;
  word-break: break-word;
  font-size: 12px;
  line-height: 1.5;
}
.srp-wq-pop-meta {
  margin-top: 6px;
  color: var(--text-muted);
  font-size: 11px;
  white-space: nowrap;
}

/* 在途区 */
.srp-pending { border-color: var(--accent); background: var(--bg-elev); }

/* 徽标 */
.srp-badge {
  font-size: 10px;
  border: 1px solid var(--border);
  border-radius: 999px;
  padding: 0 6px;
  color: var(--text-muted);
  background: var(--bg-secondary);
}
/* 会话工具分节（2026-11-19 拆分迁移）：分享/委派/思考强度三键 */
.srp-tools-close {
  margin-left: auto;
  border: none;
  background: transparent;
  color: var(--text-muted);
  cursor: pointer;
  font-size: 13px;
  line-height: 1;
  padding: 2px 6px;
  border-radius: 6px;
}
.srp-tools-close:hover { color: var(--text); background: var(--bg-secondary); }
.srp-tools-row {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  padding: 8px 10px;
}
.srp-tool-btn {
  border: 1px solid var(--border);
  background: var(--bg-elev);
  color: var(--text);
  border-radius: 6px;
  padding: 4px 10px;
  cursor: pointer;
  font-size: 12px;
}
.srp-tool-btn:hover:not(:disabled) { color: var(--accent); border-color: var(--accent); }
.srp-tool-btn:disabled { opacity: 0.45; cursor: not-allowed; }
/* 激活态（构建/计划=plan、放行/语音开关=on）：描边+底色同 accent 系，
   与 ChatPanel 工具栏 .voice-btn.active 同语义。 */
.srp-tool-btn.active {
  color: var(--accent);
  border-color: var(--accent);
  background: var(--bg-secondary);
}
.srp-effort-select {
  appearance: auto;
}
.srp-effort-select:focus {
  outline: none;
  border-color: var(--accent);
}
.srp-badge.warn { color: var(--accent); border-color: var(--accent); }

/* 行 */
.srp-rows { list-style: none; margin: 0; padding: 0; }
.srp-row {
  display: flex;
  gap: 6px;
  align-items: baseline;
  padding: 2px 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.srp-row.muted { color: var(--text-muted); }
.srp-key { color: var(--text-muted); flex-shrink: 0; }

/* C1：subagents 子代理行（状态点 + 图标 + 任务/摘要两行主体） */
.subagent-row { align-items: flex-start; }
.subagent-icon { flex-shrink: 0; line-height: 1.4; }
.subagent-main { display: flex; flex-direction: column; gap: 1px; min-width: 0; flex: 1; }
.subagent-task {
  color: var(--text);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.subagent-meta {
  font-size: 11px;
  color: var(--text-muted);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.subagent-meta.live { color: var(--accent); }
.subagent-meta.err { color: var(--danger); }

/* CTA / 表单 */
.srp-cta {
  margin-top: 6px;
  border: 1px solid var(--accent);
  background: transparent;
  color: var(--accent);
  border-radius: 6px;
  padding: 3px 10px;
  cursor: pointer;
  font-size: 11px;
}
.srp-cta:hover:not(:disabled) { background: var(--accent-bg, var(--bg-secondary)); }
.srp-cta:disabled { opacity: 0.5; cursor: default; }
.srp-cta.ghost { border-color: var(--border); color: var(--text-muted); }
.srp-form { display: flex; flex-direction: column; gap: 6px; margin-top: 6px; }
.srp-form-row { display: flex; gap: 6px; }
.srp-input {
  width: 100%;
  border: 1px solid var(--border);
  background: var(--bg-primary);
  color: var(--text);
  border-radius: 6px;
  padding: 4px 8px;
  font-size: 11px;
}
.srp-input:focus { outline: none; border-color: var(--accent); }

/* 开关 */
.srp-switch-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  padding: 2px 0;
  cursor: pointer;
}

/* 指标行 */
.srp-metrics { display: flex; flex-wrap: wrap; gap: 10px; color: var(--text-muted); }
.srp-pre {
  margin: 0;
  font-size: 10px;
  color: var(--text-muted);
  white-space: pre-wrap;
  word-break: break-all;
  max-height: 140px;
  overflow-y: auto;
}

/* chips */
.srp-chips { display: flex; flex-wrap: wrap; gap: 4px; padding: 0 10px 8px; }
.srp-chip {
  font-size: 10px;
  border: 1px solid var(--border);
  border-radius: 999px;
  padding: 1px 8px;
  color: var(--text-muted);
  background: var(--bg-secondary);
}

/* 空类目行 / Unavailable */
.srp-empty-row {
  border: 1px dashed var(--border);
  background: transparent;
  color: var(--text-muted);
  border-radius: 8px;
  padding: 4px 10px;
  cursor: pointer;
  text-align: left;
  font-size: 11px;
}
.srp-empty-detail { padding: 4px 0 0; }
.srp-unavail {
  color: var(--text-muted);
  border: 1px dashed var(--border);
  border-radius: 8px;
  padding: 4px 10px;
  font-size: 11px;
}

/* 加载/错误 */
.srp-empty-hint { color: var(--text-muted); padding: 8px 0; }
.srp-error {
  color: var(--color-error, #e5484d);
  border: 1px solid var(--border);
  border-radius: 8px;
  padding: 6px 10px;
  word-break: break-all;
}
</style>
