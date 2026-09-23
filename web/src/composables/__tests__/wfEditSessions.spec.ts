/**
 * wfEditSessions 纯函数单测：computeDraftDiff（草稿 ↔ 已注册定义 diff）。
 *
 * 只测纯逻辑（键序不敏感比较、added/removed/changed 分类、isNew）。
 * 会话登记表（localStorage 单例）不在 jsdom 单测里驱动——它由组件
 * E2E（Playwright）覆盖。
 */
import { describe, it, expect } from 'vitest'
import { computeDraftDiff } from '../wfEditSessions'
import type { WorkflowDef } from '../../types/workflow'

function wf(nodes: { id: string; node_type: string; config?: Record<string, unknown> }[]): WorkflowDef {
  return {
    name: 'demo',
    description: '',
    version: '1.0.0',
    triggers: [],
    nodes: nodes.map(n => ({ id: n.id, node_type: n.node_type, config: n.config ?? {} })),
    edges: [],
    variables: {},
    metadata: {},
  }
}

describe('computeDraftDiff', () => {
  it('全新工作流（无旧定义）→ 全部节点记 added，isNew=true', () => {
    const draft = wf([
      { id: 'start', node_type: 'llm' },
      { id: 'end', node_type: 'transform' },
    ])
    const diff = computeDraftDiff(draft, null)
    expect(diff.isNew).toBe(true)
    expect(diff.added.map(r => r.nodeId).sort()).toEqual(['end', 'start'])
    expect(diff.removed).toHaveLength(0)
    expect(diff.changed).toHaveLength(0)
  })

  it('空草稿 + 无旧定义 → added 为空但不炸', () => {
    const diff = computeDraftDiff(wf([]), null)
    expect(diff.isNew).toBe(true)
    expect(diff.added).toHaveLength(0)
  })

  it('节点完全相同 → 无差异', () => {
    const a = wf([{ id: 'n1', node_type: 'llm', config: { prompt: 'hi' } }])
    const diff = computeDraftDiff(a, structuredClone(a))
    expect(diff.isNew).toBe(false)
    expect(diff.added).toHaveLength(0)
    expect(diff.removed).toHaveLength(0)
    expect(diff.changed).toHaveLength(0)
  })

  it('config 值变化（键序不同）→ changed，且键序不敏感', () => {
    const old = wf([{ id: 'n1', node_type: 'llm', config: { prompt: 'hi', model: 'a' } }])
    const draft = wf([{ id: 'n1', node_type: 'llm', config: { model: 'a', prompt: 'changed' } }])
    const diff = computeDraftDiff(draft, old)
    expect(diff.changed).toHaveLength(1)
    expect(diff.changed[0]).toMatchObject({ nodeId: 'n1', nodeType: 'llm', oldNodeType: 'llm' })
  })

  it('同键序同值嵌套对象 → 不算 changed（键序不敏感递归）', () => {
    const old = wf([{ id: 'n1', node_type: 'http', config: { headers: { A: '1', B: '2' }, url: 'http://x' } }])
    const draft = wf([{ id: 'n1', node_type: 'http', config: { url: 'http://x', headers: { B: '2', A: '1' } } }])
    const diff = computeDraftDiff(draft, old)
    expect(diff.changed).toHaveLength(0)
  })

  it('新增 + 删除 + 修改混合 → 三桶各归其位', () => {
    const old = wf([
      { id: 'keep', node_type: 'llm' },
      { id: 'gone', node_type: 'delay', config: { seconds: 1 } },
      { id: 'mutate', node_type: 'transform', config: { expression: 'identity' } },
    ])
    const draft = wf([
      { id: 'keep', node_type: 'llm' },
      { id: 'mutate', node_type: 'transform', config: { expression: 'trim' } },
      { id: 'fresh', node_type: 'http', config: { url: 'http://x' } },
    ])
    const diff = computeDraftDiff(draft, old)
    expect(diff.isNew).toBe(false)
    expect(diff.added.map(r => r.nodeId)).toEqual(['fresh'])
    expect(diff.removed.map(r => r.nodeId)).toEqual(['gone'])
    expect(diff.removed[0]?.oldNodeType).toBe('delay')
    expect(diff.changed.map(r => r.nodeId)).toEqual(['mutate'])
  })

  it('node_type 变化（同 id）→ changed，带 oldNodeType', () => {
    const old = wf([{ id: 'n1', node_type: 'delay' }])
    const draft = wf([{ id: 'n1', node_type: 'llm' }])
    const diff = computeDraftDiff(draft, old)
    expect(diff.changed).toHaveLength(1)
    expect(diff.changed[0]?.nodeType).toBe('llm')
    expect(diff.changed[0]?.oldNodeType).toBe('delay')
  })
})
