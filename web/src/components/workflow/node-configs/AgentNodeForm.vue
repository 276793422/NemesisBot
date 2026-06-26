<script setup lang="ts">
/**
 * Agent node — spawns a sub-agent with its own tool loop. The agent runs
 * `max_turns` rounds max, then returns its final message in `output.response`.
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
const agentId = computed(() => typeof props.config.agent_id === 'string' ? props.config.agent_id : '')
const maxTurns = computed(() => typeof props.config.max_turns === 'number' ? props.config.max_turns : 5)
const model = computed(() => typeof props.config.model === 'string' ? props.config.model : '')

function setPrompt(v: string) { emit('update', { prompt: v }) }
function setAgentId(v: string) { emit('update', { agent_id: v }) }
function setMaxTurns(v: string) {
  const n = Number(v)
  if (Number.isFinite(n) && n > 0) emit('update', { max_turns: Math.floor(n) })
}
function setModel(v: string) { emit('update', { model: v }) }
</script>

<template>
  <FormField label="Prompt" required hint="发给 Agent 的初始消息，可用 {{变量}} 插值（输入 @ 召出变量选择器）">
    <TextField
      :model-value="prompt"
      :variables="props.variables"
      :multiline="true"
      :rows="5"
      placeholder="请帮我搜索 {{topic}} 的最新进展"
      @update:model-value="setPrompt"
    />
  </FormField>
  <div class="row-pair">
    <FormField label="Agent ID" hint="（可选）指定 Agent 实例">
      <input
        type="text"
        class="form-input"
        :value="agentId"
        placeholder="（使用默认 Agent）"
        @input="setAgentId(($event.target as HTMLInputElement).value)"
      />
    </FormField>
    <FormField label="最大轮数" hint="工具调用循环上限">
      <input
        type="number"
        class="form-input"
        min="1"
        step="1"
        :value="maxTurns"
        @input="setMaxTurns(($event.target as HTMLInputElement).value)"
      />
    </FormField>
  </div>
  <FormField label="模型" hint="（可选）覆盖默认模型">
    <input
      type="text"
      class="form-input"
      :value="model"
      placeholder="（使用默认）"
      @input="setModel(($event.target as HTMLInputElement).value)"
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
