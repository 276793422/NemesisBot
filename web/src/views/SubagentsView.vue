<script setup lang="ts">
// 子代理任务页（子代理可见性 D1/D4/D6 前端面，2026-11-03）。
//
// 数据面单一真相源 = 后端 subagents WSAPI（registry 台账 + chat_event_log 环）：
// - 列表：subagents.list（status/origin 过滤在服务端做）；有运行行时 5s 轮询，
//   纯终态 20s 慢拍（台账行落盘，进程重启后仍在——环不在，见详情注记）。
// - 详情（D4 全深度）：subagents.detail + subagents.events 回放对话环
//   （工具卡 ToolStarted/ToolFinished + RoundText 轮次文本）。运行中每 2s
//   after_seq 增量拉，running=false 停轮询；gap=true 全量重拉（环逐出诚实上报）。
// - 实时帧：addMessageHandler 过滤 push 帧 data.subagent_id === 详情 id
//   （泵侧 D3 臂注入；带 seq → 与回放去重共用一条时间线）。
// - 终止（D6）：subagents.cancel。delegate 行诚实注记：取消只代表本节点
//   「不再等待显示」，B 端真实结果回灌仍会覆盖终态（bus.rs 侧钉死语义）。
import { ref, computed, onMounted, onBeforeUnmount } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'
import { addMessageHandler, removeMessageHandler } from '../composables/useWebSocket'

const { request } = useWSAPI()
const toast = useToast()
const route = useRoute()
const router = useRouter()

interface SubagentRow {
  id: string
  parent_session_key: string
  parent_chat_id: string
  origin: string
  agent_id: string
  task: string
  role: string
  tools_profile: string
  depth: number
  background: boolean
  loop_origin: string
  status: string
  started_at_ms: number
  finished_at_ms?: number | null
  rounds?: number | null
  summary?: string | null
  error?: string | null
  bg_task_id?: string | null
  running: boolean
  duration_ms?: number
}

// --- 列表 ---
const tasks = ref<SubagentRow[]>([])
const loading = ref(true)
const filterStatus = ref('')
const filterOrigin = ref('')
let listTimer: ReturnType<typeof setInterval> | null = null

async function loadList() {
  try {
    const data: any = {}
    if (filterStatus.value) data.status = filterStatus.value
    if (filterOrigin.value) data.origin = filterOrigin.value
    const r = await request('subagents', 'list', data)
    tasks.value = (r?.tasks as SubagentRow[]) || []
  } catch { /* WS 未就绪——下轮再试 */ }
  loading.value = false
}

const hasRunning = computed(() => tasks.value.some((t) => t.running))

function armListTimer() {
  if (listTimer) clearInterval(listTimer)
  listTimer = setInterval(loadList, hasRunning.value ? 5000 : 20000)
}

// --- 详情抽屉（modal 形态）---
const detail = ref<SubagentRow | null>(null)
const detailLoading = ref(false)
const detailNotFound = ref(false)
interface TimelineItem {
  seq: number
  kind: string
  data: any
}
const timeline = ref<TimelineItem[]>([])
const seenSeqs = new Set<number>()
let lastSeq = 0
let localSeq = -1 // 无 seq 的实时帧兜底（理论上子代理事件恒有 seq）
let detailPoll: ReturnType<typeof setInterval> | null = null

function pushTimeline(kind: string, data: any, seq: number) {
  if (seq > 0) {
    if (seenSeqs.has(seq)) return
    seenSeqs.add(seq)
    if (seq > lastSeq) lastSeq = seq
  } else {
    seq = localSeq--
  }
  timeline.value.push({ seq, kind, data })
}

function ingestRingEvents(events: any[]) {
  for (const ev of events || []) {
    // 环条目两形态：kind==='tool' → tool 字段携带完整 push 载荷；
    // 普通 chat 行（role/content）——子代理会话不产生，兜底渲染为文本。
    if (ev?.kind === 'tool' && ev?.tool) {
      pushTimeline(String(ev.tool.kind ?? 'Unknown'), ev.tool.data ?? {}, Number(ev.seq ?? 0))
    } else {
      pushTimeline('ChatLine', { content: ev?.content ?? '', role: ev?.role ?? '' }, Number(ev?.seq ?? 0))
    }
  }
  timeline.value.sort((a, b) => a.seq - b.seq)
}

async function openDetail(id: string) {
  detailNotFound.value = false
  detailLoading.value = true
  timeline.value = []
  seenSeqs.clear()
  lastSeq = 0
  try {
    const t = await request('subagents', 'detail', { id })
    detail.value = t as SubagentRow
    const r = await request('subagents', 'events', { id, after_seq: 0 })
    ingestRingEvents(r?.events || [])
    if (r?.gap) toast.warn('事件环有缺口（逐出/重启），仅显示保留部分')
    armDetailPoll(r?.running === true)
  } catch (e: any) {
    detail.value = null
    detailNotFound.value = true
  } finally {
    detailLoading.value = false
  }
}

function armDetailPoll(running: boolean) {
  if (detailPoll) { clearInterval(detailPoll); detailPoll = null }
  if (running) detailPoll = setInterval(pollDetailEvents, 2000)
}

async function pollDetailEvents() {
  if (!detail.value) return
  try {
    const r = await request('subagents', 'events', { id: detail.value.id, after_seq: lastSeq })
    if (r?.gap) {
      // 环逐出：seq 游标失效，全量重拉（seenSeqs 去重防重复渲染）
      const full = await request('subagents', 'events', { id: detail.value.id, after_seq: 0 })
      ingestRingEvents(full?.events || [])
    } else {
      ingestRingEvents(r?.events || [])
    }
    // 台账侧终态推进（cancel 被回灌覆盖等）同步刷详情行
    if (typeof r?.running === 'boolean') detail.value.running = r.running
    if (r?.status && detail.value.status !== r.status) {
      detail.value.status = r.status
      loadList()
    }
    if (r?.running === false) armDetailPoll(false)
  } catch { /* 断线窗口——下个 tick 再试 */ }
}

function closeDetail() {
  detail.value = null
  detailNotFound.value = false
  if (detailPoll) { clearInterval(detailPoll); detailPoll = null }
}

// 实时帧（泵 D3 臂）：data.subagent_id 精确对准当前详情 id。
function handleWSMessage(frame: any) {
  if (frame?.type !== 'push' || frame?.cmd !== 'tool_event') return
  const ev = frame.data
  const sid = ev?.subagent_id
  if (!sid) return
  if (detail.value && sid === detail.value.id) {
    pushTimeline(String(ev.kind ?? 'Unknown'), ev.data ?? {}, Number(ev.seq ?? 0))
    timeline.value.sort((a, b) => a.seq - b.seq)
  }
}

// --- 终止（D6）---
async function cancelTask(t: SubagentRow) {
  const note = t.origin === 'delegate'
    ? `终止「${excerpt(t.task, 40)}」？\n集群委派任务：终止只代表本节点不再等待显示，B 端真实结果回灌后仍会覆盖终态。`
    : `终止「${excerpt(t.task, 40)}」？运行中的执行链将收到取消信号。`
  if (!confirm(note)) return
  try {
    await request('subagents', 'cancel', { id: t.id })
    toast.success('已发送终止')
    if (detail.value?.id === t.id) pollDetailEvents()
    else loadList()
  } catch (e: any) {
    toast.error('终止失败: ' + (e?.message ?? e))
  }
}

// --- 展示辅助 ---
function excerpt(s: string | null | undefined, n: number): string {
  const t = (s ?? '').replace(/\s+/g, ' ').trim()
  return t.length > n ? t.slice(0, n) + '…' : t
}

const STATUS_BADGE: Record<string, string> = {
  running: 'badge-info',
  done: 'badge-success',
  failed: 'badge-error',
  cancelled: 'badge-neutral',
  interrupted: 'badge-warning',
}
const STATUS_LABEL: Record<string, string> = {
  running: '运行中',
  done: '已完成',
  failed: '失败',
  cancelled: '已终止',
  interrupted: '已中断',
}
const ORIGIN_LABEL: Record<string, string> = {
  detached: '内联',
  bg_spawn: '后台',
  delegate: '集群委派',
}

function statusBadge(s: string): string {
  return STATUS_BADGE[s] ?? 'badge-neutral'
}
function statusLabel(s: string): string {
  return STATUS_LABEL[s] ?? s
}
function originLabel(o: string): string {
  return ORIGIN_LABEL[o] ?? o
}
function originTitle(o: string): string {
  if (o === 'detached') return '内联 detached 子代理（父 loop spawn 的独立执行环）'
  if (o === 'bg_spawn') return '后台子代理（__BG_SPAWN__ 启动，父 loop 不等待）'
  if (o === 'delegate') return '集群委派（cluster_rpc 派发到对端节点，本节点只记 A 端生命周期）'
  return o
}

function fmtDuration(row: SubagentRow): string {
  const ms = row.duration_ms ?? (row.running ? Date.now() - row.started_at_ms : 0)
  if (!ms || ms < 0) return '—'
  if (ms < 1000) return `${ms}ms`
  const s = Math.floor(ms / 1000)
  if (s < 60) return `${s}s`
  return `${Math.floor(s / 60)}m${String(s % 60).padStart(2, '0')}s`
}

function fmtTime(ms?: number | null): string {
  if (!ms) return '—'
  return new Date(ms).toLocaleTimeString()
}

function sessionTail(key: string): string {
  const t = String(key ?? '').split(':').pop()
  return t || key || '—'
}

function toolLabel(item: TimelineItem): string {
  if (item.kind === 'ToolStarted') return String(item.data?.tool ?? 'tool')
  if (item.kind === 'ToolFinished') return String(item.data?.tool ?? 'tool')
  return item.kind
}

// ChatPanel 抽屉深链：/subagents?focus=<id> 自动开详情。
async function openFromQuery() {
  const focus = route.query.focus
  if (typeof focus === 'string' && focus) {
    await openDetail(focus)
    // 用完即清（刷新页面不再自动弹）
    router.replace({ query: { ...route.query, focus: undefined } })
  }
}

onMounted(async () => {
  addMessageHandler(handleWSMessage)
  await loadList()
  armListTimer()
  await openFromQuery()
})

onBeforeUnmount(() => {
  if (listTimer) clearInterval(listTimer)
  if (detailPoll) clearInterval(detailPoll)
  removeMessageHandler(handleWSMessage)
})
</script>

<template>
  <div class="page-subagents">
    <div class="page-header">
      <h2>子代理任务</h2>
      <p class="page-desc">
        Agent 派生的子执行环台账：内联 / 后台子代理 + 集群委派任务（A 端生命周期）。
        台账落盘重启保留；对话事件环是进程内存态，重启后不可回放。
      </p>
    </div>
    <div class="page-body">
      <div class="subagent-filters">
        <select class="form-select" v-model="filterStatus" @change="loadList(); armListTimer()" style="max-width: 160px;">
          <option value="">全部状态</option>
          <option value="running">运行中</option>
          <option value="done">已完成</option>
          <option value="failed">失败</option>
          <option value="cancelled">已终止</option>
          <option value="interrupted">已中断</option>
        </select>
        <select class="form-select" v-model="filterOrigin" @change="loadList(); armListTimer()" style="max-width: 160px;">
          <option value="">全部来源</option>
          <option value="detached">内联</option>
          <option value="bg_spawn">后台</option>
          <option value="delegate">集群委派</option>
        </select>
        <span v-if="hasRunning" class="badge badge-info">● {{ tasks.filter((t) => t.running).length }} 个运行中</span>
        <div style="flex: 1;"></div>
        <button class="btn btn-sm" @click="loadList">刷新</button>
      </div>

      <div v-if="loading" style="text-align: center; padding: var(--space-8);">
        <div class="spinner spinner-lg" style="margin: 0 auto;"></div>
      </div>

      <div v-else-if="tasks.length === 0" class="empty-state">
        <h3>暂无子代理任务</h3>
        <p>Agent 派生子代理（spawn / 后台任务 / 集群委派）后会出现在这里</p>
      </div>

      <div v-else class="table-wrap">
        <table>
          <thead>
            <tr>
              <th>任务</th><th>状态</th><th>来源</th><th>宿主</th>
              <th>轮次</th><th>时长</th><th>开始</th><th>操作</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="t in tasks" :key="t.id" class="subagent-row" @click="openDetail(t.id)">
              <td style="max-width: 320px;">
                <div style="font-weight: 500;">{{ excerpt(t.task, 60) || '（无任务文本）' }}</div>
                <div style="font-size: var(--text-xs); color: var(--text-muted); font-family: var(--font-mono);" :title="t.id">{{ t.id }}</div>
              </td>
              <td><span class="badge" :class="statusBadge(t.status)">{{ statusLabel(t.status) }}</span></td>
              <td>
                <span class="badge badge-neutral" :title="originTitle(t.origin)">{{ originLabel(t.origin) }}</span>
                <div v-if="t.depth > 0" style="font-size: var(--text-xs); color: var(--text-muted);">深度 {{ t.depth }}</div>
              </td>
              <td style="font-size: var(--text-sm); color: var(--text-secondary);">{{ t.loop_origin || 'main' }}</td>
              <td style="font-size: var(--text-sm);">{{ t.rounds != null ? t.rounds : '—' }}</td>
              <td style="font-size: var(--text-sm);">{{ fmtDuration(t) }}</td>
              <td style="font-size: var(--text-sm); color: var(--text-muted);">{{ fmtTime(t.started_at_ms) }}</td>
              <td @click.stop>
                <div style="display: flex; gap: var(--space-1);">
                  <button class="btn btn-sm" @click="openDetail(t.id)">详情</button>
                  <button v-if="t.running" class="btn btn-sm btn-danger" @click="cancelTask(t)">终止</button>
                </div>
              </td>
            </tr>
          </tbody>
        </table>
      </div>
    </div>

    <!-- 详情 modal（D4 全深度：台账字段 + 对话环时间线） -->
    <div v-if="detail || detailNotFound" class="modal-backdrop" @click.self="closeDetail()">
      <div class="modal modal-wide">
        <div class="modal-header">
          <h3 v-if="detail">子代理详情 · {{ statusLabel(detail.status) }}</h3>
          <h3 v-else>子代理详情</h3>
          <button class="modal-close" @click="closeDetail()">✕</button>
        </div>
        <div class="modal-body">
          <div v-if="detailLoading" style="text-align: center; padding: var(--space-6);">
            <div class="spinner" style="margin: 0 auto;"></div>
          </div>

          <div v-else-if="detailNotFound" class="empty-state">
            <h3>任务不在册</h3>
            <p>该任务不在当前进程的注册表中（已从台账逐出，或属于另一个实例）</p>
          </div>

          <template v-else-if="detail">
            <!-- 集群委派诚实注记：B 端过程不在本节点 -->
            <div v-if="detail.origin === 'delegate'" class="delegate-note">
              📡 集群委派任务在 B 端节点执行——本节点只记录 A 端生命周期（派发与回灌终态），
              过程事件环为空是诚实形态；B 端节点侧的过程记录留在该节点的会话日志里。
            </div>

            <div class="detail-grid">
              <div class="kv-row"><span class="kv-key">任务 ID</span><span class="kv-value" style="font-family: var(--font-mono);">{{ detail.id }}</span></div>
              <div class="kv-row"><span class="kv-key">任务文本</span><span class="kv-value">{{ detail.task || '（无任务文本）' }}</span></div>
              <div class="kv-row"><span class="kv-key">状态</span><span class="kv-value"><span class="badge" :class="statusBadge(detail.status)">{{ statusLabel(detail.status) }}</span></span></div>
              <div class="kv-row"><span class="kv-key">来源 / 深度</span><span class="kv-value">{{ originLabel(detail.origin) }}<template v-if="detail.depth > 0"> · 深度 {{ detail.depth }}</template><template v-if="detail.background"> · 后台</template></span></div>
              <div class="kv-row"><span class="kv-key">宿主 loop</span><span class="kv-value">{{ detail.loop_origin || 'main' }}</span></div>
              <div class="kv-row" v-if="detail.agent_id"><span class="kv-key">执行者</span><span class="kv-value">{{ detail.agent_id }}</span></div>
              <div class="kv-row" v-if="detail.role"><span class="kv-key">角色 / 工具面</span><span class="kv-value">{{ detail.role }}<template v-if="detail.tools_profile"> · {{ detail.tools_profile }}</template></span></div>
              <div class="kv-row"><span class="kv-key">父会话</span><span class="kv-value">{{ sessionTail(detail.parent_session_key) }} <span style="color: var(--text-muted); font-size: var(--text-xs);">({{ detail.parent_session_key }})</span></span></div>
              <div class="kv-row"><span class="kv-key">开始 / 时长</span><span class="kv-value">{{ fmtTime(detail.started_at_ms) }} · {{ fmtDuration(detail) }}</span></div>
              <div class="kv-row" v-if="detail.rounds != null"><span class="kv-key">轮次</span><span class="kv-value">{{ detail.rounds }}</span></div>
              <div class="kv-row" v-if="detail.bg_task_id"><span class="kv-key">后台任务 ID</span><span class="kv-value" style="font-family: var(--font-mono);">{{ detail.bg_task_id }}</span></div>
              <div class="kv-row" v-if="detail.summary"><span class="kv-key">结果摘要</span><span class="kv-value">{{ detail.summary }}</span></div>
              <div class="kv-row" v-if="detail.error"><span class="kv-key">错误</span><span class="kv-value" style="color: var(--error);">{{ detail.error }}</span></div>
            </div>

            <div v-if="detail.running" style="display: flex; gap: var(--space-2); margin-top: var(--space-3);">
              <button class="btn btn-sm btn-danger" @click="cancelTask(detail)">终止</button>
              <span style="font-size: var(--text-xs); color: var(--text-muted); align-self: center;">
                {{ detail.origin === 'delegate' ? '集群委派：终止 = 本节点不再等待显示，B 端结果回灌仍会覆盖终态' : '终止将向执行链发送取消信号' }}
              </span>
            </div>

            <!-- 对话环时间线（D4 全深度：工具卡 + 轮次文本） -->
            <div v-if="timeline.length" class="subagent-timeline">
              <div class="timeline-title">执行时间线</div>
              <template v-for="(item, i) in timeline" :key="item.seq">
                <div v-if="item.kind === 'ToolStarted'" class="timeline-item tool">
                  <span class="timeline-mark start">▶</span>
                  <span class="timeline-body">
                    <code>{{ toolLabel(item) }}</code>
                    <span v-if="item.data?.args_preview" class="timeline-dim" :title="item.data.args_preview">{{ excerpt(item.data.args_preview, 120) }}</span>
                  </span>
                </div>
                <div v-else-if="item.kind === 'ToolFinished'" class="timeline-item tool">
                  <span class="timeline-mark" :class="item.data?.ok ? 'ok' : 'fail'">{{ item.data?.ok ? '✓' : '✕' }}</span>
                  <span class="timeline-body">
                    <code>{{ toolLabel(item) }}</code>
                    <span v-if="item.data?.duration_ms != null" class="timeline-dim">{{ item.data.duration_ms }}ms</span>
                    <span v-if="item.data?.result_preview" class="timeline-dim" :title="item.data.result_preview">{{ excerpt(item.data.result_preview, 160) }}</span>
                  </span>
                </div>
                <div v-else-if="item.kind === 'RoundText'" class="timeline-round">
                  <div v-if="item.data?.content" class="round-text">{{ item.data.content }}</div>
                  <details v-if="item.data?.reasoning" class="round-reasoning">
                    <summary>思考过程</summary>
                    <pre>{{ item.data.reasoning }}</pre>
                  </details>
                </div>
                <div v-else-if="item.kind === 'ChatLine'" class="timeline-round">
                  <div class="round-text">{{ item.data?.content }}</div>
                </div>
                <div v-else-if="item.kind === 'ModeChanged'" class="timeline-item tool">
                  <span class="timeline-mark neutral">◆</span>
                  <span class="timeline-body timeline-dim">模式切换 → {{ item.data?.mode }}</span>
                </div>
                <div v-else class="timeline-item tool">
                  <span class="timeline-mark neutral">·</span>
                  <span class="timeline-body timeline-dim">{{ item.kind }}</span>
                </div>
              </template>
            </div>
            <div v-else-if="!detailLoading" class="timeline-empty">
              {{
                detail.origin === 'delegate'
                  ? '集群委派任务无本节点过程事件环（见上方注记）'
                  : detail.running
                    ? '等待事件…（运行中每 2 秒增量拉取）'
                    : '无过程事件环（重启后环即清空，只剩台账行）'
              }}
            </div>
          </template>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.page-desc {
  color: var(--text-muted);
  font-size: var(--text-sm);
  margin-top: var(--space-1);
}
.subagent-filters {
  display: flex;
  align-items: center;
  gap: var(--space-3);
  margin-bottom: var(--space-4);
}
.subagent-row {
  cursor: pointer;
}
.delegate-note {
  background: var(--bg-secondary);
  border-left: 3px solid var(--warning);
  padding: var(--space-2) var(--space-3);
  font-size: var(--text-sm);
  color: var(--text-secondary);
  margin-bottom: var(--space-3);
}
.detail-grid {
  display: flex;
  flex-direction: column;
  gap: var(--space-1);
}
.kv-row {
  display: flex;
  gap: var(--space-3);
  font-size: var(--text-sm);
  line-height: 1.6;
}
.kv-key {
  flex: 0 0 92px;
  color: var(--text-muted);
}
.kv-value {
  flex: 1;
  min-width: 0;
  word-break: break-all;
  white-space: pre-wrap;
}
.subagent-timeline {
  margin-top: var(--space-4);
  border-top: 1px solid var(--border-color);
  padding-top: var(--space-3);
}
.timeline-title {
  font-size: var(--text-sm);
  font-weight: 600;
  margin-bottom: var(--space-2);
  color: var(--text-secondary);
}
.timeline-item {
  display: flex;
  align-items: baseline;
  gap: var(--space-2);
  font-size: var(--text-sm);
  padding: 2px 0;
}
.timeline-mark {
  flex: 0 0 16px;
  text-align: center;
}
.timeline-mark.start { color: var(--info, #4a90e2); }
.timeline-mark.ok { color: var(--success, #2ecc71); }
.timeline-mark.fail { color: var(--error, #e74c3c); }
.timeline-mark.neutral { color: var(--text-muted); }
.timeline-body {
  display: flex;
  gap: var(--space-2);
  flex-wrap: wrap;
  min-width: 0;
}
.timeline-dim {
  color: var(--text-muted);
  font-size: var(--text-xs);
  overflow: hidden;
  text-overflow: ellipsis;
}
.timeline-round {
  margin: var(--space-2) 0;
  padding: var(--space-2) var(--space-3);
  background: var(--bg-secondary);
  border-left: 3px solid var(--border-color);
  font-size: var(--text-sm);
}
.round-text {
  white-space: pre-wrap;
  word-break: break-word;
}
.round-reasoning {
  margin-top: var(--space-2);
}
.round-reasoning summary {
  cursor: pointer;
  color: var(--text-muted);
  font-size: var(--text-xs);
}
.round-reasoning pre {
  white-space: pre-wrap;
  word-break: break-word;
  font-size: var(--text-xs);
  color: var(--text-secondary);
  margin-top: var(--space-1);
}
.timeline-empty {
  margin-top: var(--space-3);
  font-size: var(--text-sm);
  color: var(--text-muted);
  text-align: center;
  padding: var(--space-4);
}
.modal-wide {
  max-width: 760px;
  width: calc(100vw - 48px);
}
</style>
