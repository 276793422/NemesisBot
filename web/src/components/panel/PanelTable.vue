<script setup lang="ts">
import { computed } from 'vue'
import type { PanelSlotState, PanelViewDecl } from '../../composables/usePanels'

// 表格块（v0 词汇）：按声明列序渲染 guest 回包行。
// 行形态兼容：`{rows: [...]}`（panel-echo 约定）或裸数组。
const props = defineProps<{ decl: PanelViewDecl; state: PanelSlotState }>()
defineEmits<{ (e: 'refresh'): void }>()

const rows = computed<any[]>(() => {
  const d = props.state.data
  if (Array.isArray(d)) return d
  if (Array.isArray(d?.rows)) return d.rows
  return []
})
</script>

<template>
  <section class="panel-block">
    <header class="panel-block-head">
      <h3>表格</h3>
      <button class="panel-mini" title="刷新" @click="$emit('refresh')">⟳</button>
    </header>
    <div v-if="state.loading" class="panel-note">加载中…</div>
    <div v-else-if="state.error" class="panel-error">{{ state.error }}</div>
    <table v-else-if="rows.length" class="panel-table">
      <thead>
        <tr>
          <th v-for="c in decl.columns" :key="c.key">{{ c.label }}</th>
        </tr>
      </thead>
      <tbody>
        <tr v-for="(row, i) in rows" :key="i">
          <td v-for="c in decl.columns" :key="c.key">{{ row?.[c.key] ?? '—' }}</td>
        </tr>
      </tbody>
    </table>
    <div v-else class="panel-note">暂无数据</div>
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
.panel-table {
  width: 100%;
  border-collapse: collapse;
  font-size: var(--text-xs);
}
.panel-table th,
.panel-table td {
  text-align: left;
  padding: 6px 10px;
  border-bottom: 1px solid var(--border);
}
.panel-table th {
  color: var(--text-muted);
  font-weight: 500;
}
.panel-table tr:last-child td {
  border-bottom: none;
}
</style>
