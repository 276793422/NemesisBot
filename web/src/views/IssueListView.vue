<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'
import { useBoardChanged } from '../composables/useBoardChanged'
import IssueDetailModal from '../components/board/IssueDetailModal.vue'
import {
  PRIORITY_BADGE,
  PRIORITY_LABEL,
  STATUS_BADGE,
  STATUSES,
  fmtTime,
  statusLabel,
  type IssueRow,
} from '../components/board/boardMeta'

// 托管 Agent 看板 · 列表页（W2 P1 列表 + P3 改造）：issue 列表 + 过滤 + 创建。
// 详情弹窗抽为共享组件 IssueDetailModal（看板/列表共用；评论线程/@提及/
// 附件在里面）。数据源：board.* WSAPI（crates/nemesis-web/src/handlers/board.rs）。
// 后端状态机仍是唯一真相源，非法转移会被拒绝并回灌错误。

const { request } = useWSAPI()
const toast = useToast()

// --- 状态 ---
const loading = ref(true)
const issues = ref<IssueRow[]>([])
const total = ref(0)
const stats = ref<Record<string, number>>({})
const projects = ref<any[]>([])

// 过滤条件
const filterStatus = ref('')
const filterQuery = ref('')
const filterProject = ref<number | ''>('')
const filterPriority = ref<number | ''>('')
// worker 指派过滤（assignee_type=worker + assignee_id；manager_self 走 'local'）。
const filterAssignee = ref('')

// 集群 worker 节点（创建表单指派下拉数据源；集群不可用时空列表 → 回退手输）。
interface ClusterNode {
  id: string
  name: string
  role: string
  online: boolean
}
const workerNodes = ref<ClusterNode[]>([])

async function loadWorkerNodes() {
  try {
    const r = await request('cluster', 'nodes.list', {})
    workerNodes.value = (r?.nodes || []).filter((n: any) => n.role === 'worker')
  } catch {
    workerNodes.value = [] // 集群未启用/未注入 → 手动输入回退
  }
}

function workerLabel(n: ClusterNode): string {
  return `${n.name} (${n.id})${n.online ? '' : ' · 离线'}`
}

// 详情弹窗（共享组件按 id 自加载；@changed 回调刷新列表）。
const detailIssueId = ref<number | null>(null)

function openDetail(issue: IssueRow) {
  detailIssueId.value = issue.id
}

// 创建弹窗
const showCreate = ref(false)
const busy = ref(false)
const createForm = ref({
  title: '',
  description: '',
  priority: 1,
  assigneeType: '',
  assigneeId: '',
  projectId: '' as number | '',
  acceptance: '',
})

// --- 数据加载 ---
async function loadIssues(silent = false) {
  const data: any = {}
  if (filterStatus.value) data.status = filterStatus.value
  if (filterQuery.value.trim()) data.query = filterQuery.value.trim()
  if (filterProject.value !== '') data.project_id = filterProject.value
  if (filterPriority.value !== '') data.priority = filterPriority.value
  if (filterAssignee.value) {
    if (filterAssignee.value === 'manager_self') {
      data.assignee_type = 'manager_self'
      data.assignee_id = 'local'
    } else {
      data.assignee_type = 'worker'
      data.assignee_id = filterAssignee.value
    }
  }
  try {
    const r = await request('board', 'issue.list', data)
    issues.value = r?.issues || []
    total.value = r?.total || 0
  } catch (e: any) {
    if (silent) console.warn('[IssueList] silent refresh failed:', e)
    else toast.error('加载 issue 失败: ' + e)
  }
}

async function loadStats() {
  try {
    const r = await request('board', 'stats', {})
    stats.value = r?.by_status || {}
  } catch {
    /* stats 失败不阻塞列表 */
  }
}

async function loadProjects() {
  try {
    const r = await request('board', 'project.list', {})
    projects.value = r?.projects || []
  } catch {
    projects.value = []
  }
}

async function refresh() {
  // loading 必须在 finally 置回 false（对齐 BoardKanban/ProjectPanel 惯例）。
  // 曾因漏写导致列表页永久停在 spinner 分支（v-if="loading"），创建成功后
  // 表格也永不渲染——「添加成功但 dashboard 不显示」的根因（2026-08-31）。
  loading.value = true
  try {
    await Promise.all([loadIssues(), loadStats(), loadProjects(), loadWorkerNodes()])
  } finally {
    loading.value = false
  }
}

// board-changed 推送（W2.5）：不闪 loading 的静默换新（refresh 保持零参
// 给 @click/@changed 用；本函数只给推送用）。stats/项目/节点加载器本就
// 静默吞错，仅 issue 列表需要显式 silent 分支。
function silentRefresh() {
  void Promise.all([loadIssues(true), loadStats(), loadProjects(), loadWorkerNodes()])
}
useBoardChanged(silentRefresh)

// --- 一键派发（W2.5：指派 ≠ 派发，列表行内直达）---
// 与 IssueDetailModal.dispatchIssue 同语义（显式 target = worker 指派）；
// 后端 dispatch_issue_core 仍是单一派发入口（状态/重复派发闸在那边）。
const DISPATCHABLE_STATUSES = ['backlog', 'todo', 'in_progress', 'in_review']

function canQuickDispatch(issue: IssueRow): boolean {
  return (
    issue.assignee === 'worker' &&
    !!issue.assignee_id &&
    DISPATCHABLE_STATUSES.includes(issue.status)
  )
}

const dispatchingId = ref<number | null>(null)

async function quickDispatch(issue: IssueRow) {
  if (dispatchingId.value !== null) return // 串行：一次只发一单
  dispatchingId.value = issue.id
  try {
    const r = await request('board', 'issue.dispatch', {
      id: issue.id,
      target: issue.assignee_id,
    })
    toast.success(`已派发 ${issue.number} → ${issue.assignee_id}（task ${(r?.task_id || '').slice(0, 8)}…），等 worker 回报后自动写回`)
    await refresh()
  } catch (e: any) {
    toast.error(`派发 ${issue.number} 失败: ` + e)
  } finally {
    dispatchingId.value = null
  }
}

// --- 创建 ---
function openCreate() {
  createForm.value = {
    title: '',
    description: '',
    priority: 1,
    assigneeType: '',
    assigneeId: '',
    projectId: '',
    acceptance: '',
  }
  showCreate.value = true
}

async function submitCreate() {
  if (!createForm.value.title.trim()) {
    toast.warn('请填写标题')
    return
  }
  if (createForm.value.assigneeType === 'worker' && !createForm.value.assigneeId.trim()) {
    toast.warn('worker 指派需要填节点 id')
    return
  }
  busy.value = true
  try {
    const payload: any = {
      title: createForm.value.title.trim(),
      description: createForm.value.description,
      priority: createForm.value.priority,
    }
    if (createForm.value.acceptance.trim()) payload.acceptance_criteria = createForm.value.acceptance.trim()
    if (createForm.value.projectId !== '') payload.project_id = createForm.value.projectId
    if (createForm.value.assigneeType) {
      payload.assignee_type = createForm.value.assigneeType
      payload.assignee_id =
        createForm.value.assigneeType === 'manager_self'
          ? 'local'
          : createForm.value.assigneeId.trim()
    }
    const r = await request('board', 'issue.create', payload)
    toast.success(`已创建 ${r?.issue?.number || ''}`)
    // 指派 ≠ 派发（W2.5）：指派给 worker 只是元数据，任务要到 worker 手里
    // 还需一步派发——创建成功即引导，别让单子静默躺在 backlog。
    if (createForm.value.assigneeType === 'worker') {
      toast.info(`已指派给 ${createForm.value.assigneeId.trim()}：点列表行的「派发」下发任务`)
    }
    showCreate.value = false
    await refresh()
  } catch (e: any) {
    toast.error('创建失败: ' + e)
  } finally {
    busy.value = false
  }
}

onMounted(refresh)
</script>

<template>
  <div>
    <!-- 统计条 -->
    <div v-if="Object.keys(stats).length" class="stats-row">
      <span v-for="s in STATUSES" :key="s.key" class="badge" :class="STATUS_BADGE[s.key]">
        {{ s.label }} {{ stats[s.key] || 0 }}
      </span>
    </div>

    <!-- 过滤条 -->
    <div class="filter-bar">
      <select class="form-select" v-model="filterStatus" style="max-width: 160px;" @change="loadIssues()">
        <option value="">全部状态</option>
        <option v-for="s in STATUSES" :key="s.key" :value="s.key">{{ s.label }}</option>
      </select>
      <select class="form-select" v-model="filterProject" style="max-width: 180px;" @change="loadIssues()">
        <option value="">全部项目</option>
        <option v-for="p in projects" :key="p.id" :value="p.id">{{ p.name }}</option>
      </select>
      <select class="form-select" v-model.number="filterPriority" style="max-width: 140px;" @change="loadIssues()">
        <option :value="''">全部优先级</option>
        <option :value="0">P0 低</option>
        <option :value="1">P1 中</option>
        <option :value="2">P2 高</option>
        <option :value="3">P3 紧急</option>
      </select>
      <select class="form-select" v-model="filterAssignee" style="max-width: 180px;" @change="loadIssues()">
        <option value="">全部指派</option>
        <option value="manager_self">manager（本机）</option>
        <option v-for="n in workerNodes" :key="n.id" :value="n.id">{{ n.name }}</option>
      </select>
      <input
        class="form-input"
        v-model="filterQuery"
        placeholder="搜索编号/标题…"
        style="max-width: 240px;"
        @keyup.enter="loadIssues()"
      />
      <button class="btn btn-sm" @click="loadIssues()">搜索</button>
      <button class="btn btn-sm" @click="refresh" title="刷新">↻</button>
      <button class="btn btn-sm btn-primary" style="margin-left: auto;" @click="openCreate">+ 新建 Issue</button>
      <span class="muted">共 {{ total }} 条</span>
    </div>

    <div v-if="loading" style="text-align: center; padding: var(--space-8);">
      <div class="spinner spinner-lg" style="margin: 0 auto;"></div>
    </div>

    <div v-else-if="issues.length === 0" class="empty-state">
      <h3>暂无 Issue</h3>
      <p>点击「新建 Issue」创建任务，或通过 `nemesisbot issue create` 从 CLI 创建</p>
    </div>

    <div v-else class="table-wrap">
      <table>
        <thead>
          <tr>
            <th>编号</th><th>标题</th><th>状态</th><th>优先级</th>
            <th>指派</th><th>项目</th><th>更新时间</th><th>操作</th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="issue in issues" :key="issue.id" class="issue-row" @click="openDetail(issue)">
            <td><code>{{ issue.number }}</code></td>
            <td style="font-weight: 500;">{{ issue.title }}</td>
            <td><span class="badge" :class="STATUS_BADGE[issue.status]">{{ statusLabel(issue.status) }}</span></td>
            <td><span class="badge" :class="PRIORITY_BADGE[issue.priority] || 'badge-neutral'">P{{ issue.priority }} {{ PRIORITY_LABEL[issue.priority] || '' }}</span></td>
            <td style="font-size: var(--text-sm);">
              {{ !issue.assignee ? '未指派' : issue.assignee === 'manager_self' ? 'manager（本机）' : `worker: ${issue.assignee_id}` }}
            </td>
            <td style="font-size: var(--text-sm); color: var(--text-muted);">
              {{ projects.find((p) => p.id === issue.project_id)?.name || '—' }}
            </td>
            <td style="font-size: var(--text-sm); color: var(--text-muted);">{{ fmtTime(issue.updated_at) }}</td>
            <td>
              <button
                v-if="canQuickDispatch(issue)"
                class="btn btn-sm btn-primary"
                :disabled="dispatchingId !== null"
                title="把任务下发给已指派的 worker（即 issue.dispatch）"
                @click.stop="quickDispatch(issue)"
              >
                {{ dispatchingId === issue.id ? '派发中…' : '派发' }}
              </button>
              <span v-else class="muted">—</span>
            </td>
          </tr>
        </tbody>
      </table>
    </div>

    <!-- 详情弹窗（共享组件；v-if 门控：issueId 置空必须收起弹层） -->
    <IssueDetailModal
      v-if="detailIssueId !== null"
      :issue-id="detailIssueId"
      @close="detailIssueId = null"
      @changed="refresh"
    />

    <!-- 新建 Issue 弹窗 -->
    <div v-if="showCreate" class="modal-backdrop" @click.self="showCreate = false">
      <div class="modal" style="max-width: 560px;">
        <div class="modal-header"><h3>新建 Issue</h3></div>
        <div class="modal-body">
          <div class="form-group">
            <label class="form-label">标题 *</label>
            <input class="form-input" v-model="createForm.title" placeholder="一句话描述任务" @keyup.enter="submitCreate" />
          </div>
          <div class="form-group">
            <label class="form-label">描述</label>
            <textarea class="form-textarea" v-model="createForm.description" style="min-height: 80px;" placeholder="背景、约束、参考资料…"></textarea>
          </div>
          <div class="form-group">
            <label class="form-label">优先级</label>
            <select class="form-select" v-model.number="createForm.priority" style="max-width: 160px;">
              <option :value="0">低</option>
              <option :value="1">中</option>
              <option :value="2">高</option>
              <option :value="3">紧急</option>
            </select>
          </div>
          <div class="form-group">
            <label class="form-label">指派（可选）</label>
            <div class="assign-row">
              <select class="form-select" v-model="createForm.assigneeType" style="max-width: 180px;">
                <option value="">暂不指派</option>
                <option value="manager_self">manager（本机）</option>
                <option value="worker">worker 节点</option>
              </select>
              <!-- 有在线节点列表 → 下拉选；集群不可用 → 回退手输 -->
              <select
                v-if="createForm.assigneeType === 'worker' && workerNodes.length"
                class="form-select"
                v-model="createForm.assigneeId"
                style="max-width: 260px;"
              >
                <option value="">选择 worker…</option>
                <option v-for="n in workerNodes" :key="n.id" :value="n.id">{{ workerLabel(n) }}</option>
              </select>
              <input
                v-else-if="createForm.assigneeType === 'worker'"
                class="form-input"
                v-model="createForm.assigneeId"
                placeholder="节点 id"
                style="max-width: 200px;"
              />
            </div>
          </div>
          <div class="form-group">
            <label class="form-label">项目（可选）</label>
            <select class="form-select" v-model="createForm.projectId" style="max-width: 240px;">
              <option value="">无</option>
              <option v-for="p in projects" :key="p.id" :value="p.id">{{ p.name }}</option>
            </select>
          </div>
          <div class="form-group">
            <label class="form-label">验收标准（可选）</label>
            <textarea class="form-textarea" v-model="createForm.acceptance" style="min-height: 60px;" placeholder="怎样算完成（自由文本）"></textarea>
          </div>
        </div>
        <div class="modal-footer">
          <button class="btn" @click="showCreate = false">取消</button>
          <button class="btn btn-primary" :disabled="busy" @click="submitCreate">创建</button>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.muted {
  color: var(--text-muted);
  font-size: var(--text-sm);
}
.stats-row {
  display: flex;
  flex-wrap: wrap;
  gap: var(--space-2);
  margin-bottom: var(--space-4);
}
.filter-bar {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: var(--space-2);
  margin-bottom: var(--space-4);
}
.filter-bar > .muted {
  margin-left: var(--space-2);
}
.issue-row {
  cursor: pointer;
}
.assign-row {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: var(--space-2);
}
</style>
