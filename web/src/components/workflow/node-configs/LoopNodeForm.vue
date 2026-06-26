<script setup lang="ts">
/**
 * Loop node — repeats its child nodes until either max_iterations is hit
 * or `condition` evaluates false. Children are full NodeDefs, edited via
 * the recursive NodeChildrenEditor.
 *
 * Two modes (mutually exclusive):
 *   1. Counted loop: just `max_iterations` (default)
 *   2. Conditional loop: empty max_iterations + `condition` expression
 */
import { computed } from 'vue'
import FormField from './FormField.vue'
import TextField from './TextField.vue'
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

const mode = computed<'count' | 'cond'>(() => {
  const cond = props.config.condition
  if (typeof cond === 'string' && cond.trim() !== '') return 'cond'
  return 'count'
})

const maxIterations = computed(() => typeof props.config.max_iterations === 'number' ? props.config.max_iterations : 3)
const condition = computed(() => typeof props.config.condition === 'string' ? props.config.condition : '')

const childNodes = computed<NodeDef[]>(() => {
  const v = props.config.nodes
  if (!Array.isArray(v)) return []
  return v.filter((x): x is NodeDef => x != null && typeof x === 'object' && 'id' in x)
})

function setMode(m: 'count' | 'cond') {
  if (m === 'count') {
    emit('update', { condition: '', max_iterations: maxIterations.value })
  } else {
    emit('update', { condition: condition.value || '{{i}} < 10', max_iterations: undefined })
  }
}

function setMaxIterations(v: string) {
  const n = Number(v)
  if (Number.isFinite(n) && n > 0) emit('update', { max_iterations: Math.floor(n) })
}
function setCondition(v: string) {
  emit('update', { condition: v })
}
function setNodes(nodes: NodeDef[]) {
  emit('update', { nodes })
}
</script>

<template>
  <FormField label="循环模式" required>
    <div class="mode-row">
      <label class="mode-opt">
        <input
          type="radio"
          :checked="mode === 'count'"
          @change="setMode('count')"
        />
        <span>按次数</span>
      </label>
      <label class="mode-opt">
        <input
          type="radio"
          :checked="mode === 'cond'"
          @change="setMode('cond')"
        />
        <span>按条件</span>
      </label>
    </div>
  </FormField>

  <FormField v-if="mode === 'count'" label="最大迭代数" required hint="子节点最多跑多少轮">
    <input
      type="number"
      class="form-input"
      min="1"
      step="1"
      :value="maxIterations"
      @input="setMaxIterations(($event.target as HTMLInputElement).value)"
    />
  </FormField>

  <FormField
    v-else
    label="继续条件"
    required
    hint="返回 true 就继续；可用 {{i}} 表示当前轮次，{{var}} 引用其他变量"
  >
    <TextField
      :model-value="condition"
      :variables="props.variables"
      :multiline="false"
      placeholder="{{i}} < 10 && {{items.length}} > 0"
      @update:model-value="setCondition"
    />
  </FormField>

  <FormField label="循环体（子节点）" required hint="每轮会依次执行这些子节点">
    <NodeChildrenEditor
      :nodes="childNodes"
      :variables="props.variables"
      :allow-terminal="true"
      @update="setNodes"
    />
  </FormField>
</template>

<style scoped>
.mode-row {
  display: flex;
  gap: var(--space-2);
}
.mode-opt {
  display: flex;
  align-items: center;
  gap: 4px;
  font-size: var(--text-sm);
  color: var(--text-primary);
  cursor: pointer;
}
</style>
