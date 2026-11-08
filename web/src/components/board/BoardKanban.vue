<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { useRouter } from 'vue-router'
import { useWSAPI } from '../../composables/useWSAPI'
import { useToast } from '../../composables/useToast'
import { useBoardChanged } from '../../composables/useBoardChanged'
import IssueDetailModal from './IssueDetailModal.vue'
import {
  PRIORITY_BADGE,
  PRIORITY_LABEL,
  STATUS_BADGE,
  STATUSES,
  TRANSITIONS,
  fmtTime,
  isInternalOrigin,
  statusLabel,
  statusTitle,
  touchpointCardSummary,
  touchpointCountdown,
  touchpointKindLabel,
  workbenchPath,
  type IssueRow,
  type TouchpointRow,
} from './boardMeta'

// 看板视图（W2 P3）：7 列 Kanban + HTML5 拖拽换列/列内排序。
// 拖拽走 issue.move（同列重排只改 position，跨列走后端状态机——非法转移
// 前端先按 TRANSITIONS 镜像拦截提示，后端仍是唯一真相源兜底）。
// 点击卡片打开共享详情弹窗（IssueDetailModal）。
// 人工触点（P1-P4）：pending 触点卡置顶「评审中」列（触点不新增列/不改
// issue 状态机——卡在承载单之上做两层展示的看板层）；⏸ 过滤片一键只亮
// 触点卡；点击触点卡进统一工作台（绝不弹 IssueDetailModal）。

const { request } = useWSAPI()
const toast = useToast()
const router = useRouter()

const loading = ref(true)
const issues = ref<IssueRow[]>([])
const projects = ref<any[]>([])
const workerNodes = ref<any[]>([])
// pending 触点（board.touchpoint.list 缺省只回活卡，deadline 升序已由后端排好）。
const touchpoints = ref<TouchpointRow[]>([])
// ⏸ 等我处理：一键只亮触点卡、其余置灰（非过滤隐藏——列结构不动）。
const tpOnly = ref(false)

const columns = computed(() =>
  STATUSES.map((s) => ({
    ...s,
    issues: issues.value
      .filter((i) => i.status === s.key)
      .sort((a, b) => a.position - b.position || a.id - b.id),
    tps: s.key === 'in_review' ? touchpoints.value : [],
  })),
)

function openWorkbench(row: TouchpointRow) {
  router.push(workbenchPath(row))
}

function projectName(id: number | null): string {
  if (id == null) return ''
  return projects.value.find((p) => p.id === id)?.name || ''
}

function assigneeShort(i: IssueRow): string {
  if (!i.assignee) return ''
  if (i.assignee === 'manager_self') return 'manager'
  // 指派目标可读名（复用本组件已拉的 nodes.list；未知 id 回退原始 id）。
  const name = workerNodes.value.find((n) => n.id === i.assignee_id)?.name
  return `@${name ?? i.assignee_id}`
}

async function load(silent = false) {
  // silent：board-changed 推送触发的静默刷新——不闪 loading、失败不弹
  // toast（SSE 连发 + 网络抖动时避免 toast 刷屏），仅控制台留痕。
  if (!silent) loading.value = true
  try {
    const [r, pr, nodes, tps] = await Promise.all([
      request('board', 'issue.list', {}),
      request('board', 'project.list', {}).catch(() => null),
      request('cluster', 'nodes.list', {}).catch(() => null),
      // 触点列表：旧后端无此命令 = 诚实降级为空（轻卡/过滤片消失，看板主体不损）。
      request('board', 'touchpoint.list', {}).catch(() => null),
    ])
    issues.value = r?.issues || []
    projects.value = pr?.projects || []
    workerNodes.value = (nodes?.nodes || []).filter((n: any) => n.role === 'worker')
    touchpoints.value = tps?.touchpoints || []
  } catch (e: any) {
    if (silent) console.warn('[BoardKanban] silent refresh failed:', e)
    else toast.error('加载看板失败: ' + e)
  } finally {
    loading.value = false
  }
}

// --- 拖拽 ---
const dragIssueId = ref<number | null>(null)
const dragOverColumn = ref('')
const dragOverCardId = ref<number | null>(null)

function onDragStart(issue: IssueRow, ev: DragEvent) {
  dragIssueId.value = issue.id
  if (ev.dataTransfer) {
    ev.dataTransfer.effectAllowed = 'move'
    ev.dataTransfer.setData('text/plain', String(issue.id))
  }
}

function onDragEnd() {
  dragIssueId.value = null
  dragOverColumn.value = ''
  dragOverCardId.value = null
}

async function onDropCard(target: IssueRow, ev: DragEvent) {
  ev.stopPropagation()
  const id = dragIssueId.value
  dragOverColumn.value = ''
  dragOverCardId.value = null
  if (id == null || id === target.id) return
  // 插到目标卡片之前（同 position，同位按 id 稳定排序）。
  await moveIssue(id, target.status, target.position)
}

async function onDropColumn(columnKey: string, ev: DragEvent) {
  const id = dragIssueId.value
  dragOverColumn.value = ''
  dragOverCardId.value = null
  if (id == null) return
  const col = columns.value.find((c) => c.key === columnKey)
  if (!col) return
  // 丢到列空白处 → 追加到末尾。
  const maxPos = col.issues.reduce((m, i) => Math.max(m, i.position), 0)
  await moveIssue(id, columnKey, maxPos + 1)
}

async function moveIssue(id: number, to: string, position: number) {
  const issue = issues.value.find((i) => i.id === id)
  if (!issue) return
  if (issue.status === to && issue.position === position) return
  // 非法转移前端先拦（后端状态机仍兜底拒绝）。
  if (issue.status !== to && !(TRANSITIONS[issue.status] || []).includes(to)) {
    toast.warn(`非法转移：${statusLabel(issue.status)} → ${statusLabel(to)}`)
    return
  }
  try {
    await request('board', 'issue.move', { id, status: to, position })
    await load()
  } catch (e: any) {
    toast.error('移动失败: ' + e)
    await load() // 回弹
  }
}

// --- 详情 ---
const detailIssueId = ref<number | null>(null)
function openDetail(issue: IssueRow) {
  detailIssueId.value = issue.id
}
function onDetailChanged() {
  load()
}

onMounted(load)
// board-changed 推送（W2.5）：任何写入方（含其他节点/CLI/autopilot）改了
// board.db → 200ms 防抖后静默换新，无需手动刷新。
useBoardChanged(() => load(true))
</script>

<template>
  <div>
    <div v-if="loading" style="text-align: center; padding: var(--space-8);">
      <div class="spinner spinner-lg" style="margin: 0 auto;"></div>
    </div>

    <template v-else>
      <!-- ⏸ 等我处理（人工触点体系 P1）：一键只亮触点卡，其余卡片置灰
           （列结构不动，非过滤隐藏——回看全貌零成本）。无活卡不渲染。 -->
      <div v-if="touchpoints.length" class="tp-toolbar">
        <button
          class="btn btn-sm tp-filter"
          :class="{ 'tp-filter-on': tpOnly }"
          data-test="tp-filter"
          @click="tpOnly = !tpOnly"
        >⏸ 等我处理（{{ touchpoints.length }}）</button>
        <span v-if="tpOnly" class="muted">只显示人工触点卡，其余已置灰；再点一次还原</span>
      </div>

      <div class="kanban">
        <div
          v-for="col in columns"
          :key="col.key"
          class="kanban-col"
          :class="{ 'drag-over': dragOverColumn === col.key }"
          @dragover.prevent="dragOverColumn = col.key"
          @dragleave="dragOverColumn === col.key && (dragOverColumn = '')"
          @drop.prevent="onDropColumn(col.key, $event)"
        >
          <div class="kanban-col-head">
            <span class="badge" :class="STATUS_BADGE[col.key]" :title="statusTitle(col.key)">{{ col.label }}</span>
            <span class="muted">{{ col.issues.length }}</span>
          </div>
          <!-- 触点卡（置顶评审中列；deadline 升序 = 后端缺省排序）：卡面限死
               徽标+标题+倒计时+一句话摘要+父单号，绝不渲染规格正文。
               ⏸ 态其余卡片置灰。点击 → 统一工作台（不弹详情模态）。 -->
          <div
            v-for="row in col.tps"
            :key="'tp-' + row.touchpoint.id"
            class="kanban-card tp-card"
            data-test="tp-card"
            @click="openWorkbench(row)"
          >
            <div class="kanban-card-top">
              <span class="badge tp-badge" data-test="tp-kind-badge">{{ touchpointKindLabel(row.touchpoint.kind) }}</span>
              <span
                class="tp-countdown"
                :class="{ urgent: touchpointCountdown(row.touchpoint.deadline_ms).urgent }"
              >{{ touchpointCountdown(row.touchpoint.deadline_ms).text }}</span>
            </div>
            <div class="kanban-card-title">{{ row.issue_title || row.issue_number }}</div>
            <div v-if="touchpointCardSummary(row)" class="tp-summary muted">{{ touchpointCardSummary(row) }}</div>
            <div class="kanban-card-meta">
              <code>{{ row.issue_number }}</code>
              <span v-if="row.overdue" class="badge badge-error">已过期</span>
            </div>
          </div>
          <div
            v-for="issue in col.issues"
            :key="issue.id"
            class="kanban-card"
            :class="{ 'dragging': dragIssueId === issue.id, 'drop-before': dragOverCardId === issue.id, 'is-decision': !!issue.needs_user, 'tp-dim': tpOnly }"
            draggable="true"
            @dragstart="onDragStart(issue, $event)"
            @dragend="onDragEnd"
            @dragover.prevent="dragIssueId !== issue.id && (dragOverCardId = issue.id)"
            @dragleave="dragOverCardId === issue.id && (dragOverCardId = null)"
            @drop="onDropCard(issue, $event)"
            @click="openDetail(issue)"
          >
            <div class="kanban-card-top">
              <code>{{ issue.number }}</code>
              <!-- P1 外循环（2026-10-25）：外部来源徽标（模板声明 origin 的
                   owner/repo#N 形态标识）；链接回跳需仓库 base URL（issue JSON
                   未携带）——v1 只标识，详情模态已有完整来源行。 -->
              <span
                v-if="issue.origin && !isInternalOrigin(issue.origin.origin_type)"
                class="badge badge-info"
                :title="`外部来源：${issue.origin.origin_type}/${issue.origin.origin_id}`"
                data-test="origin-badge"
              >🔗 {{ issue.origin.origin_id }}</span>
              <span class="badge" :class="PRIORITY_BADGE[issue.priority] || 'badge-neutral'">
                P{{ issue.priority }} {{ PRIORITY_LABEL[issue.priority] || '' }}
              </span>
            </div>
            <div class="kanban-card-title">
              {{ issue.title }}
              <!-- 决策可见性（2026-10-15 批）：列内扫视也要看到「在等人」。 -->
              <span
                v-if="issue.needs_user"
                class="badge badge-decision"
                :title="`停车待裁决：${issue.needs_user.stop_kind_label || ''}${issue.needs_user.hint ? ' — ' + issue.needs_user.hint : ''}`"
                data-test="needs-user-badge"
              >✋ 待你决策</span>
              <span
                v-if="issue.plan_ready"
                class="badge badge-plan"
                :title="`拆解完成待确认：${issue.plan_ready.subs_count} 个子单，点开确认发车`"
                data-test="plan-ready-badge"
              >📋 计划待确认</span>
              <span
                v-if="issue.attention"
                class="badge badge-attention"
                title="执行档案停滞（哨兵）：交付已收但变更集未合入——先观察，持续停滞再人工介入"
                data-test="attention-badge"
              >⚠ 停滞</span>
            </div>
            <div class="kanban-card-meta">
              <!-- C2（goal P1）：环节徽标——后端 issue.list 附 stage（单一真相源 issue_stage） -->
              <span v-if="issue.stage" class="badge badge-info">{{ issue.stage }}</span>
              <span v-if="projectName(issue.project_id)" class="muted">{{ projectName(issue.project_id) }}</span>
              <span v-if="assigneeShort(issue)" class="muted">{{ assigneeShort(issue) }}</span>
              <span class="muted kanban-card-time">{{ fmtTime(issue.updated_at) }}</span>
            </div>
          </div>
          <div v-if="col.issues.length === 0 && col.tps.length === 0" class="kanban-empty muted">拖拽卡片到此处</div>
        </div>
      </div>
    </template>

    <!-- v-if 门控：issueId 置空必须收起弹层（与 IssueListView 同款修复） -->
    <IssueDetailModal
      v-if="detailIssueId !== null"
      :issue-id="detailIssueId"
      @close="detailIssueId = null"
      @changed="onDetailChanged"
    />
  </div>
</template>

<style scoped>
.muted {
  color: var(--text-muted);
  font-size: var(--text-xs);
}
.kanban {
  display: flex;
  gap: var(--space-3);
  align-items: flex-start;
  overflow-x: auto;
  padding-bottom: var(--space-4);
}
.kanban-col {
  flex: 1 0 220px;
  min-width: 220px;
  background: var(--bg-secondary);
  border-radius: var(--radius-md);
  padding: var(--space-2);
  min-height: 300px;
}
.kanban-col.drag-over {
  outline: 2px dashed var(--accent);
  outline-offset: -2px;
}
.kanban-col-head {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  padding: var(--space-1) var(--space-1) var(--space-2);
}
.kanban-card {
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
  padding: var(--space-2) var(--space-3);
  margin-bottom: var(--space-2);
  cursor: grab;
}
.kanban-card:hover {
  border-color: var(--accent);
}
.kanban-card.dragging {
  opacity: 0.5;
}
.kanban-card.drop-before {
  border-top: 2px solid var(--accent);
}
/* 决策可见性（2026-10-15 批）：待决策卡 amber 描边（mockup is-decision）。 */
.kanban-card.is-decision {
  border-color: var(--warning);
  box-shadow: 0 0 0 1px var(--warning);
}
.kanban-card-top {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--space-2);
  margin-bottom: var(--space-1);
}
.kanban-card-title {
  font-size: var(--text-sm);
  font-weight: 500;
  margin-bottom: var(--space-1);
  word-break: break-word;
}
.kanban-card-meta {
  display: flex;
  flex-wrap: wrap;
  gap: var(--space-2);
}
.kanban-card-time {
  margin-left: auto;
}
.kanban-empty {
  text-align: center;
  padding: var(--space-4) 0;
  border: 1px dashed var(--border);
  border-radius: var(--radius-md);
}
/* 人工触点卡（P1）：评审中列置顶；kind 徽标 + 倒计时（≤1h/过期橙）+
   一句话摘要 + 父单号。点击进统一工作台（不弹详情模态）。 */
.tp-toolbar {
  display: flex;
  align-items: center;
  gap: var(--space-3);
  margin-bottom: var(--space-3);
}
.tp-filter {
  border: 1px solid var(--border);
  background: var(--surface);
}
.tp-filter-on {
  border-color: var(--warning);
  background: var(--warning);
  color: #fff;
}
.kanban-card.tp-card {
  border-color: var(--warning);
  box-shadow: 0 0 0 1px var(--warning);
  cursor: pointer;
}
.tp-badge {
  background: var(--warning);
  color: #fff;
}
.tp-countdown {
  font-size: var(--text-xs);
  color: var(--text-muted);
  white-space: nowrap;
}
.tp-countdown.urgent {
  color: var(--warning);
  font-weight: 600;
}
.tp-summary {
  margin: var(--space-1) 0;
  word-break: break-word;
}
/* ⏸ 等我处理态：非触点卡片整体压暗（非隐藏——列结构不动，回看全貌零成本）。
   触点卡不挂 tp-dim（模板层区分），恒亮。 */
.kanban-card.tp-dim {
  opacity: 0.35;
  pointer-events: none;
}
</style>
