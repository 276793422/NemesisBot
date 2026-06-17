<script setup lang="ts">
import { ref, computed, watch } from 'vue'
import {
  formatTime, formatRelative,
  type ClusterTaskEntry, type LlmRequestEntry,
} from './mockData'

const props = defineProps<{
  tasks: ClusterTaskEntry[]
  selectedId: string | null
  requests: LlmRequestEntry[]
}>()

const emit = defineEmits<{
  (e: 'select', id: string): void
  (e: 'view-request', requestId: string): void
}>()

const search = ref('')
const viewMode = ref<'time' | 'node'>('time')
const nodeFilter = ref('all')
const perspective = ref<'local' | 'remote'>('local')

const filtered = computed(() => {
  let result = props.tasks
  if (search.value) {
    const k = search.value.toLowerCase()
    result = result.filter(t =>
      t.id.toLowerCase().includes(k) ||
      t.firstMessage.toLowerCase().includes(k) ||
      t.peerNode.toLowerCase().includes(k)
    )
  }
  if (nodeFilter.value !== 'all') {
    result = result.filter(t => t.peerNode === nodeFilter.value)
  }
  return result
})

const selected = computed(() => {
  return props.tasks.find(t => t.id === props.selectedId) || filtered.value[0]
})

const relatedRequest = computed(() => {
  if (!selected.value?.relatedRequestId) return null
  return props.requests.find(r => r.id === selected.value!.relatedRequestId) || null
})

// 按节点分组
const groupedByNode = computed(() => {
  const groups: Record<string, ClusterTaskEntry[]> = {}
  filtered.value.forEach(t => {
    if (!groups[t.peerNode]) groups[t.peerNode] = []
    groups[t.peerNode].push(t)
  })
  return Object.entries(groups).sort((a, b) => b[1].length - a[1].length)
})

const allNodes = computed(() => Array.from(new Set(props.tasks.map(t => t.peerNode))).sort())

function statusIcon(s: ClusterTaskEntry['status']): string {
  return s === 'completed' ? '✅' : s === 'failed' ? '❌' : '⏱'
}

function statusLabel(s: ClusterTaskEntry['status']): string {
  return s === 'completed' ? '完成' : s === 'failed' ? '失败' : '超时'
}

// Auto-load detail when a task becomes selected (either by user click or by
// the auto-select fallback). The list API returns entries with iterations=[],
// so without this the detail panel shows up empty until the user manually clicks.
watch(
  () => selected.value?.id,
  (newId, oldId) => {
    if (newId && newId !== oldId && (selected.value?.iterations?.length ?? 0) === 0) {
      emit('select', newId)
    }
  },
  { immediate: true },
)
</script>

<template>
  <div class="explorer-layout">
    <!-- 左：列表 -->
    <div class="explorer-list">
      <div class="list-toolbar">
        <input
          class="form-input"
          type="text"
          placeholder="搜索 task_id 或内容..."
          v-model="search"
        >
        <div class="view-toggle">
          <button
            class="toggle-btn"
            :class="{ active: viewMode === 'time' }"
            @click="viewMode = 'time'"
          >⏱ 时间</button>
          <button
            class="toggle-btn"
            :class="{ active: viewMode === 'node' }"
            @click="viewMode = 'node'"
          >📁 节点</button>
        </div>
      </div>

      <div class="list-toolbar" v-if="viewMode === 'time'">
        <select class="form-select" v-model="nodeFilter">
          <option value="all">全部节点</option>
          <option v-for="n in allNodes" :key="n" :value="n">{{ n }}</option>
        </select>
      </div>

      <div class="list-items scrollable">
        <!-- 时间视图：扁平列表 -->
        <template v-if="viewMode === 'time'">
          <div
            v-for="t in filtered"
            :key="t.id"
            class="list-item"
            :class="{ active: selected && t.id === selected.id }"
            @click="emit('select', t.id)"
          >
            <div class="item-header">
              <span class="direction-tag" :class="t.direction">
                {{ t.direction === 'outbound' ? '📱本机→' + t.peerNode : '📥' + t.peerNode + '→本机' }}
              </span>
              <span class="item-time">{{ formatRelative(t.timestamp) }}</span>
            </div>
            <div class="item-preview">{{ t.firstMessage }}</div>
            <div class="item-meta">
              <span>任务 {{ t.id.slice(0, 8) }}</span>
              <span>🔧 {{ t.toolCallCount }}</span>
              <span>⏱ {{ (t.duration_ms / 1000).toFixed(1) }}s</span>
              <span>{{ statusIcon(t.status) }} {{ statusLabel(t.status) }}</span>
            </div>
          </div>
        </template>

        <!-- 节点视图：树形 -->
        <template v-else>
          <div v-for="[node, tasks] in groupedByNode" :key="node" class="node-group">
            <div class="node-header">
              <span>📁 {{ node }}</span>
              <span class="node-count">{{ tasks.length }} 个任务</span>
            </div>
            <div
              v-for="t in tasks"
              :key="t.id"
              class="list-item node-child"
              :class="{ active: selected && t.id === selected.id }"
              @click="emit('select', t.id)"
            >
              <div class="item-header">
                <span class="direction-tag" :class="t.direction">
                  {{ t.direction === 'outbound' ? '📱 out' : '📥 in' }}
                </span>
                <span class="item-time">{{ formatRelative(t.timestamp) }}</span>
              </div>
              <div class="item-preview">{{ t.firstMessage }}</div>
              <div class="item-meta">
                <span>{{ t.id.slice(0, 8) }}</span>
                <span>{{ statusIcon(t.status) }}</span>
              </div>
            </div>
          </div>
        </template>

        <div v-if="filtered.length === 0" class="empty-state">
          <p>暂无任务</p>
        </div>
      </div>
    </div>

    <!-- 右：详情 -->
    <div class="explorer-detail">
      <template v-if="selected">
        <!-- 视角切换 -->
        <div class="perspective-bar">
          <button
            class="perspective-btn"
            :class="{ active: perspective === 'local' }"
            @click="perspective = 'local'"
          >📱 本机视角</button>
          <button
            class="perspective-btn"
            :class="{ active: perspective === 'remote' }"
            @click="perspective = 'remote'"
            :disabled="!selected.relatedRequestId"
            :title="!selected.relatedRequestId ? '对端日志未同步' : ''"
          >🌐 对端视角 {{ selected.peerNode }}</button>
        </div>

        <!-- 任务信息条 -->
        <div class="detail-header">
          <div class="detail-meta">
            <span>task_id <code>{{ selected.id }}</code></span>
            <span>action <code>{{ selected.action }}</code></span>
            <span>{{ selected.direction === 'outbound' ? '📱 本机 → ' + selected.peerNode : '📥 ' + selected.peerNode + ' → 本机' }}</span>
            <span>⏱ {{ (selected.duration_ms / 1000).toFixed(2) }}s</span>
            <span>{{ statusIcon(selected.status) }} {{ statusLabel(selected.status) }}</span>
          </div>
          <div class="detail-actions">
            <button
              v-if="relatedRequest"
              class="btn btn-sm btn-ghost"
              @click="emit('view-request', relatedRequest.id)"
            >⬅ 关联本地调用</button>
            <button class="btn btn-sm btn-ghost">💬 返回会话</button>
          </div>
        </div>

        <!-- 迭代列表 -->
        <div class="iterations scrollable">
          <div
            v-for="iter in selected.iterations"
            :key="`${selected.id}-${iter.index}`"
            class="iteration-card"
          >
            <div class="iteration-header">
              <span class="iteration-index">迭代 {{ iter.index }}</span>
              <span class="iteration-summary">
                {{ perspective === 'local' ? '本机视角' : '远端视角' }} ·
                <template v-if="iter.response.content">
                  {{ iter.response.content.slice(0, 60) }}{{ iter.response.content.length > 60 ? '...' : '' }}
                </template>
                <template v-else-if="iter.response.toolCalls?.length">
                  🔧 {{ iter.response.toolCalls.length }} 个工具调用
                </template>
                <template v-else>（无内容）</template>
              </span>
              <span class="iteration-duration">{{ iter.response.duration_ms }}ms</span>
            </div>
            <div class="iteration-body">
              <div class="iter-section">
                <div class="iter-section-title">📥 {{ perspective === 'local' ? '本机 Request' : '远端 Request' }}</div>
                <div class="iter-section-body">
                  <div class="msg-line" v-for="(m, mi) in iter.request.messages" :key="mi">
                    <span class="msg-role-tag">{{ m.role }}</span>
                    <span class="msg-text">{{ m.content }}</span>
                  </div>
                </div>
              </div>
              <div class="iter-section">
                <div class="iter-section-title">📤 {{ perspective === 'local' ? '本机 Response' : '远端 Response' }}</div>
                <div class="iter-section-body">
                  <div class="msg-text">{{ iter.response.content }}</div>
                </div>
              </div>
              <div v-if="iter.toolResults" class="iter-section">
                <div class="iter-section-title">🔧 Tool Results</div>
                <div class="iter-section-body">
                  <div v-for="(tr, tri) in iter.toolResults" :key="tri" class="tool-result">
                    <div class="tool-call-id">call: {{ tr.callId }}</div>
                    <pre>{{ JSON.stringify(tr.result, null, 2) }}</pre>
                  </div>
                </div>
              </div>
            </div>
          </div>
        </div>
      </template>
      <div v-else class="empty-state">
        <h3>选择一个集群任务</h3>
        <p>从左侧列表选择任务查看详情</p>
      </div>
    </div>
  </div>
</template>

<style scoped>
.explorer-layout {
  display: grid;
  grid-template-columns: 320px 1fr;
  height: 100%;
}

.explorer-list {
  display: flex;
  flex-direction: column;
  border-right: 1px solid var(--border-light);
  background: var(--bg-secondary);
}

.list-toolbar {
  display: flex;
  gap: var(--space-2);
  padding: var(--space-2) var(--space-3);
  border-bottom: 1px solid var(--border-light);
}

.list-toolbar .form-input { flex: 1; }

.view-toggle {
  display: flex;
  background: var(--bg-tertiary);
  border-radius: var(--radius-md);
  padding: 2px;
}

.toggle-btn {
  padding: 4px 8px;
  border: none;
  background: transparent;
  color: var(--text-muted);
  cursor: pointer;
  border-radius: var(--radius-sm);
  font-size: var(--text-xs);
}

.toggle-btn.active {
  background: var(--bg-primary);
  color: var(--accent);
  font-weight: 600;
}

.list-items { flex: 1; }

.list-item {
  padding: var(--space-3);
  border-bottom: 1px solid var(--border-light);
  cursor: pointer;
  transition: background 0.15s;
}

.list-item:hover { background: var(--bg-hover); }
.list-item.active {
  background: var(--accent-muted);
  border-left: 3px solid var(--accent);
  padding-left: calc(var(--space-3) - 3px);
}

.node-child {
  padding-left: var(--space-6);
}

.node-group { margin-bottom: var(--space-2); }

.node-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: var(--space-2) var(--space-3);
  background: var(--bg-tertiary);
  font-size: var(--text-sm);
  font-weight: 600;
}

.node-count {
  font-size: var(--text-xs);
  color: var(--text-muted);
  font-weight: normal;
}

.item-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  margin-bottom: var(--space-1);
}

.direction-tag {
  padding: 2px 8px;
  border-radius: var(--radius-sm);
  font-size: var(--text-xs);
  font-weight: 600;
}

.direction-tag.outbound {
  background: rgba(59, 130, 246, 0.15);
  color: #3b82f6;
}

.direction-tag.inbound {
  background: rgba(168, 85, 247, 0.15);
  color: #a855f7;
}

.item-time {
  color: var(--text-muted);
  font-size: var(--text-xs);
}

.item-preview {
  color: var(--text-primary);
  font-size: var(--text-sm);
  margin-bottom: var(--space-1);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.item-meta {
  display: flex;
  gap: var(--space-2);
  font-size: var(--text-xs);
  color: var(--text-muted);
  font-family: monospace;
}

.explorer-detail {
  display: flex;
  flex-direction: column;
  background: var(--bg-primary);
}

.perspective-bar {
  display: flex;
  gap: var(--space-1);
  padding: var(--space-2) var(--space-4);
  background: var(--bg-tertiary);
  border-bottom: 1px solid var(--border-light);
}

.perspective-btn {
  padding: 6px 14px;
  border: 1px solid var(--border);
  background: var(--bg-primary);
  color: var(--text-secondary);
  border-radius: var(--radius-md);
  cursor: pointer;
  font-size: var(--text-sm);
}

.perspective-btn.active {
  background: var(--accent);
  color: white;
  border-color: var(--accent);
}

.perspective-btn:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

.detail-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: var(--space-3) var(--space-4);
  border-bottom: 1px solid var(--border-light);
  background: var(--bg-secondary);
  flex-wrap: wrap;
  gap: var(--space-2);
}

.detail-meta {
  display: flex;
  gap: var(--space-3);
  font-size: var(--text-sm);
  color: var(--text-muted);
  flex-wrap: wrap;
  align-items: center;
}

.detail-meta code {
  background: var(--bg-tertiary);
  padding: 1px 6px;
  border-radius: var(--radius-sm);
  font-family: monospace;
  font-size: var(--text-xs);
  color: var(--text-primary);
}

.detail-actions {
  display: flex;
  gap: var(--space-2);
}

.iterations {
  flex: 1;
  padding: var(--space-3) var(--space-4);
}

.iteration-card {
  border: 1px solid var(--border-light);
  border-radius: var(--radius-md);
  margin-bottom: var(--space-3);
  background: var(--bg-secondary);
  overflow: hidden;
}

.iteration-header {
  display: flex;
  align-items: center;
  gap: var(--space-3);
  padding: var(--space-2) var(--space-3);
  background: var(--bg-tertiary);
  font-size: var(--text-sm);
}

.iteration-index {
  font-weight: 600;
  color: var(--accent);
  min-width: 60px;
}

.iteration-summary {
  flex: 1;
  color: var(--text-secondary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.iteration-duration {
  color: var(--text-muted);
  font-size: var(--text-xs);
  font-family: monospace;
}

.iteration-body {
  padding: var(--space-3);
}

.iter-section {
  margin-bottom: var(--space-3);
}

.iter-section-title {
  font-size: var(--text-xs);
  font-weight: 600;
  color: var(--text-muted);
  text-transform: uppercase;
  margin-bottom: var(--space-1);
}

.iter-section-body {
  background: var(--bg-primary);
  padding: var(--space-2) var(--space-3);
  border-radius: var(--radius-sm);
  border-left: 3px solid var(--accent-muted);
}

.msg-line {
  display: flex;
  gap: var(--space-2);
  margin-bottom: var(--space-1);
  font-size: var(--text-sm);
}

.msg-role-tag {
  display: inline-block;
  padding: 1px 6px;
  background: var(--bg-tertiary);
  color: var(--text-muted);
  border-radius: var(--radius-sm);
  font-size: var(--text-xs);
  font-weight: 600;
  min-width: 60px;
  text-align: center;
}

.msg-text {
  color: var(--text-primary);
  word-break: break-word;
}

.tool-result {
  margin-top: var(--space-2);
}

.tool-call-id {
  font-size: var(--text-xs);
  color: var(--text-muted);
  font-family: monospace;
  margin-bottom: var(--space-1);
}

.tool-result pre {
  background: var(--bg-tertiary);
  padding: var(--space-2);
  border-radius: var(--radius-sm);
  font-size: var(--text-xs);
  font-family: 'Cascadia Code', monospace;
  overflow-x: auto;
  margin: 0;
}

.scrollable { overflow-y: auto; }
</style>
