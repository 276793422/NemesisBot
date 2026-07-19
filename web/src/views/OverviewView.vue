<script setup lang="ts">
import { ref, onMounted, onUnmounted } from 'vue'
import { httpGet } from '../composables/useWebSocket'
import { useWSAPI } from '../composables/useWSAPI'
import { on as sseOn, off as sseOff } from '../composables/useSSE'
import { useToast } from '../composables/useToast'
import { usePageTab } from '../lib/pageTab'
import UsageView from './UsageView.vue'
import LogsView from './LogsView.vue'
import MemoryView from './MemoryView.vue'

const { request } = useWSAPI()
const toast = useToast()

const usageOn = import.meta.env.VITE_FEATURE_USAGE !== 'false'
const memoryOn = import.meta.env.VITE_FEATURE_MEMORY !== 'false'

const pageTab = ref('summary')
const { setTab } = usePageTab(pageTab, ['summary', 'usage', 'logs', 'memory'] as const, 'summary')

interface StatusData {
  version?: string
  uptime_seconds?: number
  ws_connected?: boolean
  session_count?: number
  model?: string
  model_base?: string
  model_has_key?: boolean
  scanner_status?: { enabled: boolean; engines?: { name: string; config?: any }[] } | null
  cluster_status?: { enabled: boolean; node_count?: number } | null
  running?: boolean
}

const status = ref<StatusData>({})
const displayUptime = ref(-1)
const loading = ref(true)
const agentRunning = ref(true)
const agentLoading = ref(false)
let _onStatus: ((data: StatusData) => void) | null = null
let uptimeTimer: ReturnType<typeof setInterval> | null = null

function formatUptime(seconds: number): string {
  if (seconds < 0) return '--'
  const d = Math.floor(seconds / 86400)
  const h = Math.floor((seconds % 86400) / 3600)
  const m = Math.floor((seconds % 3600) / 60)
  const s = seconds % 60
  const parts: string[] = []
  if (d > 0) parts.push(d + '天')
  if (h > 0) parts.push(h + '小时')
  if (m > 0) parts.push(m + '分钟')
  if (s > 0 && d === 0) parts.push(s + '秒')
  return parts.join(' ') || '0秒'
}

function startUptimeTimer(baseSeconds: number) {
  stopUptimeTimer()
  displayUptime.value = baseSeconds
  uptimeTimer = setInterval(() => {
    displayUptime.value++
  }, 1000)
}

function stopUptimeTimer() {
  if (uptimeTimer) {
    clearInterval(uptimeTimer)
    uptimeTimer = null
  }
}

async function loadAgentStatus() {
  try {
    const data = await request('agent', 'status')
    agentRunning.value = data?.running ?? true
  } catch { /* ignore */ }
}

async function startAgent() {
  agentLoading.value = true
  try {
    const data = await request('agent', 'start')
    if (data?.started) {
      agentRunning.value = true
      toast.success('Agent 已启动，配置已重新加载')
      loadAgentStatus()
    }
  } catch (e: any) {
    toast.error('启动失败: ' + e)
  }
  agentLoading.value = false
}

async function stopAgent() {
  if (!confirm('确定停止 Agent Loop 吗？\n\nGateway 和 Web UI 将继续运行，但 Agent 将暂停处理消息。')) return
  agentLoading.value = true
  try {
    const data = await request('agent', 'stop')
    if (data?.stopped) {
      agentRunning.value = false
      toast.success('Agent 已停止，组件已卸载')
    }
  } catch (e: any) {
    toast.error('停止失败: ' + e)
  }
  agentLoading.value = false
}

onMounted(async () => {
  try {
    status.value = await httpGet<StatusData>('/api/status')
    if (status.value.uptime_seconds != null) {
      startUptimeTimer(status.value.uptime_seconds)
    }
  } catch (err) {
    console.error('[Overview] Failed to load status:', err)
  }
  loading.value = false

  await loadAgentStatus()

  _onStatus = (data: StatusData) => {
    status.value = { ...status.value, ...data }
    loading.value = false
    if (data.uptime_seconds != null) {
      startUptimeTimer(data.uptime_seconds)
    }
  }
  sseOn('status', _onStatus)
})

onUnmounted(() => {
  if (_onStatus) sseOff('status', _onStatus)
  stopUptimeTimer()
})
</script>

<template>
  <div class="page-overview page-home">
    <div class="page-header"><h2>主页</h2></div>
    <div class="page-body">
      <!-- Hub tabs: 概览（摘要）+ 详细（用量/日志/记忆） -->
      <div class="tabs" style="margin-bottom: var(--space-4);">
        <button class="tab" :class="{ active: pageTab === 'summary' }" @click="setTab('summary')">概览</button>
        <button v-if="usageOn" class="tab" :class="{ active: pageTab === 'usage' }" @click="setTab('usage')">用量</button>
        <button class="tab" :class="{ active: pageTab === 'logs' }" @click="setTab('logs')">日志</button>
        <button v-if="memoryOn" class="tab" :class="{ active: pageTab === 'memory' }" @click="setTab('memory')">记忆</button>
      </div>

      <!-- Detail panels -->
      <div v-if="pageTab === 'usage' && usageOn">
        <UsageView embedded />
      </div>
      <div v-if="pageTab === 'logs'">
        <LogsView embedded />
      </div>
      <div v-if="pageTab === 'memory' && memoryOn">
        <MemoryView embedded />
      </div>

      <!-- Summary / home dashboard -->
      <template v-if="pageTab === 'summary'">
        <p class="home-lead">系统状态一览。用量、日志与记忆请切换上方「详细」标签。</p>
        <div v-if="loading" class="stats-grid">
          <div class="skeleton skeleton-card"></div>
          <div class="skeleton skeleton-card"></div>
          <div class="skeleton skeleton-card"></div>
          <div class="skeleton skeleton-card"></div>
        </div>

        <div v-if="!loading" class="stats-grid">
          <div class="stat-card">
            <div class="stat-label">版本</div>
            <div class="stat-value">{{ status.version || '--' }}</div>
          </div>
          <div class="stat-card">
            <div class="stat-label">运行时间</div>
            <div class="stat-value">{{ formatUptime(displayUptime) }}</div>
          </div>
          <div class="stat-card">
            <div class="stat-label">WebSocket 连接</div>
            <div class="stat-value">
              <span class="badge" :class="status.ws_connected ? 'badge-success' : 'badge-error'">{{ status.ws_connected ? '已连接' : '未连接' }}</span>
            </div>
          </div>
          <div class="stat-card">
            <div class="stat-label">会话数</div>
            <div class="stat-value">{{ status.session_count || 0 }}</div>
          </div>
        </div>

        <div v-if="!loading" class="home-quick" style="margin-top: var(--space-4); display: flex; flex-wrap: wrap; gap: var(--space-2);">
          <button v-if="usageOn" type="button" class="btn btn-sm" @click="setTab('usage')">查看用量 →</button>
          <button type="button" class="btn btn-sm" @click="setTab('logs')">查看日志 →</button>
          <button v-if="memoryOn" type="button" class="btn btn-sm" @click="setTab('memory')">查看记忆 →</button>
        </div>

        <div v-if="!loading" style="margin-top: var(--space-6);">
          <div class="card" style="margin-bottom: var(--space-4);">
            <div class="card-header">
              <h3>Agent</h3>
              <div style="display: flex; align-items: center; gap: var(--space-3);">
                <span class="badge" :class="agentRunning ? 'badge-success' : 'badge-error'">{{ agentRunning ? '运行中' : '已停止' }}</span>
                <div v-if="agentLoading" class="spinner" style="width: 14px; height: 14px;"></div>
                <div v-else class="toggle" :class="{ active: agentRunning }" @click="agentRunning ? stopAgent() : startAgent()"></div>
              </div>
            </div>
            <div class="card-body">
              <div class="settings-grid">
                <span class="settings-key">模型</span>
                <span class="settings-value">{{ status.model || '未配置' }}</span>
                <span class="settings-key">地址</span>
                <span class="settings-value">{{ status.model_base || '--' }}</span>
                <span class="settings-key">Key</span>
                <span class="settings-value" v-if="status.model_has_key">******</span>
                <span class="settings-value" v-else style="color: var(--text-muted);">--</span>
              </div>
            </div>
          </div>

          <div class="card" style="margin-bottom: var(--space-4);">
            <div class="card-header"><h3>扫描器</h3></div>
            <div class="card-body">
              <template v-if="status.scanner_status">
                <span class="badge" :class="status.scanner_status.enabled ? 'badge-success' : 'badge-neutral'">{{ status.scanner_status.enabled ? '已启用' : '未启用' }}</span>
                <div v-if="status.scanner_status.engines && status.scanner_status.engines.length > 0" style="margin-top: var(--space-2); display: flex; gap: var(--space-2);">
                  <span v-for="e in status.scanner_status.engines" :key="e.name" class="badge badge-info">{{ e.name }}</span>
                </div>
              </template>
              <template v-else><span class="badge badge-neutral">未知</span></template>
            </div>
          </div>

          <div class="card">
            <div class="card-header"><h3>集群</h3></div>
            <div class="card-body">
              <template v-if="status.cluster_status">
                <span class="badge" :class="status.cluster_status.enabled ? 'badge-success' : 'badge-neutral'">{{ status.cluster_status.enabled ? '已启用' : '未启用' }}</span>
                <span v-if="status.cluster_status.enabled" style="margin-left: var(--space-2); font-size: var(--text-sm); color: var(--text-muted);">节点数: {{ status.cluster_status.node_count || 0 }}</span>
              </template>
              <template v-else><span class="badge badge-neutral">未知</span></template>
            </div>
          </div>
        </div>
      </template>
    </div>
  </div>
</template>

<style scoped>
.home-lead {
  color: var(--text-muted);
  font-size: var(--text-sm);
  margin: 0 0 var(--space-4);
}
</style>
