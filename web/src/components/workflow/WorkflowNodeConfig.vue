<script setup lang="ts">
import { ref, watch, computed } from 'vue'
import type { NodeDef } from '../../types/workflow'
import { NODE_CATALOG } from '../../types/workflow'

const props = defineProps<{
  node: NodeDef
}>()

const emit = defineEmits<{
  (e: 'update', patch: Partial<NodeDef>): void
  (e: 'close'): void
}>()

const catalogEntry = computed(() =>
  NODE_CATALOG.find(e => e.type === props.node.node_type),
)

const local = ref<NodeDef>({ ...props.node })
const configJson = ref(JSON.stringify(props.node.config ?? {}, null, 2))
const configError = ref<string | null>(null)
const condition = ref<string>('')

watch(() => props.node, (n) => {
  local.value = { ...n }
  configJson.value = JSON.stringify(n.config ?? {}, null, 2)
  configError.value = null
}, { deep: true })

function applyConfig() {
  configError.value = null
  try {
    const parsed = JSON.parse(configJson.value || '{}')
    emit('update', { config: parsed })
  } catch (e: any) {
    configError.value = `JSON 解析错误：${e?.message || e}`
  }
}

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

function applyAll() {
  applyBasic()
  applyConfig()
}

function setLabel(label: string) {
  const cfg = { ...local.value.config, label }
  emit('update', { config: cfg })
}

function setDepends(value: string) {
  const list = value.split(',').map(s => s.trim()).filter(Boolean)
  emit('update', { depends_on: list })
}

const dependsText = computed(() => (local.value.depends_on ?? []).join(', '))
</script>

<template>
  <div class="node-config">
    <div class="config-header">
      <div class="config-title">
        <span :class="`cat-tag cat-${catalogEntry?.category ?? 'basic'}`">
          {{ catalogEntry?.label ?? node.node_type }}
        </span>
        <span class="config-id">{{ node.id }}</span>
      </div>
      <button class="btn-close" @click="emit('close')">✕</button>
    </div>

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
            :value="(local.config?.label as string) || ''"
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
          配置 (config JSON)
          <button class="btn-apply" @click="applyConfig">应用</button>
        </div>
        <textarea
          v-model="configJson"
          class="config-textarea"
          spellcheck="false"
          rows="10"
        ></textarea>
        <div v-if="configError" class="config-error">⚠ {{ configError }}</div>
        <div v-if="catalogEntry" class="config-hint">
          {{ catalogEntry.description }}
        </div>
      </div>
    </div>

    <div class="config-footer">
      <button class="btn btn-primary" @click="applyAll">应用全部</button>
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
  padding: 1px var(--space-2);
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

.config-footer {
  padding: var(--space-2) var(--space-3);
  border-top: 1px solid var(--border);
  display: flex;
  justify-content: flex-end;
}
</style>
