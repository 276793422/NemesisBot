<script setup lang="ts">
/**
 * Tool node — calls a registered bot tool by name.
 *
 * The tool name comes from the host bot's tool registry via the `tools.list`
 * WSAPI command (so the picker is always in sync with what the workflow can
 * actually execute). Once a tool is picked, its parameter JSON Schema drives a
 * dynamic form — the user sees exactly which args to fill, with type / required
 * hints — instead of a blank key/value grid.
 *
 * Config contract (backend RealToolNodeExecutor):
 *   `name` (preferred) or `tool` (legacy): tool name
 *   `args`: object of tool arguments; `{{var}}` placeholders resolved at runtime
 */
import { computed, onMounted, ref } from 'vue'
import FormField from './FormField.vue'
import TextField from './TextField.vue'
import KeyValueList from './KeyValueList.vue'
import { useToolsApi, type ToolSchema } from '../../../composables/useToolsApi'

interface JsonSchemaProp {
  type?: string
  description?: string
  enum?: unknown[]
  default?: unknown
}
interface JsonSchema {
  type?: string
  properties?: Record<string, JsonSchemaProp>
  required?: string[]
}

const props = defineProps<{
  config: Record<string, unknown>
  variables?: import('./useVariablePicker').VariableOption[]
}>()

const emit = defineEmits<{
  (e: 'update', patch: Record<string, unknown>): void
}>()

const { list } = useToolsApi()
const tools = ref<ToolSchema[]>([])
const loadError = ref('')

onMounted(async () => {
  try {
    tools.value = await list()
  } catch (e: unknown) {
    loadError.value = String(e)
  }
})

// Read tool name from `name` (preferred) or legacy `tool`.
const toolName = computed(() => {
  const n = props.config.name
  if (typeof n === 'string' && n) return n
  const t = props.config.tool
  return typeof t === 'string' ? t : ''
})

const selected = computed<ToolSchema | undefined>(() =>
  tools.value.find((t) => t.name === toolName.value),
)

const schema = computed<JsonSchema>(() => {
  const p = selected.value?.parameters
  return p && typeof p === 'object' ? (p as JsonSchema) : {}
})

const properties = computed<{ key: string; prop: JsonSchemaProp }[]>(() => {
  const obj = schema.value.properties
  if (!obj || typeof obj !== 'object') return []
  return Object.entries(obj).map(([key, prop]) => ({ key, prop }))
})

const requiredSet = computed(() => new Set(schema.value.required ?? []))

const args = computed<Record<string, unknown>>(() =>
  props.config.args && typeof props.config.args === 'object'
    ? (props.config.args as Record<string, unknown>)
    : {},
)

function setTool(v: string) {
  // Write `name` (preferred field). Reset args when switching tools so stale
  // keys from a previous tool don't leak into the new one.
  emit('update', { name: v, args: {} })
}

function setArg(key: string, value: unknown) {
  const next = { ...args.value }
  if (value === '' || value === null || value === undefined) {
    delete next[key]
  } else {
    next[key] = value
  }
  emit('update', { args: next })
}

function fieldAsString(key: string): string {
  const v = args.value[key]
  return typeof v === 'string' ? v : v == null ? '' : String(v)
}
</script>

<template>
  <FormField label="工具" required hint="从宿主工具注册表选择（需 agent 已启动）">
    <select
      v-if="tools.length > 0"
      class="form-input"
      :value="toolName"
      @change="setTool(($event.target as HTMLSelectElement).value)"
    >
      <option value="" disabled>— 选择工具 —</option>
      <option v-for="t in tools" :key="t.name" :value="t.name">{{ t.name }}</option>
    </select>
    <!-- Fallback free-text input when the list isn't available yet (agent not
         running / WS not ready) so the form is never blocking. -->
    <input
      v-else
      type="text"
      class="form-input"
      :value="toolName"
      placeholder="web_search / file_write / …"
      spellcheck="false"
      @input="setTool(($event.target as HTMLInputElement).value)"
    />
  </FormField>

  <div v-if="selected?.description" class="tool-desc">{{ selected.description }}</div>
  <div v-else-if="loadError" class="tool-desc tool-desc-error">
    工具列表加载失败（可手填工具名）：{{ loadError }}
  </div>

  <!-- Schema-driven parameter form for the selected tool. -->
  <template v-if="properties.length > 0">
    <FormField
      v-for="p in properties"
      :key="p.key"
      :label="p.key"
      :hint="p.prop.description"
      :required="requiredSet.has(p.key)"
    >
      <!-- string with enum → dropdown -->
      <select
        v-if="p.prop.type === 'string' && Array.isArray(p.prop.enum)"
        class="form-input"
        :value="fieldAsString(p.key)"
        @change="setArg(p.key, ($event.target as HTMLSelectElement).value)"
      >
        <option value="">—</option>
        <option v-for="(opt, i) in p.prop.enum" :key="i" :value="String(opt)">{{ opt }}</option>
      </select>
      <!-- number / integer → numeric input -->
      <input
        v-else-if="p.prop.type === 'number' || p.prop.type === 'integer'"
        type="number"
        class="form-input"
        :value="args[p.key] ?? ''"
        :step="p.prop.type === 'integer' ? 1 : 'any'"
        @input="
          setArg(
            p.key,
            ($event.target as HTMLInputElement).value === ''
              ? ''
              : Number(($event.target as HTMLInputElement).value),
          )
        "
      />
      <!-- boolean → checkbox -->
      <label v-else-if="p.prop.type === 'boolean'" class="bool-row">
        <input
          type="checkbox"
          :checked="args[p.key] === true"
          @change="setArg(p.key, ($event.target as HTMLInputElement).checked)"
        />
        <span>{{ p.prop.description || '启用' }}</span>
      </label>
      <!-- array / object → multiline text (JSON or {{var}}) -->
      <TextField
        v-else-if="p.prop.type === 'array' || p.prop.type === 'object'"
        :model-value="fieldAsString(p.key)"
        :variables="props.variables"
        :multiline="true"
        :rows="4"
        placeholder='[] 或 {}（JSON 文本，或 {{变量}}）'
        @update:model-value="setArg(p.key, $event)"
      />
      <!-- string (default) → TextField with @ variable picker -->
      <TextField
        v-else
        :model-value="fieldAsString(p.key)"
        :variables="props.variables"
        :multiline="false"
        :placeholder="p.prop.description || p.key"
        @update:model-value="setArg(p.key, $event)"
      />
    </FormField>
  </template>

  <!-- No schema available (tool not in list / list empty) but a name is set:
       fall back to the generic key/value editor so the node stays usable. -->
  <FormField
    v-else-if="toolName"
    label="参数（手动键值对）"
    hint="未获取到该工具的参数 schema；按 key/value 手填。值可用 {{变量}}"
  >
    <KeyValueList
      :model-value="args"
      :variables="props.variables"
      key-placeholder="path"
      value-placeholder="{{var}} 或值"
      @update:model-value="(v: Record<string, unknown>) => emit('update', { args: v })"
    />
  </FormField>
</template>

<style scoped>
.tool-desc {
  font-size: var(--text-xs);
  color: var(--text-muted);
  margin: -4px 0 8px;
  line-height: 1.4;
}
.tool-desc-error {
  color: var(--danger, #e74c3c);
}
.bool-row {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: var(--text-sm);
  color: var(--text-secondary);
}
</style>
