<script setup lang="ts">
import { ref, onMounted, onUnmounted } from 'vue'
import { useWSAPI } from '../../composables/useWSAPI'
import { useToast } from '../../composables/useToast'
import HealthScore from './HealthScore.vue'
import ActivityFeed from './ActivityFeed.vue'

const { request } = useWSAPI()
const toast = useToast()

const running = ref(false)
const loading = ref(true)
const healthScore = ref(0)
const onlineNodes = ref(0)
const totalNodes = ref(0)
const activeTasks = ref(0)
const todayCompleted = ref(0)
const totalTasks = ref(0)
const successRate = ref<number | null>(null)
const avgDuration = ref('--')
const events = ref<any[]>([])
const actionRunning = ref(false)

let refreshTimer: ReturnType<typeof setInterval> | null = null

async function loadData() {
  try {
    const data = await request('cluster', 'runtime.status')
    if (!data) return
    running.value = data.running ?? false
    healthScore.value = data.health_score ?? 0
    onlineNodes.value = data.online_nodes ?? 0
    totalNodes.value = data.total_nodes ?? 0
    activeTasks.value = data.active_tasks ?? 0
    todayCompleted.value = data.today_completed ?? 0
    totalTasks.value = data.total_tasks ?? 0
    successRate.value = data.success_rate ?? null
    avgDuration.value = data.avg_duration ?? '--'
    if (data.recent_events) {
      events.value = data.recent_events
    }
  } catch { /* backend not ready */ }
}

async function startCluster() {
  actionRunning.value = true
  try {
    await request('cluster', 'runtime.start')
    toast.success('集群已启动')
    await loadData()
  } catch (e: any) {
    toast.error('启动失败: ' + (e || '未知错误'))
  } finally {
    actionRunning.value = false
  }
}

async function stopCluster() {
  actionRunning.value = true
  try {
    await request('cluster', 'runtime.stop')
    toast.success('集群已停止')
    await loadData()
  } catch (e: any) {
    toast.error('停止失败: ' + (e || '未知错误'))
  } finally {
    actionRunning.value = false
  }
}

onMounted(async () => {
  await loadData()
  loading.value = false
  refreshTimer = setInterval(loadData, 10000)
})

onUnmounted(() => {
  if (refreshTimer) clearInterval(refreshTimer)
})
</script>

<template>
  <div v-if="loading" style="text-align:center;padding:var(--space-8)">
    <div class="spinner spinner-lg" style="margin:0 auto" />
  </div>

  <div v-if="!loading">
    <div class="stats-grid">
      <div class="stat-card" style="display:flex;align-items:center;justify-content:center">
        <HealthScore :score="healthScore" />
      </div>
      <div class="stat-card">
        <div class="stat-label">在线节点</div>
        <div class="stat-value">{{ onlineNodes }}<span style="color:var(--text-muted);font-size:var(--text-sm)"> / {{ totalNodes }}</span></div>
      </div>
      <div class="stat-card">
        <div class="stat-label">活跃任务</div>
        <div class="stat-value">{{ activeTasks }}</div>
      </div>
      <div class="stat-card">
        <div class="stat-label">今日完成</div>
        <div class="stat-value">{{ todayCompleted }}</div>
      </div>
    </div>

    <div class="stats-grid" style="margin-top:var(--space-3)">
      <div class="stat-card">
        <div class="stat-label">总任务</div>
        <div class="stat-value">{{ totalTasks }}</div>
      </div>
      <div class="stat-card">
        <div class="stat-label">成功率</div>
        <div class="stat-value">{{ successRate != null ? (successRate * 100).toFixed(1) + '%' : '--' }}</div>
      </div>
      <div class="stat-card">
        <div class="stat-label">平均耗时</div>
        <div class="stat-value">{{ avgDuration }}</div>
      </div>
      <div class="stat-card">
        <div class="stat-label">集群状态</div>
        <div class="stat-value" style="display:flex;align-items:center;gap:var(--space-2)">
          <span class="badge" :class="running ? 'badge-success' : 'badge-neutral'">{{ running ? '运行中' : '已停止' }}</span>
          <button
            v-if="running"
            class="btn btn-sm btn-danger"
            :disabled="actionRunning"
            @click="stopCluster"
            title="停止集群网络服务（节点发现、RPC 通信）。会同步更新集群配置，重启后不会自动启动。"
          >停止集群</button>
          <button
            v-else
            class="btn btn-sm btn-success"
            :disabled="actionRunning"
            @click="startCluster"
            title="启动集群网络服务（节点发现、RPC 通信）。会同步更新集群配置，重启后也会自动启动。"
          >启动集群</button>
        </div>
      </div>
    </div>

    <div class="card" style="margin-top:var(--space-4)">
      <div class="card-header"><h3>实时活动</h3></div>
      <div class="card-body">
        <ActivityFeed :events="events" />
      </div>
    </div>
  </div>
</template>
