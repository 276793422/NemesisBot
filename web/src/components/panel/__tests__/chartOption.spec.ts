import { describe, it, expect } from 'vitest'
import { buildChartOption } from '../chartOption'

// chart 视图 option 构造（2026-11-01 决策 A）——纯函数面：三图表词表形状、
// 数值归一、pie 单环语义（第一系列=值列、x_key=扇区名）、未知类型诚实 null。
const SERIES = [
  { key: 'count', label: '数量' },
  { key: 'score', label: '分数' },
]
const ROWS = [
  { day: '周一', count: 3, score: 8.5 },
  { day: '周二', count: 5, score: 7 },
  { day: '周三', count: 4, score: 'bad' },
]

describe('buildChartOption', () => {
  it('line：类目轴 + 每系列一条线，数值列 Number 归一（坏值落 0）', () => {
    const o = buildChartOption('line', 'day', SERIES, ROWS) as any
    expect(o.xAxis).toEqual({ type: 'category', data: ['周一', '周二', '周三'] })
    expect(o.yAxis).toEqual({ type: 'value' })
    expect(o.series).toHaveLength(2)
    expect(o.series[0]).toMatchObject({ name: '数量', type: 'line' })
    // 'bad' → NaN → 0（诚实落底，不炸渲染）。
    expect(o.series[0].data).toEqual([3, 5, 4])
    expect(o.series[1].data).toEqual([8.5, 7, 0])
  })

  it('bar：与 line 同数据形状，type=bar', () => {
    const o = buildChartOption('bar', 'day', SERIES, ROWS) as any
    expect(o.series.every((s: any) => s.type === 'bar')).toBe(true)
    expect(o.series[0].data).toEqual([3, 5, 4])
  })

  it('pie：单环——第一系列做值列，x_key 做扇区名', () => {
    const o = buildChartOption('pie', 'day', SERIES, ROWS) as any
    expect(o.series).toHaveLength(1)
    expect(o.series[0].type).toBe('pie')
    expect(o.series[0].data).toEqual([
      { name: '周一', value: 3 },
      { name: '周二', value: 5 },
      { name: '周三', value: 4 },
    ])
  })

  it('未知图表类型 → null（面板侧诚实占位，不猜渲染）', () => {
    expect(buildChartOption('treemap', 'day', SERIES, ROWS)).toBeNull()
    expect(buildChartOption('', 'day', SERIES, ROWS)).toBeNull()
  })

  it('空行集：形状照常（空类目/空系列数据），不炸不猜', () => {
    const o = buildChartOption('line', 'day', SERIES, []) as any
    expect(o.xAxis.data).toEqual([])
    expect(o.series[0].data).toEqual([])
    const p = buildChartOption('pie', 'day', SERIES, []) as any
    expect(p.series[0].data).toEqual([])
  })

  it('缺 x_key / 空系列：字段名空串兜底，值列落 0', () => {
    const o = buildChartOption('bar', '', [], ROWS) as any
    expect(o.xAxis.data).toEqual(['', '', ''])
    expect(o.series).toEqual([])
    const p = buildChartOption('pie', 'day', [], ROWS) as any
    expect(p.series[0].data).toEqual([
      { name: '周一', value: 0 },
      { name: '周二', value: 0 },
      { name: '周三', value: 0 },
    ])
  })
})
