<script setup lang="ts">
/**
 * Dispatcher for node config forms.
 *
 * Routes to a per-type form component in `./node-configs/`. Each form:
 *   - declares `defineProps<{ config, variables }>()`
 *   - emits `update` with a patch of `Record<string, unknown>` (config keys only)
 *
 * For unknown types, falls back to a raw JSON textarea (power-user escape hatch).
 *
 * `embedded: true` suppresses the outer card/header/footer — used when this
 * component is rendered inline by NodeChildrenEditor (recursion).
 */
import { ref, watch, computed } from 'vue'
import type { NodeDef } from '../../types/workflow'
import { NODE_CATALOG } from '../../types/workflow'
import type { VariableOption } from './node-configs/useVariablePicker'
import LiveJsonPreview from './node-configs/LiveJsonPreview.vue'

import DelayNodeForm from './node-configs/DelayNodeForm.vue'
import ScriptNodeForm from './node-configs/ScriptNodeForm.vue'
import ConditionNodeForm from './node-configs/ConditionNodeForm.vue'
import LLMNodeForm from './node-configs/LLMNodeForm.vue'
import TransformNodeForm from './node-configs/TransformNodeForm.vue'
import HumanReviewNodeForm from './node-configs/HumanReviewNodeForm.vue'
import HttpNodeForm from './node-configs/HttpNodeForm.vue'
import ToolNodeForm from './node-configs/ToolNodeForm.vue'
import AgentNodeForm from './node-configs/AgentNodeForm.vue'
import SubWorkflowNodeForm from './node-configs/SubWorkflowNodeForm.vue'
import QuestionClassifierNodeForm from './node-configs/QuestionClassifierNodeForm.vue'
import ParameterExtractorNodeForm from './node-configs/ParameterExtractorNodeForm.vue'
import LoopNodeForm from './node-configs/LoopNodeForm.vue'
import ParallelNodeForm from './node-configs/ParallelNodeForm.vue'

const props = defineProps<{
  node: NodeDef
  /** Suppress outer chrome when nested inside NodeChildrenEditor. */
  embedded?: boolean
  /** Available @-variables for the picker (workflow vars + sibling node outputs). */
  variables?: VariableOption[]
}>()

const emit = defineEmits<{
  (e: 'update', patch: Partial<NodeDef>): void
  (e: 'close'): void
}>()

const catalogEntry = computed(() =>
  NODE_CATALOG.find(e => e.type === props.node.node_type),
)

// Map node_type → form component. Unknown types resolve to null (raw JSON path).
const FORM_MAP: Record<string, unknown> = {
  delay: DelayNodeForm,
  script: ScriptNodeForm,
  condition: ConditionNodeForm,
  llm: LLMNodeForm,
  transform: TransformNodeForm,
  human_review: HumanReviewNodeForm,
  http: HttpNodeForm,
  tool: ToolNodeForm,
  agent: AgentNodeForm,
  sub_workflow: SubWorkflowNodeForm,
  question_classifier: QuestionClassifierNodeForm,
  parameter_extractor: ParameterExtractorNodeForm,
  loop: LoopNodeForm,
  parallel: ParallelNodeForm,
}

const formComponent = computed<unknown>(() => FORM_MAP[props.node.node_type] ?? null)
const hasForm = computed(() => formComponent.value !== null)

// Basic-fields mirror. We track these locally and emit on change.
const local = ref<NodeDef>({ ...props.node })

// Raw JSON fallback state.
const configJson = ref(JSON.stringify(props.node.config ?? {}, null, 2))
const configError = ref<string | null>(null)

watch(() => props.node, (n) => {
  local.value = { ...n }
  configJson.value = JSON.stringify(n.config ?? {}, null, 2)
  configError.value = null
}, { deep: true })

// Per-form update: patch is config-only Record<string, unknown>.
function applyFormPatch(patch: Record<string, unknown>) {
  const nextConfig = { ...(props.node.config ?? {}), ...patch }
  // Drop keys explicitly set to undefined so they don't clutter the JSON.
  for (const [k, v] of Object.entries(nextConfig)) {
    if (v === undefined) delete nextConfig[k]
  }
  emit('update', { config: nextConfig })
}

// Basic-fields handlers (id, label, depends_on, retry, timeout, is_terminal).
function applyBasic() {
  const patch: Partial<NodeDef> = {
    id: local.value.id,
    node_type: local.value.node_type,
  }
  if (local.value.retry_count !== undefined) patch.retry_count = local.value.retry_count
  if (local.value.timeout !== undefined) patch.timeout = local.value.timeout
  if (local.value.is_terminal !== undefined) patch.is_terminal = local.value.is_terminal
  if (local.value.depends_on !== undefined) patch.depends_on = local.value.depends_on
  emit('update', patch)
}

function setLabel(label: string) {
  const cfg = { ...(local.value.config ?? {}), label }
  emit('update', { config: cfg })
}

function setDepends(value: string) {
  const list = value.split(',').map(s => s.trim()).filter(Boolean)
  emit('update', { depends_on: list })
}

const dependsText = computed(() => (local.value.depends_on ?? []).join(', '))

function applyRawJson() {
  configError.value = null
  try {
    const parsed = JSON.parse(configJson.value || '{}')
    emit('update', { config: parsed })
  } catch (e: any) {
    configError.value = `JSON 解析错误：${e?.message || e}`
  }
}
</script>

<template>
  <div class="node-config" :class="{ embedded }">
    <template v-if="!embedded">
      <div class="config-header">
        <div class="config-title">
          <span :class="`cat-tag cat-${catalogEntry?.category ?? 'basic'}`">
            {{ catalogEntry?.label ?? node.node_type }}
          </span>
          <span class="config-id">{{ node.id }}</span>
        </div>
        <button class="btn-close" @click="emit('close')">✕</button>
      </div>
    </template>

    <div class="config-body">
      <div class="config-section">
        <div class="section-title">基本信息</div>
        <div class="form-row">
          <label>节点 ID</label>
          <input v-model="local.id" class="form-input" @change="applyBasic" />
        </div>
        <div class="form-row">
          <label>显示名称</label>
          <input
            :value="((local.config?.label as string | undefined) ?? '') as string"
            class="form-input"
            placeholder="（可选）"
            @input="setLabel(($event.target as HTMLInputElement).value)"
          />
        </div>
        <div class="form-row">
          <label>节点类型</label>
          <input v-model="local.node_type" class="form-input" @change="applyBasic" />
        </div>
        <div class="form-row">
          <label>依赖节点（逗号分隔）</label>
          <input
            :value="dependsText"
            class="form-input"
            placeholder="node_a, node_b"
            @input="setDepends(($event.target as HTMLInputElement).value)"
          />
        </div>
        <div class="form-row-inline">
          <div class="form-row">
            <label>重试次数</label>
            <input
              v-model.number="local.retry_count"
              type="number"
              min="0"
              class="form-input"
              @change="applyBasic"
            />
          </div>
          <div class="form-row">
            <label>超时（秒）</label>
            <input
              v-model.number="local.timeout"
              type="number"
              min="0"
              class="form-input"
              @change="applyBasic"
            />
          </div>
        </div>
        <div class="form-row">
          <label class="checkbox-label">
            <input
              type="checkbox"
              v-model="local.is_terminal"
              @change="applyBasic"
            />
            终结节点（is_terminal）
          </label>
        </div>
      </div>

      <div class="config-section">
        <div class="section-title">
          配置
          <span v-if="!hasForm" class="fallback-hint">未识别类型，使用 JSON 模式</span>
        </div>

        <component
          v-if="hasForm"
          :is="formComponent"
          :config="(node.config ?? {})"
          :variables="(props.variables ?? [])"
          @update="applyFormPatch"
        />

        <template v-else>
          <textarea
            v-model="configJson"
            class="config-textarea"
            spellcheck="false"
            rows="10"
          ></textarea>
          <button class="btn-apply" @click="applyRawJson">应用 JSON</button>
          <div v-if="configError" class="config-error">⚠ {{ configError }}</div>
        </template>

        <div v-if="catalogEntry" class="config-hint">
          {{ catalogEntry.description }}
        </div>
      </div>

      <div class="config-section">
        <LiveJsonPreview :config="(node.config ?? {})" empty-hint="(尚未配置)" />
      </div>
    </div>
  </div>
</template>

<style scoped>
.node-config {
  width: 340px;
  border-left: 1px solid var(--border);
  background: var(--bg-secondary);
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

.node-config.embedded {
  width: 100%;
  border-left: none;
  background: transparent;
}

.config-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: var(--space-2) var(--space-3);
  border-bottom: 1px solid var(--border);
}

.config-title {
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.cat-tag {
  display: inline-block;
  padding: 1px var(--space-2);
  border-radius: var(--radius-sm);
  font-size: var(--text-xs);
  font-weight: 600;
  width: fit-content;
}

.cat-tag.cat-ai { background: rgba(52, 152, 219, 0.15); color: var(--info, #3498db); }
.cat-tag.cat-control { background: rgba(243, 156, 18, 0.15); color: var(--warning, #f39c12); }
.cat-tag.cat-basic { background: rgba(46, 204, 113, 0.15); color: var(--success, #2ecc71); }

.config-id {
  font-family: monospace;
  font-size: var(--text-xs);
  color: var(--text-muted);
}

.btn-close {
  background: transparent;
  border: none;
  color: var(--text-muted);
  font-size: var(--text-lg);
  cursor: pointer;
  padding: 0 var(--space-1);
}

.btn-close:hover {
  color: var(--text-primary);
}

.config-body {
  flex: 1;
  overflow-y: auto;
  padding: var(--space-2) var(--space-3);
}

.config-section {
  margin-bottom: var(--space-3);
}

.section-title {
  font-size: var(--text-xs);
  font-weight: 600;
  text-transform: uppercase;
  color: var(--text-secondary);
  margin-bottom: var(--space-2);
  display: flex;
  justify-content: space-between;
  align-items: center;
}

.fallback-hint {
  font-weight: 400;
  font-style: italic;
  text-transform: none;
  color: var(--text-muted);
}

.form-row {
  display: flex;
  flex-direction: column;
  gap: var(--space-1);
  margin-bottom: var(--space-2);
}

.form-row-inline {
  display: flex;
  gap: var(--space-2);
}

.form-row-inline .form-row {
  flex: 1;
}

.form-row label {
  font-size: var(--text-xs);
  color: var(--text-secondary);
}

.form-input {
  padding: var(--space-1) var(--space-2);
  background: var(--bg-primary);
  border: 1px solid var(--border);
  border-radius: var(--radius-sm);
  color: var(--text-primary);
  font-size: var(--text-sm);
  width: 100%;
  box-sizing: border-box;
}

.form-input:focus {
  outline: none;
  border-color: var(--accent);
}

.checkbox-label {
  display: flex !important;
  align-items: center;
  gap: var(--space-1);
  cursor: pointer;
}

.config-textarea {
  width: 100%;
  font-family: 'Consolas', 'Courier New', monospace;
  font-size: var(--text-xs);
  padding: var(--space-2);
  background: var(--bg-primary);
  border: 1px solid var(--border);
  border-radius: var(--radius-sm);
  color: var(--text-primary);
  resize: vertical;
  box-sizing: border-box;
}

.config-textarea:focus {
  outline: none;
  border-color: var(--accent);
}

.config-error {
  margin-top: var(--space-1);
  font-size: var(--text-xs);
  color: var(--danger, #e74c3c);
}

.config-hint {
  margin-top: var(--space-1);
  font-size: var(--text-xs);
  color: var(--text-muted);
  font-style: italic;
}

.btn-apply {
  margin-top: var(--space-1);
  padding: 2px var(--space-2);
  font-size: var(--text-xs);
  background: transparent;
  border: 1px solid var(--border);
  border-radius: var(--radius-sm);
  color: var(--text-secondary);
  cursor: pointer;
}

.btn-apply:hover {
  background: var(--bg-primary);
  color: var(--accent);
}
</style>
