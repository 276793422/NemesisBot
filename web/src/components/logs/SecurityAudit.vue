<script setup lang="ts">
import { ref, computed } from 'vue'
import { formatTime, type AuditEntry, type RiskLevel } from './mockData'

const props = defineProps<{
  entries: AuditEntry[]
}>()

const riskFilter = ref<Set<RiskLevel>>(new Set())
const operationFilter = ref('all')
const decisionFilter = ref('all')
const selectedId = ref<string | null>(null)

const filtered = computed(() => {
  return props.entries.filter(e => {
    if (riskFilter.value.size > 0 && !riskFilter.value.has(e.risk_level)) return false
    if (operationFilter.value !== 'all' && e.operation !== operationFilter.value) return false
    if (decisionFilter.value !== 'all' && e.decision !== decisionFilter.value) return false
    return true
  })
})

const selected = computed(() => {
  return props.entries.find(e => e.id === selectedId.value) || null
})

const allOperations = computed(() => {
  return Array.from(new Set(props.entries.map(e => e.operation))).sort()
})

function toggleRisk(r: RiskLevel) {
  if (riskFilter.value.has(r)) riskFilter.value.delete(r)
  else riskFilter.value.add(r)
  riskFilter.value = new Set(riskFilter.value)
}

function riskClass(r: RiskLevel): string {
  return r.toLowerCase()
}

function riskIcon(r: RiskLevel): string {
  return r === 'CRITICAL' ? '🔴' : r === 'HIGH' ? '🟠' : r === 'MEDIUM' ? '🟡' : '🟢'
}

function resultIcon(result: string): string {
  return result === 'allow' ? '✓' : '✗'
}

function exportFiltered() {
  const text = filtered.value.map(e =>
    `[${e.timestamp}] [${e.risk_level}] [${e.decision}] ${e.operation}: ${e.target} - ${e.reason}`
  ).join('\n')
  const blob = new Blob([text], { type: 'text/plain' })
  const url = URL.createObjectURL(blob)
  const a = document.createElement('a')
  a.href = url
  a.download = `audit-${Date.now()}.txt`
  a.click()
  URL.revokeObjectURL(url)
}
</script>

<template>
  <div class="audit-view">
    <!-- 筛选条 -->
    <div class="audit-toolbar">
      <div class="filter-group">
        <span class="filter-label">风险级别</span>
        <div class="chip-row">
          <button
            v-for="r in ['LOW', 'MEDIUM', 'HIGH', 'CRITICAL'] as RiskLevel[]"
            :key="r"
            class="chip chip-risk"
            :class="[{ active: riskFilter.has(r), [r.toLowerCase()]: true }]"
            @click="toggleRisk(r)"
          >{{ riskIcon(r) }} {{ r }}</button>
        </div>
      </div>

      <div class="filter-group">
        <span class="filter-label">操作</span>
        <select class="form-select" v-model="operationFilter">
          <option value="all">全部</option>
          <option v-for="op in allOperations" :key="op" :value="op">{{ op }}</option>
        </select>
      </div>

      <div class="filter-group">
        <span class="filter-label">决策</span>
        <select class="form-select" v-model="decisionFilter">
          <option value="all">全部</option>
          <option value="allow">允许</option>
          <option value="deny">拒绝</option>
        </select>
      </div>

      <div class="filter-actions">
        <span class="count-info">显示 {{ filtered.length }} / {{ entries.length }}</span>
        <button class="btn btn-sm btn-ghost" @click="exportFiltered">⤓ 导出</button>
      </div>
    </div>

    <!-- 表格 -->
    <div class="audit-table-wrap scrollable">
      <table>
        <thead>
          <tr>
            <th style="width: 160px;">时间</th>
            <th style="width: 140px;">操作</th>
            <th style="width: 110px;">风险级别</th>
            <th>目标</th>
            <th style="width: 60px;">结果</th>
            <th style="width: 90px;">决策</th>
          </tr>
        </thead>
        <tbody>
          <tr
            v-for="e in filtered"
            :key="e.id"
            :class="{ selected: selected && selected.id === e.id }"
            @click="selectedId = e.id"
          >
            <td class="cell-time">{{ formatTime(e.timestamp, true) }}</td>
            <td class="cell-op">{{ e.operation }}</td>
            <td>
              <span class="risk-badge" :class="riskClass(e.risk_level)">
                {{ riskIcon(e.risk_level) }} {{ e.risk_level }}
              </span>
            </td>
            <td class="cell-target" :title="e.target">{{ e.target }}</td>
            <td>
              <span class="result-icon" :class="e.result">{{ resultIcon(e.result) }}</span>
            </td>
            <td>
              <span class="decision-badge" :class="e.decision">{{ e.decision }}</span>
            </td>
          </tr>
          <tr v-if="filtered.length === 0">
            <td colspan="6" class="empty-state"><p>暂无审计事件</p></td>
          </tr>
        </tbody>
      </table>
    </div>

    <!-- 详情侧滑 -->
    <Transition name="slide">
      <div v-if="selected" class="audit-detail">
        <div class="detail-header">
          <h3>事件详情</h3>
          <button class="btn btn-sm btn-ghost" @click="selectedId = null">✕</button>
        </div>
        <div class="detail-body scrollable">
          <div class="detail-row"><span>event_id</span><code>{{ selected.id }}</code></div>
          <div class="detail-row"><span>timestamp</span><code>{{ selected.timestamp }}</code></div>
          <div class="detail-row"><span>operation</span><code>{{ selected.operation }}</code></div>
          <div class="detail-row">
            <span>risk</span>
            <span class="risk-badge" :class="riskClass(selected.risk_level)">
              {{ riskIcon(selected.risk_level) }} {{ selected.risk_level }}
            </span>
          </div>
          <div class="detail-row"><span>decision</span><code>{{ selected.decision }}</code></div>
          <div class="detail-row" v-if="selected.user"><span>user</span><code>{{ selected.user }}</code></div>
          <div class="detail-row"><span>target</span><code>{{ selected.target }}</code></div>
          <div class="detail-row" v-if="selected.reason"><span>reason</span><code>{{ selected.reason }}</code></div>
          <div class="detail-row" v-if="selected.policy"><span>policy</span><code>{{ selected.policy }}</code></div>

          <div class="raw-json">
            <div class="raw-title">完整 AuditEvent JSON</div>
            <pre>{{ JSON.stringify(selected.raw || {
              event_id: selected.id,
              timestamp: selected.timestamp,
              operation: selected.operation,
              risk_level: selected.risk_level,
              decision: selected.decision,
              target: selected.target,
              reason: selected.reason,
              policy: selected.policy,
            }, null, 2) }}</pre>
          </div>

          <div class="detail-actions">
            <button class="btn btn-sm btn-ghost">📋 复制</button>
            <button class="btn btn-sm btn-ghost">🔍 关联日志</button>
          </div>
        </div>
      </div>
    </Transition>
  </div>
</template>

<style scoped>
.audit-view {
  display: flex;
  flex-direction: column;
  height: 100%;
  background: var(--bg-primary);
  position: relative;
}

.audit-toolbar {
  display: flex;
  gap: var(--space-4);
  padding: var(--space-3) var(--space-4);
  background: var(--bg-secondary);
  border-bottom: 1px solid var(--border-light);
  align-items: flex-end;
  flex-wrap: wrap;
}

.filter-group {
  display: flex;
  flex-direction: column;
  gap: var(--space-1);
}

.filter-label {
  font-size: var(--text-xs);
  color: var(--text-muted);
  text-transform: uppercase;
  letter-spacing: 0.5px;
}

.chip-row {
  display: flex;
  gap: var(--space-1);
}

.chip {
  padding: 4px 10px;
  border: 1px solid var(--border);
  background: var(--bg-primary);
  color: var(--text-secondary);
  border-radius: var(--radius-sm);
  font-size: var(--text-xs);
  cursor: pointer;
  transition: all 0.15s;
}

.chip-risk.low.active      { background: #10b981; color: white; border-color: #10b981; }
.chip-risk.medium.active   { background: #eab308; color: white; border-color: #eab308; }
.chip-risk.high.active     { background: #f97316; color: white; border-color: #f97316; }
.chip-risk.critical.active { background: #ef4444; color: white; border-color: #ef4444; }

.filter-actions {
  margin-left: auto;
  display: flex;
  align-items: center;
  gap: var(--space-3);
}

.count-info {
  font-size: var(--text-xs);
  color: var(--text-muted);
}

.audit-table-wrap {
  flex: 1;
  overflow: auto;
}

table {
  width: 100%;
  border-collapse: collapse;
}

th, td {
  padding: var(--space-2) var(--space-3);
  text-align: left;
  border-bottom: 1px solid var(--border-light);
  font-size: var(--text-sm);
}

th {
  background: var(--bg-secondary);
  font-weight: 600;
  font-size: var(--text-xs);
  text-transform: uppercase;
  color: var(--text-muted);
  position: sticky;
  top: 0;
  z-index: 1;
}

tbody tr {
  cursor: pointer;
}

tbody tr:hover { background: var(--bg-hover); }

tbody tr.selected {
  background: var(--accent-muted);
}

.cell-time {
  font-family: monospace;
  font-size: var(--text-xs);
  color: var(--text-muted);
}

.cell-op {
  font-family: monospace;
}

.cell-target {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  max-width: 300px;
  font-family: monospace;
  font-size: var(--text-xs);
}

.risk-badge {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 2px 8px;
  border-radius: var(--radius-sm);
  font-size: var(--text-xs);
  font-weight: 600;
}

.risk-badge.low      { background: rgba(16,185,129,0.15); color: #10b981; }
.risk-badge.medium   { background: rgba(234,179,8,0.15); color: #eab308; }
.risk-badge.high     { background: rgba(249,115,22,0.15); color: #f97316; }
.risk-badge.critical { background: rgba(239,68,68,0.15); color: #ef4444; }

.result-icon {
  display: inline-block;
  width: 20px;
  height: 20px;
  border-radius: 50%;
  text-align: center;
  line-height: 20px;
  font-weight: 700;
}

.result-icon.allow {
  background: rgba(16,185,129,0.2);
  color: #10b981;
}

.result-icon.deny {
  background: rgba(239,68,68,0.2);
  color: #ef4444;
}

.decision-badge {
  padding: 2px 6px;
  border-radius: var(--radius-sm);
  font-size: var(--text-xs);
  font-weight: 600;
}

.decision-badge.allow { background: rgba(16,185,129,0.15); color: #10b981; }
.decision-badge.deny  { background: rgba(239,68,68,0.15); color: #ef4444; }

.audit-detail {
  position: absolute;
  top: 0;
  right: 0;
  bottom: 0;
  width: 480px;
  background: var(--bg-secondary);
  border-left: 1px solid var(--border-light);
  box-shadow: -4px 0 12px rgba(0,0,0,0.08);
  display: flex;
  flex-direction: column;
  z-index: 10;
}

.detail-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: var(--space-3) var(--space-4);
  border-bottom: 1px solid var(--border-light);
}

.detail-body {
  flex: 1;
  padding: var(--space-3) var(--space-4);
  overflow-y: auto;
}

.detail-row {
  display: flex;
  justify-content: space-between;
  padding: var(--space-1) 0;
  border-bottom: 1px solid var(--border-light);
  font-size: var(--text-sm);
}

.detail-row span:first-child {
  color: var(--text-muted);
  text-transform: uppercase;
  font-size: var(--text-xs);
}

.detail-row code {
  font-family: monospace;
  font-size: var(--text-xs);
  color: var(--text-primary);
  background: var(--bg-tertiary);
  padding: 1px 6px;
  border-radius: var(--radius-sm);
  max-width: 280px;
  word-break: break-all;
  text-align: right;
}

.raw-json {
  margin-top: var(--space-3);
}

.raw-title {
  font-size: var(--text-xs);
  color: var(--text-muted);
  text-transform: uppercase;
  margin-bottom: var(--space-1);
}

.raw-json pre {
  background: var(--bg-primary);
  padding: var(--space-3);
  border-radius: var(--radius-md);
  font-size: var(--text-xs);
  font-family: 'Cascadia Code', monospace;
  overflow-x: auto;
  margin: 0;
}

.detail-actions {
  display: flex;
  gap: var(--space-2);
  margin-top: var(--space-3);
}

.slide-enter-active, .slide-leave-active {
  transition: transform 0.2s ease;
}

.slide-enter-from, .slide-leave-to {
  transform: translateX(100%);
}

.scrollable { overflow-y: auto; }
</style>
