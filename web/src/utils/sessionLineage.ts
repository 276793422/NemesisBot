// WS9/P17（能力扩展，2026-09-26）：会话谱系前端投影工具。
//
// 后端 sessions.list 已把 sidecar meta 的平面谱系字段（parent /
// forked_at_turn / fork_reason / last_rewind / branch_summary）组合成
// lineage 组合视图（crates/nemesis-web/src/handlers/logs.rs 的
// scan_session_logs 后处理遍历——祖先链沿 parent 逐级上溯，深度帽 8，
// seen 集防环）。这里只做两件纯视图事（真相源在 meta，前端不落盘）：
// - arrangeLineage：同组会话按谱系树重排（子会话紧跟父会话之后）并给出
//   每行缩进深度——侧栏「同组内按谱系树缩进」的排序语义（v1 不做整棵
//   DAG 树视图）；
// - lineageChainLines：祖先链（后端给的是「最近父 → 根」顺序）转成
//   「根 → 父」自上而下的 ↳ 缩进行——会话信息弹窗「谱系」节的渲染文本。

/** sessions.list 行的 lineage 组合视图（镜像 logs.rs 的投影形态）。 */
export interface SessionLineage {
  parent_session_id: string
  fork_point_seq?: number
  reason: string
  ancestors: Array<{ id: string; title: string }>
}

/** 缩进深度帽（与后端 ancestors 深度帽同值——病态长链诚实截断显示）。 */
export const LINEAGE_MAX_DEPTH = 8

/** Web 会话 key 的规范前缀——`parent` 存的是完整 key
 *  （`agent:main:session:{sid}`），行 `id` 是裸 sid；匹配前先剥前缀
 *  （与 SessionSidebar.parentSid 同一规范化）。 */
const CANON_SESSION_PREFIX = 'agent:main:session:'

export interface LineageArrangement<T> {
  /** 树序（DFS：父在前、子随后、兄弟保持输入相对序）。 */
  ordered: T[]
  /** 行缩进深度（根 = 0；超过帽的链段钳在帽——显示层不无限缩进）。 */
  depthOf: (id: string) => number
}

/**
 * 谱系树重排：输入任意顺序的会话（同一分组内，调用方先按
 * recent/created/name 排好），输出树序 + 深度表。
 *
 * 规则：
 * - parent 先剥 `agent:main:session:` 规范前缀再比对行 id（parent 存
 *   完整 key、id 是裸 sid——与侧栏 parentSid 同一规范化）；
 * - 根 = 无 parent 或 parent 不在本批输入里（父已删/跨组 = 本组根，
 *   诚实降级）——根之间保持输入相对序；
 * - 子会话紧跟父会话之后（DFS 前序），兄弟间保持输入相对序；
 * - 单亲模型 + 深度表守卫：纯环（a↔b 互指/自指，无任何根可达）兜底
 *   按输入相对序追加在尾部、深度 0——不丢行、不死循环。
 */
export function arrangeLineage<T extends { id: string; parent?: string }>(items: T[]): LineageArrangement<T> {
  const byId = new Map<string, T>()
  for (const it of items) byId.set(it.id, it)
  const children = new Map<string, T[]>()
  const roots: T[] = []
  for (const it of items) {
    const raw = it.parent ?? ''
    // 剥规范前缀后再比对行 id（parent 存完整 key、id 是裸 sid）。
    const pid = raw.startsWith(CANON_SESSION_PREFIX) ? raw.slice(CANON_SESSION_PREFIX.length) : raw
    // 自指（parent === id）当根处理；父在本批才挂子链。
    if (pid && pid !== it.id && byId.has(pid)) {
      let b = children.get(pid)
      if (!b) {
        b = []
        children.set(pid, b)
      }
      b.push(it)
    } else {
      roots.push(it)
    }
  }
  const ordered: T[] = []
  const depths = new Map<string, number>()
  // 显式栈 DFS（逆序压栈保兄弟序）——不递归，长链不爆调用栈。
  const visit = (root: T) => {
    const stack: Array<{ node: T; depth: number }> = [{ node: root, depth: 0 }]
    while (stack.length > 0) {
      const { node, depth } = stack.pop()!
      ordered.push(node)
      depths.set(node.id, Math.min(depth, LINEAGE_MAX_DEPTH))
      const kids = children.get(node.id) ?? []
      for (let i = kids.length - 1; i >= 0; i--) stack.push({ node: kids[i], depth: depth + 1 })
    }
  }
  for (const r of roots) visit(r)
  for (const it of items) {
    if (!depths.has(it.id)) {
      ordered.push(it)
      depths.set(it.id, 0)
    }
  }
  return { ordered, depthOf: (id: string) => depths.get(id) ?? 0 }
}

/**
 * 祖先链 → 自上而下 ↳ 缩进行（弹窗「谱系」节渲染文本）。后端
 * ancestors 是「最近父 → 根」顺序；展示按人类阅读习惯反转成
 * 「根 → 父」，逐级缩进两个空格；`selfTitle` 给出时末尾追加当前会话行
 * （更深层级、`·` 前缀——整条链一眼读到底）。空 title 的祖先回退显示
 * id 前 12 位（异源/标题缺失不渲染空行）。
 */
export function lineageChainLines(
  ancestors: Array<{ id: string; title: string }>,
  selfTitle?: string,
): string[] {
  const chain = [...ancestors].reverse()
  const lines = chain.map(
    (a, i) => `${'  '.repeat(i)}↳ ${a.title?.trim() || a.id.slice(0, 12)}`,
  )
  if (selfTitle !== undefined) {
    lines.push(`${'  '.repeat(chain.length)}· ${selfTitle}`)
  }
  return lines
}
