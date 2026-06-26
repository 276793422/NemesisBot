<script setup lang="ts">
/**
 * Transform node — applies a named transform to its input. The backend
 * ships a small set of built-in transforms (identity, json_extract,
 * regex_match, etc). v1 lets the user pick from the known list and supply
 * the input source via @-variable.
 *
 * `output_type` (text/markdown/xml) tries to unwrap the {text: "..."}
 * envelope into a bare string so workflow_chat replies show clean text
 * instead of JSON. If the picked expression produces a different shape
 * (e.g. split_lines → {lines: [...]}), unwrap is skipped and the original
 * output flows through — observer JSON-dumps it as fallback. So any
 * combination is valid; this field is just a hint.
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

const EXPRESSIONS = [
  { value: 'identity', label: 'identity — 原样输出' },
  { value: 'json_extract', label: 'json_extract — 从 JSON 中取字段' },
  { value: 'regex_match', label: 'regex_match — 正则匹配' },
  { value: 'split_lines', label: 'split_lines — 按行切分' },
  { value: 'first_line', label: 'first_line — 取首行' },
  { value: 'last_line', label: 'last_line — 取末行' },
  { value: 'trim', label: 'trim — 去首尾空白' },
]

const OUTPUT_TYPES = [
  { value: '', label: 'JSON（默认）— 保留原结构' },
  { value: 'text', label: 'text — 纯文本回复' },
  { value: 'markdown', label: 'markdown — Markdown 格式回复' },
  { value: 'xml', label: 'xml — XML 内容回复' },
]

const expression = computed(() =>
  typeof props.config.expression === 'string' ? props.config.expression : 'identity',
)
const input = computed(() =>
  typeof props.config.input === 'string' ? props.config.input : '',
)
const arg = computed(() =>
  typeof props.config.arg === 'string' ? props.config.arg : '',
)
const outputType = computed(() =>
  typeof props.config.output_type === 'string' ? props.config.output_type : '',
)

function setExpression(v: string) { emit('update', { expression: v }) }
function setInput(v: string) { emit('update', { input: v }) }
function setArg(v: string) { emit('update', { arg: v }) }
function setOutputType(v: string) { emit('update', { output_type: v }) }
</script>

<template>
  <FormField label="变换类型" required>
    <select class="form-input" :value="expression" @change="setExpression(($event.target as HTMLSelectElement).value)">
      <option v-for="opt in EXPRESSIONS" :key="opt.value" :value="opt.value">{{ opt.label }}</option>
    </select>
  </FormField>
  <FormField label="输入" hint="要变换的内容，可用 {{变量}} 引用节点输出（输入 @ 召出变量选择器）">
    <TextField
      :model-value="input"
      :variables="props.variables"
      :multiline="true"
      :rows="3"
      placeholder="{{prev_node.output.text}}"
      @update:model-value="setInput"
    />
  </FormField>
  <FormField
    v-if="expression === 'json_extract' || expression === 'regex_match'"
    label="参数"
    :hint="expression === 'json_extract' ? 'JSON 字段路径，如 data.name' : '正则表达式'"
  >
    <TextField
      :model-value="arg"
      :variables="props.variables"
      :multiline="false"
      @update:model-value="setArg"
    />
  </FormField>
  <FormField
    label="输出类型"
    hint="设为 text/markdown/xml 时，工作流聊天回复直接显示纯文本（不再 JSON 包裹）。若变换产出非 {text: ...} 形状，自动回退到 JSON 输出"
  >
    <select class="form-input" :value="outputType" @change="setOutputType(($event.target as HTMLSelectElement).value)">
      <option v-for="opt in OUTPUT_TYPES" :key="opt.value" :value="opt.value">{{ opt.label }}</option>
    </select>
  </FormField>
</template>
