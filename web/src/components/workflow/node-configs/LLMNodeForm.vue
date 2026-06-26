<script setup lang="ts">
/**
 * LLM node — single-shot LLM call (no tool loop). Sends the prompt (+
 * optional system_prompt) to the configured model, returns the text.
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

const prompt = computed(() => typeof props.config.prompt === 'string' ? props.config.prompt : '')
const systemPrompt = computed(() => typeof props.config.system_prompt === 'string' ? props.config.system_prompt : '')
const model = computed(() => typeof props.config.model === 'string' ? props.config.model : '')
const temperature = computed(() => typeof props.config.temperature === 'number' ? props.config.temperature : 0.7)
const maxTokens = computed(() => typeof props.config.max_tokens === 'number' ? props.config.max_tokens : 1024)

function setPrompt(v: string) { emit('update', { prompt: v }) }
function setSystem(v: string) { emit('update', { system_prompt: v }) }
function setModel(v: string) { emit('update', { model: v }) }
function setTemperature(v: string) {
  const n = Number(v)
  if (Number.isFinite(n)) emit('update', { temperature: n })
}
function setMaxTokens(v: string) {
  const n = Number(v)
  if (Number.isFinite(n) && n > 0) emit('update', { max_tokens: Math.floor(n) })
}
</script>

<template>
  <FormField label="Prompt" required hint="发给模型的文本，可用 {{变量}} 插值（输入 @ 召出变量选择器）">
    <TextField
      :model-value="prompt"
      :variables="props.variables"
      :multiline="true"
      :rows="5"
      placeholder="请把这段话翻译成英文：{{input}}"
      @update:model-value="setPrompt"
    />
  </FormField>
  <FormField label="System Prompt" hint="（可选）覆盖默认系统提示">
    <TextField
      :model-value="systemPrompt"
      :variables="props.variables"
      :multiline="true"
      :rows="3"
      placeholder="你是一名资深翻译"
      @update:model-value="setSystem"
    />
  </FormField>
  <div class="row-pair">
    <FormField label="模型" hint="（可选）覆盖默认模型，如 zhipu/glm-4.7">
      <input
        type="text"
        class="form-input"
        :value="model"
        placeholder="（使用默认）"
        @input="setModel(($event.target as HTMLInputElement).value)"
      />
    </FormField>
    <FormField label="Temperature" hint="0~2">
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
  </div>
  <FormField label="Max Tokens" hint="输出长度上限">
    <input
      type="number"
      class="form-input"
      min="1"
      step="1"
      :value="maxTokens"
      @input="setMaxTokens(($event.target as HTMLInputElement).value)"
    />
  </FormField>
</template>

<style scoped>
.row-pair {
  display: flex;
  gap: var(--space-2);
}
.row-pair :deep(.form-field) {
  flex: 1;
  margin-bottom: 0;
}
</style>
