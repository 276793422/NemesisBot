import { describe, it, expect } from 'vitest'
import { fmtTokens, fmtCost, fmtUsageLine } from '../useUsageFormat'

// M5（2026-09-05）：会话用量格式化单测——SessionSidebar 侧栏小字与
// ChatPanel 常驻条共用同一 helper，显示字符串只有这一处真相。

describe('fmtTokens', () => {
  it('缺省/零 → 空串（调用方据此不渲染）', () => {
    expect(fmtTokens(undefined)).toBe('')
    expect(fmtTokens(0)).toBe('')
  })
  it('小数字原样 + 单位', () => {
    expect(fmtTokens(42)).toBe('42 tok')
  })
  it('千位 → k', () => {
    expect(fmtTokens(1200)).toBe('1.2k tok')
  })
  it('百万位 → M', () => {
    expect(fmtTokens(1_500_000)).toBe('1.5M tok')
  })
})

describe('fmtCost', () => {
  it('缺省/零 → 空串', () => {
    expect(fmtCost(undefined)).toBe('')
    expect(fmtCost(0)).toBe('')
  })
  it('≥1 两位小数', () => {
    expect(fmtCost(1.5)).toBe('$1.50')
    expect(fmtCost(12)).toBe('$12.00')
  })
  it('<1 保留四位有效、去尾零', () => {
    expect(fmtCost(0.0031)).toBe('$0.0031')
    expect(fmtCost(0.5)).toBe('$0.50')
    expect(fmtCost(0.25)).toBe('$0.25')
  })
})

describe('fmtUsageLine', () => {
  it('两者都有 → tok · cost', () => {
    expect(fmtUsageLine({ tokens: 1200, cost: 0.5 })).toBe('1.2k tok · $0.50')
  })
  it('只有 cost → 只显示 cost', () => {
    expect(fmtUsageLine({ tokens: 0, cost: 0.25 })).toBe('$0.25')
  })
  it('只有 tokens → 只显示 tokens', () => {
    expect(fmtUsageLine({ tokens: 42, cost: 0 })).toBe('42 tok')
  })
  it('都缺 → 空串', () => {
    expect(fmtUsageLine({})).toBe('')
  })
})
