<script setup lang="ts">
defineProps<{
  modelValue: string
}>()

const emit = defineEmits<{
  (e: 'update:modelValue', tab: string): void
}>()

const tabs = [
  { id: 'kanban', label: '看板' },
  { id: 'list', label: '列表' },
  { id: 'projects', label: '项目' },
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
