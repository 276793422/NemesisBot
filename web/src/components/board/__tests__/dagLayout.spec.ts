import { describe, it, expect } from 'vitest'
import {
  computeLayout,
  renderMode,
  svgSize,
  nodeDims,
  COMPACT_THRESHOLD,
  HINT_THRESHOLD,
  FOLD_MIN,
  type DagIssue,
  type DagLayout,
} from '../dagLayout'

// DAG 全景流程图布局纯函数单测（2026-10-15 需求②③）：Kahn 分层 / 环桶 /
// external 边 / compact 折叠 / 阈值切档；+ v4 零交叉层级走线（传递约简 +
// Sugiyama 哑节点/barycenter/转置）。布局正确性 = 渲染正确性的前提；
// 零交叉是用户钦定硬要求，用「逐段严格相交计数 = 0」钉死。

function iss(id: number, over: Partial<DagIssue> = {}): DagIssue {
  return { id, number: `NB-${id}`, title: `t${id}`, status: 'todo', depends_on: [], ...over }
}

function dep(id: number, status = 'todo') {
  return { id, number: `NB-${id}`, status }
}

/** 真实看板形状 replica（2026-10-15 快照，id 平移到 1..20）：根 1 + 三 PRD
 *  子单 + 顺序执行链。45 条执行依赖中 26 条传递冗余——全量画非平面（离线
 *  实证 20 处交叉），约简后近纯链恒平面。此图是零交叉的生产形状回归锁。 */
function realGraph(): DagIssue[] {
  const deps: Record<number, number[]> = {
    5: [2, 3, 4],
    6: [5],
    7: [5, 6],
    8: [7],
    9: [7],
    10: [8, 9],
    11: [8, 10],
    12: [8, 11],
    13: [8, 11, 12],
    14: [8, 12, 13],
    15: [8, 12, 13, 14],
    16: [8, 14, 15],
    17: [8, 9, 10, 13, 14, 15, 16],
    18: [10, 11, 12, 13, 14, 15, 16, 17],
    19: [18],
    20: [18, 19],
  }
  return Array.from({ length: 20 }, (_, i) => {
    const id = i + 1
    return iss(id, {
      status: 'done',
      parent_issue_id: id === 1 ? null : 1,
      depends_on: (deps[id] ?? []).map((d) => dep(d, 'done')),
    })
  })
}

// --- 零交叉检查器：path（M + C* 序列）采样成弦段，异边弦段严格相交计数 ---
// 端点接触不算（同目标汇入同一点 = 用户钦定的「目标一致合并」，合法）。
function chordsOf(p: string): Array<[number, number, number, number]> {
  const nums = p.match(/[\d.]+/g)!.map(Number)
  const pts: Array<[number, number]> = [[nums[0], nums[1]]]
  for (let i = 2; i < nums.length; i += 6) {
    const [x1, y1, x2, y2, x, y] = nums.slice(i, i + 6)
    const [px, py] = pts[pts.length - 1]
    for (let s = 1; s <= 8; s++) {
      const t = s / 8
      const mt = 1 - t
      pts.push([
        mt * mt * mt * px + 3 * mt * mt * t * x1 + 3 * mt * t * t * x2 + t * t * t * x,
        mt * mt * mt * py + 3 * mt * mt * t * y1 + 3 * mt * t * t * y2 + t * t * t * y,
      ])
    }
  }
  const segs: Array<[number, number, number, number]> = []
  for (let i = 1; i < pts.length; i++) segs.push([pts[i - 1][0], pts[i - 1][1], pts[i][0], pts[i][1]])
  return segs
}

function ccw(ax: number, ay: number, bx: number, by: number, cx: number, cy: number): number {
  return (bx - ax) * (cy - ay) - (by - ay) * (cx - ax)
}

function properCross(s: [number, number, number, number], t: [number, number, number, number]): boolean {
  const d1 = ccw(t[0], t[1], t[2], t[3], s[0], s[1])
  const d2 = ccw(t[0], t[1], t[2], t[3], s[2], s[3])
  const d3 = ccw(s[0], s[1], s[2], s[3], t[0], t[1])
  const d4 = ccw(s[0], s[1], s[2], s[3], t[2], t[3])
  return ((d1 > 0 && d2 < 0) || (d1 < 0 && d2 > 0)) && ((d3 > 0 && d4 < 0) || (d3 < 0 && d4 > 0))
}

function crossingsOf(l: DagLayout): number {
  const per = l.edges.filter((e) => !e.external).map((e) => chordsOf(e.path))
  let n = 0
  for (let a = 0; a < per.length; a++) {
    for (let b = a + 1; b < per.length; b++) {
      for (const sa of per[a]) {
        for (const sb of per[b]) {
          if (properCross(sa, sb)) n++
        }
      }
    }
  }
  return n
}

describe('renderMode 阈值切档', () => {
  it('≤300 full / 301–2000 compact / >2000 hint（规模不设限，只是渲染档位）', () => {
    expect(renderMode(0)).toBe('full')
    expect(renderMode(COMPACT_THRESHOLD)).toBe('full')
    expect(renderMode(COMPACT_THRESHOLD + 1)).toBe('compact')
    expect(renderMode(HINT_THRESHOLD)).toBe('compact')
    expect(renderMode(HINT_THRESHOLD + 1)).toBe('hint')
    expect(renderMode(50000)).toBe('hint')
  })
})

describe('Kahn 分层', () => {
  it('线性链 A→B→C：层号 0/1/2，上→下叙事（纵向主轴）', () => {
    const l = computeLayout([
      iss(1, { depends_on: [] }),
      iss(2, { depends_on: [dep(1)] }),
      iss(3, { depends_on: [dep(2)] }),
    ])
    expect(l.unlayeredCount).toBe(0)
    expect(l.layerCount).toBe(3)
    const byId = new Map(l.nodes.map((n) => [n.id, n]))
    expect(byId.get(1)!.layer).toBe(0)
    expect(byId.get(2)!.layer).toBe(1)
    expect(byId.get(3)!.layer).toBe(2)
    expect(byId.get(1)!.y).toBeLessThan(byId.get(2)!.y)
    expect(byId.get(2)!.y).toBeLessThan(byId.get(3)!.y)
  })

  it('并行无依赖节点同层横排（x 递增、y 相同）', () => {
    const l = computeLayout([iss(1), iss(2), iss(3)])
    const byId = new Map(l.nodes.map((n) => [n.id, n]))
    expect(l.layerCount).toBe(1)
    for (const id of [1, 2, 3]) expect(byId.get(id)!.layer).toBe(0)
    expect(byId.get(1)!.y).toBe(byId.get(2)!.y)
    expect(byId.get(1)!.x).toBeLessThan(byId.get(2)!.x)
    expect(byId.get(2)!.x).toBeLessThan(byId.get(3)!.x)
  })

  it('多上游取 max(上游层)+1：菱形汇合点不被浅上游压扁', () => {
    const l = computeLayout([
      iss(1),
      iss(2, { depends_on: [dep(1)] }),
      iss(3, { depends_on: [dep(2)] }),
      iss(4, { depends_on: [dep(1), dep(3)] }),
    ])
    const byId = new Map(l.nodes.map((n) => [n.id, n]))
    expect(byId.get(4)!.layer).toBe(3)
    expect(l.layerCount).toBe(4)
  })
})

describe('环防御（lenient 拆解漏网）', () => {
  it('纯环 A→B→A：全员 unlayered 落桶置底，计数可提示', () => {
    const l = computeLayout([
      iss(1, { depends_on: [dep(2)] }),
      iss(2, { depends_on: [dep(1)] }),
    ])
    expect(l.unlayeredCount).toBe(2)
    for (const n of l.nodes) {
      expect(n.unlayered).toBe(true)
      expect(n.layer).toBe(l.layerCount - 1)
    }
  })

  it('混合 A→B、B↔C 环：A 正常分层，B/C 落环桶（不拖累健康部分）', () => {
    const l = computeLayout([
      iss(1),
      iss(2, { depends_on: [dep(1), dep(3)] }), // B→C 回边成环
      iss(3, { depends_on: [dep(2)] }),
    ])
    expect(l.unlayeredCount).toBe(2)
    const byId = new Map(l.nodes.map((n) => [n.id, n]))
    expect(byId.get(1)!.layer).toBe(0)
    expect(byId.get(1)!.unlayered).toBe(false)
    expect(byId.get(2)!.unlayered).toBe(true)
    expect(byId.get(3)!.unlayered).toBe(true)
    // 环桶在健康层之后（置底）。
    expect(byId.get(2)!.layer).toBeGreaterThan(byId.get(1)!.layer)
  })
})

describe('边分类（blocking / external）', () => {
  it('上游 done → 边已满足；上游 todo → 边阻塞（琥珀高亮数据源）', () => {
    const l = computeLayout([
      iss(1, { status: 'done' }),
      iss(2, { status: 'todo', depends_on: [dep(1, 'done')] }),
      iss(3, { status: 'done' }),
      iss(4, { status: 'todo', depends_on: [dep(3, 'todo')] }),
    ])
    const e2 = l.edges.find((e) => e.to === 2)!
    const e4 = l.edges.find((e) => e.to === 4)!
    expect(e2.blocking).toBe(false)
    expect(e4.blocking).toBe(true)
  })

  it('external 边：上游不在视图（跨项目/归档）→ external + 恒 blocking + 不参与分层', () => {
    const l = computeLayout([
      iss(10, { depends_on: [dep(99, 'in_progress')] }),
      iss(11, { depends_on: [dep(99, 'done')] }),
    ])
    const e10 = l.edges.find((e) => e.to === 10)!
    const e11 = l.edges.find((e) => e.to === 11)!
    expect(e10.external).toBe(true)
    expect(e10.blocking).toBe(true)
    expect(e10.fromNumber).toBe('NB-99')
    expect(e11.external).toBe(true)
    // external 不参与分层：两个节点都落 layer 0。
    expect(l.nodes.every((n) => n.layer === 0)).toBe(true)
    expect(l.unlayeredCount).toBe(0)
  })
})

describe('compact 折叠（同层同状态 ×N）', () => {
  it('full 档不折叠（少量同态保留可点）', () => {
    const issues = Array.from({ length: 20 }, (_, i) => iss(i + 1))
    const l = computeLayout(issues)
    expect(l.mode).toBe('full')
    expect(l.folds).toHaveLength(0)
    expect(l.nodes.every((n) => !n.folded)).toBe(true)
  })

  it('compact 档同层同状态 ≥ FOLD_MIN → 折叠成组，成员 folded，槽位压缩', () => {
    const issues = Array.from({ length: COMPACT_THRESHOLD + 10 }, (_, i) => iss(i + 1))
    const l = computeLayout(issues)
    expect(l.mode).toBe('compact')
    expect(l.folds).toHaveLength(1)
    expect(l.folds[0].size).toBe(COMPACT_THRESHOLD + 10)
    expect(l.folds[0].status).toBe('todo')
    expect(l.nodes.every((n) => n.folded)).toBe(true)
    // 全部成员共享组坐标（边自然汇聚组矩形）。
    expect(new Set(l.nodes.map((n) => `${n.x},${n.y}`)).size).toBe(1)
    expect(l.maxLayerSlots).toBe(1)
    const size = svgSize(l)
    // 纵向压缩：一组一槽（h = 2*pad + 1*(26+10) − 10 + 64 外部签头部余量 ≈ 122），
    // 而不是 310 槽（11160+）。
    expect(size.h).toBeLessThan(130)
  })

  it('混合状态：各自达到 FOLD_MIN 的状态折叠成组，少数派保留独立可点', () => {
    const issues = Array.from({ length: COMPACT_THRESHOLD + 10 }, (_, i) =>
      iss(i + 1, { status: i < 5 ? 'done' : i < 7 ? 'blocked' : 'todo' }),
    )
    const l = computeLayout(issues)
    // done(5) 与 todo(308) 都 ≥ FOLD_MIN 各成一组；blocked(2) 不折。
    expect(l.folds).toHaveLength(2)
    expect(l.folds.map((f) => f.status).sort()).toEqual(['done', 'todo'])
    const unfolded = l.nodes.filter((n) => !n.folded)
    expect(unfolded).toHaveLength(2) // 2 blocked 独立
    expect(unfolded.every((n) => n.status === 'blocked')).toBe(true)
    expect(l.maxLayerSlots).toBe(4) // done 组 1 + blocked 独立 2 + todo 组 1
  })

  it('FOLD_MIN 边界：FOLD_MIN-1 个同态不折叠，多数派照常折叠', () => {
    const issues = Array.from({ length: COMPACT_THRESHOLD + 1 }, (_, i) =>
      iss(i + 1, { status: i < FOLD_MIN - 1 ? 'done' : 'todo' }),
    )
    const l = computeLayout(issues)
    expect(l.folds).toHaveLength(1) // 只有 todo 折叠
    expect(l.folds[0].status).toBe('todo')
    const unfolded = l.nodes.filter((n) => !n.folded)
    expect(unfolded).toHaveLength(FOLD_MIN - 1)
    expect(unfolded.every((n) => n.status === 'done')).toBe(true)
  })
})

describe('拆解分层与拆解边（parent_issue_id）', () => {
  it('根需求独占首行：无 depends_on 的子单靠 parent 下界落到第 1 层（不再与根同排）', () => {
    const l = computeLayout([
      iss(1),
      iss(2, { parent_issue_id: 1 }),
      iss(3, { parent_issue_id: 1 }),
      iss(4, { parent_issue_id: 1 }),
    ])
    const byId = new Map(l.nodes.map((n) => [n.id, n]))
    expect(byId.get(1)!.layer).toBe(0)
    for (const id of [2, 3, 4]) {
      expect(byId.get(id)!.layer).toBe(1)
      expect(byId.get(id)!.y).toBeGreaterThan(byId.get(1)!.y)
    }
    expect(l.layerCount).toBe(2)
  })

  it('父层抬升沿 depends_on 下游传播：孙单层号同步下移', () => {
    const l = computeLayout([
      iss(1),
      iss(2, { parent_issue_id: 1 }), // Kahn 层 0 → 松弛 1
      iss(3, { depends_on: [dep(2)] }), // Kahn 层 1 → 传播 2
    ])
    const byId = new Map(l.nodes.map((n) => [n.id, n]))
    expect(byId.get(2)!.layer).toBe(1)
    expect(byId.get(3)!.layer).toBe(2)
  })

  it('拆解边只为「无视图内执行上游」的子单补画（有 depends_on 的不双线）', () => {
    const l = computeLayout([
      iss(1),
      iss(2, { parent_issue_id: 1 }),
      iss(3, { parent_issue_id: 1, depends_on: [dep(2)] }),
    ])
    const decomp = l.edges.filter((e) => e.decomposition)
    expect(decomp).toHaveLength(1)
    expect(decomp[0]).toMatchObject({ from: 1, to: 2, blocking: false })
    // 执行边照旧、非拆解。
    expect(l.edges.some((e) => !e.decomposition && e.from === 2 && e.to === 3)).toBe(true)
  })

  it('父单不在视图（跨项目/归档）：无下界、无拆解边，节点落层 0', () => {
    const l = computeLayout([iss(1, { parent_issue_id: 99 })])
    const byId = new Map(l.nodes.map((n) => [n.id, n]))
    expect(byId.get(1)!.layer).toBe(0)
    expect(l.edges).toHaveLength(0)
  })

  it('环防御不受影响：拆解下界不把节点拖出环桶，健康部分照常分层', () => {
    const l = computeLayout([
      iss(1),
      iss(2, { parent_issue_id: 1, depends_on: [dep(3)] }), // B→C 回边成环
      iss(3, { depends_on: [dep(2)] }),
      iss(4, { parent_issue_id: 1 }),
    ])
    const byId = new Map(l.nodes.map((n) => [n.id, n]))
    expect(l.unlayeredCount).toBe(2)
    expect(byId.get(2)!.unlayered).toBe(true)
    expect(byId.get(3)!.unlayered).toBe(true)
    expect(byId.get(1)!.layer).toBe(0)
    expect(byId.get(4)!.layer).toBe(1) // parent 下界照常生效
  })
})

describe('传递约简（v4 画面前提：链上可替代的边不画线，信息并入 +N 徽标）', () => {
  it('1→4 被 1→2→3→4 隐含 → 不画（indirectDeps 记 NB-1）；3→4 保留', () => {
    const l = computeLayout([
      iss(1),
      iss(2, { depends_on: [dep(1)] }),
      iss(3, { depends_on: [dep(2)] }),
      iss(4, { depends_on: [dep(1), dep(3)] }),
    ])
    expect(l.edges.some((e) => !e.decomposition && e.from === 1 && e.to === 4)).toBe(false)
    expect(l.edges.some((e) => !e.decomposition && e.from === 3 && e.to === 4)).toBe(true)
    expect(l.hiddenEdges).toBe(1)
    expect(l.nodes.find((n) => n.id === 4)!.indirectDeps).toEqual(['NB-1'])
  })

  it('并行汇入互不隐含：3 上游边全保留（无替代路径）', () => {
    const l = computeLayout([iss(1), iss(2), iss(3), iss(4, { depends_on: [dep(1), dep(2), dep(3)] })])
    expect(l.edges.filter((e) => e.to === 4)).toHaveLength(3)
    expect(l.hiddenEdges).toBe(0)
    expect(l.nodes.find((n) => n.id === 4)!.indirectDeps).toEqual([])
  })

  it('拆解边不参与约简（父子叙事恒画，不计 hiddenEdges）', () => {
    const l = computeLayout([iss(1), iss(2, { parent_issue_id: 1 })])
    expect(l.edges.filter((e) => e.decomposition)).toHaveLength(1)
    expect(l.hiddenEdges).toBe(0)
  })

  it('external 边不参与约简（视图外不成路径，恒画）', () => {
    const l = computeLayout([
      iss(1, { depends_on: [dep(99)] }),
      iss(2, { depends_on: [dep(1), dep(99)] }),
    ])
    // 99→2 external 恒画；1→2 无替代路径保留。
    expect(l.edges.filter((e) => e.external)).toHaveLength(2)
    expect(l.edges.some((e) => !e.decomposition && e.from === 1 && e.to === 2)).toBe(true)
  })
})

describe('v4 零交叉层级走线（哑节点 + barycenter + 转置）', () => {
  it('构造翻转案例零交叉（naive id 序必交叉，barycenter 换序消除）', () => {
    // L0=[1,2]；3←[1,2]、4←[1]。id 序 [3,4] 时 2→3 × 1→4 交叉；
    // barycenter 下扫把 4 排到 3 前（bc(4)=0 < bc(3)=0.5）→ 0 交叉。
    const l = computeLayout([
      iss(1),
      iss(2),
      iss(3, { depends_on: [dep(1), dep(2)] }),
      iss(4, { depends_on: [dep(1)] }),
    ])
    expect(crossingsOf(l)).toBe(0)
  })

  it('扇入汇合同点：各入边终点 = 目标顶边中点（目标一致合并）；路径互异', () => {
    const l = computeLayout([iss(1), iss(2), iss(3), iss(4, { depends_on: [dep(1), dep(2), dep(3)] })])
    const dim = nodeDims(l.mode)
    const n4 = l.nodes.find((n) => n.id === 4)!
    const ins = l.edges.filter((e) => e.to === 4)
    expect(ins).toHaveLength(3)
    for (const e of ins) {
      const nums = e.path.match(/[\d.]+/g)!.map(Number)
      expect(nums[nums.length - 2]).toBeCloseTo(n4.x + dim.w / 2)
      expect(nums[nums.length - 1]).toBeCloseTo(n4.y)
    }
    const paths = ins.map((e) => e.path)
    expect(new Set(paths).size).toBe(paths.length)
  })

  it('扇出同点：出边起点 = 源底边中点', () => {
    const l = computeLayout([iss(1), iss(2, { depends_on: [dep(1)] }), iss(3, { depends_on: [dep(1)] })])
    const dim = nodeDims(l.mode)
    const n1 = l.nodes.find((n) => n.id === 1)!
    const outs = l.edges.filter((e) => e.from === 1)
    expect(outs).toHaveLength(2)
    for (const e of outs) {
      const nums = e.path.match(/[\d.]+/g)!.map(Number)
      expect(nums[0]).toBeCloseTo(n1.x + dim.w / 2)
      expect(nums[1]).toBeCloseTo(n1.y + dim.h)
    }
  })

  it('相邻短边端点：底边中点 → 顶边中点（垂直切线贝塞尔）', () => {
    const l = computeLayout([iss(1), iss(2, { depends_on: [dep(1)] })])
    const dim = nodeDims(l.mode)
    const e = l.edges.find((x) => x.from === 1 && x.to === 2)!
    const n1 = l.nodes.find((n) => n.id === 1)!
    const n2 = l.nodes.find((n) => n.id === 2)!
    const nums = e.path.match(/[\d.]+/g)!.map(Number)
    expect(nums[0]).toBeCloseTo(n1.x + dim.w / 2)
    expect(nums[1]).toBeCloseTo(n1.y + dim.h)
    expect(nums[nums.length - 2]).toBeCloseTo(n2.x + dim.w / 2)
    expect(nums[nums.length - 1]).toBeCloseTo(n2.y)
  })

  it('长边降为逐层短段：路径含哑节点所在层的顶 y 与底 y（穿行带）', () => {
    // 2→4 跨 2 层（4 被另一上游 3 抬到 L2，2→4 无替代路径 = 幸存长边）：
    // 中间层 1 有哑节点，path y 序列含 y(1) 与 y(1)+h。
    const l = computeLayout([
      iss(1),
      iss(2),
      iss(3, { depends_on: [dep(1)] }),
      iss(4, { depends_on: [dep(2), dep(3)] }),
    ])
    const dim = nodeDims(l.mode)
    const e24 = l.edges.find((e) => e.from === 2 && e.to === 4)!
    expect(e24).toBeDefined()
    const ys = e24.path.match(/[\d.]+/g)!.map(Number).filter((_, i) => i % 2 === 1)
    const y1top = dim.pad + 1 * (dim.h + dim.gapY)
    expect(ys).toContain(y1top)
    expect(ys).toContain(y1top + dim.h)
  })

  it('哑节点窄缝不撑宽画布（长边图宽 ≪ 同层多实体宽）', () => {
    const long = computeLayout([
      iss(1),
      iss(2),
      iss(3, { depends_on: [dep(1)] }),
      iss(4, { depends_on: [dep(2), dep(3)] }),
    ])
    const wide = computeLayout([iss(1), iss(2), iss(3), iss(4, { depends_on: [dep(1), dep(3)] })])
    expect(svgSize(long).w).toBeLessThan(svgSize(wide).w)
  })

  it('真实 20 节点依赖图 replica：26 条冗余边隐藏、零交叉（生产形状回归锁）', () => {
    const l = computeLayout(realGraph())
    // 45 执行依赖 − 19 保留 = 26 隐含（+3 拆解边恒画 = 22 drawn）。
    expect(l.hiddenEdges).toBe(26)
    expect(l.edges).toHaveLength(22)
    expect(crossingsOf(l)).toBe(0)
  })
})

describe('svgSize', () => {
  it('高度随层数增长（纵向主轴）、宽度随最大槽位数增长', () => {
    const chain = [iss(1), iss(2, { depends_on: [dep(1)] }), iss(3, { depends_on: [dep(2)] })]
    const s1 = svgSize(computeLayout(chain))
    const parallel = [iss(1), iss(2), iss(3)]
    const s2 = svgSize(computeLayout(parallel))
    expect(s1.h).toBeGreaterThan(s2.h)
    expect(s2.w).toBeGreaterThan(s1.w)
  })
})
