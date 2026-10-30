<script setup lang="ts">
defineProps<{
  modelValue: string
  counts: Record<string, number>
}>()

const emit = defineEmits<{
  (e: 'update:modelValue', tab: string): void
}>()

const tabs = [
  { id: 'events',   label: '实时事件流', icon: '📡' },
  { id: 'sessions', label: '会话浏览器', icon: '💬' },
  // 会话回收站（2026-10-30）：sessions.delete 默认落站，90 天 TTL，可恢复。
  { id: 'trash',    label: '回收站',    icon: '🗑️' },
  { id: 'search',   label: '会话检索',   icon: '🔍' },
  { id: 'audit',    label: '安全审计',   icon: '🔒' },
  { id: 'chain',    label: '审计链',     icon: '🔗' },
  // 行动账本（2026-10-14 AI 原生行动代理）：connectors.receipts 只读视图。
  { id: 'ledger',   label: '行动账本',   icon: '📒' },
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
    >
      <span class="tab-icon">{{ tab.icon }}</span>
      <span class="tab-label">{{ tab.label }}</span>
      <span v-if="counts[tab.id]" class="tab-count">{{ counts[tab.id] }}</span>
    </button>
  </div>
</template>

<style scoped>
/* 走全局 .tabs / .tab 的下划线风格；padding 由父容器 .page-body 提供 */

.tab-icon {
  margin-right: var(--space-1);
  opacity: 0.55;
  font-size: var(--text-sm);
  transition: opacity var(--duration-fast);
}

.tab:hover .tab-icon {
  opacity: 0.85;
}

.tab.active .tab-icon {
  opacity: 1;
}

.tab-count {
  margin-left: var(--space-2);
  font-size: var(--text-xs);
  color: var(--text-muted);
  font-weight: 400;
  font-variant-numeric: tabular-nums;
}

.tab.active .tab-count {
  color: var(--accent);
  font-weight: 500;
}
</style>
