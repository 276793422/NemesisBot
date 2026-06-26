<script setup lang="ts">
/**
 * Sub-workflow node — calls another workflow by name. `input` is a key/
 * value map passed to the child workflow's variables.
 *
 * Workflow names are free-text in v1 (no picker). A picker could be added
 * by querying the workflow list — left as a follow-up.
 */
import { computed } from 'vue'
import FormField from './FormField.vue'
import KeyValueList from './KeyValueList.vue'

const props = defineProps<{
  config: Record<string, unknown>
  variables?: import('./useVariablePicker').VariableOption[]
}>()

const emit = defineEmits<{
  (e: 'update', patch: Record<string, unknown>): void
}>()

const workflow = computed(() => typeof props.config.workflow === 'string' ? props.config.workflow : '')
const input = computed<Record<string, unknown>>(() =>
  props.config.input && typeof props.config.input === 'object'
    ? props.config.input as Record<string, unknown>
    : {},
)

function setWorkflow(v: string) { emit('update', { workflow: v }) }
function setInput(v: Record<string, unknown>) { emit('update', { input: v }) }
</script>

<template>
  <FormField label="子工作流名称" required hint="目标工作流的 name">
    <input
      type="text"
      class="form-input"
      :value="workflow"
      placeholder="sub_flow_1"
      spellcheck="false"
      @input="setWorkflow(($event.target as HTMLInputElement).value)"
    />
  </FormField>
  <FormField label="入参" hint="传给子工作流的变量，值可用 {{变量}}（输入 @ 召出变量选择器）">
    <KeyValueList
      :model-value="input"
      :variables="props.variables"
      key-placeholder="topic"
      value-placeholder="{{user_input}}"
      @update:model-value="setInput"
    />
  </FormField>
</template>
