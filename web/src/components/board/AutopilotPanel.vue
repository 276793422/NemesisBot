<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { useWSAPI } from '../../composables/useWSAPI'
import { useToast } from '../../composables/useToast'
import { fmtTime, PRIORITY_LABEL, PRIORITY_BADGE, statusLabel, STATUS_BADGE } from './boardMeta'

// 自动化面板（W2 P4）：autopilot 规则列表 + 创建/编辑 + 启停 + 立即运行 +
// run 历史。规则到点由 gateway 的 cron on_job（board-ap:{id}）按模板建单，
// 派发目标非空时自动派发给对应 worker。后端唯一真相源：
// crates/nemesis-web/src/handlers/board.rs（autopilot.* 命令）。

const { request } = useWSAPI()
const toast = useToast()

interface Autopilot {
  id: number
  name: string
  title: string
  cron: string
  description: string
  priority: number
  project_id: number | null
  target: string
  enabled: boolean
  cron_job_id: string | null
  last_run_at: number | null
  created_at: number
  updated_at: number
}

interface RunIssue {
  id: number
  number: string
  title: string
  status: string
  created_at: number
}

const loading = ref(true)
const autopilots = ref<Autopilot[]>([])
const busy = ref(false)

// 创建/编辑共用弹窗（editing=null = 创建模式）。
const showForm = ref(false)
const editing = ref<Autopilot | null>(null)
const form = ref({
  name: '',
  cron: '',
  title: '',
  description: '',
  priority: 1,
  target: '',
  enabled: true,
})

// run 历史弹窗。
const viewingRuns = ref<Autopilot | null>(null)
const runs = ref<RunIssue[]>([])
const runsLoading = ref(false)

async function load() {
  loading.value = true
  try {
    const r = await request('board', 'autopilot.list', {})
    autopilots.value = r?.autopilots || []
  } catch (e: any) {
    toast.error('加载自动化规则失败: ' + e)
  } finally {
    loading.value = false
  }
}

function openCreate() {
  editing.value = null
  form.value = { name: '', cron: '', title: '', description: '', priority: 1, target: '', enabled: true }
  showForm.value = true
}

function openEdit(ap: Autopilot) {
  editing.value = ap
  form.value = {
    name: ap.name,
    cron: ap.cron,
    title: ap.title,
    description: ap.description || '',
    priority: ap.priority,
    target: ap.target || '',
    enabled: ap.enabled,
  }
  showForm.value = true
}

async function submitForm() {
  if (!form.value.name.trim() || !form.value.cron.trim() || !form.value.title.trim()) {
    toast.warn('请填写规则名、cron 表达式和标题模板')
    return
  }
  busy.value = true
  try {
    if (editing.value) {
      await request('board', 'autopilot.update', { id: editing.value.id, ...form.value })
      toast.success('已更新规则')
    } else {
      await request('board', 'autopilot.create', { ...form.value })
      toast.success('已创建规则')
    }
    showForm.value = false
    await load()
  } catch (e: any) {
    toast.error((editing.value ? '更新失败: ' : '创建失败: ') + e)
  } finally {
    busy.value = false
  }
}

async function toggle(ap: Autopilot) {
  try {
    await request('board', 'autopilot.update', { id: ap.id, enabled: !ap.enabled })
    toast.success(ap.enabled ? '已停用' : '已启用')
    await load()
  } catch (e: any) {
    toast.error('操作失败: ' + e)
  }
}

async function remove(ap: Autopilot) {
  if (!window.confirm(`确认删除自动化规则「${ap.name}」？已创建的 issue 不受影响`)) return
  try {
    await request('board', 'autopilot.remove', { id: ap.id })
    toast.success('已删除规则')
    await load()
  } catch (e: any) {
    toast.error('删除失败: ' + e)
  }
}

async function runNow(ap: Autopilot) {
  busy.value = true
  try {
    const r = await request('board', 'autopilot.run', { id: ap.id })
    toast.success(
      r?.dispatch
        ? `已建单 ${r.issue_number} 并派发`
        : `已建单 ${r.issue_number}（未配置派发目标）`,
    )
    await load()
  } catch (e: any) {
    toast.error('运行失败: ' + e)
  } finally {
    busy.value = false
  }
}

async function openRuns(ap: Autopilot) {
  viewingRuns.value = ap
  runsLoading.value = true
  runs.value = []
  try {
    const r = await request('board', 'autopilot.runs', { id: ap.id })
    runs.value = r?.issues || []
  } catch (e: any) {
    toast.error('加载运行历史失败: ' + e)
  } finally {
    runsLoading.value = false
  }
}

onMounted(load)
</script>

<template>
  <div>
    <div class="panel-toolbar">
      <button class="btn btn-primary" @click="openCreate">+ 新建规则</button>
      <span class="muted">
        共 {{ autopilots.length }} 条规则（启用 {{ autopilots.filter((a) => a.enabled).length }}）
      </span>
    </div>

    <div v-if="loading" style="text-align: center; padding: var(--space-8);">
      <div class="spinner spinner-lg" style="margin: 0 auto;"></div>
    </div>

    <div v-else-if="autopilots.length === 0" class="empty-state">
      <h3>暂无自动化规则</h3>
      <p>按 cron 计划自动建单/派发：如每天 9 点生成站会任务（0 9 * * *），标题模板支持 {&#8203;date} 占位</p>
    </div>

    <div v-else class="table-wrap">
      <table>
        <thead>
          <tr>
            <th>规则</th>
            <th>计划</th>
            <th>优先级</th>
            <th>派发目标</th>
            <th>状态</th>
            <th>最近运行</th>
            <th style="width: 280px;">操作</th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="ap in autopilots" :key="ap.id">
            <td>
              <div style="font-weight: 500;">{{ ap.name }}</div>
              <div class="muted">{{ ap.title }}</div>
            </td>
            <td><code>{{ ap.cron }}</code></td>
            <td>
              <span class="badge" :class="PRIORITY_BADGE[ap.priority] || 'badge-neutral'">
                {{ PRIORITY_LABEL[ap.priority] ?? ap.priority }}
              </span>
            </td>
            <td>
              <span v-if="ap.target" class="badge badge-info">{{ ap.target }}</span>
              <span v-else class="muted">仅建单</span>
            </td>
            <td>
              <span class="badge" :class="ap.enabled ? 'badge-success' : 'badge-neutral'">
                {{ ap.enabled ? '启用' : '停用' }}
              </span>
            </td>
            <td class="muted">{{ ap.last_run_at ? fmtTime(ap.last_run_at) : '从未运行' }}</td>
            <td class="actions-cell">
              <button class="btn btn-sm btn-primary" :disabled="busy" @click="runNow(ap)">立即运行</button>
              <button class="btn btn-sm" @click="openRuns(ap)">历史</button>
              <button class="btn btn-sm" @click="openEdit(ap)">编辑</button>
              <button class="btn btn-sm" @click="toggle(ap)">{{ ap.enabled ? '停用' : '启用' }}</button>
              <button class="btn btn-sm btn-danger" @click="remove(ap)">删除</button>
            </td>
          </tr>
        </tbody>
      </table>
    </div>

    <!-- 创建/编辑弹窗 -->
    <div v-if="showForm" class="modal-backdrop" @click.self="showForm = false">
      <div class="modal" style="max-width: 520px;">
        <div class="modal-header"><h3>{{ editing ? '编辑规则' : '新建自动化规则' }}</h3></div>
        <div class="modal-body">
          <div class="form-group">
            <label class="form-label">规则名 *</label>
            <input class="form-input" v-model="form.name" placeholder="如：每日站会任务" />
          </div>
          <div class="form-group">
            <label class="form-label">cron 表达式 *</label>
            <input class="form-input" v-model="form.cron" placeholder="0 9 * * *" />
            <div class="muted" style="margin-top: var(--space-1);">标准 crontab 五段表达式，按服务器本地时区触发</div>
          </div>
          <div class="form-group">
            <label class="form-label">标题模板 *</label>
            <input class="form-input" v-model="form.title" placeholder="每日站会 {date}" />
            <div class="muted" style="margin-top: var(--space-1);">{&#8203;date} 会替换为建单日（YYYY-MM-DD）</div>
          </div>
          <div class="form-group">
            <label class="form-label">描述</label>
            <textarea class="form-textarea" v-model="form.description" style="min-height: 60px;"></textarea>
          </div>
          <div class="form-group">
            <label class="form-label">优先级</label>
            <select class="form-input" v-model.number="form.priority">
              <option :value="0">低</option>
              <option :value="1">中</option>
              <option :value="2">高</option>
              <option :value="3">紧急</option>
            </select>
          </div>
          <div class="form-group">
            <label class="form-label">派发目标（worker 节点名，可空）</label>
            <input class="form-input" v-model="form.target" placeholder="留空 = 只建单不派发" />
            <div class="muted" style="margin-top: var(--space-1);">配置后每次触发会把 issue 自动派发给该节点执行（需集群运行）</div>
          </div>
          <label class="muted" style="display: flex; align-items: center; gap: var(--space-1); cursor: pointer;">
            <input type="checkbox" v-model="form.enabled" />
            创建后立即启用
          </label>
        </div>
        <div class="modal-footer">
          <button class="btn" @click="showForm = false">取消</button>
          <button class="btn btn-primary" :disabled="busy" @click="submitForm">
            {{ editing ? '保存' : '创建' }}
          </button>
        </div>
      </div>
    </div>

    <!-- run 历史弹窗 -->
    <div v-if="viewingRuns" class="modal-backdrop" @click.self="viewingRuns = null">
      <div class="modal" style="max-width: 560px;">
        <div class="modal-header"><h3>运行历史 — {{ viewingRuns.name }}</h3></div>
        <div class="modal-body">
          <div v-if="runsLoading" style="text-align: center; padding: var(--space-6);">
            <div class="spinner spinner-lg" style="margin: 0 auto;"></div>
          </div>
          <div v-else-if="runs.length === 0" class="empty-state">
            <h3>还没有运行记录</h3>
            <p>到点自动触发或手动「立即运行」后，生成的 issue 会列在这里</p>
          </div>
          <div v-else class="runs-list">
            <div v-for="ri in runs" :key="ri.id" class="runs-item">
              <span class="badge" :class="STATUS_BADGE[ri.status] || 'badge-neutral'">{{ statusLabel(ri.status) }}</span>
              <strong>{{ ri.number }}</strong>
              <span class="runs-title">{{ ri.title }}</span>
              <span class="muted" style="margin-left: auto;">{{ fmtTime(ri.created_at) }}</span>
            </div>
          </div>
        </div>
        <div class="modal-footer">
          <button class="btn" @click="viewingRuns = null">关闭</button>
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
.panel-toolbar {
  display: flex;
  align-items: center;
  gap: var(--space-3);
  margin-bottom: var(--space-4);
}
.actions-cell {
  white-space: nowrap;
}
.actions-cell .btn + .btn {
  margin-left: var(--space-1);
}
.runs-list {
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
}
.runs-item {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  background: var(--bg-secondary);
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
  padding: var(--space-2) var(--space-3);
}
.runs-title {
  font-size: var(--text-sm);
}
</style>
