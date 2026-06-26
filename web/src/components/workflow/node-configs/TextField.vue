<script setup lang="ts">
/**
 * Text input with built-in variable picker (`@` trigger). This is the
 * default input control for string fields in node forms.
 *
 * Two-way binds via v-model. Parent passes the available variables via
 * `variables` prop.
 */
import { ref, watch } from 'vue'
import { useVariablePicker, type VariableOption } from './useVariablePicker'
import VariableDropdown from './VariableDropdown.vue'

const props = withDefaults(
  defineProps<{
    modelValue: string
    variables?: VariableOption[]
    placeholder?: string
    multiline?: boolean
    rows?: number
    disabled?: boolean
  }>(),
  {
    variables: () => [],
    placeholder: '',
    multiline: false,
    rows: 3,
    disabled: false,
  },
)

const emit = defineEmits<{
  (e: 'update:modelValue', value: string): void
}>()

const inputRef = ref<HTMLInputElement | HTMLTextAreaElement | null>(null)

const { state, onInput, onKeyDown, insert } = useVariablePicker(
  () => inputRef.value,
  () => props.variables,
)

function onValueChange(e: Event) {
  const t = e.target as HTMLInputElement | HTMLTextAreaElement
  emit('update:modelValue', t.value)
  onInput(e)
}

function onInsert(v: string) {
  insert(v)
}

watch(
  () => props.modelValue,
  () => {
    // No-op: input value is bound via :value below. The picker state is
    // managed by onInput, not by external modelValue changes.
  },
)
</script>

<template>
  <div class="text-field">
    <textarea
      v-if="multiline"
      ref="inputRef"
      class="form-textarea"
      :value="modelValue"
      :placeholder="placeholder"
      :rows="rows"
      :disabled="disabled"
      spellcheck="false"
      @input="onValueChange"
      @keydown="onKeyDown"
    ></textarea>
    <input
      v-else
      ref="inputRef"
      class="form-input"
      type="text"
      :value="modelValue"
      :placeholder="placeholder"
      :disabled="disabled"
      spellcheck="false"
      @input="onValueChange"
      @keydown="onKeyDown"
    />
    <VariableDropdown :state="state" @insert="onInsert" @close="state.open = false" />
  </div>
</template>

<style scoped>
.text-field {
  position: relative;
  width: 100%;
}

.form-input,
.form-textarea {
  width: 100%;
  padding: 6px 8px;
  background: var(--bg-primary);
  border: 1px solid var(--border);
  border-radius: var(--radius-sm);
  color: var(--text-primary);
  font-size: var(--text-sm);
  font-family: inherit;
  box-sizing: border-box;
}

.form-textarea {
  font-family: 'Consolas', 'Courier New', monospace;
  resize: vertical;
  min-height: 60px;
}

.form-input:focus,
.form-textarea:focus {
  outline: none;
  border-color: var(--accent);
}

.form-input:disabled,
.form-textarea:disabled {
  opacity: 0.6;
  cursor: not-allowed;
}
</style>
