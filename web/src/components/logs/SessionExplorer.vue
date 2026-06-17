<script setup lang="ts">
import { ref, computed, watch } from 'vue'
import SessionList from './SessionList.vue'
import RequestList from './RequestList.vue'
import ClusterTaskList from './ClusterTaskList.vue'
import { useWSAPI } from '../../composables/useWSAPI'
import type { SessionEntry, LlmRequestEntry, ClusterTaskEntry, SessionMessage, LlmIteration } from './mockData'

const props = defineProps<{
  sessions: SessionEntry[]
  requests: LlmRequestEntry[]
  tasks: ClusterTaskEntry[]
}>()

const emit = defineEmits<{
  (e: 'navigate', target: { type: 'request' | 'task' | 'session'; id: string }): void
  (e: 'reload'): void
}>()

const { request } = useWSAPI()

const subTab = ref<'sessions' | 'requests' | 'tasks'>('sessions')

const selectedRequestId = ref<string | null>(null)
const selectedTaskId = ref<string | null>(null)
const selectedSessionId = ref<string | null>(null)

// Lazy-loaded detail data (keyed by id)
const sessionMessages = ref<Record<string, SessionMessage[]>>({})
const requestIterations = ref<Record<string, LlmIteration[]>>({})
const taskIterations = ref<Record<string, LlmIteration[]>>({})

const loadingDetail = ref<string | null>(null)

async function selectSession(id: string) {
  selectedSessionId.value = id
  if (sessionMessages.value[id]) return
  loadingDetail.value = id
  try {
    const res = await request('logs', 'session_detail', { session: id })
    sessionMessages.value[id] = res?.messages ?? []
    // Inject messages into the session entry so the list/detail panel sees them.
    const target = props.sessions.find(s => s.id === id)
    if (target) target.messages = res?.messages ?? []
  } catch (e) {
    console.error('[SessionExplorer] session_detail failed', e)
  } finally {
    loadingDetail.value = null
  }
}

async function selectRequest(id: string) {
  selectedRequestId.value = id
  if (requestIterations.value[id]) return
  loadingDetail.value = id
  try {
    const res = await request('logs', 'request_detail', { id })
    const iterations = res?.iterations ?? []
    requestIterations.value[id] = iterations
    const target = props.requests.find(r => r.id === id)
    if (target) target.iterations = iterations
  } catch (e) {
    console.error('[SessionExplorer] request_detail failed', e)
  } finally {
    loadingDetail.value = null
  }
}

async function selectTask(id: string) {
  selectedTaskId.value = id
  if (taskIterations.value[id]) return
  loadingDetail.value = id
  try {
    const res = await request('logs', 'cluster_task_detail', { task_id: id })
    const iterations = res?.iterations ?? []
    taskIterations.value[id] = iterations
    const target = props.tasks.find(t => t.id === id)
    if (target) target.iterations = iterations
  } catch (e) {
    console.error('[SessionExplorer] cluster_task_detail failed', e)
  } finally {
    loadingDetail.value = null
  }
}

function viewSessionRequests(sessionId: string, requestIds: string[]) {
  subTab.value = 'requests'
  if (requestIds[0]) selectRequest(requestIds[0])
  emit('navigate', { type: 'request', id: requestIds[0] ?? '' })
}

function viewRequestSession(sessionId: string) {
  subTab.value = 'sessions'
  selectSession(sessionId)
  emit('navigate', { type: 'session', id: sessionId })
}

function viewRequestTask(taskId: string) {
  subTab.value = 'tasks'
  selectTask(taskId)
  emit('navigate', { type: 'task', id: taskId })
}

function viewTaskRequest(requestId: string) {
  subTab.value = 'requests'
  selectRequest(requestId)
  emit('navigate', { type: 'request', id: requestId })
}

const counts = computed(() => ({
  sessions: props.sessions.length,
  requests: props.requests.length,
  tasks: props.tasks.length,
}))

const subTabs = [
  { id: 'sessions', label: '对话历史', icon: '💬', key: 'sessions' as const },
  { id: 'requests', label: '本地 LLM 调用', icon: '🤖', key: 'requests' as const },
  { id: 'tasks',    label: '集群 RPC 任务', icon: '🌐', key: 'tasks' as const },
]

// Re-fetch the active sub-tab list when the user re-enters the parent tab.
// The first activation is handled by LogsView.vue's loadSessionsTab which
// populates the props. If the user clicks reload in the toolbar, this fires.
watch(() => props.sessions.length + props.requests.length + props.tasks.length, () => {
  // props changed — no action needed; child components are reactive
})
</script>

<template>
  <div class="session-explorer">
    <div class="sub-tabs">
      <button
        v-for="t in subTabs"
        :key="t.id"
        class="sub-tab"
        :class="{ active: subTab === t.key }"
        @click="subTab = t.key"
      >
        <span>{{ t.icon }}</span>
        <span>{{ t.label }}</span>
        <span class="sub-tab-count">{{ counts[t.key] }}</span>
      </button>
      <button class="sub-tab sub-tab-reload" @click="emit('reload')">⟳ 刷新</button>
      <span v-if="loadingDetail" class="loading-hint">⟳ 加载详情...</span>
    </div>

    <div class="explorer-content">
      <SessionList
        v-if="subTab === 'sessions'"
        :sessions="sessions"
        :selected-id="selectedSessionId"
        :requests="requests"
        @select="selectSession"
        @view-requests="viewSessionRequests"
      />
      <RequestList
        v-else-if="subTab === 'requests'"
        :requests="requests"
        :selected-id="selectedRequestId"
        :sessions="sessions"
        :tasks="tasks"
        @select="selectRequest"
        @view-session="viewRequestSession"
        @view-task="viewRequestTask"
      />
      <ClusterTaskList
        v-else
        :tasks="tasks"
        :selected-id="selectedTaskId"
        :requests="requests"
        @select="selectTask"
        @view-request="viewTaskRequest"
      />
    </div>
  </div>
</template>

<style scoped>
.session-explorer {
  display: flex;
  flex-direction: column;
  height: 100%;
}

.sub-tabs {
  display: flex;
  gap: var(--space-1);
  padding: var(--space-2) var(--space-4);
  background: var(--bg-secondary);
  border-bottom: 1px solid var(--border-light);
  align-items: center;
}

.sub-tab {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 6px 12px;
  border: 1px solid transparent;
  background: transparent;
  color: var(--text-secondary);
  border-radius: var(--radius-md);
  cursor: pointer;
  font-size: var(--text-sm);
  transition: all 0.15s;
}

.sub-tab:hover { background: var(--bg-hover); }

.sub-tab.active {
  background: var(--accent);
  color: white;
}

.sub-tab-count {
  padding: 0 6px;
  background: var(--bg-tertiary);
  color: var(--text-muted);
  border-radius: var(--radius-sm);
  font-size: var(--text-xs);
}

.sub-tab.active .sub-tab-count {
  background: rgba(255,255,255,0.25);
  color: white;
}

.sub-tab-reload {
  margin-left: auto;
}

.loading-hint {
  font-size: var(--text-xs);
  color: var(--text-muted);
  padding-left: var(--space-2);
}

.explorer-content {
  flex: 1;
  overflow: hidden;
}
</style>
