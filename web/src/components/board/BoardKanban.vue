<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { useWSAPI } from '../../composables/useWSAPI'
import { useToast } from '../../composables/useToast'
import IssueDetailModal from './IssueDetailModal.vue'
import {
  PRIORITY_BADGE,
  PRIORITY_LABEL,
  STATUS_BADGE,
  STATUSES,
  TRANSITIONS,
  fmtTime,
  statusLabel,
  type IssueRow,
} from './boardMeta'

// 看板视图（W2 P3）：7 列 Kanban + HTML5 拖拽换列/列内排序。
// 拖拽走 issue.move（同列重排只改 position，跨列走后端状态机——非法转移
// 前端先按 TRANSITIONS 镜像拦截提示，后端仍是唯一真相源兜底）。
// 点击卡片打开共享详情弹窗（IssueDetailModal）。

const { request } = useWSAPI()
const toast = useToast()

const loading = ref(true)
const issues = ref<IssueRow[]>([])
const projects = ref<any[]>([])
const workerNodes = ref<any[]>([])

const columns = computed(() =>
  STATUSES.map((s) => ({
    ...s,
    issues: issues.value
      .filter((i) => i.status === s.key)
      .sort((a, b) => a.position - b.position || a.id - b.id),
  })),
)

function projectName(id: number | null): string {
  if (id == null) return ''
  return projects.value.find((p) => p.id === id)?.name || ''
}

function assigneeShort(i: IssueRow): string {
  if (!i.assignee) return ''
  return i.assignee === 'manager_self' ? 'manager' : `@${i.assignee_id}`
}

async function load() {
  loading.value = true
  try {
    const [r, pr, nodes] = await Promise.all([
      request('board', 'issue.list', {}),
      request('board', 'project.list', {}).catch(() => null),
      request('cluster', 'nodes.list', {}).catch(() => null),
    ])
    issues.value = r?.issues || []
    projects.value = pr?.projects || []
    workerNodes.value = (nodes?.nodes || []).filter((n: any) => n.role === 'worker')
  } catch (e: any) {
    toast.error('加载看板失败: ' + e)
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
</script>

<template>
  <div>
    <div v-if="loading" style="text-align: center; padding: var(--space-8);">
      <div class="spinner spinner-lg" style="margin: 0 auto;"></div>
    </div>

    <div v-else class="kanban">
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
          <span class="badge" :class="STATUS_BADGE[col.key]">{{ col.label }}</span>
          <span class="muted">{{ col.issues.length }}</span>
        </div>
        <div
          v-for="issue in col.issues"
          :key="issue.id"
          class="kanban-card"
          :class="{ 'dragging': dragIssueId === issue.id, 'drop-before': dragOverCardId === issue.id }"
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
            <span class="badge" :class="PRIORITY_BADGE[issue.priority] || 'badge-neutral'">
              P{{ issue.priority }} {{ PRIORITY_LABEL[issue.priority] || '' }}
            </span>
          </div>
          <div class="kanban-card-title">{{ issue.title }}</div>
          <div class="kanban-card-meta">
            <span v-if="projectName(issue.project_id)" class="muted">{{ projectName(issue.project_id) }}</span>
            <span v-if="assigneeShort(issue)" class="muted">{{ assigneeShort(issue) }}</span>
            <span class="muted kanban-card-time">{{ fmtTime(issue.updated_at) }}</span>
          </div>
        </div>
        <div v-if="col.issues.length === 0" class="kanban-empty muted">拖拽卡片到此处</div>
      </div>
    </div>

    <IssueDetailModal
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
</style>
