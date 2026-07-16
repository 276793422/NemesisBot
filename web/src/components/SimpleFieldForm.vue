<script setup lang="ts">
/**
 * Edit a flat object of primitives with toggles / text / password — no JSON.
 */
import { computed } from 'vue'
import { fieldMetaFor, type FriendlyField } from '../lib/friendlyFields'

const props = defineProps<{
  modelValue: Record<string, any>
  metaTable?: Record<string, FriendlyField>
  /** Keys to hide entirely */
  hideKeys?: string[]
}>()

const emit = defineEmits<{
  (e: 'update:modelValue', v: Record<string, any>): void
}>()

const rows = computed(() => {
  const hide = new Set(props.hideKeys || [])
  const table = props.metaTable || {}
  return Object.entries(props.modelValue || {})
    .filter(([k, v]) => !hide.has(k) && (v === null || ['string', 'number', 'boolean'].includes(typeof v)))
    .map(([k, v]) => ({ meta: fieldMetaFor(k, v, table), value: v }))
})

function setField(key: string, value: any) {
  emit('update:modelValue', { ...props.modelValue, [key]: value })
}
</script>

<template>
  <div class="simple-field-form">
    <div v-if="rows.length === 0" class="empty-hint">暂无需要填写的简单选项</div>
    <div v-for="{ meta, value } in rows" :key="meta.key" class="field-row">
      <div class="field-label">
        <span>{{ meta.label }}</span>
        <span v-if="meta.hint" class="field-hint">{{ meta.hint }}</span>
      </div>
      <div class="field-control">
        <div
          v-if="meta.kind === 'toggle'"
          class="toggle"
          :class="{ active: !!value }"
          role="switch"
          :aria-checked="!!value"
          @click="setField(meta.key, !value)"
        />
        <input
          v-else-if="meta.kind === 'number'"
          class="form-input"
          type="number"
          :value="value ?? ''"
          @input="setField(meta.key, Number(($event.target as HTMLInputElement).value))"
        />
        <input
          v-else-if="meta.kind === 'password'"
          class="form-input"
          type="password"
          :value="value ?? ''"
          autocomplete="off"
          placeholder="已保存则保持不变，或粘贴新密钥"
          @input="setField(meta.key, ($event.target as HTMLInputElement).value)"
        />
        <input
          v-else
          class="form-input"
          type="text"
          :value="value ?? ''"
          @input="setField(meta.key, ($event.target as HTMLInputElement).value)"
        />
      </div>
    </div>
  </div>
</template>

<style scoped>
.simple-field-form {
  display: flex;
  flex-direction: column;
  gap: var(--space-4);
}
.field-row {
  display: grid;
  grid-template-columns: minmax(120px, 200px) 1fr;
  gap: var(--space-3);
  align-items: center;
}
.field-label {
  display: flex;
  flex-direction: column;
  gap: 2px;
  font-size: var(--text-sm);
  font-weight: 500;
}
.field-hint {
  font-size: var(--text-xs);
  color: var(--text-muted);
  font-weight: 400;
}
.empty-hint {
  color: var(--text-muted);
  font-size: var(--text-sm);
  padding: var(--space-4);
  text-align: center;
}
@media (max-width: 640px) {
  .field-row {
    grid-template-columns: 1fr;
  }
}
</style>
