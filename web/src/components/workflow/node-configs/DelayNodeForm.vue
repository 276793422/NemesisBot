<script setup lang="ts">
/** Delay node — waits N seconds before proceeding. */
import FormField from './FormField.vue'

const props = defineProps<{
  config: Record<string, unknown>
  variables?: import('./useVariablePicker').VariableOption[]
}>()

const emit = defineEmits<{
  (e: 'update', patch: Record<string, unknown>): void
}>()

function setSeconds(v: string) {
  const n = Number(v)
  emit('update', { seconds: Number.isFinite(n) && n > 0 ? n : 1 })
}
</script>

<template>
  <FormField label="等待秒数" required hint="节点执行时会 sleep 这么多秒">
    <input
      type="number"
      class="form-input"
      min="0"
      step="1"
      :value="Number(props.config.seconds ?? 1)"
      @input="setSeconds(($event.target as HTMLInputElement).value)"
    />
  </FormField>
</template>
