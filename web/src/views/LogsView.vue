<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted, nextTick } from 'vue'
import { httpGet } from '../composables/useWebSocket'
import { on as sseOn, off as sseOff } from '../composables/useSSE'

interface LogEntry {
  source?: string
  level?: string
  timestamp?: string
  component?: string
  message?: string
}

const entries = ref<LogEntry[]>([])
const source = ref('general')
const level = ref('')
const filter = ref('')
const autoScroll = ref(true)
const loading = ref(false)
const logsList = ref<HTMLDivElement | null>(null)

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
  if (logsList.value) {
    logsList.value.scrollTop = logsList.value.scrollHeight
  }
}

function formatTime(ts?: string): string {
  if (!ts) return ''
  const d = new Date(ts)
  return d.toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit', second: '2-digit', hour12: false })
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

function clearLogs() {
  entries.value = []
}

onMounted(() => {
  loadInitial()

  _onLog = (entry: LogEntry) => {
    if (entry.source && entry.source !== source.value) return
    if (level.value && entry.level !== level.value) return
    entries.value.push(entry)
    if (entries.value.length > 1000) {
      entries.value = entries.value.slice(-500)
    }
    if (autoScroll.value) {
      nextTick(() => scrollToBottom())
    }
  }
  sseOn('log', _onLog)
})

onUnmounted(() => {
  if (_onLog) sseOff('log', _onLog)
})
</script>

<template>
  <div class="page-logs">
    <div class="page-header">
      <h2>日志管理</h2>
    </div>

    <!-- Toolbar -->
    <div class="logs-toolbar">
      <div class="logs-tabs">
        <button
          v-for="src in sources"
          :key="src.id"
          class="logs-tab"
          :class="{ active: source === src.id }"
          @click="switchSource(src.id)"
        >{{ src.label }}</button>
      </div>
      <select class="form-select" style="width: auto;" v-model="level">
        <option v-for="lv in levels" :key="lv.id" :value="lv.id">{{ lv.label }}</option>
      </select>
      <input class="logs-filter" type="text" placeholder="搜索关键词..." v-model="filter">
      <label style="display: flex; align-items: center; gap: 4px; font-size: var(--text-xs); color: var(--text-muted); cursor: pointer;">
        <input type="checkbox" v-model="autoScroll">
        自动滚动
      </label>
      <button class="btn btn-sm btn-ghost" @click="clearLogs()">清除</button>
    </div>

    <!-- Log entries -->
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

      <div v-if="!loading && entries.length === 0" class="empty-state">
        <p>暂无日志</p>
      </div>
    </div>
  </div>
</template>
