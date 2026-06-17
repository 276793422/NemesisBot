<script setup lang="ts">
import { ref, computed, watch } from 'vue'
import { formatTime, formatRelative, type SessionEntry, type LlmRequestEntry } from './mockData'

const props = defineProps<{
  sessions: SessionEntry[]
  selectedId: string | null
  requests: LlmRequestEntry[]
}>()

const emit = defineEmits<{
  (e: 'select', id: string): void
  (e: 'view-requests', sessionId: string, requestIds: string[]): void
}>()

const search = ref('')
const timeFilter = ref('all')

const filtered = computed(() => {
  let result = props.sessions
  if (search.value) {
    const k = search.value.toLowerCase()
    result = result.filter(s =>
      s.id.toLowerCase().includes(k) ||
      s.firstMessage.toLowerCase().includes(k) ||
      s.channel.toLowerCase().includes(k)
    )
  }
  return result
})

const selected = computed(() => {
  return props.sessions.find(s => s.id === props.selectedId) || filtered.value[0]
})

// 找出 session 关联的 request
const relatedRequests = computed(() => {
  if (!selected.value) return []
  return props.requests.filter(r => r.sessionId === selected.value.id)
})

const channelColors: Record<string, string> = {
  web: '#3b82f6',
  discord: '#5865f2',
  telegram: '#0088cc',
  feishu: '#ff6b35',
}

function viewRequests() {
  if (!selected.value) return
  emit('view-requests', selected.value.id, relatedRequests.value.map(r => r.id))
}

// Auto-load messages when a session becomes selected (either by user click or
// by the auto-select fallback). The list API returns entries with messages=[],
// so without this the detail panel shows up empty until the user manually clicks.
watch(
  () => selected.value?.id,
  (newId, oldId) => {
    if (newId && newId !== oldId && (selected.value?.messages?.length ?? 0) === 0) {
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
          placeholder="搜索 session 或内容..."
          v-model="search"
        >
        <select class="form-select" v-model="timeFilter">
          <option value="all">全部时间</option>
          <option value="today">今天</option>
          <option value="week">本周</option>
        </select>
      </div>

      <div class="list-items scrollable">
        <div
          v-for="s in filtered"
          :key="s.id"
          class="list-item"
          :class="{ active: selected && s.id === selected.id }"
          @click="emit('select', s.id)"
        >
          <div class="item-header">
            <span class="channel-tag" :style="{ background: channelColors[s.channel] || '#6b7280' }">
              {{ s.channel }}
            </span>
            <span class="item-time">{{ formatRelative(s.lastTime) }}</span>
          </div>
          <div class="item-preview">{{ s.firstMessage }}</div>
          <div class="item-meta">
            <span>💬 {{ s.messageCount }} 条</span>
            <span>🤖 {{ s.model }}</span>
            <span v-if="s.triggerCluster">⚠️ 集群</span>
          </div>
          <div class="item-id">{{ s.id }}</div>
        </div>
        <div v-if="filtered.length === 0" class="empty-state">
          <p>暂无对话</p>
        </div>
      </div>
    </div>

    <!-- 右：详情 -->
    <div class="explorer-detail">
      <template v-if="selected">
        <!-- 顶部信息条 -->
        <div class="detail-header">
          <div class="detail-meta">
            <span class="channel-tag" :style="{ background: channelColors[selected.channel] || '#6b7280' }">
              {{ selected.channel }}
            </span>
            <span>{{ selected.messageCount }} 条消息</span>
            <span>首次 {{ formatTime(selected.startTime) }}</span>
            <span>最后 {{ formatRelative(selected.lastTime) }}</span>
          </div>
          <div class="detail-actions">
            <button
              class="btn btn-sm btn-primary"
              :disabled="relatedRequests.length === 0"
              @click="viewRequests"
            >
              🔍 查看 {{ relatedRequests.length }} 次调用
            </button>
            <button class="btn btn-sm btn-ghost">⤓ 导出</button>
          </div>
        </div>

        <!-- 对话气泡 -->
        <div class="chat-stream scrollable">
          <div
            v-for="(msg, idx) in selected.messages"
            :key="idx"
            class="chat-msg"
            :class="msg.role"
          >
            <div class="msg-bubble">
              <div class="msg-content">{{ msg.content }}</div>
              <div class="msg-meta">
                <span class="msg-role">{{ msg.role === 'user' ? '👤 user' : '🤖 assistant' }}</span>
                <span class="msg-time">{{ formatTime(msg.timestamp) }}</span>
                <button
                  v-if="msg.role === 'assistant' && msg.toolCalls && msg.toolCalls > 0"
                  class="msg-jump"
                  @click="viewRequests"
                >🔍 查看 {{ msg.toolCalls }} 次调用</button>
                <span v-if="msg.triggerCluster" class="msg-tag">⚠️ 触发集群</span>
              </div>
            </div>
          </div>
        </div>
      </template>
      <div v-else class="empty-state">
        <h3>选择一个会话</h3>
        <p>从左侧列表选择会话查看详情</p>
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

.list-items {
  flex: 1;
}

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

.channel-tag {
  padding: 2px 8px;
  color: white;
  border-radius: var(--radius-sm);
  font-size: var(--text-xs);
  font-weight: 600;
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
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
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
}

.detail-meta {
  display: flex;
  gap: var(--space-3);
  align-items: center;
  font-size: var(--text-sm);
  color: var(--text-muted);
}

.detail-actions {
  display: flex;
  gap: var(--space-2);
}

.chat-stream {
  flex: 1;
  padding: var(--space-4);
  overflow-y: auto;
}

.chat-msg {
  margin-bottom: var(--space-3);
  display: flex;
}

.chat-msg.user { justify-content: flex-end; }
.chat-msg.assistant { justify-content: flex-start; }

.msg-bubble {
  max-width: 80%;
  padding: var(--space-3) var(--space-4);
  border-radius: var(--radius-lg);
  background: var(--bg-secondary);
}

.chat-msg.user .msg-bubble {
  background: var(--accent);
  color: white;
}

.msg-content {
  font-size: var(--text-sm);
  line-height: 1.5;
  margin-bottom: var(--space-2);
  word-break: break-word;
}

.msg-meta {
  display: flex;
  gap: var(--space-3);
  font-size: var(--text-xs);
  opacity: 0.8;
  align-items: center;
}

.msg-jump {
  padding: 2px 8px;
  background: rgba(255,255,255,0.2);
  border: none;
  color: inherit;
  border-radius: var(--radius-sm);
  cursor: pointer;
  font-size: var(--text-xs);
}

.chat-msg.assistant .msg-jump {
  background: var(--accent-muted);
  color: var(--accent);
}

.msg-jump:hover { opacity: 0.85; }

.msg-tag {
  padding: 2px 6px;
  background: rgba(239, 68, 68, 0.2);
  color: #ef4444;
  border-radius: var(--radius-sm);
}

.scrollable { overflow-y: auto; }
</style>
