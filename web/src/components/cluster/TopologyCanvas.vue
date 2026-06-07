<script setup lang="ts">
import { computed } from 'vue'

const props = defineProps<{
  nodes: Array<{
    id: string
    name: string
    role: string
    online: boolean
    x?: number
    y?: number
  }>
  connections: Array<{ from: string; to: string; active: boolean }>
  traces: Array<{
    id: string
    hops: Array<{ node: string; duration: string }>
    failed: boolean
  }>
}>()

const emit = defineEmits<{
  (e: 'selectNode', id: string): void
}>()

const viewBox = computed(() => {
  const w = Math.max(400, props.nodes.length * 200)
  return `0 0 ${w} 300`
})

function nodePos(index: number, total: number) {
  if (index < 0 || total <= 0) return { x: 0, y: 0 }
  if (total <= 1) return { x: w() / 2, y: 150 }
  const cx = w() / 2
  const cy = 150
  const r = Math.min(120, w() / 3)
  const angle = (2 * Math.PI * index) / total - Math.PI / 2
  return { x: cx + r * Math.cos(angle), y: cy + r * Math.sin(angle) }
}

function w() { return Math.max(400, props.nodes.length * 200) }

function connPath(fromIdx: number, toIdx: number, total: number) {
  const a = nodePos(fromIdx, total)
  const b = nodePos(toIdx, total)
  return `M${a.x},${a.y} L${b.x},${b.y}`
}

function nodeColor(role: string) {
  const map: Record<string, string> = { manager: '#3b82f6', coordinator: '#8b5cf6', worker: '#22c55e', observer: '#6b7280', standby: '#f59e0b' }
  return map[role] || '#6b7280'
}

const nodeIds = computed(() => new Set(props.nodes.map(n => n.id)))
const validConnections = computed(() => props.connections.filter(c => nodeIds.value.has(c.from) && nodeIds.value.has(c.to)))
</script>

<template>
  <div class="topology-wrapper">
    <svg v-if="nodes.length" :viewBox="viewBox" class="topology-svg">
      <!-- Connections -->
      <path
        v-for="(conn, i) in validConnections"
        :key="'c' + i"
        :d="connPath(nodes.findIndex(n => n.id === conn.from), nodes.findIndex(n => n.id === conn.to), nodes.length)"
        :stroke="conn.active ? 'var(--accent)' : 'var(--border)'"
        :stroke-width="conn.active ? 2 : 1"
        :stroke-dasharray="conn.active ? 'none' : '4 4'"
        fill="none"
      />

      <!-- Nodes -->
      <g
        v-for="(node, i) in nodes"
        :key="node.id"
        :transform="`translate(${nodePos(i, nodes.length).x}, ${nodePos(i, nodes.length).y})`"
        class="topo-node"
        @click="emit('selectNode', node.id)"
      >
        <circle r="28" :fill="node.online ? nodeColor(node.role) : 'var(--border)'" :opacity="node.online ? 1 : 0.5" />
        <text y="4" text-anchor="middle" fill="white" font-size="10" font-weight="600">{{ node.name.replace('Node-', 'N') }}</text>
        <text y="44" text-anchor="middle" fill="var(--text-muted)" font-size="9">{{ node.role }}</text>
      </g>
    </svg>

    <!-- Empty state -->
    <div v-else class="empty-state" style="padding:var(--space-8)">
      <p>暂无节点数据</p>
    </div>

    <!-- Legend -->
    <div v-if="nodes.length" class="topo-legend">
      <span><span style="display:inline-block;width:20px;height:2px;background:var(--accent);vertical-align:middle"></span> 通信中</span>
      <span><span style="display:inline-block;width:20px;height:2px;background:var(--border);vertical-align:middle;border-top:1px dashed var(--text-muted)"></span> 已断开</span>
    </div>

    <!-- RPC Traces -->
    <div v-if="traces.length" style="margin-top:var(--space-4)">
      <h4 style="margin-bottom:var(--space-2)">RPC 链路追踪</h4>
      <div v-for="trace in traces" :key="trace.id" class="trace-row">
        <span class="trace-id">#{{ trace.id }}</span>
        <span class="trace-hops">
          <template v-for="(hop, hi) in trace.hops" :key="hi">
            <span class="hop-node">{{ hop.node }}</span>
            <span v-if="hi < trace.hops.length - 1" class="hop-arrow">→</span>
          </template>
        </span>
        <span class="badge" :class="trace.failed ? 'badge-error' : 'badge-success'">{{ trace.failed ? '失败' : '完成' }}</span>
      </div>
    </div>
  </div>
</template>

<style scoped>
.topology-wrapper { padding: var(--space-2); }
.topology-svg { width: 100%; height: 280px; }
.topo-node { cursor: pointer; }
.topo-node:hover circle { opacity: 0.8; }
.topo-legend { display: flex; gap: var(--space-4); font-size: var(--text-xs); color: var(--text-muted); margin-top: var(--space-2); }
.trace-row {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  padding: var(--space-1) 0;
  font-size: var(--text-sm);
  border-bottom: 1px solid var(--border);
}
.trace-id { font-family: var(--font-mono); font-weight: 600; min-width: 40px; }
.trace-hops { flex: 1; display: flex; align-items: center; gap: var(--space-1); }
.hop-node { color: var(--accent); }
.hop-arrow { color: var(--text-muted); }
</style>
