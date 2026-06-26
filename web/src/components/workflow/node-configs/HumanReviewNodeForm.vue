<script setup lang="ts">
/**
 * Human-review node — pauses execution until a human approves/rejects.
 * The `message` is what reviewers see in the approval UI.
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

const message = computed(() => typeof props.config.message === 'string' ? props.config.message : '')

function set(v: string) { emit('update', { message: v }) }
</script>

<template>
  <FormField
    label="审核提示文案"
    required
    hint="会展示给审核人，说明本次审核要做什么"
  >
    <TextField
      :model-value="message"
      :variables="props.variables"
      :multiline="true"
      :rows="4"
      placeholder="请审核是否发送给客户：{{draft}}"
      @update:model-value="set"
    />
  </FormField>
</template>
