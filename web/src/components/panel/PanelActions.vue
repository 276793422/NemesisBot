<script setup lang="ts">
import type { PanelButton } from '../../composables/usePanels'

// 动作块（v0 词汇）：纯按钮面，无 bind 取数。confirm 门在组件内把守
//（window.confirm，测试可 mock），放行后把按钮上抛给 PanelView 执行。
defineProps<{ buttons: PanelButton[]; busy: boolean }>()
const emit = defineEmits<{ (e: 'run', button: PanelButton): void }>()

function onClick(button: PanelButton) {
  if (button.confirm && !window.confirm(`确认执行「${button.label}」？`)) return
  emit('run', button)
}
</script>

<template>
  <section class="panel-block">
    <header class="panel-block-head">
      <h3>操作</h3>
    </header>
    <div v-if="!buttons.length" class="panel-note">无动作</div>
    <div v-else class="panel-actions">
      <button
        v-for="(b, i) in buttons"
        :key="i"
        class="panel-btn"
        :class="{ danger: b.danger }"
        :disabled="busy"
        :data-test="`pa-btn-${i}`"
        @click="onClick(b)"
      >
        {{ b.label }}
      </button>
    </div>
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
.panel-note {
  color: var(--text-muted);
  font-size: var(--text-xs);
  padding: var(--space-2) 0;
}
.panel-actions {
  display: flex;
  flex-wrap: wrap;
  gap: var(--space-2);
}
.panel-btn {
  background: var(--accent);
  border: none;
  border-radius: 6px;
  color: #fff;
  padding: 6px 16px;
  font-size: var(--text-xs);
  cursor: pointer;
}
.panel-btn:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}
.panel-btn.danger {
  background: var(--error-bg);
  color: var(--error);
  border: 1px solid var(--error);
}
</style>
