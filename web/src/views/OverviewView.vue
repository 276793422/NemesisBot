<script setup lang="ts">
import { ref, onMounted, onUnmounted } from 'vue'
import { httpGet } from '../composables/useWebSocket'
import { on as sseOn, off as sseOff } from '../composables/useSSE'

interface StatusData {
  version?: string
  uptime_seconds?: number
  ws_connected?: boolean
  session_count?: number
  model?: string
  scanner_status?: { enabled: boolean; engines?: string[] } | null
  cluster_status?: { enabled: boolean; node_count?: number } | null
}

const status = ref<StatusData>({})
const loading = ref(true)
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

function getConnectionBadge(connected?: boolean): string {
  return connected ? '已连接' : '未连接'
}

function getConnectionBadgeClass(connected?: boolean): string {
  return connected ? 'badge-success' : 'badge-error'
}

onMounted(async () => {
  try {
    status.value = await httpGet<StatusData>('/api/status')
  } catch (err) {
    console.error('[Overview] Failed to load status:', err)
  }
  loading.value = false

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
    <div class="page-header">
      <h2>系统概览</h2>
    </div>
    <div class="page-body">
      <!-- Loading skeleton -->
      <div v-if="loading" class="stats-grid">
        <div class="skeleton skeleton-card"></div>
        <div class="skeleton skeleton-card"></div>
        <div class="skeleton skeleton-card"></div>
        <div class="skeleton skeleton-card"></div>
      </div>

      <!-- Stats -->
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
            <span class="badge" :class="getConnectionBadgeClass(status.ws_connected)">{{ getConnectionBadge(status.ws_connected) }}</span>
          </div>
        </div>
        <div class="stat-card">
          <div class="stat-label">会话数</div>
          <div class="stat-value">{{ status.session_count || 0 }}</div>
        </div>
      </div>

      <!-- Details -->
      <div v-if="!loading" style="margin-top: var(--space-6);">
        <div class="card" style="margin-bottom: var(--space-4);">
          <div class="card-header"><h3>模型</h3></div>
          <div class="card-body">
            <span class="badge badge-info">{{ status.model || '未配置' }}</span>
          </div>
        </div>

        <div class="card" style="margin-bottom: var(--space-4);">
          <div class="card-header"><h3>扫描器</h3></div>
          <div class="card-body">
            <template v-if="status.scanner_status">
              <div>
                <span class="badge" :class="status.scanner_status.enabled ? 'badge-success' : 'badge-neutral'">{{ status.scanner_status.enabled ? '已启用' : '未启用' }}</span>
                <div v-if="status.scanner_status.engines && status.scanner_status.engines.length > 0" style="margin-top: var(--space-2); display: flex; gap: var(--space-2);">
                  <span v-for="e in status.scanner_status.engines" :key="e" class="badge badge-info">{{ e }}</span>
                </div>
              </div>
            </template>
            <template v-else>
              <span class="badge badge-neutral">未知</span>
            </template>
          </div>
        </div>

        <div class="card">
          <div class="card-header"><h3>集群</h3></div>
          <div class="card-body">
            <template v-if="status.cluster_status">
              <div>
                <span class="badge" :class="status.cluster_status.enabled ? 'badge-success' : 'badge-neutral'">{{ status.cluster_status.enabled ? '已启用' : '未启用' }}</span>
                <span v-if="status.cluster_status.enabled" style="margin-left: var(--space-2); font-size: var(--text-sm); color: var(--text-muted);">节点数: {{ status.cluster_status.node_count || 0 }}</span>
              </div>
            </template>
            <template v-else>
              <span class="badge badge-neutral">未知</span>
            </template>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>
