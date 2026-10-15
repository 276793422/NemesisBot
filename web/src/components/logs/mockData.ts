// Logs Dashboard 共享类型和工具函数。
// 后端 SSE / WSAPI 接入后，类型契约保留在这里复用；mock 生成器已全部删除。

// ============================================================================
// 实时事件流
// ============================================================================

export type LogSource = 'general' | 'cluster' | 'security' | 'heartbeat' | 'llm'
export type LogLevel = 'DEBUG' | 'INFO' | 'WARN' | 'ERROR'

export interface LogEntry {
  id: string
  source: LogSource
  level: LogLevel
  timestamp: string // RFC3339
  component: string
  message: string
  fields?: Record<string, string | number | boolean>
}

export const SOURCE_META: Record<LogSource, { label: string; color: string; icon: string }> = {
  general:  { label: '应用',   color: '#3b82f6', icon: '🔵' }, // blue
  cluster:  { label: '集群',   color: '#a855f7', icon: '🟣' }, // purple
  security: { label: '安全',   color: '#ef4444', icon: '🔴' }, // red
  heartbeat:{ label: '心跳',   color: '#eab308', icon: '🟡' }, // yellow
  llm:      { label: 'AI通信', color: '#10b981', icon: '🟢' }, // green
}

// ============================================================================
// 对话历史 (session_logs) — Phase C 接 WSAPI logs.session_list/detail
// ============================================================================

export interface SessionMessage {
  role: 'user' | 'assistant' | 'system'
  content: string
  timestamp: string
  toolCalls?: number // 关联的本地 LLM 调用次数（用于跳转）
  triggerCluster?: boolean // 是否触发集群工具
}

export interface SessionEntry {
  id: string // session_key，如 web_chat1
  channel: string // web/discord/telegram/...
  startTime: string
  lastTime: string
  messageCount: number
  model: string
  firstMessage: string
  triggerCluster: boolean
  messages: SessionMessage[]
}

// ============================================================================
// 本地 LLM 调用 (request_logs) — Phase C 接 WSAPI logs.requests/request_detail
// ============================================================================

export interface LlmIteration {
  index: number
  request: {
    model: string
    messages: Array<{ role: string; content: string }>
    tools?: Array<{ name: string; args: Record<string, unknown> }>
  }
  response: {
    content: string
    toolCalls?: Array<{ id: string; name: string; args: Record<string, unknown> }>
    duration_ms: number
  }
  toolResults?: Array<{ callId: string; result: Record<string, unknown> }>
}

export interface LlmRequestEntry {
  id: string // 目录名 2026-06-17_14-23-45_a3f
  timestamp: string
  model: string
  duration_ms: number
  toolCallCount: number
  messageCount: number
  firstMessage: string
  sessionId?: string // 关联的 session（如有）
  clusterTaskId?: string // 关联的集群任务（如有）
  iterations: LlmIteration[]
}

// ============================================================================
// 集群 RPC 任务 (cluster_request_logs) — Phase C 接 WSAPI logs.cluster_task_list/detail
// ============================================================================

export interface ClusterTaskEntry {
  id: string // task_id，如 t8x7a3f9
  timestamp: string
  duration_ms: number
  direction: 'outbound' | 'inbound' // 📱本机发起 / 📥远端请求本机
  peerNode: string // 对端节点名
  action: string // rpc action，如 peer_chat
  firstMessage: string
  toolCallCount: number
  status: 'completed' | 'failed' | 'timeout'
  relatedRequestId?: string // 关联的本地调用（如有）
  iterations: LlmIteration[] // 复用 LLM 迭代结构
}

// ============================================================================
// 安全审计 (audit.jsonl) — Phase C 接 /api/logs?source=security
// ============================================================================

export type RiskLevel = 'LOW' | 'MEDIUM' | 'HIGH' | 'CRITICAL'

export interface AuditEntry {
  id: string
  timestamp: string
  operation: string // file_write/process_exec/...
  risk_level: RiskLevel
  target: string
  result: 'allow' | 'deny'
  decision: string
  user?: string
  reason?: string
  policy?: string
  raw: Record<string, unknown>
}

// ============================================================================
// 审计链 (integrity) — Phase E 接 audit_chain.jsonl 验证 API
// ============================================================================

export interface ChainSegment {
  index: number
  timestamp: string
  hash: string
  prevHash: string
  valid: boolean
  breakReason?: string
  payloadSummary: string
}

// ============================================================================
// 行动账本 (connectors.receipts) — ConnectorsView 回执区块 / Logs 行动账本
// 页签共享类型与徽标映射（后端契约已冻结；未知 risk/outcome 值诚实降级，
// 不猜不崩）
// ============================================================================

export type ConnectorRisk = 'low' | 'medium' | 'high' | 'critical'
export type ConnectorOutcome = 'executed' | 'denied_approval' | 'budget_blocked' | 'disabled' | 'error'

export interface ConnectorReceipt {
  receipt_id: string
  ts: string // RFC3339
  connector: string
  op: string
  risk: ConnectorRisk
  class: string
  outcome: ConnectorOutcome
  args_digest: string
  summary?: string
  receipt_key?: string
  receipt_value?: string
  budget_consumed?: Record<string, number>
  approval_ref?: string
  session_key?: string
  undoable?: boolean
}

/** risk → 徽标色类（low=绿 medium=黄 high=橙 critical=红）。 */
export function connectorRiskClass(risk: string): string {
  switch (risk) {
    case 'low': return 'risk-low'
    case 'medium': return 'risk-medium'
    case 'high': return 'risk-high'
    case 'critical': return 'risk-critical'
    default: return 'risk-unknown'
  }
}

const OUTCOME_META: Record<string, { cls: string; label: string }> = {
  executed: { cls: 'badge-success', label: '已执行' },
  denied_approval: { cls: 'badge-error', label: '审批拒绝' },
  budget_blocked: { cls: 'badge-warning', label: '限额拦截' },
  disabled: { cls: 'badge-neutral', label: '已停用' },
  error: { cls: 'badge-error', label: '错误' },
}

/** outcome → 全局 badge 色类（未知值灰色）。 */
export function connectorOutcomeClass(outcome: string): string {
  return OUTCOME_META[outcome]?.cls ?? 'badge-neutral'
}

/** outcome → 中文标签（未知值原样回显）。 */
export function connectorOutcomeLabel(outcome: string): string {
  return OUTCOME_META[outcome]?.label ?? outcome
}

// ============================================================================
// 工具函数
// ============================================================================

export function formatTime(ts: string, withMs = false): string {
  const d = new Date(ts)
  const h = d.getHours().toString().padStart(2, '0')
  const m = d.getMinutes().toString().padStart(2, '0')
  const s = d.getSeconds().toString().padStart(2, '0')
  if (!withMs) return `${h}:${m}:${s}`
  const ms = d.getMilliseconds().toString().padStart(3, '0')
  return `${h}:${m}:${s}.${ms}`
}

export function formatRelative(ts: string): string {
  const diff = Date.now() - new Date(ts).getTime()
  if (diff < 60_000) return `${Math.floor(diff / 1000)}秒前`
  if (diff < 3600_000) return `${Math.floor(diff / 60_000)}分钟前`
  if (diff < 86_400_000) return `${Math.floor(diff / 3600_000)}小时前`
  return `${Math.floor(diff / 86_400_000)}天前`
}

export function shortHash(hash: string, len = 8): string {
  return hash.slice(0, len)
}
