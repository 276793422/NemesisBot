<script setup lang="ts">
defineProps<{
  modelValue: string
  // B35「需要人的事」页签徽标：需要人介入的信号总数（0/undefined 不显示）。
  needsHuman?: number
}>()

const emit = defineEmits<{
  (e: 'update:modelValue', tab: string): void
}>()

// 排序即使用依赖链（2026-08-31 用户定义）：先建项目 → 项目下配任务列表
// → 列表内容上看板 → 全景流程图纵览依赖 → 收件箱接收回流 → 需要人的事
// （B35 注意力经济：人只看需要人的信号，D4 拍板 projects 落地不变）→
// 自动化定时建单收尾 → 讨论跨节点沟通 → 决策流审计（全自动流转 P5/E2，
// 回滚误判）→ 配置调自动化开关（P1/A4）。
const tabs = [
  { id: 'projects', label: '项目' },
  { id: 'list', label: '列表' },
  { id: 'kanban', label: '看板' },
  { id: 'dag', label: '流程图' },
  { id: 'inbox', label: '收件箱' },
  { id: 'needs_human', label: '需要人的事' },
  { id: 'autopilot', label: '自动化' },
  { id: 'discussion', label: '讨论' },
  { id: 'audit', label: '决策流' },
  { id: 'config', label: '配置' },
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
    >{{ tab.label }}<span
      v-if="tab.id === 'needs_human' && (needsHuman ?? 0) > 0"
      class="tab-badge"
      data-test="needs-human-badge"
    >{{ needsHuman }}</span></button>
  </div>
</template>

<style scoped>
.tab-badge {
  margin-left: var(--space-1, 4px);
  background: var(--danger-bg, rgba(220, 53, 69, 0.15));
  color: var(--danger-text, #dc3545);
  border-radius: 999px;
  font-size: var(--text-xs, 12px);
  padding: 0 var(--space-1-5, 6px);
  min-width: 18px;
  line-height: 18px;
  text-align: center;
  display: inline-block;
}
</style>
