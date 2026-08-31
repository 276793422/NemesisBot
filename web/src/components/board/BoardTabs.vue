<script setup lang="ts">
defineProps<{
  modelValue: string
}>()

const emit = defineEmits<{
  (e: 'update:modelValue', tab: string): void
}>()

// 排序即使用依赖链（2026-08-31 用户定义）：先建项目 → 项目下配任务列表
// → 列表内容上看板 → 收件箱接收回流 → 自动化定时建单收尾。
const tabs = [
  { id: 'projects', label: '项目' },
  { id: 'list', label: '列表' },
  { id: 'kanban', label: '看板' },
  { id: 'inbox', label: '收件箱' },
  { id: 'autopilot', label: '自动化' },
]
</script>

<template>
  <div class="tabs">
    <button
      v-for="tab in tabs"
      :key="tab.id"
      class="tab"
      :class="{ active: modelValue === tab.id }"
      @click="emit('update:modelValue', tab.id)"
    >{{ tab.label }}</button>
  </div>
</template>
