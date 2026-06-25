<script setup lang="ts">
import { ref } from 'vue'
import { storeToRefs } from 'pinia'
import { useWorkflowStore } from '../../stores/workflow'

const store = useWorkflowStore()
const { runs, runsLoading, runsError, selectedRun } = storeToRefs(store)

const filterWorkflow = ref('')
const filterState = ref('')
const limit = ref(50)

function applyFilter() {
  store.fetchRuns({
    workflow_name: filterWorkflow.value || undefined,
    state: filterState.value || undefined,
    limit: limit.value,
  })
}

function stateClass(state: string): string {
  switch (state) {
    case 'Completed': return 'state-ok'
    case 'Failed': return 'state-err'
    case 'Running': return 'state-run'
    case 'Cancelled': return 'state-cancel'
    case 'Waiting': return 'state-wait'
    default: return 'state-pending'
  }
}

function formatDate(s: string | null): string {
  if (!s) return '—'
  try {
    return new Date(s).toLocaleString()
  } catch {
    return s
  }
}
</script>

<template>
  <div class="wf-history">
    <div class="filter-bar">
      <input
        v-model="filterWorkflow"
        class="filter-input"
        placeholder="工作流名称（可选）"
      />
      <select v-model="filterState" class="filter-select">
        <option value="">全部状态</option>
        <option value="Pending">Pending</option>
        <option value="Running">Running</option>
        <option value="Waiting">Waiting</option>
        <option value="Completed">Completed</option>
        <option value="Failed">Failed</option>
        <option value="Cancelled">Cancelled</option>
      </select>
      <input v-model.number="limit" type="number" min="1" max="500" class="filter-limit" />
      <button class="btn btn-primary" @click="applyFilter">查询</button>
    </div>

    <div v-if="runsLoading" class="empty">⟳ 加载执行历史...</div>
    <div v-else-if="runsError" class="empty error">⚠ {{ runsError }}</div>
    <div v-else-if="runs.length === 0" class="empty">无匹配的执行记录</div>

    <div v-else class="history-body">
      <table class="runs-table">
        <thead>
          <tr>
            <th>执行 ID</th>
            <th>工作流</th>
            <th>状态</th>
            <th>开始时间</th>
            <th>结束时间</th>
            <th>错误</th>
            <th class="col-actions">操作</th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="run in runs" :key="run.execution_id">
            <td class="col-id">{{ run.execution_id.substring(0, 8) }}</td>
            <td>{{ run.workflow_name }}</td>
            <td>
              <span class="state-pill" :class="stateClass(run.state)">{{ run.state }}</span>
            </td>
            <td class="col-date">{{ formatDate(run.started_at) }}</td>
            <td class="col-date">{{ formatDate(run.ended_at) }}</td>
            <td class="col-err">
              <span v-if="run.has_error" class="err-mark">⚠</span>
              <span v-else>—</span>
            </td>
            <td class="col-actions">
              <button class="btn btn-small" @click="store.fetchRunDetail(run.execution_id)">
                详情
              </button>
              <button
                v-if="run.state === 'Running' || run.state === 'Waiting'"
                class="btn btn-small"
                @click="store.cancelRun(run.execution_id)"
              >
                取消
              </button>
            </td>
          </tr>
        </tbody>
      </table>

      <div v-if="selectedRun" class="run-detail-panel">
        <div class="detail-header">
          <h4>执行详情：{{ selectedRun.execution_id }}</h4>
          <button class="btn btn-small" @click="store.selectedRun = null">关闭</button>
        </div>
        <div class="detail-meta">
          <div>工作流：{{ selectedRun.workflow_name }}</div>
          <div>状态：<span class="state-pill" :class="stateClass(selectedRun.state)">{{ selectedRun.state }}</span></div>
          <div>开始：{{ formatDate(selectedRun.started_at) }}</div>
          <div>结束：{{ formatDate(selectedRun.ended_at) }}</div>
          <div v-if="selectedRun.error" class="detail-error">错误：{{ selectedRun.error }}</div>
        </div>
        <div class="detail-nodes">
          <div class="detail-section-title">节点结果 ({{ selectedRun.node_results.length }})</div>
          <table class="nodes-table">
            <thead>
              <tr><th>节点</th><th>状态</th><th>开始</th><th>结束</th><th>错误</th></tr>
            </thead>
            <tbody>
              <tr v-for="(nr, i) in selectedRun.node_results" :key="i">
                <td>{{ nr.node_id }}</td>
                <td>
                  <span class="state-pill" :class="stateClass(nr.state)">{{ nr.state }}</span>
                </td>
                <td class="col-date">{{ formatDate(nr.started_at ?? null) }}</td>
                <td class="col-date">{{ formatDate(nr.ended_at ?? null) }}</td>
                <td class="col-err">{{ nr.error || '—' }}</td>
              </tr>
            </tbody>
          </table>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.wf-history {
  display: flex;
  flex-direction: column;
  height: 100%;
  padding: var(--space-3);
  gap: var(--space-3);
  overflow: auto;
}

.filter-bar {
  display: flex;
  gap: var(--space-2);
  align-items: center;
  flex-wrap: wrap;
}

.filter-input,
.filter-select,
.filter-limit {
  padding: var(--space-1) var(--space-2);
  background: var(--bg-primary);
  border: 1px solid var(--border);
  border-radius: var(--radius-sm);
  color: var(--text-primary);
  font-size: var(--text-sm);
}

.filter-input {
  flex: 1;
  min-width: 200px;
}

.filter-limit {
  width: 70px;
}

.empty {
  padding: var(--space-6);
  text-align: center;
  color: var(--text-muted);
}

.empty.error {
  color: var(--danger, #e74c3c);
}

.history-body {
  display: flex;
  flex-direction: column;
  gap: var(--space-3);
}

.runs-table,
.nodes-table {
  width: 100%;
  border-collapse: collapse;
  font-size: var(--text-sm);
}

.runs-table th,
.runs-table td,
.nodes-table th,
.nodes-table td {
  padding: var(--space-2) var(--space-3);
  text-align: left;
  border-bottom: 1px solid var(--border);
}

.runs-table th,
.nodes-table th {
  font-weight: 600;
  color: var(--text-secondary);
  background: var(--bg-secondary);
}

.col-id {
  font-family: monospace;
  color: var(--text-muted);
}

.col-date {
  font-size: var(--text-xs);
  color: var(--text-muted);
  font-variant-numeric: tabular-nums;
  white-space: nowrap;
}

.col-err {
  font-size: var(--text-xs);
  color: var(--danger, #e74c3c);
  max-width: 300px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.err-mark {
  color: var(--danger, #e74c3c);
}

.col-actions {
  display: flex;
  gap: var(--space-1);
  justify-content: flex-end;
  white-space: nowrap;
}

.state-pill {
  display: inline-block;
  padding: 1px var(--space-2);
  border-radius: var(--radius-sm);
  font-size: var(--text-xs);
  font-weight: 500;
}

.state-ok { background: rgba(46, 204, 113, 0.15); color: var(--success, #2ecc71); }
.state-err { background: rgba(231, 76, 60, 0.15); color: var(--danger, #e74c3c); }
.state-run { background: rgba(52, 152, 219, 0.15); color: var(--info, #3498db); }
.state-wait { background: rgba(243, 156, 18, 0.15); color: var(--warning, #f39c12); }
.state-cancel { background: rgba(149, 165, 166, 0.15); color: var(--text-muted); }
.state-pending { background: rgba(149, 165, 166, 0.1); color: var(--text-muted); }

.run-detail-panel {
  background: var(--bg-secondary);
  border-radius: var(--radius-md);
  padding: var(--space-3);
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
}

.detail-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
}

.detail-meta {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(200px, 1fr));
  gap: var(--space-2);
  font-size: var(--text-sm);
}

.detail-error {
  color: var(--danger, #e74c3c);
  grid-column: 1 / -1;
}

.detail-section-title {
  font-weight: 600;
  margin-top: var(--space-2);
  color: var(--text-secondary);
}

.btn-small {
  padding: 2px var(--space-2);
  font-size: var(--text-xs);
}
</style>
