<script setup lang="ts">
import { ref, onMounted, onUnmounted } from 'vue'
import { httpGet } from '../composables/useWebSocket'
import { useWSAPI } from '../composables/useWSAPI'
import { on as sseOn, off as sseOff } from '../composables/useSSE'
import { useToast } from '../composables/useToast'

const { request } = useWSAPI()
const toast = useToast()

interface StatusData {
  version?: string
  uptime_seconds?: number
  ws_connected?: boolean
  session_count?: number
  model?: string
  scanner_status?: { enabled: boolean; engines?: string[] } | null
  cluster_status?: { enabled: boolean; node_count?: number } | null
  running?: boolean
}

const status = ref<StatusData>({})
const loading = ref(true)
const agentRunning = ref(true)
const agentLoading = ref(false)
let _onStatus: ((data: StatusData) => void) | null = null

function formatUptime(seconds?: number): string {
  if (!seconds) return '--'
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

async function loadAgentStatus() {
  try {
    const data = await request('agent', 'status')
    agentRunning.value = data?.running ?? true
  } catch { /* ignore */ }
}

async function startAgent() {
  agentLoading.value = true
  try {
    await request('agent', 'start')
    agentRunning.value = true
    toast.success('Agent 已启动')
  } catch (e: any) {
    toast.error('启动失败: ' + e)
  }
  agentLoading.value = false
}

async function stopAgent() {
  if (!confirm('确定停止 Agent Loop 吗？\n\nGateway 和 Web UI 将继续运行，但 Agent 将暂停处理消息。')) return
  agentLoading.value = true
  try {
    await request('agent', 'stop')
    agentRunning.value = false
    toast.success('Agent 已暂停')
  } catch (e: any) {
    toast.error('停止失败: ' + e)
  }
  agentLoading.value = false
}

onMounted(async () => {
  try {
    status.value = await httpGet<StatusData>('/api/status')
  } catch (err) {
    console.error('[Overview] Failed to load status:', err)
  }
  loading.value = false

  await loadAgentStatus()

  _onStatus = (data: StatusData) => {
    status.value = data
    loading.value = false
  }
  sseOn('status', _onStatus)
})

onUnmounted(() => {
  if (_onStatus) sseOff('status', _onStatus)
})
</script>

<template>
  <div class="page-overview">
    <div class="page-header"><h2>系统概览</h2></div>
    <div class="page-body">
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
          <div class="stat-value">{{ formatUptime(status.uptime_seconds) }}</div>
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

      <div v-if="!loading" style="margin-top: var(--space-6);">
        <!-- Agent control -->
        <div class="card" style="margin-bottom: var(--space-4);">
          <div class="card-header">
            <h3>Agent</h3>
            <div style="display: flex; align-items: center; gap: var(--space-3);">
              <span class="badge" :class="agentRunning ? 'badge-success' : 'badge-warning'">
                {{ agentRunning ? '运行中' : '已暂停' }}
              </span>
              <div v-if="agentLoading" class="spinner" style="width: 16px; height: 16px;"></div>
              <template v-else>
                <button v-if="!agentRunning" class="btn btn-sm btn-primary" @click="startAgent">启动</button>
                <button v-else class="btn btn-sm btn-danger" @click="stopAgent">暂停</button>
              </template>
            </div>
          </div>
          <div class="card-body">
            <div class="stat-card">
              <div class="stat-label">当前模型</div>
              <div class="stat-value" style="font-size: var(--text-lg);">{{ status.model || '未配置' }}</div>
            </div>
          </div>
        </div>

        <div class="card" style="margin-bottom: var(--space-4);">
          <div class="card-header"><h3>扫描器</h3></div>
          <div class="card-body">
            <template v-if="status.scanner_status">
              <span class="badge" :class="status.scanner_status.enabled ? 'badge-success' : 'badge-neutral'">{{ status.scanner_status.enabled ? '已启用' : '未启用' }}</span>
              <div v-if="status.scanner_status.engines && status.scanner_status.engines.length > 0" style="margin-top: var(--space-2); display: flex; gap: var(--space-2);">
                <span v-for="e in status.scanner_status.engines" :key="e" class="badge badge-info">{{ e }}</span>
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
    </div>
  </div>
</template>
