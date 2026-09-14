// H1（goal P1）：displayActor 解析层纯函数测试。
// 已知 id → kind/可读名；短可读 id 原样；UUID 长串 → 未知节点(短id)。

import { describe, expect, it } from 'vitest'
import { actorDualIn, displayActorIn, shortId } from '../useBoardActors'

const map = new Map([
  ['node-node-8c45eb69-d4f2-4b7a-98d7-07ba4159ddd1', { id: 'node-node-8c45eb69-d4f2-4b7a-98d7-07ba4159ddd1', name: 'Alex', role: 'worker', category: 'general', online: true }],
  ['board', { id: 'board', name: '看板', role: 'system', category: 'system', online: true }],
])

describe('useBoardActors 纯函数', () => {
  it('已知 id → kind/可读名', () => {
    expect(displayActorIn(map, 'agent', 'node-node-8c45eb69-d4f2-4b7a-98d7-07ba4159ddd1')).toBe('agent/Alex')
  })

  it('未知 UUID 长串 → 未知节点(短id) 回退', () => {
    const out = displayActorIn(map, 'agent', 'node-laptop-fgo6hj0e-2176e13a-8eef-43c6-93cf-ae9d031980a6')
    expect(out).toContain('未知节点')
    expect(out).toContain('…')
  })

  it('短可读 id（未注册）原样保留（board/admin 等系统 actor）', () => {
    expect(displayActorIn(map, 'admin', 'zoo')).toBe('admin/zoo')
  })

  it('actorDualIn：定向场景名+短id 双显', () => {
    const id = 'node-node-8c45eb69-d4f2-4b7a-98d7-07ba4159ddd1'
    const out = actorDualIn(map, id)
    expect(out).toContain('Alex')
    expect(out).toContain('node-node-8c…')
  })

  it('shortId 截断（>14 取前 12 + 省略号）', () => {
    expect(shortId('abcdefgh')).toBe('abcdefgh')
    expect(shortId('abcdefghijklmnop')).toBe('abcdefghijkl…')
  })

  it('displayActorIn：短可读 id（含连字符如 node-b）原样保留', () => {
    expect(displayActorIn(map, 'agent', 'node-b')).toBe('agent/node-b')
  })
})
