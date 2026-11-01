/**
 * echarts 按需装载器（2026-11-01 决策 A）：动态 import → Vite 把 echarts
 * 拆成独立 chunk，只有面板页出现 chart 视图才会拉取（chart 未用的面板零
 * 载荷成本）。模块化注册（echarts/core + 三图表 + 三组件），与 UsageView
 * 的按需注册同款词表；Promise 记忆化防并发重复 import。
 */
import type { EChartsType } from 'echarts/core'

let memo: Promise<typeof import('echarts/core')> | null = null

export function loadECharts(): Promise<typeof import('echarts/core')> {
  memo ??= (async () => {
    const [core, renderers, charts, components] = await Promise.all([
      import('echarts/core'),
      import('echarts/renderers'),
      import('echarts/charts'),
      import('echarts/components'),
    ])
    core.use([
      renderers.CanvasRenderer,
      charts.LineChart,
      charts.BarChart,
      charts.PieChart,
      components.GridComponent,
      components.TooltipComponent,
      components.LegendComponent,
    ])
    return core
  })()
  return memo
}

export type { EChartsType }
