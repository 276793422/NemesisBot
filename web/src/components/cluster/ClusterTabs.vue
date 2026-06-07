<script setup lang="ts">
defineProps<{
  modelValue: string
}>()

const emit = defineEmits<{
  (e: 'update:modelValue', tab: string): void
}>()

const tabs = [
  { id: 'overview', label: '概览' },
  { id: 'nodes', label: '节点' },
  { id: 'tasks', label: '任务' },
  { id: 'topology', label: '拓扑' },
  { id: 'identity', label: '身份' },
  { id: 'settings', label: '配置' },
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
