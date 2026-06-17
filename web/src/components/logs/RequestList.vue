<script setup lang="ts">
import { ref, computed, watch } from 'vue'
import {
  formatTime, formatRelative,
  type LlmRequestEntry, type SessionEntry, type ClusterTaskEntry,
} from './mockData'

const props = defineProps<{
  requests: LlmRequestEntry[]
  selectedId: string | null
  sessions: SessionEntry[]
  tasks: ClusterTaskEntry[]
}>()

const emit = defineEmits<{
  (e: 'select', id: string): void
  (e: 'view-session', sessionId: string): void
  (e: 'view-task', taskId: string): void
}>()

const search = ref('')
const modelFilter = ref('all')
const expandedIterations = ref<Set<string>>(new Set())

const filtered = computed(() => {
  let result = props.requests
  if (search.value) {
    const k = search.value.toLowerCase()
    result = result.filter(r =>
      r.id.toLowerCase().includes(k) ||
      r.firstMessage.toLowerCase().includes(k)
    )
  }
  if (modelFilter.value !== 'all') {
    result = result.filter(r => r.model === modelFilter.value)
  }
  return result
})

const selected = computed(() => {
  return props.requests.find(r => r.id === props.selectedId) || filtered.value[0]
})

const relatedSession = computed(() => {
  if (!selected.value?.sessionId) return null
  return props.sessions.find(s => s.id === selected.value!.sessionId) || null
})

const relatedTask = computed(() => {
  if (!selected.value?.clusterTaskId) return null
  return props.tasks.find(t => t.id === selected.value!.clusterTaskId) || null
})

function toggleIteration(key: string) {
  if (expandedIterations.value.has(key)) expandedIterations.value.delete(key)
  else expandedIterations.value.add(key)
  expandedIterations.value = new Set(expandedIterations.value)
}

// Auto-load detail when a request becomes selected (either by user click or by
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
          placeholder="搜索内容或 trace_id..."
          v-model="search"
        >
        <select class="form-select" v-model="modelFilter">
          <option value="all">全部模型</option>
          <option value="glm-4.7">glm-4.7</option>
          <option value="claude-sonnet-4-5">claude-sonnet-4-5</option>
        </select>
      </div>

      <div class="list-items scrollable">
        <div
          v-for="r in filtered"
          :key="r.id"
          class="list-item"
          :class="{ active: selected && r.id === selected.id }"
          @click="emit('select', r.id)"
        >
          <div class="item-header">
            <span class="item-time">{{ formatRelative(r.timestamp) }}</span>
            <span class="item-duration">{{ (r.duration_ms / 1000).toFixed(1) }}s</span>
          </div>
          <div class="item-preview">{{ r.firstMessage }}</div>
          <div class="item-meta">
            <span>🤖 {{ r.model }}</span>
            <span>🔧 {{ r.toolCallCount }} 工具</span>
            <span>💬 {{ r.messageCount }} 条</span>
          </div>
          <div class="item-id">{{ r.id }}</div>
        </div>
        <div v-if="filtered.length === 0" class="empty-state">
          <p>暂无调用记录</p>
        </div>
      </div>
    </div>

    <!-- 右：详情 -->
    <div class="explorer-detail">
      <template v-if="selected">
        <!-- 顶部关联条 -->
        <div class="detail-header">
          <div class="detail-meta">
            <span>模型 {{ selected.model }}</span>
            <span>耗时 {{ (selected.duration_ms / 1000).toFixed(2) }}s</span>
            <span>工具 {{ selected.toolCallCount }} 次</span>
            <span>消息 {{ selected.messageCount }} 条</span>
          </div>
          <div class="detail-actions">
            <button
              v-if="relatedSession"
              class="btn btn-sm btn-ghost"
              @click="emit('view-session', relatedSession.id)"
            >⬅ 所属会话：{{ relatedSession.id }}</button>
            <button
              v-if="relatedTask"
              class="btn btn-sm btn-primary"
              @click="emit('view-task', relatedTask.id)"
            >🌐 关联集群任务: {{ relatedTask.id }}</button>
          </div>
        </div>

        <!-- 迭代列表 -->
        <div class="iterations scrollable">
          <div
            v-for="iter in selected.iterations"
            :key="`${selected.id}-${iter.index}`"
            class="iteration-card"
          >
            <div
              class="iteration-header"
              @click="toggleIteration(`${selected.id}-${iter.index}`)"
            >
              <span class="iteration-index">迭代 {{ iter.index }}</span>
              <span class="iteration-summary">
                <template v-if="iter.response.content">
                  {{ iter.response.content.slice(0, 60) }}{{ iter.response.content.length > 60 ? '...' : '' }}
                </template>
                <template v-else-if="iter.response.toolCalls?.length">
                  🔧 {{ iter.response.toolCalls.length }} 个工具调用
                </template>
                <template v-else>（无内容）</template>
              </span>
              <span class="iteration-duration">{{ iter.response.duration_ms }}ms</span>
              <span class="iteration-toggle">
                {{ expandedIterations.has(`${selected.id}-${iter.index}`) ? '▼' : '▶' }}
              </span>
            </div>

            <div v-if="expandedIterations.has(`${selected.id}-${iter.index}`)" class="iteration-body">
              <!-- Request -->
              <div class="iter-section">
                <div class="iter-section-title">📥 Request</div>
                <div class="iter-section-body">
                  <div class="msg-line" v-for="(m, mi) in iter.request.messages" :key="mi">
                    <span class="msg-role-tag">{{ m.role }}</span>
                    <span class="msg-text">{{ m.content }}</span>
                  </div>
                  <div v-if="iter.request.tools" class="iter-tools">
                    <div class="tools-title">Tools:</div>
                    <pre>{{ JSON.stringify(iter.request.tools, null, 2) }}</pre>
                  </div>
                </div>
              </div>

              <!-- Response -->
              <div class="iter-section">
                <div class="iter-section-title">📤 Response</div>
                <div class="iter-section-body">
                  <div class="msg-text">{{ iter.response.content }}</div>
                  <div v-if="iter.response.toolCalls" class="iter-tools">
                    <div class="tools-title">Tool Calls:</div>
                    <pre>{{ JSON.stringify(iter.response.toolCalls, null, 2) }}</pre>
                  </div>
                </div>
              </div>

              <!-- Tool Results -->
              <div v-if="iter.toolResults" class="iter-section">
                <div class="iter-section-title">🔧 Tool Results</div>
                <div class="iter-section-body">
                  <div
                    v-for="(tr, tri) in iter.toolResults"
                    :key="tri"
                    class="tool-result"
                  >
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
        <h3>选择一个调用</h3>
        <p>从左侧列表选择 LLM 调用查看详情</p>
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

.item-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  margin-bottom: var(--space-1);
}

.item-time {
  color: var(--text-muted);
  font-size: var(--text-xs);
}

.item-duration {
  font-size: var(--text-xs);
  color: var(--accent);
  font-weight: 600;
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
}

.item-id {
  font-size: var(--text-xs);
  color: var(--text-muted);
  font-family: monospace;
  margin-top: var(--space-1);
}

.explorer-detail {
  display: flex;
  flex-direction: column;
  background: var(--bg-primary);
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
}

.detail-actions {
  display: flex;
  gap: var(--space-2);
  flex-wrap: wrap;
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
  cursor: pointer;
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

.iteration-toggle {
  color: var(--text-muted);
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

.iter-tools {
  margin-top: var(--space-2);
}

.tools-title {
  font-size: var(--text-xs);
  color: var(--text-muted);
  margin-bottom: var(--space-1);
}

.iter-tools pre, .tool-result pre {
  background: var(--bg-tertiary);
  padding: var(--space-2);
  border-radius: var(--radius-sm);
  font-size: var(--text-xs);
  font-family: 'Cascadia Code', monospace;
  overflow-x: auto;
  margin: 0;
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

.scrollable { overflow-y: auto; }
</style>
