// 看板领域常量（W2 P3）：IssueListView / IssueDetailModal / BoardKanban 共享。
// 后端仍是唯一真相源——状态机镜像 crates/nemesis-board/src/state_machine.rs，
// 徽标/标签镜像 crates/nemesis-board/src/models.rs（IssueStatus/Priority）。

export const STATUSES = [
  { key: 'backlog', label: 'Backlog' },
  { key: 'todo', label: 'Todo' },
  { key: 'in_progress', label: '进行中' },
  { key: 'in_review', label: '评审中' },
  { key: 'done', label: '已完成' },
  { key: 'blocked', label: '受阻' },
  { key: 'cancelled', label: '已取消' },
] as const

export const STATUS_LABEL: Record<string, string> = Object.fromEntries(
  STATUSES.map((s) => [s.key, s.label]),
)

export const STATUS_BADGE: Record<string, string> = {
  backlog: 'badge-neutral',
  todo: 'badge-info',
  in_progress: 'badge-info',
  in_review: 'badge-warning',
  done: 'badge-success',
  blocked: 'badge-error',
  cancelled: 'badge-neutral',
}

export const PRIORITY_LABEL: Record<number, string> = {
  0: '低',
  1: '中',
  2: '高',
  3: '紧急',
}

export const PRIORITY_BADGE: Record<number, string> = {
  0: 'badge-neutral',
  1: 'badge-info',
  2: 'badge-warning',
  3: 'badge-error',
}

// 镜像 state_machine::can_transition（终态无出边；自转移不允许）。
export const TRANSITIONS: Record<string, string[]> = {
  backlog: ['todo', 'in_progress', 'done', 'blocked', 'cancelled'],
  todo: ['in_progress', 'done', 'blocked', 'cancelled'],
  in_progress: ['in_review', 'done', 'blocked', 'cancelled'],
  in_review: ['in_progress', 'done', 'blocked', 'cancelled'],
  blocked: ['todo', 'in_progress', 'cancelled'],
  done: [],
  cancelled: [],
}

export function statusLabel(s: string): string {
  return STATUS_LABEL[s] || s
}

export function fmtTime(sec: number | null | undefined): string {
  if (!sec) return '—'
  try {
    return new Date(sec * 1000).toLocaleString()
  } catch {
    return '—'
  }
}

export interface IssueRow {
  id: number
  number: string
  title: string
  description: string
  status: string
  priority: number
  assignee: string | null
  assignee_id: string | null
  creator: { kind: string; id: string }
  project_id: number | null
  due_date: number | null
  position: number
  acceptance_criteria: string | null
  origin: { origin_type: string; origin_id: string } | null
  created_at: number
  updated_at: number
  comments?: any[]
  activity?: any[]
  subscribers?: any[]
}
