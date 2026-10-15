<script setup lang="ts">
// 行动连接器页（AI 原生行动代理，2026-10-14）：三个区块——
// ① Connectors 卡片列表（注册表；损坏条目 error 在场 = 红字诚实展示原文，
//    不隐藏；enabled=false 置灰「已停用」）；
// ② 回执（行动账本）：connectors.receipts.list 最近 50 条，每笔操作留底；
// ③ 纸面券商账本：cash + 持仓 + 最近订单（最多 10 条）。
// 数据全部来自 WSAPI（connectors.list / connectors.receipts.list /
// paperbroker.ledger，契约已冻结）。本页只读，无写操作。
import { ref, computed, onMounted } from 'vue'
import { useWSAPI } from '../composables/useWSAPI'
import {
  connectorRiskClass, connectorOutcomeClass, connectorOutcomeLabel,
  type ConnectorReceipt,
} from '../components/logs/mockData'

interface ConnectorOp {
  id: string
  intent: string[]
  risk: string
  class: string
  confirm_required: boolean
  finance_budget: boolean
}

interface ConnectorInfo {
  id: string
  name: string
  version: string
  kind: string
  enabled: boolean
  description: string
  ops: ConnectorOp[]
  /** 在场 = 该 connector 损坏（加载/解析失败），诚实展示错误原文 */
  error?: string
}

interface LedgerPosition { symbol: string; qty: number; avg_cost: number }
interface LedgerOrder { order_id: string; ts: string; side: string; symbol: string; qty: number; price: number; status: string }
interface PaperLedger { cash: number; positions: LedgerPosition[]; orders: LedgerOrder[] }

const { request } = useWSAPI()

const loading = ref(true)
// connectors.list 是本页主数据源：失败整页错误态（照 SandboxView 错误卡）；
// receipts / ledger 失败只降级各自区块的诚实错误文案。
const loadError = ref('')
const connectors = ref<ConnectorInfo[]>([])
const receipts = ref<ConnectorReceipt[]>([])
const receiptsError = ref('')
const ledger = ref<PaperLedger | null>(null)
const ledgerError = ref('')

const displayOrders = computed<LedgerOrder[]>(() => (ledger.value?.orders ?? []).slice(0, 10))

function intentFirst(op: ConnectorOp): string {
  return Array.isArray(op.intent) && op.intent.length > 0 ? op.intent[0] : ''
}

// RFC3339 → 本地时间展示；解析失败原样回显
function formatTs(ts: string): string {
  const d = new Date(ts)
  return isNaN(d.getTime()) ? ts : d.toLocaleString()
}

// 千分位 + 两位小数；非有限数诚实归 0（账本是模拟数据，字段可能缺省）
function formatMoney(n: unknown): string {
  const v = typeof n === 'number' && Number.isFinite(n) ? n : 0
  return v.toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 2 })
}

function errMsg(reason: unknown): string {
  if (reason == null) return ''
  if (typeof reason === 'string') return reason
  const e = reason as { message?: string }
  return e.message ?? String(reason)
}

async function refreshAll() {
  loading.value = true
  loadError.value = ''
  receiptsError.value = ''
  ledgerError.value = ''
  try {
    const [listRes, rcptRes, ledgerRes] = await Promise.allSettled([
      request('connectors', 'list', {}),
      request('connectors', 'receipts.list', { limit: 50 }),
      request('paperbroker', 'ledger', {}),
    ])
    if (listRes.status === 'rejected') {
      loadError.value = errMsg(listRes.reason)
      return
    }
    connectors.value = Array.isArray(listRes.value?.connectors) ? listRes.value.connectors : []
    if (rcptRes.status === 'fulfilled') {
      receipts.value = Array.isArray(rcptRes.value?.receipts) ? rcptRes.value.receipts : []
    } else {
      receiptsError.value = errMsg(rcptRes.reason)
    }
    if (ledgerRes.status === 'fulfilled') {
      ledger.value = (ledgerRes.value ?? null) as PaperLedger | null
    } else {
      ledgerError.value = errMsg(ledgerRes.reason)
    }
  } finally {
    loading.value = false
  }
}

onMounted(refreshAll)
</script>

<template>
  <div class="page-connectors">
    <div class="page-header">
      <h2>Connectors</h2>
      <span v-if="!loadError" class="conn-count-badge">{{ connectors.length }} 个连接器</span>
    </div>
    <div class="page-body">

      <!-- 加载失败错误态（connectors.list 失败；照 SandboxView 错误卡风格） -->
      <div v-if="loadError" class="card">
        <div class="card-body conn-error" data-test="load-error">
          无法获取 Connectors 列表（connectors.list）— {{ loadError }}
          <button class="btn btn-sm" style="margin-left: var(--space-2);" @click="refreshAll" :disabled="loading">重试</button>
        </div>
      </div>

      <template v-else>
        <!-- ════════ 区块一：Connectors 卡片列表 ════════ -->
        <div class="card" data-test="connectors-card">
          <div class="card-header" style="display: flex; justify-content: space-between; align-items: center;">
            <h3 style="margin: 0;">Connectors</h3>
            <button class="btn btn-sm" data-test="connectors-refresh" @click="refreshAll" :disabled="loading">刷新</button>
          </div>
          <div class="card-body">
            <div v-if="loading" style="color: var(--text-secondary); font-size: var(--text-sm);">加载中…</div>
            <div v-else-if="connectors.length === 0" style="color: var(--text-secondary); font-size: var(--text-sm);">
              暂无已注册 connector。放置连接器目录后由网关扫描注册。
            </div>
            <div v-else style="display: flex; flex-direction: column; gap: var(--space-3);">
              <div
                v-for="c in connectors"
                :key="c.id"
                class="connector-card"
                :class="{ 'connector-disabled': !c.enabled }"
                data-test="connector-card"
              >
                <div class="connector-head">
                  <span class="connector-name">{{ c.name || c.id }}</span>
                  <code class="connector-id">{{ c.id }}</code>
                  <span v-if="c.version" class="conn-meta">v{{ c.version }}</span>
                  <span v-if="c.kind" class="badge badge-info">{{ c.kind }}</span>
                  <span class="badge" :class="c.enabled ? 'badge-success' : 'badge-neutral'" data-test="connector-enabled">{{ c.enabled ? '已启用' : '已停用' }}</span>
                </div>
                <div v-if="c.description" class="connector-desc">{{ c.description }}</div>
                <!-- 损坏条目：红字诚实展示错误原文，不隐藏 -->
                <div v-if="c.error" class="connector-error" data-test="connector-error">⚠ {{ c.error }}</div>
                <div v-else-if="c.ops && c.ops.length" class="table-wrap connector-ops">
                  <table>
                    <thead>
                      <tr><th>op</th><th>意图</th><th>风险</th><th>约束</th></tr>
                    </thead>
                    <tbody>
                      <tr v-for="op in c.ops" :key="op.id">
                        <td><code>{{ op.id }}</code></td>
                        <td>{{ intentFirst(op) }}</td>
                        <td>
                          <span class="risk-badge" :class="connectorRiskClass(op.risk)" :data-risk="op.risk">{{ op.risk }}</span>
                        </td>
                        <td>
                          <span v-if="op.confirm_required" class="badge badge-warning">需确认</span>
                          <span v-if="op.finance_budget" class="badge badge-info" style="margin-left: var(--space-1);">资金限额</span>
                          <span v-if="!op.confirm_required && !op.finance_budget" style="color: var(--text-muted);">—</span>
                        </td>
                      </tr>
                    </tbody>
                  </table>
                </div>
              </div>
            </div>
          </div>
        </div>

        <!-- ════════ 区块二：回执（行动账本） ════════ -->
        <div class="card" data-test="receipts-card">
          <div class="card-header" style="display: flex; justify-content: space-between; align-items: center;">
            <h3 style="margin: 0;">
              回执（行动账本）
              <span style="font-size: var(--text-sm); font-weight: 400; color: var(--text-secondary);">最近 {{ receipts.length }} 条</span>
            </h3>
            <button class="btn btn-sm" data-test="receipts-refresh" @click="refreshAll" :disabled="loading">刷新</button>
          </div>
          <div class="card-body">
            <div v-if="receiptsError" class="connector-error">回执加载失败：{{ receiptsError }}</div>
            <div v-else-if="receipts.length === 0" data-test="receipts-empty" style="color: var(--text-secondary); font-size: var(--text-sm);">
              暂无回执。connector 操作执行（或被拒绝/限额拦截）后，每笔都会在这里留底。
            </div>
            <div v-else class="table-wrap">
              <table data-test="receipts-table">
                <thead>
                  <tr>
                    <th>回执 ID</th><th>时间</th><th>connector.op</th><th>风险</th><th>结果</th><th>摘要</th><th>主键</th>
                  </tr>
                </thead>
                <tbody>
                  <tr v-for="r in receipts" :key="r.receipt_id" data-test="receipt-row">
                    <td><code>{{ r.receipt_id }}</code></td>
                    <td class="conn-ts">{{ formatTs(r.ts) }}</td>
                    <td><code>{{ r.connector }}.{{ r.op }}</code></td>
                    <td><span class="risk-badge" :class="connectorRiskClass(r.risk)">{{ r.risk }}</span></td>
                    <td><span class="badge" :class="connectorOutcomeClass(r.outcome)">{{ connectorOutcomeLabel(r.outcome) }}</span></td>
                    <td class="conn-summary" :title="r.summary">{{ r.summary }}</td>
                    <td class="conn-receipt-value">
                      <span v-if="r.receipt_value"><code>{{ r.receipt_key ? r.receipt_key + ': ' : '' }}{{ r.receipt_value }}</code></span>
                      <span v-if="r.undoable" class="badge badge-info undo-badge" data-test="undo-badge">可撤销</span>
                    </td>
                  </tr>
                </tbody>
              </table>
            </div>
          </div>
        </div>

        <!-- ════════ 区块三：纸面券商账本 ════════ -->
        <div class="card" data-test="ledger-card">
          <div class="card-header" style="display: flex; justify-content: space-between; align-items: center;">
            <h3 style="margin: 0;">纸面券商账本</h3>
            <button class="btn btn-sm" @click="refreshAll" :disabled="loading">刷新</button>
          </div>
          <div class="card-body">
            <div v-if="ledgerError" class="connector-error">账本加载失败：{{ ledgerError }}</div>
            <template v-else-if="ledger">
              <div class="ledger-cash" data-test="ledger-cash">
                现金 <code data-test="ledger-cash-value">{{ formatMoney(ledger.cash) }}</code>
              </div>
              <div v-if="!ledger.positions?.length && !ledger.orders?.length" style="margin-top: var(--space-2); color: var(--text-secondary); font-size: var(--text-sm);">
                账本为空。纸面下单后持仓与订单会出现在这里。
              </div>
              <template v-else>
                <div v-if="ledger.positions?.length" class="table-wrap" style="margin-top: var(--space-3);">
                  <table data-test="positions-table">
                    <thead><tr><th>标的</th><th>数量</th><th>均价</th></tr></thead>
                    <tbody>
                      <tr v-for="p in ledger.positions" :key="p.symbol">
                        <td><code>{{ p.symbol }}</code></td>
                        <td>{{ p.qty }}</td>
                        <td>{{ formatMoney(p.avg_cost) }}</td>
                      </tr>
                    </tbody>
                  </table>
                </div>
                <div v-if="ledger.orders?.length" class="table-wrap" style="margin-top: var(--space-3);">
                  <div style="font-size: var(--text-xs); color: var(--text-muted); margin-bottom: var(--space-1);">最近订单（最多 10 条）</div>
                  <table data-test="orders-table">
                    <thead><tr><th>订单</th><th>时间</th><th>方向</th><th>标的</th><th>数量</th><th>价格</th><th>状态</th></tr></thead>
                    <tbody>
                      <tr v-for="o in displayOrders" :key="o.order_id">
                        <td><code>{{ o.order_id }}</code></td>
                        <td class="conn-ts">{{ formatTs(o.ts) }}</td>
                        <td>{{ o.side }}</td>
                        <td><code>{{ o.symbol }}</code></td>
                        <td>{{ o.qty }}</td>
                        <td>{{ formatMoney(o.price) }}</td>
                        <td>{{ o.status }}</td>
                      </tr>
                    </tbody>
                  </table>
                </div>
              </template>
            </template>
            <div v-else style="color: var(--text-secondary); font-size: var(--text-sm);">
              账本暂不可用（纸面券商未就绪）。
            </div>
          </div>
        </div>
      </template>

    </div>
  </div>
</template>

<style scoped>
code { background: var(--bg-secondary, rgba(0,0,0,0.05)); padding: 1px 4px; border-radius: 3px; font-size: var(--text-xs); }

.conn-count-badge {
  font-size: var(--text-xs);
  color: var(--text-secondary);
  border: 1px solid var(--border);
  border-radius: 999px;
  padding: 2px 10px;
  margin-left: var(--space-3);
}

.conn-error {
  color: var(--error, #ef4444);
  font-size: var(--text-sm);
}

/* 损坏条目红字（诚实展示，不隐藏） */
.connector-error {
  margin-top: var(--space-2);
  padding: var(--space-2) var(--space-3);
  border: 1px solid rgba(239, 68, 68, 0.4);
  background: rgba(239, 68, 68, 0.08);
  border-radius: var(--radius-md);
  color: var(--error, #ef4444);
  font-size: var(--text-sm);
  word-break: break-all;
}

.connector-card {
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
  padding: var(--space-3) var(--space-4);
}

.connector-card.connector-disabled { opacity: 0.65; }

.connector-head {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  flex-wrap: wrap;
}

.connector-name { font-weight: 600; }

.connector-id {
  color: var(--text-secondary);
  font-family: var(--font-mono, monospace);
}

.conn-meta { font-size: var(--text-xs); color: var(--text-secondary); }

.connector-desc {
  margin-top: var(--space-1);
  font-size: var(--text-sm);
  color: var(--text-secondary);
}

.connector-ops { margin-top: var(--space-2); font-size: var(--text-sm); }

/* risk 徽标四色（low=绿 medium=黄 high=橙 critical=红；未知灰） */
.risk-badge {
  display: inline-flex;
  align-items: center;
  padding: 2px 8px;
  border-radius: var(--radius-full, 999px);
  font-size: var(--text-xs);
  font-weight: 600;
}
.risk-low      { background: rgba(16,185,129,0.15); color: #10b981; }
.risk-medium   { background: rgba(234,179,8,0.15); color: #eab308; }
.risk-high     { background: rgba(249,115,22,0.15); color: #f97316; }
.risk-critical { background: rgba(239,68,68,0.15); color: #ef4444; }
.risk-unknown  { background: var(--surface, rgba(0,0,0,0.05)); color: var(--text-muted); }

.conn-ts {
  font-family: var(--font-mono, monospace);
  font-size: var(--text-xs);
  color: var(--text-secondary);
  white-space: nowrap;
}

.conn-summary {
  max-width: 260px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.conn-receipt-value { white-space: nowrap; }
.undo-badge { margin-left: var(--space-1); }

.ledger-cash { font-size: var(--text-sm); }
.ledger-cash code { font-size: var(--text-base, 1rem); font-weight: 600; }
</style>
