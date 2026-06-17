<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted } from 'vue'
import { formatTime, SOURCE_META, type LogEntry, type LogSource, type LogLevel } from './mockData'
import {
  on as sseOn,
  off as sseOff,
  connectEvents,
  sseReadyState,
} from '../../composables/useSSE'
import MultiSelectDialog, { type SelectableItem } from './MultiSelectDialog.vue'

const entries = ref<LogEntry[]>([])
const sources = ref<Set<LogSource>>(new Set(['general', 'cluster', 'security', 'heartbeat', 'llm']))
const levels = ref<Set<LogLevel>>(new Set())
const componentFilter = ref<Set<string>>(new Set())
const keyword = ref('')
const paused = ref(false)
const autoScroll = ref(true)
const connected = ref(false)
const eventCount = ref(0)

// 历史加载状态：
// - `historyLoaded`：从 /api/logs?source=general 加载完成？
// - `pendingDuringLoad`：历史加载期间到达的 SSE 事件先缓冲，加载完按 seq 去重 flush
// - `maxSeqSeen`：见过的最大 seq（前端去重依据）。后端每个事件都带 seq，
//   跨进程唯一（boot_ms<<20 | counter），所以单一变量即可正确去重。
const historyLoaded = ref(false)
const pendingDuringLoad: Array<{ data: any; seq: number }> = []
const maxSeqSeen = ref(0)

// 三个弹窗的显示状态
const showSourceDialog = ref(false)
const showLevelDialog = ref(false)
const showComponentDialog = ref(false)

// 单调递增的序号，给 LogEntry.id 用（前端列表 key 需要）
let entrySeq = 0
let pollTimer: ReturnType<typeof setInterval> | null = null

const LEVEL_META: Record<LogLevel, { label: string; color: string }> = {
  DEBUG: { label: 'DEBUG', color: '#6b7280' },
  INFO:  { label: 'INFO',  color: '#3b82f6' },
  WARN:  { label: 'WARN',  color: '#eab308' },
  ERROR: { label: 'ERROR', color: '#ef4444' },
}

const sourceItems = computed<SelectableItem[]>(() =>
  (Object.keys(SOURCE_META) as LogSource[]).map(key => ({
    value: key,
    label: SOURCE_META[key].label,
    icon: SOURCE_META[key].icon,
    color: SOURCE_META[key].color,
  }))
)

const levelItems = computed<SelectableItem[]>(() =>
  (['DEBUG', 'INFO', 'WARN', 'ERROR'] as LogLevel[]).map(l => ({
    value: l,
    label: LEVEL_META[l].label,
    color: LEVEL_META[l].color,
  }))
)

const knownComponents = computed(() => {
  const set = new Set<string>()
  entries.value.forEach(e => set.add(e.component))
  return Array.from(set).sort()
})

const componentCounts = computed(() => {
  const counts = new Map<string, number>()
  entries.value.forEach(e => {
    counts.set(e.component, (counts.get(e.component) || 0) + 1)
  })
  return counts
})

const componentItems = computed<SelectableItem[]>(() =>
  knownComponents.value.map(c => ({
    value: c,
    label: c,
    count: componentCounts.value.get(c) || 0,
  }))
)

const filtered = computed(() => {
  return entries.value.filter(e => {
    if (!sources.value.has(e.source)) return false
    if (levels.value.size > 0 && !levels.value.has(e.level)) return false
    if (componentFilter.value.size > 0 && !componentFilter.value.has(e.component)) return false
    if (keyword.value) {
      const k = keyword.value.toLowerCase()
      if (!e.message.toLowerCase().includes(k) && !e.component.toLowerCase().includes(k)) return false
    }
    return true
  })
})

// ---------------------------------------------------------------------------
// SSE 事件接入：后端 GlobalSseLogLayer 把每个 tracing 事件 publish 到 EventHub，
// 走 /api/events/stream，前端 useSSE 已注册 'log' 监听器。
// 这里把后端的字段（含 source/level/timestamp/component/message/fields）映射到
// 前端 LogEntry 结构。
// ---------------------------------------------------------------------------
type SseLogLevel = 'TRACE' | 'DEBUG' | 'INFO' | 'WARN' | 'ERROR'

function sseLevelToLogLevel(level: string): LogLevel | null {
  const upper = level.toUpperCase() as SseLogLevel
  if (upper === 'DEBUG' || upper === 'INFO' || upper === 'WARN' || upper === 'ERROR') {
    return upper
  }
  // TRACE 走 DEBUG 渠道（前端没有 TRACE 级别），其他未知级别丢弃
  if (upper === 'TRACE') return 'DEBUG'
  return null
}

function normalizeSource(src: unknown): LogSource {
  if (typeof src === 'string') {
    if (src === 'general' || src === 'cluster' || src === 'security' || src === 'llm' || src === 'heartbeat') {
      return src
    }
  }
  return 'general'
}

function appendLogEntry(data: any) {
  const level = sseLevelToLogLevel(String(data.level ?? ''))
  if (!level) return

  const source = normalizeSource(data.source)
  const component = typeof data.component === 'string' && data.component ? data.component : 'unknown'
  const message = typeof data.message === 'string' ? data.message : ''
  const timestamp = typeof data.timestamp === 'string' ? data.timestamp : new Date().toISOString()
  const fields = (data.fields && typeof data.fields === 'object')
    ? data.fields as Record<string, string | number | boolean>
    : undefined

  // file:line 前缀（如果后端给了）
  let displayMessage = message
  if (typeof data.file === 'string' && data.file && typeof data.line === 'number' && data.line > 0) {
    const shortFile = data.file.split(/[\\/]/).pop() || data.file
    displayMessage = `${shortFile}:${data.line} ${message}`
  }

  const entry: LogEntry = {
    id: `log-${Date.now()}-${++entrySeq}`,
    source,
    level,
    timestamp,
    component,
    message: displayMessage,
    fields,
  }

  entries.value.push(entry)
  // 上限 2000 条；超过则保留最新 1000 条
  if (entries.value.length > 2000) {
    entries.value = entries.value.slice(-1000)
  }
}

function onLogEvent(data: any) {
  eventCount.value++
  if (paused.value) return
  if (!data || typeof data !== 'object') return

  const seq = typeof data.seq === 'number' ? data.seq : 0

  // 历史加载期间：缓冲 SSE 事件，等历史加载完统一 flush 去重
  if (!historyLoaded.value) {
    pendingDuringLoad.push({ data, seq })
    return
  }

  // 正常路径：用 seq 去重，丢弃旧事件（避免历史→SSE 边界重复）
  if (seq <= maxSeqSeen.value) return
  maxSeqSeen.value = seq
  appendLogEntry(data)
  requestAnimationFrame(() => scrollToBottom())
}

// ---------------------------------------------------------------------------
// 历史加载：打开页面立刻拉最近 N 条历史日志，避免空白等待 SSE
// ---------------------------------------------------------------------------
async function loadHistory() {
  try {
    const resp = await fetch('/api/logs?source=general&n=500')
    if (resp.ok) {
      const payload = await resp.json()
      const events: any[] = Array.isArray(payload.entries) ? payload.entries : []
      // 历史 API 返回顺序是文件中的写入顺序（旧→新），直接 append 即可保持时序
      for (const ev of events) {
        appendLogEntry(ev)
        if (typeof ev.seq === 'number' && ev.seq > maxSeqSeen.value) {
          maxSeqSeen.value = ev.seq
        }
      }
    }
  } catch (e) {
    console.warn('[EventStream] load history failed', e)
  } finally {
    // Flush 缓冲的 SSE 事件：只保留 seq 大于历史最大值的事件
    // （race window 内到达的 SSE 事件，多数已在历史里）
    for (const p of pendingDuringLoad) {
      if (p.seq > maxSeqSeen.value) {
        maxSeqSeen.value = p.seq
        appendLogEntry(p.data)
      }
    }
    pendingDuringLoad.length = 0
    historyLoaded.value = true
    requestAnimationFrame(() => scrollToBottom())
  }
}

function handleSourceConfirm(next: Set<string>) {
  sources.value = next as Set<LogSource>
  showSourceDialog.value = false
}

function handleLevelConfirm(next: Set<string>) {
  levels.value = next as Set<LogLevel>
  showLevelDialog.value = false
}

function handleComponentConfirm(next: Set<string>) {
  componentFilter.value = next
  showComponentDialog.value = false
}

function clearAll() {
  entries.value = []
}

function exportFiltered() {
  const text = filtered.value.map(e =>
    `[${e.timestamp}] [${e.level}] [${e.source}] ${e.component}: ${e.message}`
  ).join('\n')
  const blob = new Blob([text], { type: 'text/plain' })
  const url = URL.createObjectURL(blob)
  const a = document.createElement('a')
  a.href = url
  a.download = `logs-export-${Date.now()}.txt`
  a.click()
  URL.revokeObjectURL(url)
}

function scrollToBottom() {
  if (!autoScroll.value) return
  const el = document.querySelector('.event-stream-list')
  if (el) el.scrollTop = el.scrollHeight
}

onMounted(() => {
  // 确保 SSE 已连接（auth login 时已经 connectEvents 过；这里防御性再调一次，幂等）
  connectEvents()
  sseOn('log', onLogEvent)
  sseOn('heartbeat', onHeartbeat)
  // 先订阅 SSE 再加载历史——race window 内到达的 SSE 事件进 pendingDuringLoad
  // 缓冲，等历史加载完按 seq 去重 flush，保证不丢不重
  loadHistory()

  // 轮询 readyState 以反映真实连接状态到 UI
  pollTimer = setInterval(() => {
    const rs = sseReadyState()
    // EventSource.OPEN === 1
    connected.value = (rs === 1)
  }, 1000)
})

onUnmounted(() => {
  sseOff('log', onLogEvent)
  sseOff('heartbeat', onHeartbeat)
  if (pollTimer) clearInterval(pollTimer)
})

function onHeartbeat() {
  // 后端定期推 heartbeat（见 server.rs:756），收到就说明 SSE 通路活着
  connected.value = true
}

function levelClass(l: LogLevel): string {
  return l.toLowerCase()
}
</script>

<template>
  <div class="event-stream">
    <!-- 工具栏 -->
    <div class="event-toolbar">
      <!-- 来源 -->
      <div class="filter-cell">
        <span class="filter-label">来源</span>
        <button
          class="chip chip-trigger"
          :class="{ active: sources.size > 0 }"
          @click="showSourceDialog = true"
        >
          <span>📍 来源</span>
          <span v-if="sources.size > 0" class="chip-badge">已选 {{ sources.size }}/{{ sourceItems.length }}</span>
          <span v-else class="chip-meta">共 {{ sourceItems.length }}</span>
        </button>
      </div>

      <div class="filter-divider"></div>

      <!-- 级别 -->
      <div class="filter-cell">
        <span class="filter-label">级别</span>
        <button
          class="chip chip-trigger"
          :class="{ active: levels.size > 0 }"
          @click="showLevelDialog = true"
        >
          <span>📋 级别</span>
          <span v-if="levels.size > 0" class="chip-badge">已选 {{ levels.size }}/{{ levelItems.length }}</span>
          <span v-else class="chip-meta">共 {{ levelItems.length }}</span>
        </button>
      </div>

      <div class="filter-divider"></div>

      <!-- 组件 -->
      <div class="filter-cell">
        <span class="filter-label">组件</span>
        <button
          class="chip chip-trigger"
          :class="{ active: componentFilter.size > 0 }"
          @click="showComponentDialog = true"
        >
          <span>⚙ 组件</span>
          <span v-if="componentFilter.size > 0" class="chip-badge">已选 {{ componentFilter.size }}/{{ knownComponents.length }}</span>
          <span v-else-if="knownComponents.length > 0" class="chip-meta">共 {{ knownComponents.length }}</span>
        </button>
      </div>

      <div class="filter-divider"></div>

      <!-- 关键词 -->
      <div class="filter-cell">
        <span class="filter-label">搜索</span>
        <input
          class="form-input filter-input"
          type="text"
          placeholder="消息或组件关键词..."
          v-model="keyword"
        >
      </div>

      <!-- 操作按钮 -->
      <div class="filter-cell filter-cell-actions">
        <span class="filter-label">&nbsp;</span>
        <div class="chip-row">
          <button
            class="btn btn-sm"
            :class="paused ? 'btn-warning' : 'btn-ghost'"
            @click="paused = !paused"
          >{{ paused ? '▶ 继续' : '⏸ 暂停' }}</button>
          <button class="btn btn-sm btn-ghost" @click="exportFiltered">⤓ 导出</button>
          <button class="btn btn-sm btn-ghost" @click="clearAll">🗑 清空</button>
        </div>
      </div>
    </div>

    <!-- 状态条 -->
    <div class="event-status">
      <span class="status-count">显示 {{ filtered.length }} / {{ entries.length }} 条（累计接收 {{ eventCount }}）</span>
      <span v-if="!historyLoaded" class="status-loading">⏳ 加载历史中…</span>
      <span class="status-dot" :class="{ paused, disconnected: !connected }">
        {{ paused ? '⏸ 已暂停' : (connected ? '● 实时连接' : '○ 未连接，等待 SSE…') }}
      </span>
      <span
        class="status-hint"
        title="其他来源（cluster/security/llm）的历史日志格式跟 SSE 推送的不一致，无法合并到本列表，靠 SSE 实时推送。"
      >ⓘ</span>
      <label class="auto-scroll-toggle">
        <input type="checkbox" v-model="autoScroll">
        <span>自动滚动</span>
      </label>
    </div>

    <!-- 空状态提示 -->
    <div v-if="entries.length === 0" class="event-empty">
      <div class="event-empty-icon">📡</div>
      <div class="event-empty-title">
        {{ connected ? '等待日志事件' : '正在连接 SSE…' }}
      </div>
      <div class="event-empty-hint">
        <template v-if="connected">
          当前没有匹配筛选条件的日志事件。后端 tracing 事件会实时推送到这里。
          <br>
          可尝试在别的页面操作 Bot（如发消息、查集群）来触发日志。
        </template>
        <template v-else>
          如果长时间停留在此状态，请打开浏览器 DevTools 的 Network 面板检查
          <code>/api/events/stream</code> 的连接状态。
        </template>
      </div>
    </div>

    <!-- 日志列表 -->
    <div class="event-stream-list">
      <div v-for="entry in filtered" :key="entry.id" class="event-row" :class="`src-${entry.source}`">
        <span class="event-source-icon" :title="SOURCE_META[entry.source].label">
          {{ SOURCE_META[entry.source].icon }}
        </span>
        <span class="event-level" :class="levelClass(entry.level)">{{ entry.level }}</span>
        <span class="event-time">{{ formatTime(entry.timestamp, true) }}</span>
        <span class="event-component">{{ entry.component }}</span>
        <span class="event-message">{{ entry.message }}</span>
      </div>
      <div v-if="filtered.length === 0" class="empty-state">
        <p>暂无匹配的日志</p>
      </div>
    </div>

    <!-- 三个筛选弹窗 -->
    <MultiSelectDialog
      :visible="showSourceDialog"
      title="选择来源"
      :items="sourceItems"
      :selected="sources"
      :search-enabled="false"
      @confirm="handleSourceConfirm"
      @cancel="showSourceDialog = false"
    />
    <MultiSelectDialog
      :visible="showLevelDialog"
      title="选择级别"
      :items="levelItems"
      :selected="levels"
      :search-enabled="false"
      @confirm="handleLevelConfirm"
      @cancel="showLevelDialog = false"
    />
    <MultiSelectDialog
      :visible="showComponentDialog"
      title="选择组件"
      :items="componentItems"
      :selected="componentFilter"
      :show-counts="true"
      @confirm="handleComponentConfirm"
      @cancel="showComponentDialog = false"
    />
  </div>
</template>

<style scoped>
.event-stream {
  display: flex;
  flex-direction: column;
  height: 100%;
  background: var(--bg-primary);
}

.event-toolbar {
  display: flex;
  flex-wrap: wrap;
  gap: var(--space-3);
  padding: var(--space-3) var(--space-4);
  background: var(--bg-secondary);
  border-bottom: 1px solid var(--border-light);
  align-items: center;
}

.filter-cell {
  display: flex;
  flex-direction: column;
  gap: var(--space-1);
  flex-shrink: 0;
}

.filter-cell-actions {
  margin-left: auto;
  flex-shrink: 0;
}

.filter-divider {
  width: 1px;
  align-self: stretch;
  background: var(--border-light);
  margin: 0 var(--space-1);
}

.filter-label {
  font-size: var(--text-xs);
  color: var(--text-muted);
  text-transform: uppercase;
  letter-spacing: 0.5px;
}

.filter-input {
  width: 200px;
  font-size: var(--text-sm);
}

.chip-row {
  display: flex;
  gap: var(--space-1);
  flex-wrap: wrap;
}

.chip {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 4px 10px;
  border: 1px solid var(--border);
  background: var(--bg-primary);
  color: var(--text-secondary);
  border-radius: var(--radius-sm);
  font-size: var(--text-sm);
  cursor: pointer;
  transition: all 0.15s;
}

.chip-trigger {
  white-space: nowrap;
}

.chip-trigger:hover {
  border-color: var(--accent);
}

.chip-trigger.active {
  background: var(--accent);
  color: white;
  border-color: var(--accent);
}

.chip-badge,
.chip-meta {
  padding: 1px 6px;
  border-radius: var(--radius-sm);
  font-size: var(--text-xs);
  white-space: nowrap;
}

.chip-trigger.active .chip-badge {
  background: rgba(255,255,255,0.25);
}

.chip-trigger:not(.active) .chip-badge,
.chip-trigger:not(.active) .chip-meta {
  background: var(--bg-tertiary);
  color: var(--text-muted);
}

.event-status {
  display: flex;
  align-items: center;
  flex-wrap: nowrap;
  gap: var(--space-4);
  padding: var(--space-2) var(--space-4);
  font-size: var(--text-xs);
  color: var(--text-muted);
  background: var(--bg-tertiary);
  border-bottom: 1px solid var(--border-light);
  white-space: nowrap;
  flex-shrink: 0;
}

.status-count,
.status-dot {
  white-space: nowrap;
  display: inline-flex;
  align-items: center;
}

.status-dot {
  color: #10b981;
}

.status-hint {
  color: var(--text-muted);
  cursor: help;
  white-space: nowrap;
}

.status-dot.paused { color: #eab308; }
.status-dot.disconnected { color: #6b7280; }

.status-loading {
  color: #eab308;
  font-style: italic;
  animation: pulse 1.5s ease-in-out infinite;
}

@keyframes pulse {
  0%, 100% { opacity: 1; }
  50% { opacity: 0.5; }
}

.auto-scroll-toggle {
  margin-left: auto;
  display: flex;
  align-items: center;
  gap: 4px;
  cursor: pointer;
  white-space: nowrap;
}

.event-stream-list {
  flex: 1;
  overflow-y: auto;
  padding: var(--space-2) var(--space-4);
  font-family: 'Cascadia Code', 'Consolas', monospace;
  font-size: var(--text-sm);
}

.event-row {
  display: grid;
  grid-template-columns: 28px 60px 100px 180px 1fr;
  gap: var(--space-2);
  padding: var(--space-1) var(--space-2);
  border-radius: var(--radius-sm);
  align-items: center;
}

.event-row:hover {
  background: var(--bg-hover);
}

.event-row.src-cluster   { border-left: 3px solid #a855f7; padding-left: calc(var(--space-2) - 3px); }
.event-row.src-security  { border-left: 3px solid #ef4444; padding-left: calc(var(--space-2) - 3px); }
.event-row.src-heartbeat { border-left: 3px solid #eab308; padding-left: calc(var(--space-2) - 3px); }
.event-row.src-llm       { border-left: 3px solid #10b981; padding-left: calc(var(--space-2) - 3px); }

.event-source-icon { font-size: 14px; text-align: center; }

.event-level {
  display: inline-block;
  padding: 2px 6px;
  border-radius: var(--radius-sm);
  font-size: var(--text-xs);
  font-weight: 600;
  text-align: center;
}

.event-level.debug { background: rgba(107,114,128,0.2); color: #6b7280; }
.event-level.info  { background: rgba(59,130,246,0.15); color: #3b82f6; }
.event-level.warn  { background: rgba(234,179,8,0.15); color: #eab308; }
.event-level.error { background: rgba(239,68,68,0.15); color: #ef4444; }

.event-time {
  color: var(--text-muted);
  font-size: var(--text-xs);
}

.event-component {
  color: var(--accent);
  font-weight: 500;
}

.event-message {
  color: var(--text-primary);
  word-break: break-word;
}

.event-empty {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  padding: var(--space-8) var(--space-4);
  text-align: center;
  color: var(--text-muted);
  gap: var(--space-2);
}

.event-empty-icon {
  font-size: 48px;
  opacity: 0.6;
}

.event-empty-title {
  font-size: var(--text-lg);
  color: var(--text-secondary);
  font-weight: 500;
}

.event-empty-hint {
  font-size: var(--text-sm);
  max-width: 480px;
  line-height: 1.6;
}

.event-empty-hint code {
  padding: 2px 6px;
  background: var(--bg-tertiary);
  border-radius: var(--radius-sm);
  font-family: 'Cascadia Code', 'Consolas', monospace;
  font-size: var(--text-xs);
}
</style>
