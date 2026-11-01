<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import type { PanelSlotState, PanelViewDecl } from '../../composables/usePanels'
import { buildChartOption } from './chartOption'
import { loadECharts, type EChartsType } from './echartsLoader'

// 图表块（2026-11-01 决策 A）：按声明 chart/x_key/series 渲染行数据。
// echarts 经 echartsLoader 动态装载（独立 chunk，chart 面板首访才拉）；
// 实例随数据变化 setOption、容器尺寸 ResizeObserver 自适应、卸载 dispose。
// 未知图表类型（旧前端配新插件声明的降级面）→ 诚实占位，不猜渲染。
const props = defineProps<{ decl: PanelViewDecl; state: PanelSlotState }>()
defineEmits<{ (e: 'refresh'): void }>()

const rows = computed<any[]>(() => {
  const d = props.state.data
  if (Array.isArray(d)) return d
  if (Array.isArray(d?.rows)) return d.rows
  return []
})

const option = computed(() =>
  buildChartOption(props.decl.chart ?? '', props.decl.x_key ?? '', props.decl.series ?? [], rows.value),
)

const el = ref<HTMLDivElement | null>(null)
const echartsReady = ref(false)
let chart: EChartsType | null = null
let ro: ResizeObserver | null = null

onMounted(async () => {
  const echarts = await loadECharts()
  // 未知图表类型（option=null）：不 init 不 setOption——占位面由模板承担，
  // 画布保持隐藏；声明修复重挂载才会装真图。
  if (!el.value || !option.value) return
  chart = echarts.init(el.value)
  chart.setOption(option.value)
  echartsReady.value = true
  // jsdom 无 ResizeObserver（vitest 面）——守卫创建，浏览器恒有。
  if (typeof ResizeObserver !== 'undefined') {
    ro = new ResizeObserver(() => chart?.resize())
    ro.observe(el.value)
  }
})

watch(option, (o) => {
  if (!chart || !o) return
  chart.setOption(o)
})

onBeforeUnmount(() => {
  ro?.disconnect()
  ro = null
  chart?.dispose()
  chart = null
})
</script>

<template>
  <section class="panel-block" data-test="panel-chart">
    <header class="panel-block-head">
      <h3>图表</h3>
      <button class="panel-mini" title="刷新" @click="$emit('refresh')">⟳</button>
    </header>
    <div v-if="state.loading" class="panel-note">加载中…</div>
    <div v-else-if="state.error" class="panel-error">{{ state.error }}</div>
    <!-- 未知类型先于空数据：声明坏了必须说「类型不认识」，不能被「暂无数据」掩蔽 -->
    <div v-else-if="!option" class="panel-note">未知图表类型：{{ decl.chart }}</div>
    <div v-else-if="!rows.length" class="panel-note">暂无数据</div>
    <div v-show="echartsReady" ref="el" class="panel-chart-canvas" />
  </section>
</template>

<style scoped>
.panel-block {
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: 8px;
  padding: var(--space-3);
  margin-bottom: var(--space-3);
}
.panel-block-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: var(--space-2);
}
.panel-block-head h3 {
  margin: 0;
  font-size: var(--text-sm);
  color: var(--text-secondary);
  text-transform: uppercase;
  letter-spacing: 0.04em;
}
.panel-mini {
  background: transparent;
  border: 1px solid var(--border);
  color: var(--text-muted);
  border-radius: 6px;
  padding: 2px 8px;
  cursor: pointer;
  font-size: var(--text-xs);
}
.panel-mini:hover {
  color: var(--text-secondary);
  border-color: var(--text-muted);
}
.panel-note {
  color: var(--text-muted);
  font-size: var(--text-xs);
  padding: var(--space-2) 0;
}
.panel-error {
  color: var(--error);
  font-size: var(--text-xs);
  padding: var(--space-2) 0;
  white-space: pre-wrap;
}
.panel-chart-canvas {
  width: 100%;
  height: 260px;
}
</style>
