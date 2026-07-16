<script setup lang="ts">
/**
 * Loop node — repeats its child nodes. Three modes (mutually exclusive):
 *   1. count  — fixed number of iterations (`max_iterations`)
 *   2. cond   — runs while `condition` is true (`{{i}}` = iteration index)
 *   3. foreach— iterates a list (`items`); binds the current element to
 *               `{{item}}` / `{{item_index}}` (name configurable via `item_var`)
 *
 * Modes 1/2 write `mode: "counter"` (the backend default when `mode` is
 * absent, so legacy loops keep working); mode 3 writes `mode: "foreach"`.
 *
 * Backend: `crates/nemesis-workflow/src/nodes.rs` LoopNodeExecutor.
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

type LoopMode = 'count' | 'cond' | 'foreach'

const mode = computed<LoopMode>(() => {
  if (props.config.mode === 'foreach') return 'foreach'
  const cond = props.config.condition
  if (typeof cond === 'string' && cond.trim() !== '') return 'cond'
  return 'count'
})

const maxIterations = computed(() => {
  if (typeof props.config.max_iterations === 'number') return props.config.max_iterations
  return mode.value === 'foreach' ? 100 : 3
})
const condition = computed(() => typeof props.config.condition === 'string' ? props.config.condition : '')
const items = computed(() => typeof props.config.items === 'string' ? props.config.items : '')
const itemVar = computed(() => {
  const v = props.config.item_var
  return typeof v === 'string' && v ? v : 'item'
})

const childNodes = computed<NodeDef[]>(() => {
  const v = props.config.nodes
  if (!Array.isArray(v)) return []
  return v.filter((x): x is NodeDef => x != null && typeof x === 'object' && 'id' in x)
})

function setMode(m: LoopMode) {
  if (m === 'count') {
    emit('update', {
      mode: 'counter',
      condition: '',
      items: undefined,
      item_var: undefined,
      max_iterations: maxIterations.value,
    })
  } else if (m === 'cond') {
    emit('update', {
      mode: 'counter',
      condition: condition.value || '{{i}} < 10',
      items: undefined,
      item_var: undefined,
      max_iterations: undefined,
    })
  } else {
    emit('update', {
      mode: 'foreach',
      condition: '',
      items: items.value,
      item_var: itemVar.value,
      max_iterations: maxIterations.value,
    })
  }
}

function setMaxIterations(v: string) {
  const n = Number(v)
  if (Number.isFinite(n) && n > 0) emit('update', { max_iterations: Math.floor(n) })
}
function setCondition(v: string) {
  emit('update', { condition: v })
}
function setItems(v: string) {
  emit('update', { items: v })
}
function setItemVar(v: string) {
  emit('update', { item_var: v || 'item' })
}
function setNodes(nodes: NodeDef[]) {
  emit('update', { nodes })
}
</script>

<template>
  <FormField label="循环模式" required>
    <div class="mode-row">
      <label class="mode-opt">
        <input type="radio" :checked="mode === 'count'" @change="setMode('count')" />
        <span>按次数</span>
      </label>
      <label class="mode-opt">
        <input type="radio" :checked="mode === 'cond'" @change="setMode('cond')" />
        <span>按条件</span>
      </label>
      <label class="mode-opt">
        <input type="radio" :checked="mode === 'foreach'" @change="setMode('foreach')" />
        <span>遍历列表</span>
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
    v-else-if="mode === 'cond'"
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

  <template v-else>
    <FormField
      label="遍历的列表"
      required
      hint="用 {{node.output}} 引用上游数组，或直接写 JSON 数组 / 每行一个元素"
    >
      <TextField
        :model-value="items"
        :variables="props.variables"
        :multiline="true"
        :rows="3"
        placeholder="{{split_urls.output}}"
        @update:model-value="setItems"
      />
    </FormField>
    <FormField label="元素变量名" hint="子节点用 {{此名}} 引用当前元素（默认 item，另有 {{item_index}}）">
      <input
        type="text"
        class="form-input"
        :value="itemVar"
        placeholder="item"
        spellcheck="false"
        @input="setItemVar(($event.target as HTMLInputElement).value)"
      />
    </FormField>
    <FormField label="截断上限" hint="最多遍历多少个元素（安全上限，默认 100）">
      <input
        type="number"
        class="form-input"
        min="1"
        step="1"
        :value="maxIterations"
        @input="setMaxIterations(($event.target as HTMLInputElement).value)"
      />
    </FormField>
  </template>

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
  flex-wrap: wrap;
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
