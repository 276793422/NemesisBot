<script setup lang="ts">
/**
 * SessionResourcesPanel — 会话资源盘点看板（2026-11-17，B3 前端主体）。
 *
 * 渲染当前会话的 16 类资源聚合（后端 WSAPI `session.resources`，Provider
 * 注册制聚合；协议见 crates/nemesis-web/src/handlers/resources.rs）：
 * - 头部：会话标题 / 项目 / 工作目录（identity.summary + system.status）
 *   +「+ 添加项目目录」（ProjectCreateModal）；
 * - 执行态置顶大卡（仅 status==='ok' 渲染；空闲 = Empty 整卡隐藏）；
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
import ProjectCreateModal from '../ProjectCreateModal.vue'

const props = defineProps<{
  /** 壳层钉死的会话 id（ChatView → ChatPanel.effectiveSid 同域）；null =
   * 尚未就绪，不拉取不监听渲染。本组件全部取数的唯一会话域。 */
  sessionId?: string | null
}>()

const router = useRouter()
const { request } = useWSAPI()
const { setSessionAllow } = useSessionAllowAll()

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

/** 在途区：待审批/待提问（ok 且 total>0 才渲染）。 */
const pendingCategories = computed(() =>
  (['approvals_pending', 'questions_pending'] as const)
    .map(id => ({ id, c: categories.value[id] }))
    .filter(x => x.c && x.c.status === 'ok' && x.c.total > 0),
)

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
    .filter(([id, c]) => c.status === 'empty' && id !== 'cron' && id !== 'hooks_global' && id !== 'session_allow')
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

async function fetchResources(): Promise<void> {
  const sid = props.sessionId
  if (!sid) return
  try {
    const data = await request('session', 'resources', { session_id: sid })
    if (!data || props.sessionId !== sid) return
    if (typeof data.revision === 'number' && data.revision < lastRevision.value) return
    if (typeof data.revision === 'number') lastRevision.value = data.revision
    resp.value = data as ResourcesResponse
    loadError.value = ''
  } catch (e) {
    // 资源盘点未装配（裁剪构建）/网络断 = 诚实错误行，不静默白板。
    loadError.value = String(e)
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

/** 会话切换：清态 + revision 归零 + 重拉（watch immediate 首拉；工作目录
 *  同拍刷新——跨实例切换后 workspace 可能不同）。 */
watch(() => props.sessionId, (sid) => {
  resp.value = null
  lastRevision.value = -1
  loadError.value = ''
  workdir.value = ''
  collapsedCards.value = new Set()
  if (sid) {
    void fetchResources()
    void fetchWorkdir()
  }
}, { immediate: true })

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
</script>

<template>
  <div class="srp-root">
    <!-- 头部：identity 常驻区 -->
    <div class="srp-header">
      <div class="srp-header-row">
        <span class="srp-title" :title="sessionTitle">{{ sessionTitle }}</span>
        <button class="srp-add-project" type="button" title="添加项目目录" @click="showProjectModal = true">＋项目</button>
      </div>
      <div v-if="projectPath" class="srp-sub" :title="projectPath">项目：{{ truncPath(projectPath, 32) }}</div>
      <div v-if="workdir" class="srp-sub" :title="workdir">目录：{{ truncPath(workdir, 36) }}</div>
    </div>

    <div v-if="loadError" class="srp-error">{{ loadError }}</div>

    <div v-if="!resp && !loadError" class="srp-empty-hint">加载中…</div>

    <template v-if="resp">
      <!-- 执行态置顶大卡 -->
      <div v-if="execution" class="srp-card srp-exec">
        <div class="srp-card-head">
          <span class="srp-dot" :class="{ running: execution.summary?.running === true }" />
          <span class="srp-card-title">{{ execution.label }}</span>
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
          <span class="srp-badge">{{ c.total }}</span>
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

          <!-- 通用：rows 行渲染（subagents/file_changes/checkpoint/background…） -->
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
.srp-dot {
  width: 8px; height: 8px; border-radius: 50%;
  background: var(--text-muted);
  flex-shrink: 0;
}
.srp-dot.running { background: var(--accent); animation: srp-pulse 1.2s infinite; }
@keyframes srp-pulse { 50% { opacity: 0.4; } }
.srp-exec-tool { padding: 0 10px 8px; color: var(--text-muted); }

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
