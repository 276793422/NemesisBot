<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted, nextTick } from 'vue'
import { httpGet } from '../composables/useWebSocket'
import { useWSAPI } from '../composables/useWSAPI'
import { on as sseOn, off as sseOff } from '../composables/useSSE'

const { request } = useWSAPI()

interface LogEntry {
  source?: string
  level?: string
  timestamp?: string
  component?: string
  message?: string
}

const activeMainTab = ref('system')
const entries = ref<LogEntry[]>([])
const source = ref('general')
const level = ref('')
const filter = ref('')
const autoScroll = ref(true)
const loading = ref(false)
const logsList = ref<HTMLDivElement | null>(null)

// Request logs state
const requestSessions = ref<any[]>([])
const requestDetail = ref<any>(null)
const reqLoading = ref(false)
const selectedSession = ref('')

// Security logs state
const securityEntries = ref<any[]>([])
const secLoading = ref(false)

let _onLog: ((entry: LogEntry) => void) | null = null

const sources = [
  { id: 'general', label: '应用日志' },
  { id: 'llm', label: 'AI 通信' },
  { id: 'security', label: '安全审计' },
  { id: 'cluster', label: '集群日志' },
]

const levels = [
  { id: '', label: '全部' },
  { id: 'DEBUG', label: 'DEBUG' },
  { id: 'INFO', label: 'INFO' },
  { id: 'WARN', label: 'WARN' },
  { id: 'ERROR', label: 'ERROR' },
]

const filteredEntries = computed(() => {
  if (!filter.value) return entries.value
  const f = filter.value.toLowerCase()
  return entries.value.filter(e =>
    (e.message && e.message.toLowerCase().includes(f)) ||
    (e.component && e.component.toLowerCase().includes(f))
  )
})

function scrollToBottom() {
  if (logsList.value) logsList.value.scrollTop = logsList.value.scrollHeight
}

function formatTime(ts?: string): string {
  if (!ts) return ''
  return new Date(ts).toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit', second: '2-digit', hour12: false })
}

async function loadInitial() {
  loading.value = true
  try {
    const data = await httpGet<{ entries: LogEntry[] }>(`/api/logs?source=${source.value}&n=200`)
    entries.value = data.entries || []
    nextTick(() => scrollToBottom())
  } catch (err) {
    console.error('[Logs] Failed to load:', err)
  }
  loading.value = false
}

function switchSource(src: string) {
  source.value = src
  entries.value = []
  loadInitial()
}

function clearLogs() { entries.value = [] }

async function loadRequestLogs() {
  reqLoading.value = true
  try {
    const data = await request('logs', 'requests', { limit: 50 })
    requestSessions.value = data?.entries || []
  } catch { /* ignore */ }
  reqLoading.value = false
}

async function loadRequestDetail(session: string) {
  selectedSession.value = session
  try {
    const data = await request('logs', 'request_detail', { session })
    requestDetail.value = data
  } catch { /* ignore */ }
}

async function loadSecurityLogs() {
  secLoading.value = true
  try {
    const data = await request('logs', 'security', { limit: 100 })
    securityEntries.value = data?.entries || []
  } catch { /* ignore */ }
  secLoading.value = false
}

function switchMainTab(tab: string) {
  activeMainTab.value = tab
  if (tab === 'requests' && requestSessions.value.length === 0) loadRequestLogs()
  if (tab === 'security' && securityEntries.value.length === 0) loadSecurityLogs()
}

onMounted(() => {
  loadInitial()
  _onLog = (entry: LogEntry) => {
    if (entry.source && entry.source !== source.value) return
    if (level.value && entry.level !== level.value) return
    entries.value.push(entry)
    if (entries.value.length > 1000) entries.value = entries.value.slice(-500)
    if (autoScroll.value) nextTick(() => scrollToBottom())
  }
  sseOn('log', _onLog)
})

onUnmounted(() => {
  if (_onLog) sseOff('log', _onLog)
})
</script>

<template>
  <div class="page-logs">
    <div class="page-header"><h2>日志管理</h2></div>

    <!-- Main tabs -->
    <div class="tabs" style="padding: 0 var(--space-4); border-bottom: 1px solid var(--border-light);">
      <button class="tab" :class="{ active: activeMainTab === 'system' }" @click="switchMainTab('system')">系统日志</button>
      <button class="tab" :class="{ active: activeMainTab === 'requests' }" @click="switchMainTab('requests')">请求日志</button>
      <button class="tab" :class="{ active: activeMainTab === 'security' }" @click="switchMainTab('security')">安全审计</button>
    </div>

    <!-- System logs -->
    <template v-if="activeMainTab === 'system'">
      <div class="logs-toolbar">
        <div class="logs-tabs">
          <button v-for="src in sources" :key="src.id" class="logs-tab" :class="{ active: source === src.id }" @click="switchSource(src.id)">{{ src.label }}</button>
        </div>
        <select class="form-select" style="width: auto;" v-model="level">
          <option v-for="lv in levels" :key="lv.id" :value="lv.id">{{ lv.label }}</option>
        </select>
        <input class="logs-filter" type="text" placeholder="搜索关键词..." v-model="filter">
        <label style="display: flex; align-items: center; gap: 4px; font-size: var(--text-xs); color: var(--text-muted); cursor: pointer;">
          <input type="checkbox" v-model="autoScroll"> 自动滚动
        </label>
        <button class="btn btn-sm btn-ghost" @click="clearLogs()">清除</button>
      </div>
      <div ref="logsList" class="logs-list">
        <div v-if="loading" style="padding: var(--space-8); text-align: center;">
          <div class="spinner spinner-lg" style="margin: 0 auto;"></div>
          <p style="margin-top: var(--space-4); color: var(--text-muted);">加载日志...</p>
        </div>
        <div v-for="(entry, idx) in filteredEntries" :key="idx" class="log-entry">
          <span class="log-level" :class="entry.level ? entry.level.toLowerCase() : ''">{{ entry.level || '' }}</span>
          <span class="log-time">{{ formatTime(entry.timestamp) }}</span>
          <span class="log-component">{{ entry.component || '' }}</span>
          <span class="log-message">{{ entry.message || '' }}</span>
        </div>
        <div v-if="!loading && entries.length === 0" class="empty-state"><p>暂无日志</p></div>
      </div>
    </template>

    <!-- Request logs -->
    <template v-if="activeMainTab === 'requests'">
      <div class="page-body">
        <div v-if="reqLoading" style="text-align: center; padding: var(--space-8);">
          <div class="spinner spinner-lg" style="margin: 0 auto;"></div>
        </div>
        <div v-if="!reqLoading && requestSessions.length === 0" class="empty-state">
          <h3>暂无请求日志</h3>
          <p>请求日志将在对话过程中自动记录</p>
        </div>
        <div v-if="!reqLoading && requestSessions.length > 0" style="display: grid; grid-template-columns: 300px 1fr; gap: var(--space-4);">
          <div class="card" style="overflow-y: auto; max-height: 500px;">
            <div style="padding: var(--space-2);">
              <div v-for="s in requestSessions" :key="s.session || s.id"
                style="padding: var(--space-2) var(--space-3); cursor: pointer; border-radius: var(--radius-md); font-size: var(--text-sm);"
                :style="{ background: selectedSession === (s.session || s.id) ? 'var(--accent-muted)' : '' }"
                @click="loadRequestDetail(s.session || s.id)">
                <div style="font-weight: 500;">{{ s.session || s.id }}</div>
                <div style="font-size: var(--text-xs); color: var(--text-muted);">{{ s.timestamp || '' }}</div>
              </div>
            </div>
          </div>
          <div class="card">
            <div class="card-header"><h3>请求详情</h3></div>
            <div class="card-body">
              <div v-if="!requestDetail" class="empty-state" style="padding: var(--space-4);"><p>选择一个会话查看详情</p></div>
              <pre v-else style="white-space: pre-wrap; font-size: var(--text-xs); max-height: 60vh; overflow-y: auto;">{{ JSON.stringify(requestDetail, null, 2) }}</pre>
            </div>
          </div>
        </div>
      </div>
    </template>

    <!-- Security logs -->
    <template v-if="activeMainTab === 'security'">
      <div class="page-body">
        <div v-if="secLoading" style="text-align: center; padding: var(--space-8);">
          <div class="spinner spinner-lg" style="margin: 0 auto;"></div>
        </div>
        <div v-if="!secLoading && securityEntries.length === 0" class="empty-state">
          <h3>暂无安全事件</h3>
          <p>安全事件将自动记录</p>
        </div>
        <div v-if="!secLoading && securityEntries.length > 0" class="table-wrap">
          <table>
            <thead><tr><th>时间</th><th>操作</th><th>风险级别</th><th>目标</th><th>结果</th></tr></thead>
            <tbody>
              <tr v-for="(e, idx) in securityEntries" :key="idx">
                <td style="font-size: var(--text-xs);">{{ e.timestamp || '--' }}</td>
                <td>{{ e.action || e.operation || '--' }}</td>
                <td>
                  <span class="badge" :class="{
                    'badge-error': e.risk_level === 'CRITICAL',
                    'badge-warning': e.risk_level === 'HIGH',
                    'badge-info': e.risk_level === 'MEDIUM',
                    'badge-neutral': e.risk_level === 'LOW',
                  }">{{ e.risk_level || '--' }}</span>
                </td>
                <td style="max-width: 200px; overflow: hidden; text-overflow: ellipsis;">{{ e.target || '--' }}</td>
                <td>{{ e.result || '--' }}</td>
              </tr>
            </tbody>
          </table>
        </div>
      </div>
    </template>
  </div>
</template>
