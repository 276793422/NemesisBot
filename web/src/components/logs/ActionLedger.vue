<script setup lang="ts">
// 行动账本（Logs Dashboard 第 5 页签，2026-10-14 AI 原生行动代理）：
// connectors.receipts.list 只读视图。进入页签（组件挂载，v-else-if 链）时经
// WSAPI 拉最近 100 条回执；connector 过滤下拉选项来自回执去重。
// 与 ConnectorsView 的回执区块同一数据源，两处都只读互不写。
import { ref, computed, onMounted } from 'vue'
import { useWSAPI } from '../../composables/useWSAPI'
import {
  connectorRiskClass, connectorOutcomeClass, connectorOutcomeLabel,
  type ConnectorReceipt,
} from './mockData'

const { request } = useWSAPI()

const receipts = ref<ConnectorReceipt[]>([])
const loading = ref(false)
const loadError = ref('')
const connectorFilter = ref('all')

const connectorOptions = computed(() =>
  Array.from(new Set(receipts.value.map((r) => r.connector))).sort(),
)

const filtered = computed(() =>
  connectorFilter.value === 'all'
    ? receipts.value
    : receipts.value.filter((r) => r.connector === connectorFilter.value),
)

// RFC3339 → 本地时间展示；解析失败原样回显
function formatTs(ts: string): string {
  const d = new Date(ts)
  return isNaN(d.getTime()) ? ts : d.toLocaleString()
}

async function load() {
  loading.value = true
  loadError.value = ''
  try {
    const res = await request('connectors', 'receipts.list', { limit: 100 })
    receipts.value = Array.isArray(res?.receipts) ? res.receipts : []
  } catch (e: any) {
    loadError.value = String(e?.message ?? e)
  } finally {
    loading.value = false
  }
}

onMounted(load)
</script>

<template>
  <div class="ledger-view">
    <!-- 工具条：connector 过滤 + 刷新 -->
    <div class="ledger-toolbar">
      <div class="filter-group">
        <span class="filter-label">Connector</span>
        <select v-model="connectorFilter" class="form-select" data-test="ledger-connector-filter">
          <option value="all">全部</option>
          <option v-for="c in connectorOptions" :key="c" :value="c">{{ c }}</option>
        </select>
      </div>
      <div class="filter-actions">
        <span class="count-info">显示 {{ filtered.length }} / {{ receipts.length }}</span>
        <button class="btn btn-sm" data-test="ledger-refresh" :disabled="loading" @click="load">刷新</button>
      </div>
    </div>

    <!-- 表格 -->
    <div class="ledger-table-wrap">
      <div v-if="loadError" class="ledger-error" data-test="ledger-error">
        回执加载失败（connectors.receipts.list）— {{ loadError }}
      </div>
      <div v-else-if="!loading && receipts.length === 0" class="empty-state" data-test="ledger-empty">
        <p>暂无回执。connector 操作执行（或被拒绝/限额拦截）后，每笔都会在这里留底。</p>
      </div>
      <table v-else data-test="ledger-table">
        <thead>
          <tr>
            <th style="width: 150px;">时间</th>
            <th style="width: 130px;">connector</th>
            <th style="width: 130px;">op</th>
            <th style="width: 90px;">风险</th>
            <th style="width: 100px;">结果</th>
            <th>摘要</th>
            <th style="width: 180px;">回执值</th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="r in filtered" :key="r.receipt_id" data-test="ledger-row">
            <td class="cell-time">{{ formatTs(r.ts) }}</td>
            <td class="cell-mono">{{ r.connector }}</td>
            <td class="cell-mono">{{ r.op }}</td>
            <td><span class="risk-badge" :class="connectorRiskClass(r.risk)">{{ r.risk }}</span></td>
            <td><span class="badge" :class="connectorOutcomeClass(r.outcome)">{{ connectorOutcomeLabel(r.outcome) }}</span></td>
            <td class="cell-summary" :title="r.summary">{{ r.summary }}</td>
            <td class="cell-mono">
              <template v-if="r.receipt_value">{{ r.receipt_key ? r.receipt_key + ': ' : '' }}{{ r.receipt_value }}</template>
              <span v-if="r.undoable" class="badge badge-info undo-badge">可撤销</span>
            </td>
          </tr>
          <tr v-if="filtered.length === 0">
            <td colspan="7" class="empty-state"><p>该 connector 暂无回执</p></td>
          </tr>
        </tbody>
      </table>
    </div>
  </div>
</template>

<style scoped>
.ledger-view {
  display: flex;
  flex-direction: column;
  height: 100%;
  background: var(--bg-primary);
}

.ledger-toolbar {
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

.ledger-table-wrap {
  flex: 1;
  overflow: auto;
}

.ledger-error {
  margin: var(--space-4);
  padding: var(--space-3);
  border: 1px solid rgba(239, 68, 68, 0.4);
  background: rgba(239, 68, 68, 0.08);
  border-radius: var(--radius-md);
  color: var(--error, #ef4444);
  font-size: var(--text-sm);
  word-break: break-all;
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

tbody tr:hover { background: var(--bg-hover); }

.cell-time {
  font-family: monospace;
  font-size: var(--text-xs);
  color: var(--text-muted);
  white-space: nowrap;
}

.cell-mono { font-family: monospace; }

.cell-summary {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  max-width: 320px;
}

.risk-badge {
  display: inline-flex;
  align-items: center;
  padding: 2px 8px;
  border-radius: var(--radius-full, 999px);
  font-size: var(--text-xs);
  font-weight: 600;
}
/* risk 四色（low=绿 medium=黄 high=橙 critical=红；未知灰）——与 ConnectorsView 同源映射 */
.risk-low      { background: rgba(16,185,129,0.15); color: #10b981; }
.risk-medium   { background: rgba(234,179,8,0.15); color: #eab308; }
.risk-high     { background: rgba(249,115,22,0.15); color: #f97316; }
.risk-critical { background: rgba(239,68,68,0.15); color: #ef4444; }
.risk-unknown  { background: var(--surface, rgba(0,0,0,0.05)); color: var(--text-muted); }

.undo-badge { margin-left: var(--space-1); }

.empty-state {
  text-align: center;
  padding: var(--space-6) var(--space-4);
  color: var(--text-muted);
  font-size: var(--text-sm);
}
</style>
