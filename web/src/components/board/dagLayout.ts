// DAG 全景流程图布局（2026-10-15 用户需求②）：纯函数、无 DOM 依赖，
// 便于单测；渲染器（BoardDagView.vue SVG）只消费输出坐标与就绪 path。
//
// 分层 = Kahn 拓扑分层（layer = max(上游 layer)+1，无依赖 = 0）；纵向主轴
// 上→下即「从头做到尾」叙事（页面滚动方向，流程图惯例），同层并行节点
// 横向排开。拆解关系（parent_issue_id）做层下界松弛：子单恒在父单下一层
// （根需求独占首行）。剥不完的剩余节点 = 环（planner 保证无环，防御
// lenient 路径漏网）落「未分层」桶置底。
//
// 边走线（2026-10-15 v4 零交叉层级，用户钦定「任何两条线不允许交叉；目标
// 一致的线可以合并；这就是层级图，没有复杂逻辑」——v1 独占沟 / v2 全局
// 共源竖干 / v3 贴列圆角干道全部废弃：横轨 × 竖干道结构性互穿，与「零交
// 叉」不可调和）。两级管线：
//
// ① 传递约简（画面前提）：执行依赖边 u→v 若存在不含该边的视图内替代路径，
//    则不另画线——其信息并入目标卡的「+N 间接上游」徽标与 tooltip（可达
//    性语义无损；调度闸照读原始 depends_on，与本图无关）。实测本链 45 边
//    约简后剩 19 条（近纯链）——全量画在层级几何下非平面（并行长链必然
//    互穿，离线实证 20 处交叉），约简后恒平面。
// ② Sugiyama 零交叉布线（对约简后的边）：
//    - 跨 ≥2 层的长边在每个中间层插入「哑节点」（只占序不渲染），任意边
//      降为一串相邻层短段；
//    - 层内序 barycenter 迭代（下扫 + 上扫交替）+ 转置收尾（相邻对换贪心，
//      目标 = 相邻层带内「序翻转」段对数），序不翻转 = 同带内连线两两不
//      交叉（单调贝塞尔的代数性质）；
//    - 出边恒走源底边中点、入边恒汇入目标顶边中点（同目标 = 同一个汇入
//      点，路线天然合并成树状收束）；段间用垂直切线三次贝塞尔圆滑。
// 哑节点槽位窄间距（18px），长边不撑宽画布。零交叉有单测钉死（构造翻转
// 案例 + 真实 20 节点依赖图 replica，逐段严格相交计数 = 0）。

export interface DagIssue {
  id: number
  number: string
  title: string
  status: string
  assignee_id?: string | null
  /** 父单 id（拆解关系，issue 序列化自带）：只用于分层下界（子 ≥ 父层+1，
   *  根需求独占首行）与拆解边渲染；不是执行依赖，不参与派发闸。 */
  parent_issue_id?: number | null
  /** 后端 issue.list/get 投影：每上游 {id, number, status}。 */
  depends_on?: Array<{ id: number; number: string; status: string }>
  /** 决策可见性投影（2026-10-15 批）穿透：needs_user=停车待裁决（amber
   *  加粗描边+✋）；plan_ready=拆解完成待发车（📋）。 */
  needs_user?: { stop_kind: string; stop_kind_label: string; hint: string } | null
  plan_ready?: { plan_id: string; subs_count: number } | null
}

export interface DagNode {
  id: number
  number: string
  title: string
  status: string
  assigneeId: string | null
  /** Kahn 层号（0 = 无上游；环内节点 = 最大层+1；parent 下界松弛后 ≥ 父层+1）。 */
  layer: number
  x: number
  y: number
  /** 环内节点（拓扑剥不完），渲染置底 + 红虚线描边 + 顶部提示。 */
  unlayered: boolean
  /** compact 折叠组成员——渲染器跳过（画组矩形），坐标=组坐标。 */
  folded: boolean
  /** 间接上游编号（传递约简后未画线的上游，按编号序）：渲染 +N 徽标与
   *  tooltip——线不画，信息不丢。 */
  indirectDeps: string[]
  /** 决策可见性穿透（2026-10-15 批）：needs_user=停车待裁决（amber 加粗
   *  描边 + ✋ 图标覆盖，最高视觉权重）；plan_ready=拆解完成待确认（📋）。 */
  needsUser: boolean
  planReady: boolean
}

export interface DagEdge {
  /** 上游 id（external 时不在 nodes 里，编号看 fromNumber）。 */
  from: number
  /** 下游 id（恒在视图内）。 */
  to: number
  /** 上游不在当前视图（跨项目/已归档/无主单）——渲染上置虚线小签，绝不静默丢。 */
  external: boolean
  /** 上游未 done（渲染琥珀高亮 = 阻塞传播可见；external 恒 true 保守示警）。 */
  blocking: boolean
  /** true = 拆解关系边（parent_issue_id）：非执行依赖、恒不阻塞，
   *  淡虚线渲染——只补「无视图内执行上游」的子单，避免与 depends_on 双线。 */
  decomposition: boolean
  fromNumber: string
  /** 渲染就绪 SVG path（布局期算好，渲染器零几何逻辑）：
   *  长边 = 逐层短段垂直切线贝塞尔链（经哑节点槽位）；短边 = 单段贝塞尔。
   *  出边走源底边中点、入边汇目标顶边中点（同目标合并）。 */
  path: string
}

/** compact 档折叠组：同层同状态 ≥ FOLD_MIN 的成员并成一个色块（×N）。 */
export interface DagFold {
  layer: number
  status: string
  size: number
  x: number
  y: number
  numbers: string[]
}

export interface DagLayout {
  nodes: DagNode[]
  edges: DagEdge[]
  folds: DagFold[]
  layerCount: number
  /** 最大层的条目数（实体 + 哑节点；SVG 宽度参考值之一）。 */
  maxLayerSlots: number
  /** 未分层（环）节点数——非 0 时渲染顶部提示。 */
  unlayeredCount: number
  mode: 'full' | 'compact' | 'hint'
  /** 内容实际占宽（含左右留白；svgSize 宽度依据）。 */
  contentWidth: number
  /** 传递约简隐藏的执行依赖边数（渲染 meta 展示用）。 */
  hiddenEdges: number
}

/** 渲染降档阈值（评估定稿：完整卡 ≤300 / 压缩 ≤2000 / 再大出诚实提示）。 */
export const COMPACT_THRESHOLD = 300
export const HINT_THRESHOLD = 2000
/** compact 折叠触发下限：同层同状态 ≥4 个才折叠（少量同态不折，保留可点）。 */
export const FOLD_MIN = 4

export const NODE_SIZE = {
  full: { w: 184, h: 56, gapX: 76, gapY: 24, pad: 24 },
  compact: { w: 44, h: 26, gapX: 24, gapY: 10, pad: 16 },
} as const

/** 哑节点槽位宽（窄间距：长边穿层只占一个细缝，不撑宽画布）。 */
const DUMMY_GAP = 18

export function renderMode(count: number): 'full' | 'compact' | 'hint' {
  if (count <= COMPACT_THRESHOLD) return 'full'
  if (count <= HINT_THRESHOLD) return 'compact'
  return 'hint'
}

export function nodeDims(mode: 'full' | 'compact' | 'hint') {
  return NODE_SIZE[mode === 'full' ? 'full' : 'compact']
}

/** SVG 画布尺寸（渲染器 viewBox / 宽高依据）。 */
export function svgSize(layout: DagLayout): { w: number; h: number } {
  const dim = nodeDims(layout.mode)
  return {
    w: Math.max(dim.pad * 2, layout.contentWidth),
    // h 由层数（纵向主轴向下滚）决定，+64 给外部依赖上置小签留头部空间
    // （签 y−28 起）。
    h: dim.pad * 2 + Math.max(1, layout.layerCount) * (dim.h + dim.gapY) - dim.gapY + 64,
  }
}

/**
 * Kahn 拓扑分层 + 零交叉层级走线坐标分配。
 * 只消费视图内节点与视图内边；视图外上游记为 external 边（不参与分层）。
 */
export function computeLayout(issues: DagIssue[]): DagLayout {
  const mode = renderMode(issues.length)
  const dim = nodeDims(mode)
  const byId = new Map<number, DagIssue>()
  for (const it of issues) byId.set(it.id, it)

  // 边集（external 单独记账，不参与分层）；path 由文末走线 pass 回填。
  let edges: DagEdge[] = []
  const inViewDeps = new Map<number, number[]>()
  for (const it of issues) {
    let hasInViewDep = false
    for (const dep of it.depends_on ?? []) {
      const external = !byId.has(dep.id)
      edges.push({
        from: dep.id,
        to: it.id,
        external,
        blocking: external || dep.status !== 'done',
        decomposition: false,
        fromNumber: dep.number,
        path: '',
      })
      if (!external) {
        hasInViewDep = true
        inViewDeps.set(it.id, [...(inViewDeps.get(it.id) ?? []), dep.id])
      }
    }
    // 拆解边（parent_issue_id）：只为「无视图内执行上游」的子单补画——
    // 第一波拆解（如根需求 → 首批子单）depends_on 恒空，否则图上断头；
    // 有执行边的子单双线是噪音，位置已由 depends_on 表达。
    const pid = it.parent_issue_id
    if (pid != null && !hasInViewDep && byId.has(pid)) {
      edges.push({
        from: pid,
        to: it.id,
        external: false,
        blocking: false,
        decomposition: true,
        fromNumber: byId.get(pid)!.number,
        path: '',
      })
    }
  }

  // --- 传递约简（画面前提）---
  // 执行依赖边 u→v 若存在不含该边的视图内替代路径（u→…→v），则不画线，
  // 信息并入目标节点 indirectDeps（+N 徽标 + tooltip）。逐边删除后 BFS 判
  // 可达（20~百级节点规模可忽略）。拆解边/external 边不参与（前者是父子
  // 叙事、后者不在视图内成路径）。分层仍用全量 inViewDeps（层号语义不变）。
  const hiddenIndirect = new Map<number, string[]>()
  {
    const adj = new Map<number, number[]>()
    for (const e of edges) {
      if (e.external || e.decomposition) continue
      adj.set(e.from, [...(adj.get(e.from) ?? []), e.to])
    }
    const reachable = (from: number, skipFrom: number, skipTo: number, target: number): boolean => {
      const seen = new Set<number>([from])
      const stack = [from]
      while (stack.length) {
        const cur = stack.pop()!
        for (const nxt of adj.get(cur) ?? []) {
          if (cur === skipFrom && nxt === skipTo) continue
          if (nxt === target) return true
          if (!seen.has(nxt)) {
            seen.add(nxt)
            stack.push(nxt)
          }
        }
      }
      return false
    }
    edges = edges.filter((e) => {
      if (e.external || e.decomposition) return true
      if (reachable(e.from, e.from, e.to, e.to)) {
        hiddenIndirect.set(e.to, [...(hiddenIndirect.get(e.to) ?? []), e.fromNumber])
        return false
      }
      return true
    })
    for (const arr of hiddenIndirect.values()) arr.sort()
  }

  // Kahn：入度 0 → layer 0；剥离后下游 layer = max(上游)+1。
  const layer = new Map<number, number>()
  const remaining = new Map<number, number>()
  const dependents = new Map<number, number[]>()
  const kahnLayered = new Set<number>()
  for (const it of issues) {
    const deps = inViewDeps.get(it.id) ?? []
    remaining.set(it.id, deps.length)
    for (const d of deps) {
      dependents.set(d, [...(dependents.get(d) ?? []), it.id])
    }
    if (deps.length === 0) {
      layer.set(it.id, 0)
      kahnLayered.add(it.id)
    }
  }
  let frontier = issues.filter((it) => (remaining.get(it.id) ?? 0) === 0).map((it) => it.id)
  while (frontier.length) {
    const next: number[] = []
    for (const id of frontier) {
      const depLayer = layer.get(id) ?? 0
      for (const child of dependents.get(id) ?? []) {
        remaining.set(child, (remaining.get(child) ?? 1) - 1)
        // 下游层号 = 已见上游层号最大值 +1（多上游逐次抬升）。
        const cur = layer.get(child)
        if (cur === undefined || depLayer + 1 > cur) layer.set(child, depLayer + 1)
        if ((remaining.get(child) ?? 0) === 0) {
          kahnLayered.add(child)
          next.push(child)
        }
      }
    }
    frontier = next
  }

  // 拆解层下界（parent_issue_id，需求①根行修正）：子单至少落在父单下一层
  // ——根需求独占首行，第一波拆解（无 depends_on 的子单）次行展开，不再与
  // 根同排。父层被抬升时沿 depends_on 下游传播（孙单同步下移），松弛到不
  // 动点；迭代上限 V 轮防病态父环死循环（正常数据 1-2 轮收敛，Bellman-Ford
  // 式 O(V·E) 对看板规模可忽略）。环内（unlayered）节点不参与——它们置底
  // 语义独立，且分层节点经 depends_on 不可能下游于环。
  {
    const boundEdges: Array<[number, number]> = []
    for (const [to, froms] of inViewDeps) for (const f of froms) boundEdges.push([f, to])
    for (const it of issues) {
      const pid = it.parent_issue_id
      if (pid != null && byId.has(pid) && kahnLayered.has(it.id) && kahnLayered.has(pid)) {
        boundEdges.push([pid, it.id])
      }
    }
    for (let pass = 0; pass < issues.length; pass++) {
      let changed = false
      for (const [f, t] of boundEdges) {
        if (!kahnLayered.has(t)) continue
        const fl = layer.get(f)
        if (fl === undefined) continue
        if (fl + 1 > (layer.get(t) ?? 0)) {
          layer.set(t, fl + 1)
          changed = true
        }
      }
      if (!changed) break
    }
  }

  // 环内剩余：落「未分层」桶（层号 = 最大层 +1，渲染置底提示）。
  const maxKahnLayer = Math.max(-1, ...[...layer.values()])
  let unlayeredCount = 0
  for (const it of issues) {
    if (!kahnLayered.has(it.id)) {
      layer.set(it.id, maxKahnLayer + 1)
      unlayeredCount++
    }
  }

  // --- v4 零交叉层级走线 ---
  // 条目化：每层条目 = 实体单 / 折叠组（compact）/ 哑节点（长边穿层占位）。
  interface Item {
    key: string
    layer: number
    /** 实体/折叠条目的成员单（哑节点为空）。 */
    issues: DagIssue[]
    fold: boolean
    /** 所属边在 edges[] 的下标（哑节点）；-1 = 实体/折叠。 */
    dummyEdge: number
  }
  const itemByKey = new Map<string, Item>()
  const itemsByLayer = new Map<number, Item[]>()
  const pushItem = (it: Item) => {
    itemByKey.set(it.key, it)
    const arr = itemsByLayer.get(it.layer)
    if (arr) arr.push(it)
    else itemsByLayer.set(it.layer, [it])
  }
  const foldKeyOf = new Map<number, string>()
  const foldedIds = new Set<number>()
  if (mode === 'compact') {
    const groups = new Map<string, DagIssue[]>()
    for (const it of issues) {
      const key = `${layer.get(it.id) ?? 0}\u0000${it.status}`
      const arr = groups.get(key)
      if (arr) arr.push(it)
      else groups.set(key, [it])
    }
    for (const members of groups.values()) {
      const l = layer.get(members[0].id) ?? 0
      if (members.length >= FOLD_MIN) {
        const key = `f:${l}:${members[0].status}`
        for (const m of members) {
          foldedIds.add(m.id)
          foldKeyOf.set(m.id, key)
        }
        pushItem({ key, layer: l, issues: members, fold: true, dummyEdge: -1 })
      } else {
        for (const m of members) {
          const key = `n:${m.id}`
          foldKeyOf.set(m.id, key)
          pushItem({ key, layer: l, issues: [m], fold: false, dummyEdge: -1 })
        }
      }
    }
  } else {
    for (const it of issues) {
      const key = `n:${it.id}`
      foldKeyOf.set(it.id, key)
      pushItem({ key, layer: layer.get(it.id) ?? 0, issues: [it], fold: false, dummyEdge: -1 })
    }
  }

  // 哑节点插入 + 边→条目链：tl > fl+1 的边在中间层逐层放哑节点，
  // 任意边降为相邻层短段链（零交叉的前提——没有跨层横跳）。
  interface Chain {
    edge: DagEdge
    /** 条目键 src → 哑节点…→ dst。 */
    items: string[]
  }
  const chains: Chain[] = []
  edges.forEach((e, ei) => {
    if (e.external || !byId.has(e.from) || !byId.has(e.to)) return
    const fl = layer.get(e.from) ?? 0
    const tl = layer.get(e.to) ?? 0
    const items = [foldKeyOf.get(e.from) ?? `n:${e.from}`]
    for (let l = fl + 1; l < tl; l++) {
      const key = `d:${ei}:${l}`
      pushItem({ key, layer: l, issues: [], fold: false, dummyEdge: ei })
      items.push(key)
    }
    items.push(foldKeyOf.get(e.to) ?? `n:${e.to}`)
    chains.push({ edge: e, items })
  })

  // 初始层内序：实体条目按最小成员 id（编号单调 = 稳定），哑节点缀后按边序。
  const minId = (it: Item) => Math.min(...it.issues.map((m) => m.id))
  for (const list of itemsByLayer.values()) {
    list.sort((a, b) => {
      if (a.dummyEdge < 0 && b.dummyEdge < 0) return minId(a) - minId(b)
      if (a.dummyEdge < 0) return -1
      if (b.dummyEdge < 0) return 1
      return a.dummyEdge - b.dummyEdge
    })
  }

  // barycenter 迭代：交替下扫（按上层邻居平均位）/上扫（按下层邻居平均位）
  // 重排层内序，使相邻层连线序一致；序不翻转 = 同带内直连线两两不交叉。
  const upper = new Map<string, string[]>()
  const lower = new Map<string, string[]>()
  for (const ch of chains) {
    for (let i = 0; i + 1 < ch.items.length; i++) {
      const u = ch.items[i]
      const d = ch.items[i + 1]
      upper.set(d, [...(upper.get(d) ?? []), u])
      lower.set(u, [...(lower.get(u) ?? []), d])
    }
  }
  const pos = new Map<string, number>()
  const layerNums = () => [...itemsByLayer.keys()].sort((a, b) => a - b)
  const reindex = () => {
    for (const list of itemsByLayer.values()) list.forEach((it, i) => pos.set(it.key, i))
  }
  reindex()
  const SWEEPS = 12
  for (let s = 0; s < SWEEPS; s++) {
    const down = s % 2 === 0
    const seq = down ? layerNums().slice(1) : layerNums().slice(0, -1).reverse()
    for (const l of seq) {
      const list = itemsByLayer.get(l)!
      const scored = list.map((it, i) => {
        const nbrs = down ? upper.get(it.key) : lower.get(it.key)
        const ps = (nbrs ?? []).map((n) => pos.get(n) ?? 0)
        return { it, v: ps.length ? ps.reduce((a, b) => a + b, 0) / ps.length : i }
      })
      scored.sort((p, q) => p.v - q.v || pos.get(p.it.key)! - pos.get(q.it.key)!)
      scored.forEach((p, i) => {
        list[i] = p.it
        pos.set(p.it.key, i)
      })
    }
  }

  // 转置收尾：相邻对换贪心（目标函数 = 相邻层带内「序翻转」段对数——即
  // 直连线交叉数的精确代理）到局部最优。同层环内弧不进目标（防御路径）。
  const bandSegs = new Map<number, Array<[string, string]>>()
  for (const ch of chains) {
    for (let i = 0; i + 1 < ch.items.length; i++) {
      const lu = itemByKey.get(ch.items[i])!.layer
      const arr = bandSegs.get(lu)
      if (arr) arr.push([ch.items[i], ch.items[i + 1]])
      else bandSegs.set(lu, [[ch.items[i], ch.items[i + 1]]])
    }
  }
  const bandFlips = (l: number) => {
    const segs = bandSegs.get(l)
    if (!segs) return 0
    let n = 0
    for (let i = 0; i < segs.length; i++) {
      for (let j = i + 1; j < segs.length; j++) {
        const dUp = pos.get(segs[i][0])! - pos.get(segs[j][0])!
        const dDown = pos.get(segs[i][1])! - pos.get(segs[j][1])!
        if (dUp * dDown < 0) n++
      }
    }
    return n
  }
  for (let round = 0; round < 60; round++) {
    let improved = false
    for (const l of layerNums()) {
      const list = itemsByLayer.get(l)!
      for (let i = 0; i + 1 < list.length; i++) {
        const before = (l > 0 ? bandFlips(l - 1) : 0) + bandFlips(l)
        ;[list[i], list[i + 1]] = [list[i + 1], list[i]]
        pos.set(list[i].key, i)
        pos.set(list[i + 1].key, i + 1)
        const after = (l > 0 ? bandFlips(l - 1) : 0) + bandFlips(l)
        if (after < before) {
          improved = true
        } else {
          ;[list[i], list[i + 1]] = [list[i + 1], list[i]]
          pos.set(list[i].key, i)
          pos.set(list[i + 1].key, i + 1)
        }
      }
    }
    if (!improved) break
  }

  // 坐标分配：y 由层号（纵向主轴向下滚）；x 每层**水平居中**（层内实占宽
  // 居中于画布）——单卡层自动对齐到画布中线 = 流程图主轴居中，不再整体
  // 贴左；层内相对序不变（刚性平移，零交叉性质不受影响）。实体占 w+gapX
  // 整槽，哑节点只占 DUMMY_GAP 细缝（长边不撑宽画布）。
  const folds: DagFold[] = []
  const posById = new Map<number, { x: number; y: number }>()
  /** 条目连接点（x = 中心；yTop = 条目顶）。 */
  const itemCenter = new Map<string, { x: number; yTop: number }>()
  const layerWidths = new Map<number, number>()
  let contentWidth = dim.pad * 2
  for (const l of layerNums()) {
    let lw = 0
    for (const it of itemsByLayer.get(l)!) lw += it.dummyEdge >= 0 ? DUMMY_GAP : dim.w + dim.gapX
    layerWidths.set(l, Math.max(0, lw - dim.gapX))
    contentWidth = Math.max(contentWidth, layerWidths.get(l)! + dim.pad * 2)
  }
  let layerCount = 0
  let maxLayerSlots = 0
  for (const l of layerNums()) {
    const y = dim.pad + l * (dim.h + dim.gapY)
    layerCount = Math.max(layerCount, l + 1)
    maxLayerSlots = Math.max(maxLayerSlots, itemsByLayer.get(l)!.length)
    let cursor = (contentWidth - layerWidths.get(l)!) / 2
    for (const it of itemsByLayer.get(l)!) {
      if (it.dummyEdge >= 0) {
        itemCenter.set(it.key, { x: cursor + DUMMY_GAP / 2, yTop: y })
        cursor += DUMMY_GAP
      } else {
        itemCenter.set(it.key, { x: cursor + dim.w / 2, yTop: y })
        for (const m of it.issues) posById.set(m.id, { x: cursor, y })
        if (it.fold) {
          folds.push({
            layer: l,
            status: it.issues[0].status,
            size: it.issues.length,
            x: cursor,
            y,
            numbers: it.issues.map((m) => m.number),
          })
        }
        cursor += dim.w + dim.gapX
      }
    }
  }

  const sorted = [...issues].sort((a, b) => a.id - b.id)
  const nodes: DagNode[] = sorted.map((it) => {
    const p = posById.get(it.id) ?? { x: dim.pad, y: dim.pad }
    return {
      id: it.id,
      number: it.number,
      title: it.title,
      status: it.status,
      assigneeId: it.assignee_id ?? null,
      layer: layer.get(it.id) ?? 0,
      x: p.x,
      y: p.y,
      unlayered: !kahnLayered.has(it.id),
      folded: foldedIds.has(it.id),
      indirectDeps: hiddenIndirect.get(it.id) ?? [],
      needsUser: !!it.needs_user,
      planReady: !!it.plan_ready,
    }
  })

  // --- 边 path：相邻层短段链 + 垂直切线贝塞尔 ---
  const yOf = (l: number) => dim.pad + l * (dim.h + dim.gapY)
  const dyCap = Math.min(14, dim.gapY / 2, dim.h / 2)
  for (const ch of chains) {
    const e = ch.edge
    const fl = layer.get(e.from) ?? 0
    const tl = layer.get(e.to) ?? 0
    if (tl === fl) {
      // 同层边（环桶内部防御路径）：底边中点 → 下方浅弧 → 目标底边中点，
      // 不穿卡片本体（弧与邻边妥协可接受——正常数据无环）。
      const a = itemCenter.get(ch.items[0])!
      const b = itemCenter.get(ch.items[1])!
      const yb = yOf(fl) + dim.h
      e.path = `M ${a.x} ${yb} C ${a.x} ${yb + 18}, ${b.x} ${yb + 18}, ${b.x} ${yb}`
      continue
    }
    const pts: Array<{ x: number; y: number }> = []
    const src = itemCenter.get(ch.items[0])!
    pts.push({ x: src.x, y: yOf(fl) + dim.h })
    for (let i = 1; i + 1 < ch.items.length; i++) {
      const c = itemCenter.get(ch.items[i])!
      const l = itemByKey.get(ch.items[i])!.layer
      // 哑节点穿过行带：入顶 y(l)、出底 y(l)+h（两段贝塞尔间为竖直线）。
      pts.push({ x: c.x, y: yOf(l) })
      pts.push({ x: c.x, y: yOf(l) + dim.h })
    }
    const dst = itemCenter.get(ch.items[ch.items.length - 1])!
    pts.push({ x: dst.x, y: yOf(tl) })
    let d = `M ${pts[0].x} ${pts[0].y}`
    for (let i = 1; i < pts.length; i++) {
      const a = pts[i - 1]
      const b = pts[i]
      const dyv = Math.min(dyCap, (b.y - a.y) / 2)
      d += ` C ${a.x} ${a.y + dyv}, ${b.x} ${b.y - dyv}, ${b.x} ${b.y}`
    }
    e.path = d
  }

  // external 短桩：目标上方 28px 处虚线小签 → 短虚线进节点顶部。
  for (const e of edges) {
    if (!e.external) continue
    const t = posById.get(e.to)
    if (!t) continue
    const x2 = t.x + dim.w / 2
    const ya = t.y - 28
    e.path = `M ${x2} ${ya} C ${x2} ${ya + 22}, ${x2} ${t.y - 22}, ${x2} ${t.y}`
  }

  return {
    nodes,
    edges,
    folds,
    layerCount,
    maxLayerSlots,
    unlayeredCount,
    mode,
    contentWidth,
    hiddenEdges: [...hiddenIndirect.values()].reduce((a, b) => a + b.length, 0),
  }
}
