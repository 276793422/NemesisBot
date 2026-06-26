<script setup lang="ts">
/**
 * Condition node — expression-based branch. Expression is evaluated by the
 * backend (typically Rust expr + {{var}} substitution). Output is bool.
 *
 * The form is a single line input with @-variable insertion.
 */
import { computed } from 'vue'
import FormField from './FormField.vue'
import TextField from './TextField.vue'

const props = defineProps<{
  config: Record<string, unknown>
  variables?: import('./useVariablePicker').VariableOption[]
}>()

const emit = defineEmits<{
  (e: 'update', patch: Record<string, unknown>): void
}>()

const condition = computed(() => typeof props.config.condition === 'string' ? props.config.condition : 'true')

function set(v: string) {
  emit('update', { condition: v })
}
</script>

<template>
  <FormField
    label="条件表达式"
    required
    hint="返回布尔值的表达式。可用 {{var}} 引用变量。例：{{count}} > 5"
  >
    <TextField
      :model-value="condition"
      :variables="props.variables"
      :multiline="false"
      placeholder="{{value}} > 0"
      @update:model-value="set"
    />
  </FormField>
</template>
