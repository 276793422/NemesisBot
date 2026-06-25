<script setup lang="ts">
import { ref, computed, watch, nextTick, onUnmounted } from 'vue'
import { storeToRefs } from 'pinia'
import {
  VueFlow,
  useVueFlow,
  type Connection,
  type NodeDragEvent,
} from '@vue-flow/core'
import { Handle, Position } from '@vue-flow/core'
import { Background } from '@vue-flow/background'
import { Controls } from '@vue-flow/controls'
import { MiniMap } from '@vue-flow/minimap'
import { useWorkflowStore } from '../../stores/workflow'
import {
  NODE_CATALOG,
  nodesByCategory,
  type NodeCatalogEntry,
  type NodeCategory,
  type NodeDef,
  type Edge as WfEdge,
} from '../../types/workflow'
import WorkflowNodeConfig from './WorkflowNodeConfig.vue'

interface WfNodeData {
  label: string
  nodeType: string
  category: NodeCategory
  def: NodeDef
}

const store = useWorkflowStore()
const { editing, editingDirty, editingIsNew, validationErrors, selectedRun } = storeToRefs(store)

const selectedNodeId = ref<string | null>(null)
const showConfigPanel = ref(false)
const paletteQuery = ref('')
const paletteCategory = ref<NodeCategory | 'all'>('all')
const draggingType = ref<string | null>(null)

const {
  onConnect,
  addEdges,
  removeNodes,
  removeEdges,
  fitView,
  onNodesChange,
  onEdgesChange,
  screenToFlowCoordinate,
  onNodeDragStop,
  onEdgeUpdate,
} = useVueFlow()

const nodes = ref<any[]>([])
const edges = ref<any[]>([])

const selectedNode = computed<NodeDef | null>(() => {
  if (!selectedNodeId.value || !editing.value) return null
  return editing.value.nodes.find(n => n.id === selectedNodeId.value) ?? null
})

const runNodeStates = computed<Record<string, string>>(() => {
  const m: Record<string, string> = {}
  if (selectedRun.value?.node_results) {
    for (const nr of selectedRun.value.node_results) {
      m[nr.node_id] = nr.state
    }
  }
  return m
})

const paletteEntries = computed<NodeCatalogEntry[]>(() => {
  let list = paletteCategory.value === 'all'
    ? NODE_CATALOG
    : nodesByCategory(paletteCategory.value)
  if (paletteQuery.value.trim()) {
    const q = paletteQuery.value.toLowerCase()
    list = list.filter(e =>
      e.label.toLowerCase().includes(q) ||
      e.type.toLowerCase().includes(q) ||
      e.description.toLowerCase().includes(q),
    )
  }
  return list
})

function nodeLabel(def: NodeDef): string {
  const entry = NODE_CATALOG.find(e => e.type === def.node_type)
  return (def.config?.['label'] as string) || entry?.label || def.node_type
}

function syncFromEditing() {
  if (!editing.value) {
    nodes.value = []
    edges.value = []
    return
  }
  // Preserve positions of existing nodes so drag/canvas placement survives
  // reactive re-syncs (e.g. when a config edit triggers syncFromEditing).
  const prevPos: Record<string, { x: number; y: number }> = {}
  for (const n of nodes.value) prevPos[n.id] = n.position

  const layout = computeGridLayout(editing.value.nodes)
  nodes.value = editing.value.nodes.map(def => {
    const pos = prevPos[def.id] ?? layout[def.id] ?? { x: 0, y: 0 }
    const entry = NODE_CATALOG.find(e => e.type === def.node_type)
    return {
      id: def.id,
      type: 'workflow',
      position: pos,
      data: {
        label: nodeLabel(def),
        nodeType: def.node_type,
        category: entry?.category ?? 'basic',
        def,
      },
    }
  })
  edges.value = editing.value.edges.map((e: WfEdge, i: number) => ({
    id: `e-${e.from_node}-${e.to_node}-${i}`,
    source: e.from_node,
    target: e.to_node,
    label: e.condition || undefined,
    animated: false,
  }))
}

function computeGridLayout(nodeDefs: NodeDef[]): Record<string, { x: number; y: number }> {
  const positions: Record<string, { x: number; y: number }> = {}
  const cols = 4
  const dx = 260
  const dy = 120
  nodeDefs.forEach((n, i) => {
    const col = i % cols
    const row = Math.floor(i / cols)
    positions[n.id] = { x: 40 + col * dx, y: 40 + row * dy }
  })
  return positions
}

watch(editing, syncFromEditing, { deep: false, immediate: true })

function makeId(prefix: string): string {
  return `${prefix}_${Date.now().toString(36)}_${Math.random().toString(36).substring(2, 6)}`
}

function addNodeFromPalette(entry: NodeCatalogEntry, position?: { x: number; y: number }) {
  if (!editing.value) {
    store.startNewWorkflow()
  }
  const id = makeId(entry.type)
  const def: NodeDef = {
    id,
    node_type: entry.type,
    config: {},
  }
  editing.value!.nodes.push(def)
  editingDirty.value = true

  // Append to flow nodes directly so we can set position freely.
  nodes.value = [
    ...nodes.value,
    {
      id,
      type: 'workflow',
      position: position ?? nextGridLayoutPosition(),
      data: {
        label: nodeLabel(def),
        nodeType: entry.type,
        category: entry.category,
        def,
      },
    },
  ]
  selectedNodeId.value = id
}

function nextGridLayoutPosition() {
  const count = nodes.value.length
  const cols = 4
  const col = count % cols
  const row = Math.floor(count / cols)
  return { x: 40 + col * 260, y: 40 + row * 120 }
}

// ----- Drag-and-drop from palette -----
function onPaletteDragStart(event: DragEvent, entry: NodeCatalogEntry) {
  if (!event.dataTransfer) return
  draggingType.value = entry.type
  event.dataTransfer.effectAllowed = 'move'
  event.dataTransfer.setData('application/x-workflow-node', entry.type)
  // Empty image so the cursor shows the browser's "dragging" state cleanly
}

function onPaletteDragEnd() {
  draggingType.value = null
}

function onFlowDragOver(event: DragEvent) {
  if (!event.dataTransfer) return
  if (draggingType.value) {
    event.preventDefault()
    event.dataTransfer.dropEffect = 'move'
  }
}

function onFlowDrop(event: DragEvent) {
  if (!event.dataTransfer) return
  const type = event.dataTransfer.getData('application/x-workflow-node')
  if (!type) return
  const entry = NODE_CATALOG.find(e => e.type === type)
  if (!entry) return
  event.preventDefault()

  // Drop coordinates are screen pixels — convert to flow coordinates so the
  // node lands where the cursor released.
  const flowPos = screenToFlowCoordinate({ x: event.clientX, y: event.clientY })
  addNodeFromPalette(entry, flowPos)
}

// ----- Connections -----
onConnect((conn: Connection) => {
  if (!editing.value) return
  if (conn.source === conn.target) return
  const edge: WfEdge = {
    from_node: conn.source,
    to_node: conn.target,
    condition: null,
  }
  const exists = editing.value.edges.some(
    e => e.from_node === edge.from_node && e.to_node === edge.to_node,
  )
  if (exists) return
  editing.value.edges.push(edge)
  editingDirty.value = true
  addEdges({
    id: `e-${conn.source}-${conn.target}-${Date.now()}`,
    source: conn.source,
    target: conn.target,
  })
})

// ----- Drag node stop: persist position back into local cache -----
onNodeDragStop((evt: NodeDragEvent) => {
  const node = evt.node
  if (!node) return
  // Find in nodes[] and update position — syncFromEditing preserves it
  const target = nodes.value.find(n => n.id === node.id)
  if (target) {
    target.position = { ...node.position }
  }
})

// ----- Click handlers -----
function handleNodeDoubleClick(evt: any) {
  const id = evt?.node?.id
  if (id) {
    selectedNodeId.value = id
    showConfigPanel.value = true
  }
}

function handlePaneClick() {
  selectedNodeId.value = null
  showConfigPanel.value = false
}

function handleNodeClick(evt: any) {
  const id = evt?.node?.id
  if (id) selectedNodeId.value = id
}

// ----- Keyboard delete -----
function onKeydown(ev: KeyboardEvent) {
  // Only fire on Delete/Backspace when not focused in an input/textarea.
  const target = ev.target as HTMLElement
  if (target && (target.tagName === 'INPUT' || target.tagName === 'TEXTAREA' || target.isContentEditable)) return
  if (ev.key !== 'Delete' && ev.key !== 'Backspace') return
  deleteSelected()
}

if (typeof window !== 'undefined') {
  window.addEventListener('keydown', onKeydown)
}

onUnmounted(() => {
  if (typeof window !== 'undefined') {
    window.removeEventListener('keydown', onKeydown)
  }
})

function deleteSelected() {
  const selNodes = nodes.value.filter((n: any) => n.selected)
  const selEdges = edges.value.filter((e: any) => e.selected)
  if (!selNodes.length && !selEdges.length) return

  for (const n of selNodes) {
    if (!editing.value) continue
    editing.value.nodes = editing.value.nodes.filter(x => x.id !== n.id)
    editing.value.edges = editing.value.edges.filter(
      e => e.from_node !== n.id && e.to_node !== n.id,
    )
    if (selectedNodeId.value === n.id) {
      selectedNodeId.value = null
      showConfigPanel.value = false
    }
  }
  for (const e of selEdges) {
    if (!editing.value) continue
    const m = e.id.match(/^e-(.+)-(.+)-\w+$/)
    if (!m) continue
    const [, from, to] = m
    editing.value.edges = editing.value.edges.filter(
      x => !(x.from_node === from && x.to_node === to),
    )
  }
  if (selNodes.length) removeNodes(selNodes.map(n => n.id))
  if (selEdges.length) removeEdges(selEdges.map(e => e.id))
  if (selNodes.length || selEdges.length) editingDirty.value = true
}

// ----- Top toolbar actions -----
function clearCanvas() {
  if (!editing.value) return
  if (editing.value.nodes.length === 0 && editing.value.edges.length === 0) return
  if (!window.confirm(`清空画布上的 ${editing.value.nodes.length} 个节点和 ${editing.value.edges.length} 条边？`)) return
  editing.value.nodes = []
  editing.value.edges = []
  editingDirty.value = true
  selectedNodeId.value = null
  showConfigPanel.value = false
  syncFromEditing()
}

function discardAndClose() {
  store.discardEditing()
  selectedNodeId.value = null
  showConfigPanel.value = false
  store.setActiveTab('list')
}

function fitViewNow() {
  nextTick(() => fitView({ padding: 0.2 }))
}

function updateNodeDef(id: string, patch: Partial<NodeDef>) {
  if (!editing.value) return
  const idx = editing.value.nodes.findIndex(n => n.id === id)
  if (idx < 0) return
  editing.value.nodes[idx] = { ...editing.value.nodes[idx], ...patch }
  editingDirty.value = true
  // Re-sync the data on the matching node in nodes[] (preserve position)
  const target = nodes.value.find(n => n.id === id)
  if (target) {
    const def = editing.value.nodes[idx]
    target.data = {
      label: nodeLabel(def),
      nodeType: def.node_type,
      category: (NODE_CATALOG.find(e => e.type === def.node_type)?.category ?? 'basic') as NodeCategory,
      def,
    }
  }
}

async function saveWorkflow() {
  if (!editing.value) return
  if (!editing.value.name.trim()) {
    const name = window.prompt('请输入工作流名称：', 'my-workflow')
    if (!name) return
    editing.value.name = name.trim()
  }
  const res = await store.saveEditing()
  if (!res.ok) {
    window.alert(`保存失败：${res.error}`)
  }
}

async function validateWorkflow() {
  if (!editing.value) return
  await store.validateEditing()
}

/**
 * Open a prompt that lets the user fire a manual trigger-event at the engine.
 * The event is published into the engine's EventDispatcher — any workflow
 * with an `event` trigger whose `event_type` glob matches will start.
 *
 * Format:
 *   事件类型: workflow.completed   (or any string, glob-allowed in triggers)
 *   数据 JSON: {"status":"success"} (optional)
 *
 * Why this lives on the canvas page: users can draw and test triggers without
 * context-switching to a separate "events" view.
 */
async function simulateEvent() {
  const eventType = window.prompt(
    '请输入事件类型（event_type）：\n提示：工作流触发器支持 glob 匹配，例如 "workflow.*" 能匹配 "workflow.completed"。',
    'workflow.completed',
  )
  if (!eventType || !eventType.trim()) return

  const dataStr = window.prompt(
    '请输入事件数据（可选，JSON 格式）：\n例如 {"status":"success","workflow_name":"demo"}',
    '{}',
  )
  let data: Record<string, unknown> = {}
  if (dataStr && dataStr.trim() && dataStr.trim() !== '{}') {
    try {
      const parsed = JSON.parse(dataStr)
      if (typeof parsed !== 'object' || parsed === null || Array.isArray(parsed)) {
        window.alert('事件数据必须是 JSON 对象，例如 {"key":"value"}')
        return
      }
      data = parsed as Record<string, unknown>
    } catch (e) {
      window.alert(`事件数据 JSON 解析失败：\n${e}`)
      return
    }
  }

  const res = await store.fireEvent(eventType.trim(), data)
  if (!res.published) {
    window.alert(`事件发布失败（可能 EventDispatcher 未就绪）`)
    return
  }
  if (res.matched.length === 0) {
    window.alert(
      `事件已发布：${eventType.trim()}\n\n但没有工作流订阅此事件。\n\n` +
      `提示：在工作流的 triggers 中添加：\n` +
      `  - trigger_type: event\n` +
      `    config:\n` +
      `      event_type: "${eventType.trim()}"  # 支持 glob，如 "workflow.*"`,
    )
    return
  }
  window.alert(
    `事件已发布：${eventType.trim()}\n\n已触发工作流：\n  • ${res.matched.join('\n  • ')}\n\n` +
    `切到「执行历史」Tab 查看运行状态。`,
  )
}

function exportToYaml() {
  store.setActiveTab('yaml')
}

const title = computed(() => {
  if (!editing.value) return '画布（空）'
  return editingIsNew.value
    ? `新建工作流：${editing.value.name || '(unnamed)'}`
    : `编辑：${editing.value.name}`
})

const stats = computed(() => {
  if (!editing.value) return { nodes: 0, edges: 0, triggers: 0 }
  return {
    nodes: editing.value.nodes.length,
    edges: editing.value.edges.length,
    triggers: editing.value.triggers.length,
  }
})
</script>

<template>
  <div class="wf-canvas">
    <div class="canvas-meta-bar">
      <div class="meta-left">
        <h3>{{ title }}</h3>
        <div class="stats">
          <span class="stat">节点 {{ stats.nodes }}</span>
          <span class="stat">边 {{ stats.edges }}</span>
          <span class="stat">触发器 {{ stats.triggers }}</span>
          <span v-if="editingDirty" class="stat dirty" title="有未保存的修改">● 未保存</span>
        </div>
      </div>
      <div class="meta-right">
        <button class="btn btn-small" @click="store.startNewWorkflow()" title="新建空白工作流（保留当前画布）">+ 新建</button>
        <button
          class="btn btn-small"
          @click="clearCanvas()"
          :disabled="!editing || stats.nodes === 0"
          title="清空当前画布上的节点和边"
        >🗑 清空</button>
        <button class="btn btn-small" @click="discardAndClose()" :disabled="!editing" title="丢弃编辑并返回列表">✕ 丢弃</button>
        <button class="btn btn-small" @click="fitViewNow()" :disabled="!editing" title="适应窗口">⤢ 适应</button>
        <button class="btn btn-small" @click="exportToYaml()" :disabled="!editing">📝 YAML</button>
        <button class="btn btn-small" @click="validateWorkflow()" :disabled="!editing">🔍 校验</button>
        <button
          class="btn btn-small"
          @click="simulateEvent()"
          title="手动发布事件到 EventDispatcher，用于测试 event 触发器"
        >⚡ 模拟事件</button>
        <button class="btn btn-small btn-primary" @click="saveWorkflow()" :disabled="!editing">💾 保存</button>
      </div>
    </div>

    <div class="canvas-body">
      <div class="palette">
        <div class="palette-header">
          <div class="palette-title">节点库</div>
          <div class="palette-hint">点击或拖拽到画布</div>
        </div>
        <input
          v-model="paletteQuery"
          class="palette-search"
          placeholder="搜索节点..."
        />
        <div class="palette-categories">
          <button
            class="cat-btn"
            :class="{ active: paletteCategory === 'all' }"
            @click="paletteCategory = 'all'"
          >全部</button>
          <button
            v-for="cat in (['ai', 'control', 'basic'] as NodeCategory[])"
            :key="cat"
            class="cat-btn"
            :class="{ active: paletteCategory === cat }"
            @click="paletteCategory = cat"
          >{{ cat === 'ai' ? 'AI' : cat === 'control' ? '控制' : '基础' }}</button>
        </div>
        <div class="palette-list">
          <div
            v-for="entry in paletteEntries"
            :key="entry.type"
            class="palette-item"
            :class="`cat-${entry.category}`"
            draggable="true"
            @dragstart="(ev) => onPaletteDragStart(ev, entry)"
            @dragend="onPaletteDragEnd"
            @click="addNodeFromPalette(entry)"
            :title="entry.description"
          >
            <svg class="palette-icon" width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <path :d="entry.icon" />
            </svg>
            <div class="palette-text">
              <div class="palette-label">{{ entry.label }}</div>
              <div class="palette-desc">{{ entry.description }}</div>
            </div>
          </div>
          <div v-if="paletteEntries.length === 0" class="palette-empty">
            无匹配节点
          </div>
        </div>
      </div>

      <div
        class="flow-container"
        @dragover="onFlowDragOver"
        @drop="onFlowDrop"
      >
        <VueFlow
          v-model:nodes="nodes"
          v-model:edges="edges"
          :min-zoom="0.2"
          :max-zoom="2"
          :default-viewport="{ x: 0, y: 0, zoom: 1 }"
          fit-view-on-init
          :nodes-draggable="true"
          :nodes-connectable="true"
          :elements-selectable="true"
          :pan-on-drag="true"
          :zoom-on-scroll="true"
          :zoom-on-pinch="true"
          :delete-key-code="'Delete'"
          class="flow"
          @node-double-click="handleNodeDoubleClick"
          @node-click="handleNodeClick"
          @pane-click="handlePaneClick"
        >
          <template #node-workflow="props">
            <div
              class="wf-node"
              :class="[
                `cat-${(props.data as WfNodeData).category}`,
                selectedNodeId === props.id ? 'selected' : '',
                runNodeStates[props.id] ? `run-${runNodeStates[props.id].toLowerCase()}` : '',
              ]"
            >
              <div class="wf-node-handle left">
                <Handle type="target" :position="Position.Left" />
              </div>
              <div class="wf-node-body">
                <div class="wf-node-icon">
                  <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                    <path :d="NODE_CATALOG.find(e => e.type === (props.data as WfNodeData).nodeType)?.icon ?? ''" />
                  </svg>
                </div>
                <div class="wf-node-text">
                  <div class="wf-node-type">{{ (props.data as WfNodeData).nodeType }}</div>
                  <div class="wf-node-label">{{ (props.data as WfNodeData).label }}</div>
                </div>
                <div v-if="runNodeStates[props.id]" class="wf-node-state">
                  {{ runNodeStates[props.id] }}
                </div>
              </div>
              <div class="wf-node-handle right">
                <Handle type="source" :position="Position.Right" />
              </div>
            </div>
          </template>

          <Background pattern-color="#aaa" :gap="16" />
          <Controls />
          <MiniMap pannable zoomable />
        </VueFlow>

        <div v-if="!editing" class="canvas-empty">
          <div class="empty-icon">🎯</div>
          <div class="empty-text">画布为空</div>
          <div class="empty-hint">从「工作流列表」选择编辑，或点击「新建」开始</div>
          <div class="empty-actions">
            <button class="btn btn-primary" @click="store.startNewWorkflow()">+ 新建空白工作流</button>
            <button class="btn" @click="store.setActiveTab('list')">查看列表</button>
          </div>
        </div>

        <div v-else class="canvas-hint">
          点击节点 = 选中 · 双击节点 = 编辑属性 · 拖拽节点 = 移动 · 拖拽端口 = 建立边 · Delete = 删除选中
        </div>
      </div>

      <WorkflowNodeConfig
        v-if="showConfigPanel && selectedNode"
        :node="selectedNode"
        @update="(patch) => updateNodeDef(selectedNode!.id, patch)"
        @close="showConfigPanel = false"
      />
    </div>

    <div v-if="validationErrors.length > 0" class="validation-errors">
      <strong>校验错误：</strong>
      <ul>
        <li v-for="(err, i) in validationErrors" :key="i">{{ err }}</li>
      </ul>
    </div>
  </div>
</template>

<style scoped>
.wf-canvas {
  display: flex;
  flex-direction: column;
  height: 100%;
  background: var(--bg-primary);
  overflow: hidden;
}

.canvas-meta-bar {
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: var(--space-2) var(--space-3);
  border-bottom: 1px solid var(--border);
  background: var(--bg-secondary);
  gap: var(--space-3);
  flex-wrap: wrap;
}

.meta-left {
  display: flex;
  flex-direction: column;
  gap: var(--space-1);
}

.meta-left h3 {
  margin: 0;
  font-size: var(--text-base);
}

.stats {
  display: flex;
  gap: var(--space-3);
  font-size: var(--text-xs);
  color: var(--text-muted);
}

.stat.dirty {
  color: var(--warning, #f39c12);
  font-weight: 600;
}

.meta-right {
  display: flex;
  gap: var(--space-1);
  flex-wrap: wrap;
}

.canvas-body {
  flex: 1;
  display: flex;
  overflow: hidden;
}

.palette {
  width: 240px;
  border-right: 1px solid var(--border);
  background: var(--bg-secondary);
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

.palette-header {
  padding: var(--space-2);
  display: flex;
  flex-direction: column;
  gap: 2px;
  border-bottom: 1px solid var(--border);
}

.palette-title {
  font-size: var(--text-xs);
  font-weight: 600;
  color: var(--text-secondary);
  text-transform: uppercase;
}

.palette-hint {
  font-size: 10px;
  color: var(--text-muted);
}

.palette-search {
  margin: var(--space-2);
  padding: var(--space-1) var(--space-2);
  background: var(--bg-primary);
  border: 1px solid var(--border);
  border-radius: var(--radius-sm);
  color: var(--text-primary);
  font-size: var(--text-xs);
}

.palette-categories {
  display: flex;
  gap: var(--space-1);
  padding: 0 var(--space-2) var(--space-2);
  border-bottom: 1px solid var(--border);
}

.cat-btn {
  flex: 1;
  padding: var(--space-1);
  background: transparent;
  border: 1px solid transparent;
  border-radius: var(--radius-sm);
  color: var(--text-muted);
  font-size: var(--text-xs);
  cursor: pointer;
}

.cat-btn.active {
  background: var(--bg-primary);
  color: var(--accent);
  border-color: var(--border);
}

.palette-list {
  flex: 1;
  overflow-y: auto;
  padding: var(--space-1);
}

.palette-item {
  display: flex;
  align-items: flex-start;
  gap: var(--space-2);
  padding: var(--space-2);
  margin-bottom: var(--space-1);
  border-radius: var(--radius-sm);
  cursor: grab;
  border-left: 3px solid transparent;
  background: var(--bg-primary);
  transition: background var(--duration-fast), transform var(--duration-fast);
  user-select: none;
}

.palette-item:hover {
  background: var(--bg-tertiary, var(--bg-primary));
  transform: translateX(2px);
}

.palette-item:active {
  cursor: grabbing;
}

.palette-item.cat-ai { border-left-color: var(--info, #3498db); }
.palette-item.cat-control { border-left-color: var(--warning, #f39c12); }
.palette-item.cat-basic { border-left-color: var(--success, #2ecc71); }

.palette-icon {
  flex-shrink: 0;
  margin-top: 2px;
  opacity: 0.7;
}

.palette-item.cat-ai .palette-icon { color: var(--info, #3498db); }
.palette-item.cat-control .palette-icon { color: var(--warning, #f39c12); }
.palette-item.cat-basic .palette-icon { color: var(--success, #2ecc71); }

.palette-text {
  flex: 1;
  min-width: 0;
}

.palette-label {
  font-size: var(--text-sm);
  font-weight: 500;
  color: var(--text-primary);
}

.palette-desc {
  font-size: var(--text-xs);
  color: var(--text-muted);
  margin-top: 2px;
  line-height: 1.3;
}

.palette-empty {
  padding: var(--space-3);
  text-align: center;
  font-size: var(--text-xs);
  color: var(--text-muted);
}

.flow-container {
  flex: 1;
  position: relative;
  display: flex;
  min-width: 0;
}

.flow {
  flex: 1;
  background: var(--bg-primary);
}

.canvas-empty {
  position: absolute;
  inset: 0;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: var(--space-2);
  color: var(--text-muted);
  background: var(--bg-primary);
  pointer-events: none;
}

.canvas-empty .empty-actions {
  margin-top: var(--space-3);
  display: flex;
  gap: var(--space-2);
  pointer-events: auto;
}

.empty-icon {
  font-size: 56px;
  opacity: 0.4;
}

.empty-text {
  font-size: var(--text-lg);
  font-weight: 500;
}

.empty-hint {
  font-size: var(--text-xs);
}

.canvas-hint {
  position: absolute;
  bottom: var(--space-2);
  left: 50%;
  transform: translateX(-50%);
  font-size: var(--text-xs);
  color: var(--text-muted);
  background: var(--bg-secondary);
  padding: var(--space-1) var(--space-3);
  border-radius: var(--radius-sm);
  pointer-events: none;
  white-space: nowrap;
  max-width: 90%;
  overflow: hidden;
  text-overflow: ellipsis;
}

.validation-errors {
  padding: var(--space-2) var(--space-3);
  background: rgba(231, 76, 60, 0.1);
  border-top: 1px solid var(--danger, #e74c3c);
  font-size: var(--text-sm);
}

.validation-errors ul {
  margin: var(--space-1) 0 0 var(--space-4);
  padding: 0;
}

/* ----- Vue Flow custom node visual ----- */
:deep(.vue-flow__node-workflow) {
  padding: 0;
  border: none;
  background: transparent;
  width: auto;
  font-size: inherit;
}

.wf-node {
  display: flex;
  align-items: stretch;
  background: var(--bg-primary);
  border: 2px solid var(--border);
  border-radius: var(--radius-md);
  min-width: 180px;
  box-shadow: 0 2px 6px rgba(0, 0, 0, 0.08);
  transition: border-color var(--duration-fast), box-shadow var(--duration-fast);
  cursor: grab;
}

.wf-node:active {
  cursor: grabbing;
}

.wf-node.cat-ai { border-color: var(--info, #3498db); }
.wf-node.cat-control { border-color: var(--warning, #f39c12); }
.wf-node.cat-basic { border-color: var(--success, #2ecc71); }

.wf-node.selected {
  border-color: var(--accent);
  box-shadow: 0 0 0 3px color-mix(in srgb, var(--accent) 30%, transparent);
}

.wf-node.run-completed { background: rgba(46, 204, 113, 0.15); }
.wf-node.Running, .wf-node.run-running {
  background: rgba(52, 152, 219, 0.15);
  animation: wf-pulse 1.5s infinite;
}
.wf-node.run-failed { background: rgba(231, 76, 60, 0.15); }

@keyframes wf-pulse {
  0%, 100% { opacity: 1; }
  50% { opacity: 0.6; }
}

.wf-node-body {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  padding: var(--space-2) var(--space-3);
  flex: 1;
}

.wf-node-icon {
  flex-shrink: 0;
  color: var(--text-secondary);
}

.wf-node.cat-ai .wf-node-icon { color: var(--info, #3498db); }
.wf-node.cat-control .wf-node-icon { color: var(--warning, #f39c12); }
.wf-node.cat-basic .wf-node-icon { color: var(--success, #2ecc71); }

.wf-node-text {
  flex: 1;
  min-width: 0;
}

.wf-node-type {
  font-size: var(--text-xs);
  color: var(--text-muted);
  font-family: monospace;
}

.wf-node-label {
  font-size: var(--text-sm);
  font-weight: 500;
  color: var(--text-primary);
}

.wf-node-state {
  font-size: var(--text-xs);
  padding: 1px var(--space-1);
  background: var(--bg-secondary);
  border-radius: var(--radius-sm);
  color: var(--text-secondary);
}

/* Hide the floating handles from Vue Flow's default theme — they overlap with
   our own positioned ones inside .wf-node-handle. */
:deep(.vue-flow__handle) {
  width: 10px;
  height: 10px;
  background: var(--accent);
  border: 2px solid var(--bg-primary);
  border-radius: 50%;
  transition: transform var(--duration-fast);
}

:deep(.vue-flow__handle:hover) {
  transform: scale(1.4);
}

.btn-small {
  padding: 2px var(--space-2);
  font-size: var(--text-xs);
}

.btn-small:disabled {
  opacity: 0.4;
  cursor: not-allowed;
}
</style>
