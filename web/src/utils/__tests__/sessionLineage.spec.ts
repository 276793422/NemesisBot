// WS9/P17（能力扩展，2026-09-26）：会话谱系前端投影纯函数测试。
// 覆盖 arrangeLineage（树序/缩进深度/跨组根/环兜底/深度帽）与
// lineageChainLines（根→父反转缩进/self 行/空 title 回退）。
import { describe, it, expect } from 'vitest'
import { arrangeLineage, lineageChainLines, LINEAGE_MAX_DEPTH } from '../sessionLineage'

interface Row {
  id: string
  parent?: string
  title?: string
}

describe('arrangeLineage 谱系树重排', () => {
  it('子会话紧跟父会话之后，深度随层级递增', () => {
    // 输入故意乱序（子在前、根在后）——输出应为根 → 子 → 孙。
    const rows: Row[] = [
      { id: 'g', parent: 'p', title: '孙' },
      { id: 'p', parent: 'root', title: '子' },
      { id: 'root', title: '根' },
    ]
    const { ordered, depthOf } = arrangeLineage(rows)
    expect(ordered.map(r => r.id)).toEqual(['root', 'p', 'g'])
    expect(depthOf('root')).toBe(0)
    expect(depthOf('p')).toBe(1)
    expect(depthOf('g')).toBe(2)
  })

  it('兄弟保持输入相对序；根之间保持输入相对序', () => {
    const rows: Row[] = [
      { id: 'r1', title: '根一' },
      { id: 'r2', title: '根二' },
      { id: 'c2', parent: 'r1', title: '子二' },
      { id: 'c1', parent: 'r1', title: '子一' },
    ]
    const { ordered, depthOf } = arrangeLineage(rows)
    // r1 树在前（输入序），r2 随后；r1 的兄弟按输入序 c2 → c1。
    expect(ordered.map(r => r.id)).toEqual(['r1', 'c2', 'c1', 'r2'])
    expect(depthOf('r2')).toBe(0)
    expect(depthOf('c1')).toBe(1)
  })

  it('父 key 带规范前缀（agent:main:session:{sid}）→ 剥前缀后挂上子链', () => {
    const rows: Row[] = [
      { id: 'c1', parent: 'agent:main:session:root', title: '子' },
      { id: 'root', title: '根' },
    ]
    const { ordered, depthOf } = arrangeLineage(rows)
    expect(ordered.map(r => r.id)).toEqual(['root', 'c1'])
    expect(depthOf('c1')).toBe(1)
  })

  it('父不在本批（已删/跨组）→ 该子会话按本组根处理（诚实降级）', () => {
    const rows: Row[] = [
      { id: 'orphan', parent: 'agent:main:session:gone', title: '孤儿 fork' },
      { id: 'plain', title: '普通' },
    ]
    const { ordered, depthOf } = arrangeLineage(rows)
    expect(ordered.map(r => r.id)).toEqual(['orphan', 'plain'])
    expect(depthOf('orphan')).toBe(0)
  })

  it('自指当根、纯环（互指）兜底：按输入序追加尾部、深度 0，不丢行不死循环', () => {
    const rows: Row[] = [
      { id: 'a', parent: 'b' },
      { id: 'b', parent: 'a' },
      { id: 'self', parent: 'self' },
      { id: 'ok', title: '正常根' },
    ]
    const { ordered, depthOf } = arrangeLineage(rows)
    // 自指（self）按规则当根处理（输入序在 ok 之前 → 根区先出）；a↔b
    // 纯环无根可达，兜底按输入序尾随、深度 0。
    expect(ordered.map(r => r.id)).toEqual(['self', 'ok', 'a', 'b'])
    expect(depthOf('ok')).toBe(0)
    expect(depthOf('a')).toBe(0)
    expect(depthOf('self')).toBe(0)
  })

  it('深度超过帽：钳在 LINEAGE_MAX_DEPTH（显示层不无限缩进）', () => {
    const n = LINEAGE_MAX_DEPTH + 3
    const rows: Row[] = Array.from({ length: n }, (_, i) => ({
      id: `n${i}`,
      parent: i === 0 ? undefined : `n${i - 1}`,
    }))
    const { ordered, depthOf } = arrangeLineage(rows)
    expect(ordered.map(r => r.id)).toEqual(rows.map(r => r.id))
    expect(depthOf('n0')).toBe(0)
    expect(depthOf(`n${n - 1}`)).toBe(LINEAGE_MAX_DEPTH)
  })

  it('空输入与无谱系输入零增量', () => {
    expect(arrangeLineage([]).ordered).toEqual([])
    const rows: Row[] = [{ id: 'x' }, { id: 'y' }]
    const { ordered, depthOf } = arrangeLineage(rows)
    expect(ordered.map(r => r.id)).toEqual(['x', 'y'])
    expect(depthOf('x')).toBe(0)
  })
})

describe('lineageChainLines 祖先链渲染行', () => {
  it('后端「最近父 → 根」顺序反转为「根 → 父」并逐级缩进 ↳', () => {
    const lines = lineageChainLines([
      { id: 'p1', title: '父会话' },
      { id: 'root', title: '根会话' },
    ])
    expect(lines).toEqual(['↳ 根会话', '  ↳ 父会话'])
  })

  it('selfTitle 追加当前会话行（更深层级 · 前缀）', () => {
    const lines = lineageChainLines([{ id: 'p1', title: '父' }], '当前')
    expect(lines).toEqual(['↳ 父', '  · 当前'])
  })

  it('空 title 回退 id 前 12 位；空祖先链只剩 self 行', () => {
    expect(lineageChainLines([{ id: 'agent_main_session_abcdefghij', title: '  ' }]))
      .toEqual(['↳ agent_main_s'])
    expect(lineageChainLines([], '只有自己')).toEqual(['· 只有自己'])
  })
})
