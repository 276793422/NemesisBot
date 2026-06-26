<script setup lang="ts">
/**
 * Parameter extractor node — LLM pulls structured params out of free text.
 * The schema is a list of {name, description, type, required} entries;
 * backend emits them as JSON for the model's system prompt.
 */
import { computed } from 'vue'
import FormField from './FormField.vue'
import TextField from './TextField.vue'

interface ParamDef {
  name: string
  description: string
  type: string
  required: boolean
}

const TYPES = ['string', 'number', 'boolean', 'array', 'object']

const props = defineProps<{
  config: Record<string, unknown>
  variables?: import('./useVariablePicker').VariableOption[]
}>()

const emit = defineEmits<{
  (e: 'update', patch: Record<string, unknown>): void
}>()

const text = computed(() => typeof props.config.text === 'string' ? props.config.text : '')
const systemPrompt = computed(() => typeof props.config.system_prompt === 'string' ? props.config.system_prompt : '')
const model = computed(() => typeof props.config.model === 'string' ? props.config.model : '')
const maxAttempts = computed(() => typeof props.config.max_attempts === 'number' ? props.config.max_attempts : 3)
const temperature = computed(() => typeof props.config.temperature === 'number' ? props.config.temperature : 0)

const parameters = computed<ParamDef[]>(() => {
  const v = props.config.parameters
  if (!Array.isArray(v)) return []
  return v.map((p: unknown) => {
    if (p && typeof p === 'object') {
      const obj = p as Record<string, unknown>
      return {
        name: typeof obj.name === 'string' ? obj.name : '',
        description: typeof obj.description === 'string' ? obj.description : '',
        type: typeof obj.type === 'string' ? obj.type : 'string',
        required: obj.required === true,
      }
    }
    return { name: '', description: '', type: 'string', required: false }
  })
})

function setText(v: string) { emit('update', { text: v }) }
function setSystem(v: string) { emit('update', { system_prompt: v }) }
function setModel(v: string) { emit('update', { model: v }) }
function setMaxAttempts(v: string) {
  const n = Number(v)
  if (Number.isFinite(n) && n > 0) emit('update', { max_attempts: Math.floor(n) })
}
function setTemperature(v: string) {
  const n = Number(v)
  if (Number.isFinite(n)) emit('update', { temperature: n })
}

function updateParam(idx: number, patch: Partial<ParamDef>) {
  const next = parameters.value.map((p, i) => (i === idx ? { ...p, ...patch } : p))
  emit('update', { parameters: next })
}
function addParam() {
  const next = [...parameters.value, {
    name: `param_${parameters.value.length + 1}`,
    description: '',
    type: 'string',
    required: false,
  }]
  emit('update', { parameters: next })
}
function deleteParam(idx: number) {
  const next = parameters.value.filter((_, i) => i !== idx)
  emit('update', { parameters: next })
}
</script>

<template>
  <FormField label="待抽取文本" required hint="可用 {{变量}}（输入 @ 召出变量选择器）">
    <TextField
      :model-value="text"
      :variables="props.variables"
      :multiline="true"
      :rows="4"
      placeholder="{{user_input}}"
      @update:model-value="setText"
    />
  </FormField>
  <FormField label="参数" required hint="LLM 会按这个 schema 抽取字段">
    <div class="param-list">
      <div v-for="(p, idx) in parameters" :key="idx" class="param-row">
        <input
          type="text"
          class="param-name"
          :value="p.name"
          placeholder="字段名"
          spellcheck="false"
          @input="updateParam(idx, { name: ($event.target as HTMLInputElement).value })"
        />
        <select
          class="param-type"
          :value="p.type"
          @change="updateParam(idx, { type: ($event.target as HTMLSelectElement).value })"
        >
          <option v-for="t in TYPES" :key="t" :value="t">{{ t }}</option>
        </select>
        <input
          type="text"
          class="param-desc"
          :value="p.description"
          placeholder="字段含义，给 LLM 看的提示"
          @input="updateParam(idx, { description: ($event.target as HTMLInputElement).value })"
        />
        <label class="param-req" title="是否必填">
          <input
            type="checkbox"
            :checked="p.required"
            @change="updateParam(idx, { required: ($event.target as HTMLInputElement).checked })"
          />
          <span>必填</span>
        </label>
        <button class="param-del" title="删除" @click="deleteParam(idx)">×</button>
      </div>
      <button class="param-add" @click="addParam">+ 添加字段</button>
    </div>
  </FormField>
  <div class="row-pair">
    <FormField label="模型" hint="（可选）">
      <input
        type="text"
        class="form-input"
        :value="model"
        placeholder="（默认）"
        @input="setModel(($event.target as HTMLInputElement).value)"
      />
    </FormField>
    <FormField label="最大尝试次数" hint="解析失败重试上限">
      <input
        type="number"
        class="form-input"
        min="1"
        step="1"
        :value="maxAttempts"
        @input="setMaxAttempts(($event.target as HTMLInputElement).value)"
      />
    </FormField>
  </div>
  <FormField label="Temperature" hint="抽取通常设为 0">
    <input
      type="number"
      class="form-input"
      min="0"
      max="2"
      step="0.1"
      :value="temperature"
      @input="setTemperature(($event.target as HTMLInputElement).value)"
    />
  </FormField>
  <FormField label="System Prompt" hint="（可选）覆盖默认">
    <TextField
      :model-value="systemPrompt"
      :variables="props.variables"
      :multiline="true"
      :rows="3"
      placeholder="（使用内置模板，会自动注入参数列表）"
      @update:model-value="setSystem"
    />
  </FormField>
</template>

<style scoped>
.param-list {
  display: flex;
  flex-direction: column;
  gap: var(--space-1);
}
.param-row {
  display: flex;
  gap: var(--space-1);
  align-items: center;
}
.param-name {
  flex: 0 0 110px;
  padding: 4px 6px;
  background: var(--bg-primary);
  border: 1px solid var(--border);
  border-radius: var(--radius-sm);
  color: var(--text-primary);
  font-size: var(--text-xs);
  font-family: 'Consolas', monospace;
}
.param-name:focus { outline: none; border-color: var(--accent); }
.param-type {
  flex: 0 0 80px;
  padding: 4px 6px;
  background: var(--bg-primary);
  border: 1px solid var(--border);
  border-radius: var(--radius-sm);
  color: var(--text-primary);
  font-size: var(--text-xs);
}
.param-type:focus { outline: none; border-color: var(--accent); }
.param-desc {
  flex: 1;
  padding: 4px 6px;
  background: var(--bg-primary);
  border: 1px solid var(--border);
  border-radius: var(--radius-sm);
  color: var(--text-primary);
  font-size: var(--text-xs);
  min-width: 0;
}
.param-desc:focus { outline: none; border-color: var(--accent); }
.param-req {
  display: flex;
  align-items: center;
  gap: 4px;
  font-size: var(--text-xs);
  color: var(--text-secondary);
  cursor: pointer;
  flex: 0 0 auto;
}
.param-del {
  background: transparent;
  border: 1px solid transparent;
  color: var(--text-muted);
  cursor: pointer;
  padding: 0 8px;
  font-size: var(--text-base);
  border-radius: var(--radius-sm);
}
.param-del:hover { color: var(--danger, #e74c3c); }
.param-add {
  align-self: flex-start;
  background: transparent;
  border: 1px dashed var(--border);
  color: var(--text-secondary);
  padding: 4px var(--space-2);
  border-radius: var(--radius-sm);
  cursor: pointer;
  font-size: var(--text-xs);
}
.param-add:hover { border-color: var(--accent); color: var(--accent); }
.row-pair { display: flex; gap: var(--space-2); }
.row-pair :deep(.form-field) { flex: 1; margin-bottom: 0; }
</style>
