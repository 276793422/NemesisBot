import { computed, reactive, ref } from 'vue'
import { on as sseOn } from './useSSE'

// 拆解运行期可观测（2026-10-11「停着」事故，前端侧）：planner 在途状态的
// 单一事实源。模块级单例——BoardView（容器横幅）、IssueListView（列表徽标）、
// ProjectPanel（项目卡片行）共享同一份在途表，任何视图挂载与否不影响数据面。
//
// 数据通道：
// - SSE board.plan_started → 立条目（实时起点）；
// - SSE board.plan_ready / board.plan_failed → 撤条目（plan_id 匹配闸）；
// - issue.list 的 planning 投影（{elapsed_secs, attempt}）→ reconcile()
//   回填/撤下兜底（页面加载、断线期间错过 SSE、SSE 丢失）。
//
// plan_id 匹配闸（superseded 防误撤）：A 计划失败 → 立刻发起第 2 次尝试
// （plan_started 覆盖条目）→ 迟到的 A 失败事件不得撤掉第 2 次尝试的在途
// 标记。live 条目只在 plan_id 相等时才被 ready/failed 撤下；投影回填条目
// （plan_id 未知，seeded=true）该单任何 ready/failed 都可信。

export interface PlanningEntry {
  planId: string
  issueId: number
  issueNumber: string
  attempt: number
  /** 毫秒时间戳：SSE 起源 = 事件到达时刻；投影回填 = now - elapsed_secs。 */
  startedAt: number
  /** true = issue.list planning 投影回填（非 SSE 实时起点，plan_id 未知）。 */
  seeded: boolean
}

// 模块级单例：reactive Map<issue_id, entry>。
const entries = reactive(new Map<number, PlanningEntry>())

// 1s 心跳：elapsed() 是 (now - startedAt) 纯函数，tick 的唯一职责是让引用
// 它的渲染 effect 每秒重算（自计时）。有条目才跑、空即停（不空转烧 CPU）。
const tick = ref(0)
let ticker: ReturnType<typeof setInterval> | null = null

function ensureTicker() {
  if (ticker === null && entries.size > 0) {
    ticker = setInterval(() => {
      tick.value++
    }, 1000)
  }
}

function maybeStopTicker() {
  if (ticker !== null && entries.size === 0) {
    clearInterval(ticker)
    ticker = null
  }
}

function onPlanStarted(data: any) {
  if (!data || typeof data.issue_id !== 'number') return
  entries.set(data.issue_id, {
    planId: String(data.plan_id ?? ''),
    issueId: data.issue_id,
    issueNumber: String(data.issue_number ?? ''),
    attempt: Number(data.attempt ?? 1) || 1,
    startedAt: Date.now(),
    seeded: false,
  })
  ensureTicker()
}

function onPlanSettled(data: any) {
  if (!data || typeof data.issue_id !== 'number') return
  const cur = entries.get(data.issue_id)
  if (!cur) return
  // plan_id 匹配闸：live 条目只在 plan_id 相等时撤；seeded 条目（plan_id
  // 未知）该单任何 ready/failed 都可信。
  if (cur.planId && cur.planId !== String(data.plan_id ?? '')) return
  entries.delete(data.issue_id)
  maybeStopTicker()
}

let registered = false
function ensureRegistered() {
  if (registered) return
  registered = true
  sseOn('board.plan_started', onPlanStarted)
  sseOn('board.plan_ready', onPlanSettled)
  sseOn('board.plan_failed', onPlanSettled)
}

/**
 * issue.list 刷新链回调（loadIssues / load 成功后调用）：按行快照校正在途表。
 * - 撤下：在途条目对应行已终态（done/cancelled）或 planning 投影消失（SSE
 *   ready/failed 丢失的兜底；board-changed 静默刷新链都会路过这里）；
 * - 回填：行带 planning 投影且无 live 条目 → 种条目（页面加载/断线期间
 *   错过 SSE，用投影 elapsed_secs 反推 startedAt）。
 * 只触碰 rows 里出现过的行——过滤视图（按状态/项目过滤后的子集）不得
 * 误撤未入列单子的在途标记。
 */
function reconcile(rows: any[]) {
  for (const row of rows || []) {
    if (!row || typeof row.id !== 'number') continue
    const cur = entries.get(row.id)
    if (cur) {
      if (row.status === 'done' || row.status === 'cancelled' || !row.planning) {
        entries.delete(row.id)
      }
    } else if (row.planning) {
      entries.set(row.id, {
        planId: '',
        issueId: row.id,
        issueNumber: String(row.number ?? ''),
        attempt: Number(row.planning.attempt ?? 1) || 1,
        startedAt: Date.now() - Number(row.planning.elapsed_secs ?? 0) * 1000,
        seeded: true,
      })
    }
  }
  ensureTicker()
  maybeStopTicker()
}

export const ABNORMAL_SECS = 1200 // >20 分钟 = 拆解异常（正常 3~8 分钟）

/** "m:ss" 计时格式（横幅/徽标共用）。 */
export function fmtElapsed(totalSecs: number): string {
  const m = Math.floor(totalSecs / 60)
  const s = totalSecs % 60
  return `${m}:${String(s).padStart(2, '0')}`
}

export function usePlanning() {
  ensureRegistered()

  // 在途列表（按开始时间升序；tick 依赖让横幅每秒重渲染）。
  const list = computed(() => {
    void tick.value
    return Array.from(entries.values()).sort((a, b) => a.startedAt - b.startedAt)
  })

  /** 单条目已进行秒数（响应式：心跳跳动 + Map 增删都会触发重算）。 */
  function elapsed(issueId: number): number {
    void tick.value
    const cur = entries.get(issueId)
    if (!cur) return 0
    return Math.max(0, Math.floor((Date.now() - cur.startedAt) / 1000))
  }

  return { list, elapsed, reconcile }
}

/** 测试专用：清空在途表 + 停心跳 + 复位 SSE 注册态（模块单例，用例间隔离）。
 *  测试的 useSSE 打桩表会在 beforeEach 清空——必须同时复位 registered，
 *  否则单例自以为已注册，清空后事件永远到不了 handler。 */
export function _resetPlanningForTest() {
  entries.clear()
  if (ticker !== null) {
    clearInterval(ticker)
    ticker = null
  }
  registered = false
}
