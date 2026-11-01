/**
 * chart 视图 option 构造（2026-11-01 决策 A）——纯函数，零 echarts 依赖。
 *
 * 数据形状 = 行数组（`{rows:[...]}` / 裸数组，PanelTable 同款兼容）；
 * 类目轴取行 JSON 的 `x_key` 字段，系列值取 `series[].key` 字段。
 * 词表对齐 manifest.rs CHART_TYPES（line|bar|pie）：line/bar 每系列一条
 * 线/一柱组；pie 是单环图——**第一系列**做值列，`x_key` 做扇区名（多系列
 * 声明对 pie 无意义，validate 面不拦但渲染面只消费第一项）。
 * 未知图表类型 → null（面板侧诚实占位，不猜渲染）。
 * 数值列经 `Number()` 归一（echarts 对 NaN 点自动断线/跳扇）。
 */
import type { PanelColumn } from '../../composables/usePanels'

export interface ChartRowLike {
  [key: string]: unknown
}

export function buildChartOption(
  chartType: string,
  xKey: string,
  seriesDefs: PanelColumn[],
  rows: ChartRowLike[],
): Record<string, unknown> | null {
  if (chartType !== 'line' && chartType !== 'bar' && chartType !== 'pie') return null
  const categories = rows.map((r) => String(r?.[xKey] ?? ''))
  const num = (v: unknown): number => {
    const n = Number(v ?? 0)
    return Number.isNaN(n) ? 0 : n
  }
  if (chartType === 'pie') {
    const valueKey = seriesDefs[0]?.key ?? ''
    return {
      tooltip: { trigger: 'item' },
      legend: { bottom: 0 },
      series: [
        {
          type: 'pie',
          radius: '62%',
          data: rows.map((r, i) => ({ name: categories[i], value: num(r?.[valueKey]) })),
        },
      ],
    }
  }
  return {
    tooltip: { trigger: 'axis' },
    legend: {},
    grid: { left: 8, right: 16, top: 32, bottom: 8, containLabel: true },
    xAxis: { type: 'category', data: categories },
    yAxis: { type: 'value' },
    series: seriesDefs.map((s) => ({
      name: s.label,
      type: chartType,
      data: rows.map((r) => num(r?.[s.key])),
    })),
  }
}
