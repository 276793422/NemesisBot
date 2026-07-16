<script setup lang="ts">
import { ref, watch, computed, onMounted } from 'vue'
import { storeToRefs } from 'pinia'
import { useWorkflowStore } from '../../stores/workflow'
import { dump as yamlDump, load as yamlLoad } from 'js-yaml'

const store = useWorkflowStore()
const { editing, editingDirty, validationErrors, editingIsNew } = storeToRefs(store)

const yamlText = ref('')
const parseError = ref<string | null>(null)
const dirty = ref(false)

// Serialize the workflow to YAML using js-yaml.
//
// This replaces a hand-written serializer that JSON.stringified any nested
// object/array config value (tool `args`, loop `nodes`) into a quoted string,
// which corrupted workflows on save — the backend then saw `nodes` as a string
// instead of an array, so loops resolved zero children and silently did
// nothing. js-yaml emits proper nested YAML for objects/arrays.
function workflowToYaml(wf: any): string {
  return yamlDump(wf, {
    lineWidth: -1, // don't wrap long lines (prompts, file content)
    noRefs: true, // never emit `&anchor` / `*alias` aliases
    sortKeys: false,
  })
}

function loadFromStore() {
  if (!editing.value) {
    yamlText.value = ''
    return
  }
  yamlText.value = workflowToYaml(editing.value)
  dirty.value = false
  parseError.value = null
}

onMounted(loadFromStore)
watch(() => editing.value, loadFromStore, { deep: false })

async function applyYaml() {
  parseError.value = null
  try {
    const parsed = yamlLoad(yamlText.value) as any
    if (parsed === undefined || parsed === null) {
      throw new Error('YAML 内容为空')
    }
    const wf: any = {
      name: parsed.name ?? editing.value?.name ?? '',
      description: parsed.description ?? '',
      version: parsed.version ?? '1.0.0',
      triggers: parsed.triggers ?? [],
      nodes: parsed.nodes ?? [],
      edges: parsed.edges ?? [],
      variables: parsed.variables ?? {},
      metadata: parsed.metadata ?? {},
    }
    if (!editing.value) {
      store.startNewWorkflow()
    }
    editing.value = wf
    editingDirty.value = true
    dirty.value = false
  } catch (e: any) {
    parseError.value = String(e?.message || e)
  }
}

const canApply = computed(() => dirty.value && yamlText.value.trim().length > 0)

async function save() {
  await applyYaml()
  const res = await store.saveEditing()
  if (!res.ok) {
    parseError.value = res.error
  }
}

async function validate() {
  await applyYaml()
  await store.validateEditing()
}
</script>

<template>
  <div class="wf-yaml">
    <div v-if="!editing" class="yaml-empty">
      <div class="empty-icon">📝</div>
      <div>没有正在编辑的工作流。先在列表 Tab 选择或新建。</div>
    </div>

    <template v-else>
      <div class="yaml-toolbar">
        <div class="yaml-info">
          <strong>{{ editingIsNew ? '新建' : '编辑' }}:</strong>
          <span class="wf-name">{{ editing.name || '(unnamed)' }}</span>
          <span v-if="dirty" class="dirty-flag">未应用</span>
          <span v-else-if="editingDirty" class="dirty-flag saved-pending">未保存</span>
        </div>
        <div class="yaml-actions">
          <button class="btn" @click="validate" :disabled="!canApply">🔍 校验</button>
          <button class="btn btn-primary" @click="save" :disabled="!canApply && !editingDirty">
            💾 保存
          </button>
          <button class="btn" @click="applyYaml" :disabled="!dirty">⚡ 应用到表单</button>
          <button class="btn" @click="loadFromStore">↺ 还原</button>
        </div>
      </div>

      <div v-if="parseError" class="yaml-error">⚠ {{ parseError }}</div>

      <div v-if="validationErrors.length > 0" class="yaml-warnings">
        <div class="warning-title">校验错误：</div>
        <ul>
          <li v-for="(err, i) in validationErrors" :key="i">{{ err }}</li>
        </ul>
      </div>

      <textarea
        v-model="yamlText"
        class="yaml-textarea"
        spellcheck="false"
        @input="dirty = true"
        placeholder="name: my-workflow
description: ...
triggers: []
nodes: []
edges: []
variables: {}
metadata: {}"
      ></textarea>

      <div class="yaml-hint">
        提示：YAML 编辑器（基于 js-yaml）。工具节点的 args、循环节点的 nodes 等嵌套结构会以正确的 YAML 对象/数组形式保存。
      </div>
    </template>
  </div>
</template>

<style scoped>
.wf-yaml {
  display: flex;
  flex-direction: column;
  height: 100%;
  padding: var(--space-3);
  gap: var(--space-2);
  overflow: hidden;
}

.yaml-empty {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: var(--space-2);
  color: var(--text-muted);
}

.empty-icon {
  font-size: 48px;
  opacity: 0.4;
}

.yaml-toolbar {
  display: flex;
  justify-content: space-between;
  align-items: center;
  gap: var(--space-2);
  flex-wrap: wrap;
}

.yaml-info {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  font-size: var(--text-sm);
}

.wf-name {
  font-family: monospace;
  color: var(--accent);
}

.dirty-flag {
  font-size: var(--text-xs);
  padding: 1px var(--space-2);
  border-radius: var(--radius-sm);
  background: rgba(243, 156, 18, 0.15);
  color: var(--warning, #f39c12);
}

.dirty-flag.saved-pending {
  background: rgba(52, 152, 219, 0.15);
  color: var(--info, #3498db);
}

.yaml-actions {
  display: flex;
  gap: var(--space-2);
}

.yaml-error,
.yaml-warnings {
  padding: var(--space-2) var(--space-3);
  background: rgba(231, 76, 60, 0.1);
  border-left: 3px solid var(--danger, #e74c3c);
  border-radius: var(--radius-sm);
  font-size: var(--text-sm);
}

.warning-title {
  font-weight: 600;
  margin-bottom: var(--space-1);
}

.yaml-textarea {
  flex: 1;
  width: 100%;
  font-family: 'Consolas', 'Courier New', monospace;
  font-size: var(--text-sm);
  padding: var(--space-3);
  background: var(--bg-secondary);
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
  color: var(--text-primary);
  resize: none;
  line-height: 1.5;
  tab-size: 2;
}

.yaml-textarea:focus {
  outline: none;
  border-color: var(--accent);
}

.yaml-hint {
  font-size: var(--text-xs);
  color: var(--text-muted);
  padding: var(--space-1) var(--space-2);
}
</style>
