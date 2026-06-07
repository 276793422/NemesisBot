<script setup lang="ts">
import { computed } from 'vue'
import VChart from 'vue-echarts'

const props = defineProps<{
  score: number
}>()

const color = computed(() => {
  if (props.score >= 80) return '#22c55e'
  if (props.score >= 60) return '#eab308'
  if (props.score >= 40) return '#f97316'
  return '#ef4444'
})

const option = computed(() => ({
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
      lineStyle: { width: 8, color: [[1, 'var(--border)']] },
    },
    axisTick: { show: false },
    splitLine: { show: false },
    axisLabel: { show: false },
    detail: {
      fontSize: 20,
      fontWeight: 700,
      color: 'var(--text)',
      offsetCenter: [0, 0],
      formatter: '{value}',
    },
    data: [{ value: props.score }],
  }],
}))
</script>

<template>
  <VChart :option="option" style="height: 100px; width: 100px;" autoresize />
</template>
