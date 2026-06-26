<script setup lang="ts">
/**
 * Floating dropdown UI for the variable picker. Bind to the state returned
 * by `useVariablePicker()` and call `insert()` on click.
 */
import type { VariablePickerState, VariableOption } from './useVariablePicker'

const props = defineProps<{
  state: VariablePickerState
}>()

const emit = defineEmits<{
  (e: 'insert', variable: string): void
  (e: 'close'): void
}>()

function click(opt: VariableOption, ev: MouseEvent) {
  ev.preventDefault()
  emit('insert', opt.value)
}

function groups(opts: VariableOption[]): { group: string; items: VariableOption[] }[] {
  const map = new Map<string, VariableOption[]>()
  for (const o of opts) {
    const g = o.group ?? '变量'
    if (!map.has(g)) map.set(g, [])
    map.get(g)!.push(o)
  }
  return Array.from(map, ([group, items]) => ({ group, items }))
}
</script>

<template>
  <div
    v-if="state.open"
    class="var-dropdown"
    :style="{ top: state.anchor?.top + 'px', left: state.anchor?.left + 'px' }"
    @mousedown.prevent
  >
    <div v-if="state.matches.length === 0" class="var-empty">无匹配变量</div>
    <template v-else>
      <div v-for="grp in groups(state.matches)" :key="grp.group" class="var-group">
        <div class="var-group-title">{{ grp.group }}</div>
        <div
          v-for="(opt, idx) in grp.items"
          :key="opt.value"
          class="var-item"
          :class="{ active: state.matches.indexOf(opt) === state.selectedIndex }"
          @click="click(opt, $event)"
        >
          <span class="var-item-value">{{ opt.value }}</span>
          <span v-if="opt.label && opt.label !== opt.value" class="var-item-label">{{ opt.label }}</span>
        </div>
      </div>
    </template>
  </div>
</template>

<style scoped>
.var-dropdown {
  position: fixed;
  z-index: 1001;
  min-width: 220px;
  max-width: 320px;
  max-height: 280px;
  overflow-y: auto;
  background: var(--bg-surface, var(--bg-primary, #fff));
  border: 1px solid var(--border);
  border-radius: var(--radius-sm);
  box-shadow: 0 8px 24px rgba(0, 0, 0, 0.18);
  padding: var(--space-1);
  font-size: var(--text-sm);
}

.var-empty {
  padding: var(--space-2);
  color: var(--text-muted);
  text-align: center;
}

.var-group + .var-group {
  border-top: 1px solid var(--border);
  margin-top: var(--space-1);
  padding-top: var(--space-1);
}

.var-group-title {
  font-size: var(--text-xs);
  color: var(--text-muted);
  text-transform: uppercase;
  padding: 2px var(--space-1);
}

.var-item {
  display: flex;
  flex-direction: column;
  gap: 1px;
  padding: 4px var(--space-1);
  border-radius: var(--radius-sm);
  cursor: pointer;
}

.var-item:hover,
.var-item.active {
  background: var(--bg-secondary);
}

.var-item-value {
  font-family: 'Consolas', monospace;
  font-size: var(--text-xs);
  color: var(--text-primary);
}

.var-item-label {
  font-size: var(--text-xs);
  color: var(--text-muted);
}
</style>
