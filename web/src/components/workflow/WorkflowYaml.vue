<script setup lang="ts">
import { ref, watch, computed, onMounted } from 'vue'
import { storeToRefs } from 'pinia'
import { useWorkflowStore } from '../../stores/workflow'

const store = useWorkflowStore()
const { editing, editingDirty, validationErrors, editingIsNew } = storeToRefs(store)

const yamlText = ref('')
const parseError = ref<string | null>(null)
const dirty = ref(false)

function workflowToYaml(wf: any): string {
  const lines: string[] = []
  lines.push(`name: ${yamlEscape(wf.name)}`)
  lines.push(`description: ${yamlEscape(wf.description || '')}`)
  lines.push(`version: ${yamlEscape(wf.version || '1.0.0')}`)
  lines.push('')
  lines.push('triggers:')
  if (wf.triggers?.length) {
    wf.triggers.forEach((t: any) => {
      lines.push(`  - trigger_type: ${yamlEscape(t.trigger_type)}`)
      lines.push('    config:')
      Object.entries(t.config || {}).forEach(([k, v]) => {
        lines.push(`      ${k}: ${yamlValue(v)}`)
      })
    })
  } else {
    lines.push('  []')
  }
  lines.push('')
  lines.push('nodes:')
  if (wf.nodes?.length) {
    wf.nodes.forEach((n: any) => {
      lines.push(`  - id: ${yamlEscape(n.id)}`)
      lines.push(`    node_type: ${yamlEscape(n.node_type)}`)
      lines.push('    config:')
      Object.entries(n.config || {}).forEach(([k, v]) => {
        lines.push(`      ${k}: ${yamlValue(v)}`)
      })
      if (n.depends_on?.length) {
        lines.push(`    depends_on: [${n.depends_on.map((d: string) => yamlEscape(d)).join(', ')}]`)
      }
      if (n.retry_count !== undefined) {
        lines.push(`    retry_count: ${n.retry_count}`)
      }
      if (n.timeout !== undefined && n.timeout !== null) {
        lines.push(`    timeout: ${n.timeout}`)
      }
      if (n.is_terminal) {
        lines.push('    is_terminal: true')
      }
    })
  } else {
    lines.push('  []')
  }
  lines.push('')
  lines.push('edges:')
  if (wf.edges?.length) {
    wf.edges.forEach((e: any) => {
      lines.push(`  - from_node: ${yamlEscape(e.from_node)}`)
      lines.push(`    to_node: ${yamlEscape(e.to_node)}`)
      if (e.condition) lines.push(`    condition: ${yamlEscape(e.condition)}`)
    })
  } else {
    lines.push('  []')
  }
  lines.push('')
  lines.push('variables:')
  if (wf.variables && Object.keys(wf.variables).length) {
    Object.entries(wf.variables).forEach(([k, v]) => {
      lines.push(`  ${k}: ${yamlValue(v)}`)
    })
  } else {
    lines.push('  {}')
  }
  lines.push('')
  lines.push('metadata:')
  if (wf.metadata && Object.keys(wf.metadata).length) {
    Object.entries(wf.metadata).forEach(([k, v]) => {
      lines.push(`  ${k}: ${yamlValue(v)}`)
    })
  } else {
    lines.push('  {}')
  }
  return lines.join('\n')
}

function yamlEscape(s: string): string {
  if (!s) return '""'
  if (/[:\[\]\{\},&*?#|\>@"'\n]/.test(s) || s.includes(' ')) {
    return `"${s.replace(/\\/g, '\\\\').replace(/"/g, '\\"')}"`
  }
  return s
}

function yamlValue(v: any): string {
  if (v === null || v === undefined) return 'null'
  if (typeof v === 'string') return yamlEscape(v)
  if (typeof v === 'number' || typeof v === 'boolean') return String(v)
  return yamlEscape(JSON.stringify(v))
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
    const parsed = parseSimpleYaml(yamlText.value)
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

function parseSimpleYaml(text: string): any {
  // Very limited YAML parser: handles `key: value`, lists with `- key: value`,
  // and inline `[a, b]` / `{}`. For complex YAML use the canvas.
  const result: any = {}
  const lines = text.split('\n')
  let i = 0

  function readValue(s: string): any {
    s = s.trim()
    if (s === '' || s === 'null' || s === '~') return null
    if (s === 'true') return true
    if (s === 'false') return false
    if (/^-?\d+(\.\d+)?$/.test(s)) return Number(s)
    if (s.startsWith('[') && s.endsWith(']')) {
      if (s === '[]') return []
      return s.slice(1, -1).split(',').map(x => readValue(x))
    }
    if (s.startsWith('{') && s.endsWith('}')) return {}
    if (s.startsWith('"') && s.endsWith('"')) {
      return s.slice(1, -1).replace(/\\"/g, '"').replace(/\\\\/g, '\\')
    }
    return s
  }

  while (i < lines.length) {
    const line = lines[i]
    if (!line.trim() || line.trim().startsWith('#')) { i++; continue }
    const m = line.match(/^(\w+):\s*(.*)$/)
    if (!m) { i++; continue }
    const key = m[1]
    const rest = m[2]?.trim() ?? ''
    if (rest === '') {
      if (key === 'variables' || key === 'metadata') {
        i++
        const obj: any = {}
        while (i < lines.length && /^\s+\w+:/.test(lines[i])) {
          const sub = lines[i].match(/^\s+(\w+):\s*(.*)$/)
          if (sub) obj[sub[1]] = readValue(sub[2])
          i++
        }
        result[key] = obj
      } else if (key === 'triggers' || key === 'nodes' || key === 'edges') {
        i++
        const arr: any[] = []
        while (i < lines.length && /^\s+-\s/.test(lines[i])) {
          const item: any = {}
          i++
          while (i < lines.length && /^\s+\w+:/.test(lines[i])) {
            const sub = lines[i].match(/^\s+(\w+):\s*(.*)$/)
            if (sub) {
              const subKey = sub[1]
              if (subKey === 'config') {
                i++
                const cfg: any = {}
                while (i < lines.length && /^\s+\w+:/.test(lines[i])) {
                  const cm = lines[i].match(/^\s+(\w+):\s*(.*)$/)
                  if (cm) cfg[cm[1]] = readValue(cm[2])
                  i++
                }
                item.config = cfg
              } else if (sub[2].trim() === '') {
                i++
                while (i < lines.length && /^\s+\w+:/.test(lines[i])) {
                  const cm = lines[i].match(/^\s+(\w+):\s*(.*)$/)
                  if (cm) item[cm[1]] = readValue(cm[2])
                  i++
                }
              } else {
                item[subKey] = readValue(sub[2])
                i++
              }
            } else {
              i++
            }
          }
          arr.push(item)
        }
        result[key] = arr
      } else {
        i++
      }
    } else {
      result[key] = readValue(rest)
      i++
    }
  }
  return result
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
        提示：此处为简易 YAML 编辑器。Phase F 将集成 CodeMirror 6 提供语法高亮。
        当前可使用基础编辑功能。
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
