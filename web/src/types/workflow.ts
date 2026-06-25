/**
 * Workflow type definitions — mirror backend schemas in
 * `crates/nemesis-workflow/src/types.rs` and `engine.rs::WorkflowSummary`.
 *
 * These are the contracts the WSAPI returns. Do not add UI-only fields here;
 * keep them in components instead so this file stays a faithful API shape.
 */

// ---------------------------------------------------------------------------
// Backend-mirrored types
// ---------------------------------------------------------------------------

export type TriggerTypeName = 'cron' | 'webhook' | 'event' | 'message'

export interface TriggerConfig {
  trigger_type: TriggerTypeName | string
  config: Record<string, unknown>
}

export interface TriggerDriverStatus {
  trigger_type: string
  driven: boolean
  reason?: string
}

export interface TriggerSummary {
  trigger_type: string
  config: Record<string, unknown>
  driven: boolean
  reason?: string
  /** ISO datetime of next cron fire; absent for non-cron triggers. */
  next_fire_at?: string
}

export interface WorkflowSummary {
  name: string
  description: string
  version: string
  node_count: number
  trigger_count: number
  triggers: TriggerSummary[]
  /** 8-hex-char hash used by the workflow-chat page URL. */
  chat_index: string
  /** True if a per-workflow chat password is set. */
  has_chat_password: boolean
}

export interface NodeListResponse {
  workflows: WorkflowSummary[]
  trigger_driver_status: Record<string, TriggerDriverStatus>
  count: number
}

export interface NodeDef {
  id: string
  node_type: string
  config: Record<string, unknown>
  depends_on?: string[]
  retry_count?: number
  timeout?: number | null
  is_terminal?: boolean
}

export interface Edge {
  from_node: string
  to_node: string
  condition?: string | null
}

export interface WorkflowDef {
  name: string
  description: string
  version: string
  triggers: TriggerConfig[]
  nodes: NodeDef[]
  edges: Edge[]
  variables: Record<string, string>
  metadata: Record<string, string>
}

export interface WorkflowGetResponse {
  workflow: WorkflowDef
  summary: WorkflowSummary
}

export interface ValidateResponse {
  valid: boolean
  errors: string[]
}

export interface RunStartResponse {
  execution_id: string
  workflow_name: string
  state: string
}

// ---------------------------------------------------------------------------
// Execution / history types (mirrors backend Execution / ExecutionSummary)
// ---------------------------------------------------------------------------

export type ExecutionState =
  | 'Pending'
  | 'Running'
  | 'Waiting'
  | 'Completed'
  | 'Failed'
  | 'Cancelled'

export interface ExecutionSummary {
  execution_id: string
  workflow_name: string
  state: string
  started_at: string
  ended_at: string | null
  has_error: boolean
}

export interface NodeResult {
  node_id: string
  state: string
  output?: unknown
  error?: string | null
  started_at?: string
  ended_at?: string
}

export interface ExecutionDetail {
  execution_id: string
  workflow_name: string
  state: string
  started_at: string
  ended_at: string | null
  node_results: NodeResult[]
  error?: string | null
  trigger_source?: unknown
}

export interface ExecutionListResponse {
  executions: ExecutionSummary[]
  count: number
  total: number
}

// ---------------------------------------------------------------------------
// Checkpoint types (mirrors backend CheckpointMeta / Checkpoint)
// ---------------------------------------------------------------------------

export interface CheckpointMeta {
  checkpoint_id: string
  execution_id: string
  workflow_name: string
  waiting_node?: string | null
  created_at: string
  terminal?: boolean
}

export interface CheckpointListResponse {
  execution_id: string
  checkpoints: CheckpointMeta[]
}

export interface Checkpoint {
  checkpoint_id: string
  execution_id: string
  workflow_name: string
  waiting_node: string | null
  terminal: boolean
  trigger_source?: unknown
  context_snapshot: unknown
  node_results: NodeResult[]
  created_at: string
}

// ---------------------------------------------------------------------------
// Node catalog (UI-only constant — used by the canvas palette)
// ---------------------------------------------------------------------------

export type NodeCategory = 'ai' | 'control' | 'basic'

export interface NodeCatalogEntry {
  type: string
  label: string
  category: NodeCategory
  description: string
  icon: string
}

export const NODE_CATALOG: NodeCatalogEntry[] = [
  // AI
  { type: 'llm', label: 'LLM', category: 'ai', description: '调用大模型生成文本', icon: 'M12 2L2 7l10 5 10-5-10-5zM2 17l10 5 10-5M2 12l10 5 10-5' },
  { type: 'agent', label: 'Agent', category: 'ai', description: '运行子 Agent，支持多轮工具', icon: 'M12 2a3 3 0 0 1 3 3v1h1a3 3 0 0 1 3 3v1h1a3 3 0 0 1 0 6h-1v1a3 3 0 0 1-3 3h-1v1a3 3 0 0 1-6 0v-1H8a3 3 0 0 1-3-3v-1H4a3 3 0 0 1 0-6h1V9a3 3 0 0 1 3-3h1V5a3 3 0 0 1 3-3z' },
  { type: 'question_classifier', label: '问题分类', category: 'ai', description: 'LLM 把问题分到预定义类别', icon: 'M3 3h18v18H3zM3 9h18M9 21V9' },
  { type: 'parameter_extractor', label: '参数提取', category: 'ai', description: 'LLM 从文本抽取结构化参数', icon: 'M4 7h16M4 12h10M4 17h7M19 14l3 3-3 3' },
  // Control
  { type: 'condition', label: '条件', category: 'control', description: '基于变量值的分支', icon: 'M12 2l3 7h7l-5.5 4.5 2 7L12 16l-6.5 4.5 2-7L2 9h7z' },
  { type: 'parallel', label: '并行', category: 'control', description: '并发执行多个分支', icon: 'M5 12h14M12 5v14M5 5l4 4M19 5l-4 4M5 19l4-4M19 19l-4-4' },
  { type: 'loop', label: '循环', category: 'control', description: '条件或固定次数循环', icon: 'M21 12a9 9 0 1 1-9-9c2.5 0 4.8 1 6.5 2.7L21 8M21 3v5h-5' },
  { type: 'sub_workflow', label: '子工作流', category: 'control', description: '调用另一个工作流', icon: 'M3 3h18v18H3zM3 9h18M9 21V9' },
  // Basic
  { type: 'tool', label: '工具', category: 'basic', description: '调用已注册工具', icon: 'M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.77-3.77a6 6 0 0 1-7.94 7.94l-6.91 6.91a2.12 2.12 0 0 1-3-3l6.91-6.91a6 6 0 0 1 7.94-7.94l-3.76 3.76z' },
  { type: 'http', label: 'HTTP', category: 'basic', description: '发起 HTTP 请求', icon: 'M2 12h20M12 2a10 10 0 0 1 10 10M12 2a10 10 0 0 0-10 10M19 5l-7 7' },
  { type: 'script', label: '脚本', category: 'basic', description: '运行 shell/python/node 脚本', icon: 'M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z M9 13l2 2 4-4' },
  { type: 'delay', label: '延迟', category: 'basic', description: '等待若干秒', icon: 'M12 6v6l4 2M12 2a10 10 0 1 0 0 20 10 10 0 0 0 0-20z' },
  { type: 'human_review', label: '人工审核', category: 'basic', description: '暂停等待人工批准', icon: 'M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z M9 12l2 2 4-4' },
  { type: 'transform', label: '转换', category: 'basic', description: '变换/过滤数据', icon: 'M3 6h18M3 12h18M3 18h18M7 4v16M17 4v16' },
]

export function nodesByCategory(category: NodeCategory): NodeCatalogEntry[] {
  return NODE_CATALOG.filter(n => n.category === category)
}
