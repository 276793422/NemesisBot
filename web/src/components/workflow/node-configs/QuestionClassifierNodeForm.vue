<script setup lang="ts">
/**
 * Question classifier node — LLM routes the question into one of N
 * predefined classes (each class has an id + description).
 *
 * The classes list is the heart of this node. Each row is editable
 * inline; the backend JSON-dumps them as `[{id, description}, ...]`.
 */
import { computed } from 'vue'
import FormField from './FormField.vue'
import TextField from './TextField.vue'

interface ClassDef { id: string; description: string }

const props = defineProps<{
  config: Record<string, unknown>
  variables?: import('./useVariablePicker').VariableOption[]
}>()

const emit = defineEmits<{
  (e: 'update', patch: Record<string, unknown>): void
}>()

const question = computed(() => typeof props.config.question === 'string' ? props.config.question : '')
const systemPrompt = computed(() => typeof props.config.system_prompt === 'string' ? props.config.system_prompt : '')
const model = computed(() => typeof props.config.model === 'string' ? props.config.model : '')
const maxAttempts = computed(() => typeof props.config.max_attempts === 'number' ? props.config.max_attempts : 3)
const temperature = computed(() => typeof props.config.temperature === 'number' ? props.config.temperature : 0)

const classes = computed<ClassDef[]>(() => {
  const v = props.config.classes
  if (!Array.isArray(v)) return []
  return v.map((c: unknown) => {
    if (c && typeof c === 'object') {
      const obj = c as Record<string, unknown>
      return {
        id: typeof obj.id === 'string' ? obj.id : '',
        description: typeof obj.description === 'string' ? obj.description : '',
      }
    }
    return { id: '', description: '' }
  })
})

function setQuestion(v: string) { emit('update', { question: v }) }
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

function updateClass(idx: number, patch: Partial<ClassDef>) {
  const next = classes.value.map((c, i) => (i === idx ? { ...c, ...patch } : c))
  emit('update', { classes: next })
}
function addClass() {
  const next = [...classes.value, { id: `class_${classes.value.length + 1}`, description: '' }]
  emit('update', { classes: next })
}
function deleteClass(idx: number) {
  const next = classes.value.filter((_, i) => i !== idx)
  emit('update', { classes: next })
}
</script>

<template>
  <FormField label="问题" required hint="要分类的问题，可用 {{变量}}（输入 @ 召出变量选择器）">
    <TextField
      :model-value="question"
      :variables="props.variables"
      :multiline="true"
      :rows="3"
      placeholder="{{user_input}}"
      @update:model-value="setQuestion"
    />
  </FormField>
  <FormField label="分类" required hint="LLM 会把问题分到下面这些类别之一">
    <div class="class-list">
      <div v-for="(c, idx) in classes" :key="idx" class="class-row">
        <input
          type="text"
          class="class-id"
          :value="c.id"
          placeholder="class_id"
          spellcheck="false"
          @input="updateClass(idx, { id: ($event.target as HTMLInputElement).value })"
        />
        <input
          type="text"
          class="class-desc"
          :value="c.description"
          placeholder="类别描述（什么情况下归到这一类）"
          @input="updateClass(idx, { description: ($event.target as HTMLInputElement).value })"
        />
        <button class="class-del" title="删除" @click="deleteClass(idx)">×</button>
      </div>
      <button class="class-add" @click="addClass">+ 添加分类</button>
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
    <FormField label="最大尝试次数" hint="解析失败后重试上限">
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
  <FormField label="Temperature" hint="分类通常设为 0">
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
  <FormField label="System Prompt" hint="（可选）覆盖默认提示">
    <TextField
      :model-value="systemPrompt"
      :variables="props.variables"
      :multiline="true"
      :rows="3"
      placeholder="（使用内置模板，会自动注入类别列表）"
      @update:model-value="setSystem"
    />
  </FormField>
</template>

<style scoped>
.class-list {
  display: flex;
  flex-direction: column;
  gap: var(--space-1);
}
.class-row {
  display: flex;
  gap: var(--space-1);
}
.class-id {
  flex: 0 0 110px;
  padding: 4px 6px;
  background: var(--bg-primary);
  border: 1px solid var(--border);
  border-radius: var(--radius-sm);
  color: var(--text-primary);
  font-size: var(--text-xs);
  font-family: 'Consolas', monospace;
}
.class-id:focus { outline: none; border-color: var(--accent); }
.class-desc {
  flex: 1;
  padding: 4px 6px;
  background: var(--bg-primary);
  border: 1px solid var(--border);
  border-radius: var(--radius-sm);
  color: var(--text-primary);
  font-size: var(--text-xs);
  min-width: 0;
}
.class-desc:focus { outline: none; border-color: var(--accent); }
.class-del {
  background: transparent;
  border: 1px solid transparent;
  color: var(--text-muted);
  cursor: pointer;
  padding: 0 8px;
  font-size: var(--text-base);
  border-radius: var(--radius-sm);
}
.class-del:hover { color: var(--danger, #e74c3c); }
.class-add {
  align-self: flex-start;
  background: transparent;
  border: 1px dashed var(--border);
  color: var(--text-secondary);
  padding: 4px var(--space-2);
  border-radius: var(--radius-sm);
  cursor: pointer;
  font-size: var(--text-xs);
}
.class-add:hover { border-color: var(--accent); color: var(--accent); }
.row-pair { display: flex; gap: var(--space-2); }
.row-pair :deep(.form-field) { flex: 1; margin-bottom: 0; }
</style>
