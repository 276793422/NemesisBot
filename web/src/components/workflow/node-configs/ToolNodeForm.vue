<script setup lang="ts">
/**
 * Tool node — calls a registered bot tool by name. `args` is a key/value
 * map; backend resolves {{var}} references in values before invocation.
 *
 * Tool names come from the host bot's tool registry — we can't enumerate
 * them all here, so the user types the name. (A picker could be added
 * later by calling the `tools.list` WSAPI cmd.)
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

const tool = computed(() => typeof props.config.tool === 'string' ? props.config.tool : '')
const args = computed<Record<string, unknown>>(() =>
  props.config.args && typeof props.config.args === 'object'
    ? props.config.args as Record<string, unknown>
    : {},
)

function setTool(v: string) { emit('update', { tool: v }) }
function setArgs(v: Record<string, unknown>) { emit('update', { args: v }) }
</script>

<template>
  <FormField label="工具名称" required hint="例：web_search、file_write、shell">
    <input
      type="text"
      class="form-input"
      :value="tool"
      placeholder="web_search"
      spellcheck="false"
      @input="setTool(($event.target as HTMLInputElement).value)"
    />
  </FormField>
  <FormField label="参数" hint="键值对，值可用 {{变量}}（输入 @ 召出变量选择器）">
    <KeyValueList
      :model-value="args"
      :variables="props.variables"
      key-placeholder="query"
      value-placeholder="{{user_input}}"
      @update:model-value="setArgs"
    />
  </FormField>
</template>
