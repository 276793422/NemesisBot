<script setup lang="ts">
/**
 * Reusable key/value list editor for object-shaped config fields like
 * HTTP headers, tool args, sub_workflow input. Each row is a (key, value)
 * pair; the value can be a string with @-variable interpolation.
 *
 * The editor operates on `Record<string, string>` shape. If the underlying
 * config holds non-string scalars (numbers, bools), they're stringified on
 * the way in and re-parsed (number/boolean/json) on the way out.
 */
import { computed } from 'vue'
import TextField from './TextField.vue'
import type { VariableOption } from './useVariablePicker'

const props = defineProps<{
  /** The object to edit (typically `config.headers`, `config.args`, etc). */
  modelValue: Record<string, unknown>
  /** Available @-variables for the value field. */
  variables?: VariableOption[]
  /** Input placeholder for the value field. */
  valuePlaceholder?: string
  /** Input placeholder for the key field. */
  keyPlaceholder?: string
}>()

const emit = defineEmits<{
  (e: 'update:modelValue', value: Record<string, unknown>): void
}>()

interface Row {
  key: string
  value: string
}

const rows = computed<Row[]>(() => {
  const obj = props.modelValue ?? {}
  return Object.entries(obj).map(([k, v]) => ({
    key: k,
    value: scalarToString(v),
  }))
})

function scalarToString(v: unknown): string {
  if (v === null || v === undefined) return ''
  if (typeof v === 'string') return v
  if (typeof v === 'number' || typeof v === 'boolean') return String(v)
  return JSON.stringify(v)
}

function parseString(s: string): unknown {
  const trimmed = s.trim()
  if (trimmed === '') return ''
  if (/^-?\d+$/.test(trimmed)) {
    const n = Number(trimmed)
    if (Number.isSafeInteger(n)) return n
  }
  if (/^-?\d+\.\d+$/.test(trimmed)) {
    const n = Number(trimmed)
    if (Number.isFinite(n)) return n
  }
  if (trimmed === 'true') return true
  if (trimmed === 'false') return false
  if (trimmed === 'null') return null
  // Leave {{var}} references as strings — backend resolves them.
  return s
}

function emitRows(next: Row[]) {
  const obj: Record<string, unknown> = {}
  for (const r of next) {
    const k = r.key.trim()
    if (!k) continue
    obj[k] = parseString(r.value)
  }
  emit('update:modelValue', obj)
}

function updateKey(idx: number, key: string) {
  const next = rows.value.map((r, i) => (i === idx ? { ...r, key } : r))
  emitRows(next)
}

function updateValue(idx: number, value: string) {
  const next = rows.value.map((r, i) => (i === idx ? { ...r, value } : r))
  emitRows(next)
}

function deleteRow(idx: number) {
  const next = rows.value.filter((_, i) => i !== idx)
  emitRows(next)
}

function addRow() {
  const next = [...rows.value, { key: '', value: '' }]
  emitRows(next)
}
</script>

<template>
  <div class="kv-list">
    <div v-if="rows.length === 0" class="kv-empty">（空）</div>
    <div v-for="(r, idx) in rows" :key="idx" class="kv-row">
      <input
        type="text"
        class="kv-key"
        :value="r.key"
        :placeholder="keyPlaceholder ?? '键'"
        spellcheck="false"
        @input="updateKey(idx, ($event.target as HTMLInputElement).value)"
      />
      <TextField
        class="kv-value"
        :model-value="r.value"
        :variables="variables"
        :placeholder="valuePlaceholder ?? '值'"
        @update:model-value="(v: string) => updateValue(idx, v)"
      />
      <button class="kv-del" title="删除" @click="deleteRow(idx)">×</button>
    </div>
    <button class="kv-add" @click="addRow">+ 添加</button>
  </div>
</template>

<style scoped>
.kv-list {
  display: flex;
  flex-direction: column;
  gap: var(--space-1);
}

.kv-empty {
  font-size: var(--text-xs);
  color: var(--text-muted);
  padding: var(--space-1);
  text-align: center;
}

.kv-row {
  display: flex;
  gap: var(--space-1);
  align-items: stretch;
}

.kv-key {
  flex: 1;
  padding: 4px 6px;
  background: var(--bg-primary);
  border: 1px solid var(--border);
  border-radius: var(--radius-sm);
  color: var(--text-primary);
  font-size: var(--text-xs);
  font-family: 'Consolas', monospace;
  min-width: 0;
}

.kv-key:focus {
  outline: none;
  border-color: var(--accent);
}

.kv-value {
  flex: 2;
  min-width: 0;
}

.kv-del {
  background: transparent;
  border: 1px solid transparent;
  color: var(--text-muted);
  cursor: pointer;
  padding: 0 8px;
  font-size: var(--text-base);
  border-radius: var(--radius-sm);
}

.kv-del:hover {
  background: var(--bg-secondary);
  color: var(--danger, #e74c3c);
}

.kv-add {
  align-self: flex-start;
  background: transparent;
  border: 1px dashed var(--border);
  color: var(--text-secondary);
  padding: 4px var(--space-2);
  border-radius: var(--radius-sm);
  cursor: pointer;
  font-size: var(--text-xs);
}

.kv-add:hover {
  border-color: var(--accent);
  color: var(--accent);
}
</style>
