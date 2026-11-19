/**
 * 会话侧栏分组派生 —— 单一真相源（原生 SessionSidebar 与皮肤投影共用）。
 *
 * 2026-11-18（focus 皮肤）从 SessionSidebar.vue 抽出：分组/排序/pin 的
 * 派生规则此前只住在侧栏组件里，皮肤投影若各写一份必然漂移（分组口径
 * 不一致 = 皮肤侧栏与原生行为分叉）。本模块持有**模块级单例响应式状态**
 * （pin 集合 ×2 + 排序键，localStorage 持久），原生侧栏与皮肤动作
 * （pin-session 等）读写同一份状态——任何一侧变更，两侧视图同步重算。
 *
 * 视图态（组折叠 / 显示更多展开 / 菜单开合）**不进本模块**——那是各消费
 * 方自己的 UI 状态（侧栏 localStorage 持久折叠；皮肤用引擎 local:toggle）。
 *
 * 迁移自 SessionSidebar.vue 的规则块（注释压缩保留原文锚点）：
 * - M6 会话 pin（组内置顶，删除时顺带清 pin 防残留占顶）
 * - B 项目组置顶（组间相对序保持注册表序不变）
 * - 会话排序键 recent/created/name（时间戳解析比较，禁裸字典序——
 *   RFC3339 Local offset 与 UTC Z 形态混排会把最新会话压底的实录教训）
 * - WS9/P17 谱系树重排（arrangeLineage：子会话紧跟父会话 + 缩进深度；
 *   pin 行不参与树排）
 * - L6++ 双分组（项目组在上 / 对话组 / 孤儿「已移除」灰组垫底）
 */
import { computed, ref } from 'vue'
import type { SessionEntry } from '../composables/useChatApi'
import { arrangeLineage } from './sessionLineage'

// ---------------------------------------------------------------------------
// M6：会话 pin（纯前端，localStorage `nb_pinned_sessions`）
// ---------------------------------------------------------------------------
const PIN_KEY = 'nb_pinned_sessions'

function loadPinned(): Set<string> {
  try {
    const raw = JSON.parse(localStorage.getItem(PIN_KEY) || '[]')
    return new Set(Array.isArray(raw) ? raw.filter((x: unknown) => typeof x === 'string') : [])
  } catch {
    return new Set()
  }
}

const pinnedIds = ref<Set<string>>(loadPinned())

function persistPinned() {
  localStorage.setItem(PIN_KEY, JSON.stringify([...pinnedIds.value]))
}

export function isPinned(id: string): boolean {
  return pinnedIds.value.has(id)
}

/** 换新 Set 触发依赖 pinnedIds 的 computed 重算。 */
export function togglePin(id: string) {
  if (pinnedIds.value.has(id)) {
    pinnedIds.value.delete(id)
  } else {
    pinnedIds.value.add(id)
  }
  pinnedIds.value = new Set(pinnedIds.value)
  persistPinned()
}

/** 删除会话成功后清 pin（残留 id 会永久占顶）。 */
export function unpinIfDeleted(id: string) {
  if (!pinnedIds.value.has(id)) return
  pinnedIds.value.delete(id)
  pinnedIds.value = new Set(pinnedIds.value)
  persistPinned()
}

// ---------------------------------------------------------------------------
// 会话排序键（纯前端偏好，localStorage `nb_session_sort`）
// ---------------------------------------------------------------------------
export type SessionSort = 'recent' | 'created' | 'name'
const SORT_KEY = 'nb_session_sort'
export const SORT_LABELS: Record<SessionSort, string> = { recent: '最近', created: '创建', name: '名称' }

function loadSort(): SessionSort {
  const v = localStorage.getItem(SORT_KEY)
  return v === 'created' || v === 'name' ? v : 'recent'
}

const sortState = ref<SessionSort>(loadSort())

/** 当前排序档（只读投影；模板 `SORT_LABELS[sessionSort]` 直接消费）。 */
export const sessionSort = computed(() => sortState.value)

export function currentSort(): SessionSort {
  return sortState.value
}

export function cycleSort() {
  const order: SessionSort[] = ['recent', 'created', 'name']
  sortState.value = order[(order.indexOf(sortState.value) + 1) % order.length]
  localStorage.setItem(SORT_KEY, sortState.value)
}

/** 会话行展示名（title > firstMessage > id 前 8 位；原生/皮肤共用）。 */
export function sessionTitle(s: { title?: string; firstMessage?: string; id: string }): string {
  return s.title || s.firstMessage || s.id.slice(0, 8)
}

function parseSessionTime(ts?: string): number {
  if (!ts) return Number.NEGATIVE_INFINITY
  const t = Date.parse(ts)
  return Number.isNaN(t) ? Number.NEGATIVE_INFINITY : t
}

function cmpSessions(a: SessionEntry, b: SessionEntry): number {
  if (sortState.value === 'name') {
    return sessionTitle(a).localeCompare(sessionTitle(b), 'zh-CN')
  }
  const key = sortState.value === 'created' ? 'startTime' : 'lastTime'
  // 必须解析成时间戳比较（服务端行 Local offset RFC3339 / 乐观行 UTC Z
  // 形态混排，裸字典序会把最新会话压底——2026-11-07 FF 全量实录）。
  const ta = parseSessionTime(a[key])
  const tb = parseSessionTime(b[key])
  if (ta === tb) return 0
  return ta > tb ? -1 : 1
}

/** 组内排序：pinned 置顶（互相对序=列表原序不变），其余按当前排序键。 */
function sortSessions(list: SessionEntry[]): SessionEntry[] {
  return [
    ...list.filter(s => pinnedIds.value.has(s.id)),
    ...list.filter(s => !pinnedIds.value.has(s.id)).sort(cmpSessions),
  ]
}

// ---------------------------------------------------------------------------
// B：项目组置顶（localStorage `nb_pinned_projects`）
// ---------------------------------------------------------------------------
const PROJ_PIN_KEY = 'nb_pinned_projects'

function loadPinnedProjects(): Set<string> {
  try {
    const raw = JSON.parse(localStorage.getItem(PROJ_PIN_KEY) || '[]')
    return new Set(Array.isArray(raw) ? raw.filter((x: unknown) => typeof x === 'string') : [])
  } catch {
    return new Set()
  }
}

const pinnedProjectIds = ref<Set<string>>(loadPinnedProjects())

function persistPinnedProjects() {
  localStorage.setItem(PROJ_PIN_KEY, JSON.stringify([...pinnedProjectIds.value]))
}

/**
 * 重读 localStorage 种子进模块单例（测试钩子；生产无调用点）。模块级
 * 状态在 import 时刻消费 localStorage——vitest 组件测试先 import 再于
 * 测试体内落种子/清库，直接 mount 会拿到 import 时刻的旧状态。挂载
 * 入口调一次本函数即可让「清库 + 落种」生效。
 */
export function reloadSessionGroupingFromStorage(): void {
  pinnedIds.value = loadPinned()
  sortState.value = loadSort()
  pinnedProjectIds.value = loadPinnedProjects()
}

export function isProjectPinned(key: string): boolean {
  return pinnedProjectIds.value.has(key)
}

export function toggleProjectPin(key: string) {
  if (pinnedProjectIds.value.has(key)) {
    pinnedProjectIds.value.delete(key)
  } else {
    pinnedProjectIds.value.add(key)
  }
  pinnedProjectIds.value = new Set(pinnedProjectIds.value)
  persistPinnedProjects()
}

/** 移除项目成功后清 pin（注册表已无此 id，残留只会占位）。 */
export function unpinProjectIfRemoved(key: string) {
  if (!pinnedProjectIds.value.has(key)) return
  pinnedProjectIds.value.delete(key)
  pinnedProjectIds.value = new Set(pinnedProjectIds.value)
  persistPinnedProjects()
}

// ---------------------------------------------------------------------------
// WS9/P17 谱系树 + L6++ 双分组派生
// ---------------------------------------------------------------------------
export type LineageRow = SessionEntry & { lineageDepth: number }

/** 组行：header=null = 对话组（无组头，模板自加区块标题条）。 */
export interface DerivedGroupRow {
  key: string
  header: { name: string; available: boolean; orphan: boolean } | null
  items: LineageRow[]
}

function groupItems(list: SessionEntry[]): LineageRow[] {
  const sorted = sortSessions(list)
  const pinned = sorted
    .filter(s => pinnedIds.value.has(s.id))
    .map(s => ({ ...s, lineageDepth: 0 }))
  const arranged = arrangeLineage(sorted.filter(s => !pinnedIds.value.has(s.id)))
  return [
    ...pinned,
    ...arranged.ordered.map(s => ({ ...s, lineageDepth: arranged.depthOf(s.id) })),
  ]
}

/** 展示顺序：置顶项目组（组内保持注册序）→ 未置顶项目组（注册序）→
 *  对话组 → 孤儿组（垫底）。`isWfEditSession` 由调用方注入（工作流编辑
 *  会话过滤是组合规则：composables 登记表 + 服务端绑定表并集，本模块
 *  不依赖那些单例）。 */
export function deriveSessionGroups(
  sessions: SessionEntry[],
  projects: { id: string; name: string; running?: boolean }[],
  isWfEditSession: (id: string) => boolean,
): DerivedGroupRow[] {
  const chatSessions = groupItems(sessions.filter(s => !s.projectId && !isWfEditSession(s.id)))
  const known = new Set(projects.map(p => p.id))
  const orphanSessions = groupItems(
    sessions.filter(s => s.projectId && !known.has(s.projectId) && !isWfEditSession(s.id)),
  )
  const all: DerivedGroupRow[] = projects.map(p => ({
    key: p.id,
    header: { name: p.name, available: p.running !== false, orphan: false },
    items: groupItems(sessions.filter(s => s.projectId === p.id && !isWfEditSession(s.id))),
  }))
  const rows = [
    ...all.filter(g => pinnedProjectIds.value.has(g.key)),
    ...all.filter(g => !pinnedProjectIds.value.has(g.key)),
  ]
  rows.push({ key: '__chat', header: null, items: chatSessions })
  if (orphanSessions.length > 0) {
    rows.push({ key: '__orphan', header: { name: '已移除', available: false, orphan: true }, items: orphanSessions })
  }
  return rows
}
