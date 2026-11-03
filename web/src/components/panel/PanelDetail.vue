<script setup lang="ts">
import { computed } from 'vue'
import type { PanelSlotState, PanelViewDecl } from '../../composables/usePanels'

// 详情块（v0 词汇）：单对象按声明列序键值排布（无对象 → 诚实空态）。
const props = defineProps<{ decl: PanelViewDecl; state: PanelSlotState }>()
defineEmits<{ (e: 'refresh'): void }>()

const isObject = computed(() => !!props.state.data && typeof props.state.data === 'object' && !Array.isArray(props.state.data))
</script>

<template>
  <section class="panel-block">
    <header class="panel-block-head">
      <h3>详情</h3>
      <button class="panel-mini" title="刷新" @click="$emit('refresh')">⟳</button>
    </header>
    <div v-if="state.loading" class="panel-note">加载中…</div>
    <div v-else-if="state.error" class="panel-error">{{ state.error }}</div>
    <div v-else-if="!isObject" class="panel-note">暂无数据</div>
    <dl v-else class="panel-detail">
      <template v-for="c in decl.columns" :key="c.key">
        <dt>{{ c.label }}</dt>
        <dd>{{ state.data?.[c.key] ?? '—' }}</dd>
      </template>
    </dl>
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
.panel-detail {
  display: grid;
  grid-template-columns: auto 1fr;
  gap: 6px 16px;
  margin: 0;
  font-size: var(--text-xs);
}
.panel-detail dt {
  color: var(--text-muted);
}
.panel-detail dd {
  margin: 0;
  color: var(--text-primary);
  word-break: break-all;
}
</style>
