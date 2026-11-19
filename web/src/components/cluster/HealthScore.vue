<script setup lang="ts">
import { computed } from 'vue'
import VChart from 'vue-echarts'
// echarts/core 按需注册（vue-echarts v7+ 不再自动带全量注册——缺失时
// init 按 series type 查模型构造器落空，报 "TypeError: ch[o] is not a
// constructor"）。gauge 全应用仅本组件使用，注册随组件自带；use() 幂等，
// 与 UsageView / echartsLoader 的词表重叠无害。
import { use } from 'echarts/core'
import { CanvasRenderer } from 'echarts/renderers'
import { GaugeChart } from 'echarts/charts'
import { useTheme } from '../../composables/useTheme'

use([CanvasRenderer, GaugeChart])

// canvas 渲染器不认 CSS 变量（zrender parse 对 var(--x) 落空返回，画布
// 还不随主题切换重绘）——option 必须放解析后的具体色值：以 theme 为依赖
// 在切换时重建 option，getComputedStyle 现读主题令牌。
const { theme } = useTheme()
const themeColor = (name: string, fallback: string): string =>
  getComputedStyle(document.documentElement).getPropertyValue(name).trim() || fallback

const props = defineProps<{
  score: number
}>()

const color = computed(() => {
  if (props.score >= 80) return '#22c55e'
  if (props.score >= 60) return '#eab308'
  if (props.score >= 40) return '#f97316'
  return '#ef4444'
})

const option = computed(() => {
  void theme.value
  return {
    series: [{
      type: 'gauge',
      startAngle: 225,
      endAngle: -45,
      min: 0,
      max: 100,
      pointer: { show: false },
      progress: {
        show: true,
        overlap: false,
        roundCap: true,
        clip: false,
        itemStyle: { color: color.value },
      },
      axisLine: {
        lineStyle: { width: 8, color: [[1, themeColor('--border', '#d4d4d4')]] },
      },
      axisTick: { show: false },
      splitLine: { show: false },
      axisLabel: { show: false },
      detail: {
        fontSize: 20,
        fontWeight: 700,
        color: themeColor('--text', '#1a1a1a'),
        offsetCenter: [0, 0],
        formatter: '{value}',
      },
      data: [{ value: props.score }],
    }],
  }
})
</script>

<template>
  <VChart :option="option" style="height: 100px; width: 100px;" autoresize />
</template>
