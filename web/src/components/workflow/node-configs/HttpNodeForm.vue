<script setup lang="ts">
/** HTTP node — fires off an HTTP request, returns status/body/headers. */
import { computed } from 'vue'
import FormField from './FormField.vue'
import TextField from './TextField.vue'
import KeyValueList from './KeyValueList.vue'

const props = defineProps<{
  config: Record<string, unknown>
  variables?: import('./useVariablePicker').VariableOption[]
}>()

const emit = defineEmits<{
  (e: 'update', patch: Record<string, unknown>): void
}>()

const METHODS = ['GET', 'POST', 'PUT', 'DELETE', 'PATCH', 'HEAD', 'OPTIONS']

const method = computed(() =>
  typeof props.config.method === 'string' ? props.config.method : 'GET',
)
const url = computed(() =>
  typeof props.config.url === 'string' ? props.config.url : '',
)
const body = computed(() =>
  typeof props.config.body === 'string' ? props.config.body : '',
)
const headers = computed<Record<string, unknown>>(() =>
  props.config.headers && typeof props.config.headers === 'object'
    ? (props.config.headers as Record<string, unknown>)
    : {},
)
const timeoutSecs = computed(() =>
  typeof props.config.timeout_secs === 'number' ? props.config.timeout_secs : 30,
)

function setMethod(v: string) { emit('update', { method: v }) }
function setUrl(v: string) { emit('update', { url: v }) }
function setBody(v: string) { emit('update', { body: v }) }
function setHeaders(v: Record<string, unknown>) { emit('update', { headers: v }) }
function setTimeoutSecs(v: string) {
  const n = Number(v)
  if (Number.isFinite(n) && n >= 0) emit('update', { timeout_secs: Math.floor(n) })
}
</script>

<template>
  <div class="http-row">
    <FormField label="方法" required class="method-field">
      <select class="form-input" :value="method" @change="setMethod(($event.target as HTMLSelectElement).value)">
        <option v-for="m in METHODS" :key="m" :value="m">{{ m }}</option>
      </select>
    </FormField>
    <FormField label="URL" required hint="可用 {{变量}} 插值（输入 @ 召出变量选择器）" class="url-field">
      <TextField
        :model-value="url"
        :variables="props.variables"
        :multiline="false"
        placeholder="https://api.example.com/v1/users/{{id}}"
        @update:model-value="setUrl"
      />
    </FormField>
  </div>
  <FormField label="请求头" hint="键值对，值可用 {{变量}}（输入 @ 召出变量选择器）">
    <KeyValueList
      :model-value="headers"
      :variables="props.variables"
      key-placeholder="Content-Type"
      value-placeholder="application/json"
      @update:model-value="setHeaders"
    />
  </FormField>
  <FormField
    v-if="method !== 'GET' && method !== 'HEAD'"
    label="请求体"
    hint="字符串或 JSON 都行；可用 {{变量}}（输入 @ 召出变量选择器）"
  >
    <TextField
      :model-value="body"
      :variables="props.variables"
      :multiline="true"
      :rows="5"
      placeholder='{&quot;name&quot;: &quot;{{user_name}}&quot;}'
      @update:model-value="setBody"
    />
  </FormField>
  <FormField label="超时（秒）" hint="0 表示用默认 30 秒">
    <input
      type="number"
      class="form-input"
      min="0"
      step="1"
      :value="timeoutSecs"
      @input="setTimeoutSecs(($event.target as HTMLInputElement).value)"
    />
  </FormField>
</template>

<style scoped>
.http-row {
  display: flex;
  gap: var(--space-2);
}
.http-row .method-field {
  flex: 0 0 100px;
  margin-bottom: 0;
}
.http-row .url-field {
  flex: 1;
  margin-bottom: 0;
}
</style>
