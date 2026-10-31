<script setup lang="ts">
import { reactive, watch } from 'vue'
import type { PanelViewDecl } from '../../composables/usePanels'

// 表单块（v0 词汇）：声明字段驱动（string→input / text→textarea /
// bool→checkbox / int→number / secret→password）。submit 把值上抛，
// bind 命令由 PanelView 统一执行（secret 值只走 guest，不回显）。
const props = defineProps<{ decl: PanelViewDecl; busy: boolean }>()
const emit = defineEmits<{ (e: 'submit', values: Record<string, unknown>): void }>()

const model = reactive<Record<string, any>>({})

watch(
  () => props.decl,
  (d) => {
    for (const k of Object.keys(model)) delete model[k]
    for (const f of d.fields) model[f.key] = f.field_type === 'bool' ? false : ''
  },
  { immediate: true },
)

function submit() {
  const values: Record<string, unknown> = {}
  for (const f of props.decl.fields) {
    const raw = model[f.key]
    if (f.field_type === 'bool') {
      values[f.key] = !!raw
    } else if (f.field_type === 'int') {
      values[f.key] = raw === '' || raw === null || raw === undefined ? null : Number(raw)
    } else {
      values[f.key] = raw ?? ''
    }
  }
  emit('submit', values)
}
</script>

<template>
  <section class="panel-block">
    <header class="panel-block-head">
      <h3>表单</h3>
    </header>
    <form class="panel-form" @submit.prevent="submit">
      <label v-for="f in decl.fields" :key="f.key" class="pf-row">
        <span class="pf-label">{{ f.label }}</span>
        <textarea
          v-if="f.field_type === 'text'"
          v-model="model[f.key]"
          :placeholder="f.placeholder || ''"
          rows="3"
          :data-test="`pf-${f.key}`"
        />
        <input
          v-else-if="f.field_type === 'bool'"
          v-model="model[f.key]"
          type="checkbox"
          :data-test="`pf-${f.key}`"
        />
        <input
          v-else-if="f.field_type === 'int'"
          v-model="model[f.key]"
          type="number"
          :placeholder="f.placeholder || ''"
          :data-test="`pf-${f.key}`"
        />
        <input
          v-else-if="f.field_type === 'secret'"
          v-model="model[f.key]"
          type="password"
          autocomplete="off"
          :placeholder="f.placeholder || ''"
          :data-test="`pf-${f.key}`"
        />
        <input
          v-else
          v-model="model[f.key]"
          type="text"
          :placeholder="f.placeholder || ''"
          :data-test="`pf-${f.key}`"
        />
        <span v-if="f.hint" class="pf-hint">{{ f.hint }}</span>
      </label>
      <div class="pf-actions">
        <button type="submit" class="panel-btn" :disabled="busy || !decl.fields.length" data-test="pf-submit">
          提交
        </button>
      </div>
    </form>
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
.panel-form {
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
}
.pf-row {
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.pf-label {
  font-size: var(--text-xs);
  color: var(--text-secondary);
}
.pf-row input[type='text'],
.pf-row input[type='password'],
.pf-row input[type='number'],
.pf-row textarea {
  background: var(--bg-secondary);
  border: 1px solid var(--border);
  border-radius: 6px;
  color: var(--text-primary);
  padding: 6px 10px;
  font-size: var(--text-xs);
  font-family: inherit;
}
.pf-row input:focus,
.pf-row textarea:focus {
  outline: none;
  border-color: var(--accent);
}
.pf-row input[type='checkbox'] {
  width: 16px;
  height: 16px;
  align-self: flex-start;
}
.pf-hint {
  font-size: var(--text-xs);
  color: var(--text-muted);
}
.pf-actions {
  display: flex;
  justify-content: flex-end;
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
</style>
