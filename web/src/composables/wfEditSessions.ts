/**
 * wfEditSessions — 对话生成（AI workflow generator）的会话登记与草稿差异类型。
 *
 * 两件事：
 * 1. **会话登记表**：哪些对话会话是「工作流编辑会话」（wf_edit）。
 *    localStorage 是真相源（`nemesisbot_wf_edit_sessions`），刷新后不丢；
 *    只影响前端展示（侧栏过滤、TAB 图标），后端会话就是普通会话。
 * 2. **草稿 ↔ 已注册定义 diff 类型**：WorkflowDraftPanel / 画布预览用的
 *    差异行模型。计算逻辑在同文件（纯函数，可单测）。
 *
 * 后端对应物：`chat.send` data.workflow_edit（metadata 透传），见
 * crates/nemesis-types/src/channel.rs::WorkflowEditTarget。
 */

import { ref } from 'vue'

/** workflow_edit 注入目标——与 Rust WorkflowEditTarget 字段一一对应。 */
export interface WorkflowEditTarget {
  /** null = 新建工作流（_new 引导会话）；字符串 = 编辑已注册工作流。 */
  workflow_name: string | null
}

const STORAGE_KEY = 'nemesisbot_wf_edit_sessions'

/** 会话登记条目：sessionId → 目标工作流名（null = 新建）。 */
type WfEditSessionMap = Record<string, string | null>

function loadMap(): WfEditSessionMap {
  try {
    const raw = localStorage.getItem(STORAGE_KEY)
    if (!raw) return {}
    const parsed = JSON.parse(raw)
    return parsed && typeof parsed === 'object' ? (parsed as WfEditSessionMap) : {}
  } catch {
    return {}
  }
}

function saveMap(map: WfEditSessionMap): void {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(map))
  } catch {
    // localStorage 满/禁用：登记降级为内存态，不影响功能本体
  }
}

/**
 * 会话登记表（模块级单例——所有组件共享同一份状态）。
 * `_new` 保留键：所有「新建工作流」对话共用一条后端会话，但登记表里
 * 每次进入仍记录为独立条目（键为真实 sessionId），方便侧栏识别。
 */
const sessionMap = ref<WfEditSessionMap>(loadMap())

export function useWfEditSessions() {
  /** 登记一个工作流编辑会话。 */
  function register(sessionId: string, workflowName: string | null): void {
    sessionMap.value = { ...sessionMap.value, [sessionId]: workflowName }
    saveMap(sessionMap.value)
  }

  /** 查询：是工作流编辑会话吗？返回目标名（null = 新建），undefined = 不是。 */
  function targetOf(sessionId: string): string | null | undefined {
    return sessionMap.value[sessionId]
  }

  /** 所有已登记会话 id。 */
  function ids(): string[] {
    return Object.keys(sessionMap.value)
  }

  /** 取消登记（会话被删除时调用；编辑会话没有独立的「退出」语义）。 */
  function unregister(sessionId: string): void {
    if (!(sessionId in sessionMap.value)) return
    const next = { ...sessionMap.value }
    delete next[sessionId]
    sessionMap.value = next
    saveMap(next)
  }

  return { register, targetOf, ids, unregister }
}

// ---------------------------------------------------------------------------
// 草稿 diff 类型与计算
// ---------------------------------------------------------------------------

/** useWorkflowApi capabilities 返回的节点能力行（子集，够渲染 diff 用）。 */
export interface NodeCapabilityLite {
  node_type: string
  summary: string
  required_config: string[]
  optional_config: string[]
  config_notes: string
}

/** capabilities WSAPI 命令返回的完整能力表（与 Rust serde 结构对齐）。 */
export interface GeneratorCapabilities {
  node_types: NodeCapabilityLite[]
  trigger_types: { trigger_type: string; summary: string; config_notes: string }[]
  structure_rules: string[]
}

/** draft_list / draft_get 行（与 Rust DraftSummary serde 对齐）。 */
export interface DraftSummary {
  name: string
  file_stem: string
  mtime_ms: number
  valid: boolean
  validation_errors: string[]
  node_count: number
  trigger_types: string[]
}

/** draft_get 返回（与 Rust DraftDetail serde 对齐；flatten 展开）。 */
export interface DraftDetail {
  name: string
  file_stem: string
  mtime_ms: number
  valid: boolean
  validation_errors: string[]
  node_count: number
  trigger_types: string[]
  workflow: import('../types/workflow').WorkflowDef
  yaml: string
}

/** 单个节点的差异行。 */
export interface DraftDiffRow {
  kind: 'added' | 'removed' | 'changed'
  /** 节点 id。 */
  nodeId: string
  /** 当前节点类型（added/changed）。 */
  nodeType?: string
  /** 旧节点类型（removed/changed）。 */
  oldNodeType?: string
}

/** 草稿与已注册定义的差异汇总。 */
export interface DraftDiff {
  added: DraftDiffRow[]
  removed: DraftDiffRow[]
  changed: DraftDiffRow[]
  /** 旧定义不存在（全新工作流）→ 全部节点记 added。 */
  isNew: boolean
}

/**
 * 计算草稿与已注册定义的节点级差异（纯函数）。
 * config 浅比较（JSON.stringify 键序不敏感化：递归排序键后比较）。
 */
export function computeDraftDiff(
  draft: import('../types/workflow').WorkflowDef,
  existing: import('../types/workflow').WorkflowDef | null,
): DraftDiff {
  if (!existing) {
    return {
      added: draft.nodes.map((n) => ({ kind: 'added' as const, nodeId: n.id, nodeType: n.node_type })),
      removed: [],
      changed: [],
      isNew: true,
    }
  }

  const oldById = new Map(existing.nodes.map((n) => [n.id, n]))
  const newById = new Map(draft.nodes.map((n) => [n.id, n]))

  const added: DraftDiffRow[] = []
  const removed: DraftDiffRow[] = []
  const changed: DraftDiffRow[] = []

  for (const n of draft.nodes) {
    const old = oldById.get(n.id)
    if (!old) {
      added.push({ kind: 'added', nodeId: n.id, nodeType: n.node_type })
    } else if (stableJson(old) !== stableJson(n)) {
      changed.push({ kind: 'changed', nodeId: n.id, nodeType: n.node_type, oldNodeType: old.node_type })
    }
  }
  for (const n of existing.nodes) {
    if (!newById.has(n.id)) {
      removed.push({ kind: 'removed', nodeId: n.id, oldNodeType: n.node_type })
    }
  }

  return { added, removed, changed, isNew: false }
}

/** 键序不敏感的稳定 JSON 序列化（递归排序对象键）。 */
function stableJson(value: unknown): string {
  return JSON.stringify(sortKeysDeep(value))
}

function sortKeysDeep(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(sortKeysDeep)
  if (value && typeof value === 'object') {
    const out: Record<string, unknown> = {}
    for (const k of Object.keys(value as Record<string, unknown>).sort()) {
      out[k] = sortKeysDeep((value as Record<string, unknown>)[k])
    }
    return out
  }
  return value
}
