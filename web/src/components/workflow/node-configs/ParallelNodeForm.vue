<script setup lang="ts">
/**
 * Parallel node — runs its children concurrently. All children start at
 * the same time; the node finishes when all of them finish.
 */
import { computed } from 'vue'
import FormField from './FormField.vue'
import NodeChildrenEditor from './NodeChildrenEditor.vue'
import type { NodeDef } from '../../../types/workflow'
import type { VariableOption } from './useVariablePicker'

const props = defineProps<{
  config: Record<string, unknown>
  variables?: VariableOption[]
}>()

const emit = defineEmits<{
  (e: 'update', patch: Record<string, unknown>): void
}>()

const childNodes = computed<NodeDef[]>(() => {
  const v = props.config.nodes
  if (!Array.isArray(v)) return []
  return v.filter((x): x is NodeDef => x != null && typeof x === 'object' && 'id' in x)
})

function setNodes(nodes: NodeDef[]) {
  emit('update', { nodes })
}
</script>

<template>
  <FormField
    label="并行分支（子节点）"
    required
    hint="所有子节点会同时开始；全部完成后本节点才完成"
  >
    <NodeChildrenEditor
      :nodes="childNodes"
      :variables="props.variables"
      @update="setNodes"
    />
  </FormField>
</template>
