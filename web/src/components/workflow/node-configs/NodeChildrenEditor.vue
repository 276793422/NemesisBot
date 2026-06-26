<script setup lang="ts">
/**
 * Recursive nested-node editor for `loop` and `parallel` containers.
 *
 * Shows each child node as a row (id + type badge + edit/delete buttons).
 * Clicking edit expands an inline `WorkflowNodeConfig` for that child —
 * which itself may contain another `NodeChildrenEditor` if the child is
 * also a container (loop-in-loop, parallel-in-parallel, etc).
 *
 * "Add node" opens the same catalog modal the canvas uses (just imported
 * here) so nested nodes have the same UX as top-level nodes.
 */
import { ref, computed } from 'vue'
import type { NodeDef, NodeCategory } from '../../../types/workflow'
import { NODE_CATALOG, nodesByCategory } from '../../../types/workflow'
import WorkflowNodeConfig from '../WorkflowNodeConfig.vue'
import type { VariableOption } from './useVariablePicker'

const props = defineProps<{
  /** Child node list (config.nodes or config.branches). */
  nodes: NodeDef[]
  /** All workflow variables + sibling node outputs, for the variable picker. */
  variables?: VariableOption[]
  /** Whether to show the "is_terminal" toggle on each child (used by loop). */
  allowTerminal?: boolean
}>()

const emit = defineEmits<{
  (e: 'update', nodes: NodeDef[]): void
}>()

const expandedIndex = ref<number | null>(null)
const showCatalog = ref(false)

// Active category filter in the catalog modal (default: show all).
const activeCategory = ref<NodeCategory | 'all'>('all')
const categories: (NodeCategory | 'all')[] = ['all', 'ai', 'control', 'basic']

const visibleCatalog = computed(() =>
  activeCategory.value === 'all'
    ? NODE_CATALOG
    : nodesByCategory(activeCategory.value as NodeCategory),
)

function genId(prefix: string): string {
  let i = 1
  while (props.nodes.some((n) => n.id === `${prefix}_${i}`)) i++
  return `${prefix}_${i}`
}

function addChild(type: string) {
  const entry = NODE_CATALOG.find((e) => e.type === type)
  const id = genId(type)
  const child: NodeDef = {
    id,
    node_type: type,
    config: defaultConfigForType(type),
  }
  const next = [...props.nodes, child]
  emit('update', next)
  showCatalog.value = false
  expandedIndex.value = next.length - 1
}

function defaultConfigForType(type: string): Record<string, unknown> {
  switch (type) {
    case 'script':
      return { language: 'bash', script: '' }
    case 'delay':
      return { seconds: 1 }
    case 'condition':
      return { condition: 'true' }
    case 'llm':
      return { prompt: '' }
    case 'tool':
      return { tool: '', args: {} }
    case 'http':
      return { url: '', method: 'GET' }
    case 'loop':
      return { max_iterations: 3, nodes: [] }
    case 'parallel':
      return { nodes: [] }
    case 'transform':
      return { expression: 'identity' }
    case 'human_review':
      return { message: '请审核' }
    case 'agent':
      return { prompt: '' }
    case 'sub_workflow':
      return { workflow: '', input: {} }
    case 'question_classifier':
      return { question: '', classes: [] }
    case 'parameter_extractor':
      return { text: '', parameters: [] }
    default:
      return {}
  }
}

function updateChild(idx: number, patch: Partial<NodeDef>) {
  const next = props.nodes.map((n, i) => (i === idx ? { ...n, ...patch } : n))
  emit('update', next)
}

function deleteChild(idx: number) {
  const next = props.nodes.filter((_, i) => i !== idx)
  emit('update', next)
  if (expandedIndex.value === idx) expandedIndex.value = null
  else if (expandedIndex.value !== null && expandedIndex.value > idx) {
    expandedIndex.value -= 1
  }
}

function moveChild(idx: number, dir: -1 | 1) {
  const target = idx + dir
  if (target < 0 || target >= props.nodes.length) return
  const next = [...props.nodes]
  const [item] = next.splice(idx, 1)
  next.splice(target, 0, item)
  emit('update', next)
  if (expandedIndex.value === idx) expandedIndex.value = target
  else if (expandedIndex.value === target) expandedIndex.value = idx
}
</script>

<template>
  <div class="node-children">
    <div v-if="nodes.length === 0" class="empty">
      <span>暂无子节点</span>
    </div>

    <div v-for="(child, idx) in nodes" :key="child.id" class="child-row">
      <div class="child-summary">
        <span class="child-id">{{ child.id }}</span>
        <span :class="`child-type cat-${NODE_CATALOG.find((e) => e.type === child.node_type)?.category ?? 'basic'}`">
          {{ child.node_type }}
        </span>
        <div class="child-actions">
          <button class="btn-icon" :title="'上移'" :disabled="idx === 0" @click="moveChild(idx, -1)">↑</button>
          <button class="btn-icon" :title="'下移'" :disabled="idx === nodes.length - 1" @click="moveChild(idx, 1)">↓</button>
          <button class="btn-icon" :title="expandedIndex === idx ? '收起' : '编辑'" @click="expandedIndex = expandedIndex === idx ? null : idx">
            {{ expandedIndex === idx ? '▾' : '▸' }}
          </button>
          <button class="btn-icon btn-danger" title="删除" @click="deleteChild(idx)">×</button>
        </div>
      </div>

      <div v-if="expandedIndex === idx" class="child-editor">
        <WorkflowNodeConfig
          :node="child"
          :embedded="true"
          :variables="props.variables"
          @update="(patch) => updateChild(idx, patch)"
          @close="expandedIndex = null"
        />
      </div>
    </div>

    <button class="btn-add" @click="showCatalog = true">+ 添加子节点</button>

    <!-- Catalog modal -->
    <Teleport to="body">
      <div v-if="showCatalog" class="catalog-overlay" @click.self="showCatalog = false">
        <div class="catalog-modal">
          <div class="catalog-header">
            <h3>选择节点类型</h3>
            <button class="btn-close" @click="showCatalog = false">✕</button>
          </div>
          <div class="catalog-categories">
            <button
              v-for="c in categories"
              :key="c"
              class="cat-filter"
              :class="{ active: activeCategory === c }"
              @click="activeCategory = c"
            >
              {{ c === 'all' ? '全部' : c }}
            </button>
          </div>
          <div class="catalog-grid">
            <button
              v-for="entry in visibleCatalog"
              :key="entry.type"
              class="catalog-tile"
              @click="addChild(entry.type)"
            >
              <div :class="`tile-icon cat-${entry.category}`">{{ entry.label }}</div>
              <div class="tile-desc">{{ entry.description }}</div>
            </button>
          </div>
        </div>
      </div>
    </Teleport>
  </div>
</template>

<style scoped>
.node-children {
  display: flex;
  flex-direction: column;
  gap: var(--space-1);
}

.empty {
  padding: var(--space-2);
  color: var(--text-muted);
  text-align: center;
  border: 1px dashed var(--border);
  border-radius: var(--radius-sm);
}

.child-row {
  border: 1px solid var(--border);
  border-radius: var(--radius-sm);
  background: var(--bg-primary);
  overflow: hidden;
}

.child-summary {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  padding: 4px var(--space-2);
  font-size: var(--text-sm);
}

.child-id {
  font-family: 'Consolas', monospace;
  font-size: var(--text-xs);
  color: var(--text-primary);
  flex: 1;
}

.child-type {
  font-size: var(--text-xs);
  padding: 1px var(--space-1);
  border-radius: var(--radius-sm);
  font-weight: 500;
}

.cat-ai { background: rgba(52, 152, 219, 0.15); color: var(--info, #3498db); }
.cat-control { background: rgba(243, 156, 18, 0.15); color: var(--warning, #f39c12); }
.cat-basic { background: rgba(46, 204, 113, 0.15); color: var(--success, #2ecc71); }

.child-actions {
  display: flex;
  gap: 2px;
}

.btn-icon {
  background: transparent;
  border: 1px solid transparent;
  color: var(--text-secondary);
  cursor: pointer;
  padding: 2px 6px;
  font-size: var(--text-xs);
  border-radius: var(--radius-sm);
}

.btn-icon:hover:not(:disabled) {
  background: var(--bg-secondary);
  color: var(--text-primary);
}

.btn-icon:disabled {
  opacity: 0.3;
  cursor: not-allowed;
}

.btn-icon.btn-danger:hover {
  color: var(--danger, #e74c3c);
}

.child-editor {
  border-top: 1px solid var(--border);
  background: var(--bg-secondary);
}

.btn-add {
  align-self: flex-start;
  background: transparent;
  border: 1px dashed var(--border);
  color: var(--text-secondary);
  padding: 4px var(--space-2);
  border-radius: var(--radius-sm);
  cursor: pointer;
  font-size: var(--text-xs);
}

.btn-add:hover {
  border-color: var(--accent);
  color: var(--accent);
}

.catalog-overlay {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.5);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 2000;
}

.catalog-modal {
  background: var(--bg-surface, var(--bg-primary, #fff));
  border-radius: 8px;
  width: 560px;
  max-width: 90vw;
  max-height: 80vh;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

.catalog-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: var(--space-2) var(--space-3);
  border-bottom: 1px solid var(--border);
}

.catalog-header h3 {
  margin: 0;
  font-size: var(--text-base);
}

.btn-close {
  background: transparent;
  border: none;
  cursor: pointer;
  font-size: var(--text-lg);
  color: var(--text-muted);
}

.catalog-categories {
  display: flex;
  gap: var(--space-1);
  padding: var(--space-2) var(--space-3);
  border-bottom: 1px solid var(--border);
}

.cat-filter {
  background: var(--bg-secondary);
  border: 1px solid var(--border);
  color: var(--text-secondary);
  padding: 4px var(--space-2);
  border-radius: var(--radius-sm);
  font-size: var(--text-xs);
  cursor: pointer;
}

.cat-filter.active {
  background: var(--accent);
  color: #fff;
  border-color: var(--accent);
}

.catalog-grid {
  display: grid;
  grid-template-columns: repeat(2, 1fr);
  gap: var(--space-2);
  padding: var(--space-3);
  overflow-y: auto;
}

.catalog-tile {
  text-align: left;
  background: var(--bg-primary);
  border: 1px solid var(--border);
  border-radius: var(--radius-sm);
  padding: var(--space-2);
  cursor: pointer;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.catalog-tile:hover {
  border-color: var(--accent);
  background: var(--bg-secondary);
}

.tile-icon {
  font-size: var(--text-sm);
  font-weight: 600;
}

.tile-desc {
  font-size: var(--text-xs);
  color: var(--text-muted);
}
</style>
