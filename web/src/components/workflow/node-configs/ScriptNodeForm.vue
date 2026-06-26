<script setup lang="ts">
/**
 * Script node — runs an external script via the configured interpreter.
 *
 * Language dropdown picks the runner (bash/powershell/python/node/bat).
 * The script body is a multiline TextField with @-variable interpolation.
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

const LANGUAGES = [
  { value: 'bash', label: 'Bash (Linux/macOS)' },
  { value: 'powershell', label: 'PowerShell (Windows)' },
  { value: 'python', label: 'Python' },
  { value: 'node', label: 'Node.js' },
  { value: 'bat', label: 'BAT (Windows 批处理)' },
]

const script = computed(() => typeof props.config.script === 'string' ? props.config.script : '')
const language = computed(() => typeof props.config.language === 'string' ? props.config.language : 'bash')

function setLanguage(v: string) {
  emit('update', { language: v })
}
function setScript(v: string) {
  emit('update', { script: v })
}
</script>

<template>
  <FormField label="脚本语言" required>
    <select class="form-input" :value="language" @change="setLanguage(($event.target as HTMLSelectElement).value)">
      <option v-for="opt in LANGUAGES" :key="opt.value" :value="opt.value">{{ opt.label }}</option>
    </select>
  </FormField>
  <FormField label="脚本内容" required hint="支持 {{变量}} 插值（输入 @ 召出变量选择器），输出会作为 node result">
    <TextField
      :model-value="script"
      :variables="props.variables"
      :multiline="true"
      :rows="8"
      placeholder="#!/usr/bin/env bash&#10;echo hello"
      @update:model-value="setScript"
    />
  </FormField>
</template>
