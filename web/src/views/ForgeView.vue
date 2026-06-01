<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'

const { request } = useWSAPI()
const toast = useToast()

// --- Tab state ---
const activeTab = ref('overview')
const loading = ref(true)

// --- Overview state ---
const enabled = ref(false)
const running = ref(false)
const stats = ref<any>(null)

// --- Experiences state ---
const expStats = ref<any>(null)

// --- Reflections state ---
const reports = ref<any[]>([])
const latestReport = ref<any>(null)
const showLatestReport = ref(false)

// --- Cycles state ---
const cycles = ref<any[]>([])

// --- Artifacts state ---
const artifacts = ref<any[]>([])
const skillDirectories = ref<any[]>([])

// --- Config state ---
const configData = ref<any>(null)

// ---------------------------------------------------------------------------
// Formatting
// ---------------------------------------------------------------------------

function formatRate(rate: number): string {
  return (rate * 100).toFixed(1) + '%'
}

function formatDuration(ms: number): string {
  if (ms < 1000) return ms.toFixed(0) + 'ms'
  return (ms / 1000).toFixed(1) + 's'
}

function formatDate(dateStr: string): string {
  if (!dateStr) return '--'
  try {
    const d = new Date(dateStr)
    return d.toLocaleString('zh-CN', { month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit' })
  } catch { return dateStr }
}

function formatCount(n: number | undefined): string {
  if (n === undefined || n === null) return '0'
  if (n >= 10000) return (n / 10000).toFixed(1) + 'w'
  if (n >= 1000) return (n / 1000).toFixed(1) + 'k'
  return n.toString()
}

function statusClass(status: string): string {
  const map: Record<string, string> = {
    Active: 'badge-success',
    Observing: 'badge-info',
    Draft: 'badge-neutral',
    Degraded: 'badge-warning',
    Negative: 'badge-error',
    Archived: 'badge-neutral',
  }
  return map[status] || 'badge-neutral'
}

function statusLabel(status: string): string {
  const map: Record<string, string> = {
    Active: '活跃',
    Observing: '观察中',
    Draft: '草稿',
    Degraded: '降级',
    Negative: '负面',
    Archived: '已归档',
  }
  return map[status] || status
}

function cycleStatusLabel(status: string): string {
  const map: Record<string, string> = {
    Running: '运行中',
    Completed: '已完成',
    Failed: '失败',
  }
  return map[status] || status
}

function cycleStatusClass(status: string): string {
  const map: Record<string, string> = {
    Running: 'badge-info',
    Completed: 'badge-success',
    Failed: 'badge-error',
  }
  return map[status] || 'badge-neutral'
}

// ---------------------------------------------------------------------------
// Data loading
// ---------------------------------------------------------------------------

async function loadAll() {
  loading.value = true
  await Promise.all([
    loadStatus(),
    loadStats(),
  ])
  loading.value = false
}

async function loadStatus() {
  try {
    const data = await request('forge', 'status')
    enabled.value = data?.enabled || false
    running.value = data?.running || false
  } catch { /* ignore */ }
}

async function loadStats() {
  try {
    const data = await request('forge', 'stats')
    stats.value = data || null
    enabled.value = data?.enabled ?? enabled.value
  } catch { /* ignore */ }
}

async function loadExperiences() {
  try {
    const data = await request('forge', 'experiences.stats')
    expStats.value = data || null
  } catch { /* ignore */ }
}

async function loadReflections() {
  try {
    const data = await request('forge', 'reflections.list')
    reports.value = data?.reports || []
  } catch { /* ignore */ }
}

async function loadLatestReport() {
  try {
    const data = await request('forge', 'reflections.latest')
    latestReport.value = data || null
    showLatestReport.value = !!data?.found
  } catch { /* ignore */ }
}

async function loadCycles() {
  try {
    const data = await request('forge', 'cycles.list')
    cycles.value = data?.cycles || []
  } catch { /* ignore */ }
}

async function loadArtifacts() {
  try {
    const data = await request('forge', 'registry.list')
    artifacts.value = data?.artifacts || []
    skillDirectories.value = data?.skill_directories || []
  } catch { /* ignore */ }
}

async function loadConfig() {
  try {
    const data = await request('forge', 'stats')
    configData.value = data?.config || null
  } catch { /* ignore */ }
}

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

async function toggleForge() {
  try {
    await request('forge', 'config.save', { enabled: !enabled.value })
    await loadStatus()
    await loadStats()
    toast.success(running.value ? '已启用并启动' : '已停止并禁用')
  } catch (e: any) {
    toast.error('操作失败: ' + e)
  }
}

async function triggerReflect() {
  try {
    const data = await request('forge', 'reflect')
    if (data?.triggered) {
      toast.success(data?.message || '反思完成')
      await loadReflections()
      await loadLatestReport()
    } else {
      toast.info(data?.message || '无法执行反思')
    }
  } catch (e: any) {
    toast.error('触发失败: ' + e)
  }
}

async function updateArtifactStatus(id: string, status: string) {
  try {
    await request('forge', 'registry.update', { id, status })
    toast.success('状态已更新')
    await loadArtifacts()
  } catch (e: any) {
    toast.error('更新失败: ' + e)
  }
}

// ---------------------------------------------------------------------------
// Tab switching with lazy loading
// ---------------------------------------------------------------------------

function switchTab(tab: string) {
  activeTab.value = tab
  if (tab === 'experiences' && !expStats.value) loadExperiences()
  else if (tab === 'reflections' && reports.value.length === 0) {
    loadReflections()
    loadLatestReport()
  }
  else if (tab === 'cycles' && cycles.value.length === 0) loadCycles()
  else if (tab === 'artifacts' && artifacts.value.length === 0) loadArtifacts()
  else if (tab === 'config' && !configData.value) loadConfig()
}

// ---------------------------------------------------------------------------
// Computed
// ---------------------------------------------------------------------------

const sortedTools = computed(() => {
  if (!expStats.value?.tools) return []
  const tools = expStats.value.tools
  return Object.entries(tools)
    .map(([name, data]: [string, any]) => ({ name, ...data }))
    .sort((a, b) => b.count - a.count)
})

const successRate = computed(() => {
  if (!expStats.value) return 0
  const total = expStats.value.total || 0
  if (total === 0) return 0
  return ((expStats.value.success || 0) / total * 100).toFixed(1)
})

onMounted(loadAll)
</script>

<template>
  <div class="page-forge">
    <div class="page-header">
      <h2>Forge 自学习</h2>
      <div class="page-header-actions">
        <div class="toggle" :class="{ active: enabled }" @click="toggleForge"></div>
        <button class="btn" :disabled="!enabled" @click="triggerReflect">触发反思</button>
      </div>
    </div>

    <div class="page-body">
      <div v-if="loading" style="text-align: center; padding: var(--space-8);">
        <div class="spinner spinner-lg" style="margin: 0 auto;"></div>
      </div>

      <template v-if="!loading">
        <!-- Tabs -->
        <div class="tabs">
          <button class="tab" :class="{ active: activeTab === 'overview' }" @click="switchTab('overview')">概览</button>
          <button class="tab" :class="{ active: activeTab === 'experiences' }" @click="switchTab('experiences')">经验</button>
          <button class="tab" :class="{ active: activeTab === 'reflections' }" @click="switchTab('reflections')">反思</button>
          <button class="tab" :class="{ active: activeTab === 'cycles' }" @click="switchTab('cycles')">学习循环</button>
          <button class="tab" :class="{ active: activeTab === 'artifacts' }" @click="switchTab('artifacts')">Artifacts</button>
          <button class="tab" :class="{ active: activeTab === 'config' }" @click="switchTab('config')">配置</button>
        </div>

      <!-- ==================== Overview ==================== -->
      <div v-if="activeTab === 'overview'">
        <!-- Status indicator -->
        <div class="card forge-status-card" :class="{ 'forge-status-card--active': enabled }" style="margin-bottom: var(--space-4);">
          <div class="card-body" style="display: flex; align-items: center; justify-content: center; gap: var(--space-3);">
            <div style="width: 12px; height: 12px; border-radius: 50%;" :style="{ background: running ? 'var(--color-success)' : enabled ? 'var(--color-warning)' : 'var(--text-muted)' }"></div>
            <span style="font-weight: 600; font-size: 1.05rem;">{{ running ? '自学习系统运行中' : enabled ? '已启用，等待启动' : '自学习系统未启用' }}</span>
          </div>
        </div>

        <!-- Stat cards grid -->
        <div style="display: grid; grid-template-columns: repeat(auto-fill, minmax(160px, 1fr)); gap: var(--space-4); margin-bottom: var(--space-4);">
          <div class="card">
            <div class="card-body" style="text-align: center; padding: var(--space-4) var(--space-5);">
              <div style="font-size: 1.6rem; font-weight: 700; color: var(--accent);">{{ formatCount(stats?.experiences?.total || 0) }}</div>
              <div style="color: var(--text-secondary); font-size: var(--text-sm); margin-top: var(--space-1);">经验记录</div>
            </div>
          </div>
          <div class="card">
            <div class="card-body" style="text-align: center; padding: var(--space-4) var(--space-5);">
              <div style="font-size: 1.6rem; font-weight: 700; color: var(--color-info);">{{ stats?.reflections?.total || 0 }}</div>
              <div style="color: var(--text-secondary); font-size: var(--text-sm); margin-top: var(--space-1);">反思报告</div>
            </div>
          </div>
          <div class="card">
            <div class="card-body" style="text-align: center; padding: var(--space-4) var(--space-5);">
              <div style="font-size: 1.6rem; font-weight: 700; color: var(--color-success);">{{ stats?.artifacts?.total || 0 }}</div>
              <div style="color: var(--text-secondary); font-size: var(--text-sm); margin-top: var(--space-1);">学习产物</div>
            </div>
          </div>
          <div class="card">
            <div class="card-body" style="text-align: center; padding: var(--space-4) var(--space-5);">
              <div style="font-size: 1.6rem; font-weight: 700;">{{ stats?.cycles?.total || 0 }}</div>
              <div style="color: var(--text-secondary); font-size: var(--text-sm); margin-top: var(--space-1);">学习循环</div>
            </div>
          </div>
        </div>

        <!-- Detailed stats -->
        <div style="display: grid; grid-template-columns: 1fr 1fr; gap: var(--space-4);">
          <!-- Experience summary -->
          <div class="card">
            <div class="card-header"><h3>经验概要</h3></div>
            <div class="card-body">
              <div v-if="!stats?.experiences?.total" class="empty-state">
                <p>暂无经验数据</p>
              </div>
              <div v-else>
                <div style="display: flex; justify-content: space-between; margin-bottom: var(--space-3);">
                  <div>
                    <div style="font-size: var(--text-sm); color: var(--text-secondary);">成功率</div>
                    <div style="font-size: 1.2rem; font-weight: 600;">{{ ((stats.experiences.success / stats.experiences.total) * 100).toFixed(1) }}%</div>
                  </div>
                  <div>
                    <div style="font-size: var(--text-sm); color: var(--text-secondary);">平均耗时</div>
                    <div style="font-size: 1.2rem; font-weight: 600;">{{ formatDuration(stats.experiences.avg_duration_ms || 0) }}</div>
                  </div>
                  <div>
                    <div style="font-size: var(--text-sm); color: var(--text-secondary);">成功/失败</div>
                    <div style="font-size: 1.2rem; font-weight: 600;">
                      <span style="color: var(--color-success);">{{ stats.experiences.success }}</span> /
                      <span style="color: var(--color-error);">{{ stats.experiences.failure }}</span>
                    </div>
                  </div>
                </div>
                <!-- Tool breakdown (top 5) -->
                <div v-if="stats.experiences.tools" style="margin-top: var(--space-3);">
                  <div style="font-size: var(--text-xs); color: var(--text-muted); margin-bottom: var(--space-2); text-transform: uppercase;">工具使用分布</div>
                  <div v-for="(tool, idx) in Object.entries(stats.experiences.tools).slice(0, 5)" :key="idx" style="display: flex; align-items: center; gap: var(--space-2); margin-bottom: var(--space-1);">
                    <span style="font-size: var(--text-sm); width: 120px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap;">{{ tool[0] }}</span>
                    <div style="flex: 1; height: 6px; background: var(--border-light); border-radius: 3px; overflow: hidden;">
                      <div :style="{ width: (tool[1] as any).success_rate * 100 + '%', height: '100%', background: 'var(--color-success)', borderRadius: '3px' }"></div>
                    </div>
                    <span style="font-size: var(--text-xs); color: var(--text-muted); width: 40px; text-align: right;">{{ ((tool[1] as any).success_rate * 100).toFixed(0) }}%</span>
                  </div>
                </div>
              </div>
            </div>
          </div>

          <!-- Recent cycle -->
          <div class="card">
            <div class="card-header"><h3>最近学习循环</h3></div>
            <div class="card-body">
              <div v-if="!stats?.cycles?.last" class="empty-state">
                <p>暂无学习循环记录</p>
              </div>
              <div v-else>
                <div style="display: flex; justify-content: space-between; margin-bottom: var(--space-3);">
                  <div>
                    <div style="font-size: var(--text-sm); color: var(--text-secondary);">状态</div>
                    <span class="badge" :class="cycleStatusClass(stats.cycles.last.status)">{{ cycleStatusLabel(stats.cycles.last.status) }}</span>
                  </div>
                  <div>
                    <div style="font-size: var(--text-sm); color: var(--text-secondary);">发现模式</div>
                    <div style="font-size: 1.2rem; font-weight: 600;">{{ stats.cycles.last.patterns_found }}</div>
                  </div>
                  <div>
                    <div style="font-size: var(--text-sm); color: var(--text-secondary);">执行动作</div>
                    <div style="font-size: 1.2rem; font-weight: 600;">{{ stats.cycles.last.actions_taken }}</div>
                  </div>
                </div>
                <div style="font-size: var(--text-sm); color: var(--text-secondary);">
                  <div>开始: {{ formatDate(stats.cycles.last.started_at) }}</div>
                  <div v-if="stats.cycles.last.completed_at">完成: {{ formatDate(stats.cycles.last.completed_at) }}</div>
                </div>
              </div>
            </div>
          </div>
        </div>
      </div>

      <!-- ==================== Experiences ==================== -->
      <div v-if="activeTab === 'experiences'">
        <!-- Experience overview cards -->
        <div style="display: grid; grid-template-columns: repeat(auto-fill, minmax(180px, 1fr)); gap: var(--space-4); margin-bottom: var(--space-4);">
          <div class="card">
            <div class="card-body" style="text-align: center; padding: var(--space-4) var(--space-5);">
              <div style="font-size: 1.6rem; font-weight: 700;">{{ formatCount(expStats?.total || 0) }}</div>
              <div style="color: var(--text-secondary); font-size: var(--text-sm);">总经验数</div>
            </div>
          </div>
          <div class="card">
            <div class="card-body" style="text-align: center; padding: var(--space-4) var(--space-5);">
              <div style="font-size: 1.6rem; font-weight: 700; color: var(--color-success);">{{ successRate }}%</div>
              <div style="color: var(--text-secondary); font-size: var(--text-sm);">成功率</div>
            </div>
          </div>
          <div class="card">
            <div class="card-body" style="text-align: center; padding: var(--space-4) var(--space-5);">
              <div style="font-size: 1.6rem; font-weight: 700;">{{ formatDuration(expStats?.avg_duration_ms || 0) }}</div>
              <div style="color: var(--text-secondary); font-size: var(--text-sm);">平均耗时</div>
            </div>
          </div>
        </div>

        <!-- Tool breakdown table -->
        <div class="card" style="margin-bottom: var(--space-4);">
          <div class="card-header"><h3>工具统计</h3></div>
          <div class="card-body">
            <div v-if="sortedTools.length === 0" class="empty-state">
              <p>暂无经验数据</p>
            </div>
            <div v-else class="table-wrap">
              <table>
                <thead>
                  <tr>
                    <th>工具名称</th>
                    <th>调用次数</th>
                    <th>成功</th>
                    <th>失败</th>
                    <th>成功率</th>
                    <th>平均耗时</th>
                  </tr>
                </thead>
                <tbody>
                  <tr v-for="tool in sortedTools" :key="tool.name">
                    <td style="font-weight: 500;">{{ tool.name }}</td>
                    <td>{{ tool.count }}</td>
                    <td style="color: var(--color-success);">{{ tool.success }}</td>
                    <td style="color: var(--color-error);">{{ tool.failure }}</td>
                    <td>
                      <div style="display: flex; align-items: center; gap: var(--space-2);">
                        <div style="width: 60px; height: 6px; background: var(--border-light); border-radius: 3px; overflow: hidden;">
                          <div :style="{ width: tool.success_rate * 100 + '%', height: '100%', background: tool.success_rate >= 0.8 ? 'var(--color-success)' : tool.success_rate >= 0.5 ? 'var(--color-warning)' : 'var(--color-error)', borderRadius: '3px' }"></div>
                        </div>
                        <span style="font-size: var(--text-xs);">{{ (tool.success_rate * 100).toFixed(1) }}%</span>
                      </div>
                    </td>
                    <td>{{ formatDuration(tool.avg_duration_ms) }}</td>
                  </tr>
                </tbody>
              </table>
            </div>
          </div>
        </div>

        <!-- Recent experiences -->
        <div class="card">
          <div class="card-header"><h3>最近记录</h3></div>
          <div class="card-body">
            <div v-if="!expStats?.recent?.length" class="empty-state">
              <p>暂无经验记录</p>
            </div>
            <div v-else class="table-wrap">
              <table>
                <thead>
                  <tr>
                    <th>时间</th>
                    <th>工具</th>
                    <th>状态</th>
                    <th>耗时</th>
                    <th>输入</th>
                    <th>输出</th>
                  </tr>
                </thead>
                <tbody>
                  <tr v-for="(exp, idx) in expStats.recent" :key="idx">
                    <td style="white-space: nowrap; font-size: var(--text-xs); color: var(--text-secondary);">{{ formatDate(exp.timestamp) }}</td>
                    <td style="font-weight: 500;">{{ exp.tool_name }}</td>
                    <td>
                      <span class="badge" :class="exp.success ? 'badge-success' : 'badge-error'">{{ exp.success ? '成功' : '失败' }}</span>
                    </td>
                    <td>{{ formatDuration(exp.duration_ms) }}</td>
                    <td style="max-width: 200px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: var(--text-xs);">{{ exp.input_summary || '--' }}</td>
                    <td style="max-width: 200px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: var(--text-xs);">{{ exp.output_summary || '--' }}</td>
                  </tr>
                </tbody>
              </table>
            </div>
          </div>
        </div>
      </div>

      <!-- ==================== Reflections ==================== -->
      <div v-if="activeTab === 'reflections'">
        <!-- Latest report -->
        <div v-if="showLatestReport && latestReport?.content" class="card" style="margin-bottom: var(--space-4);">
          <div class="card-header">
            <h3>最新反思报告</h3>
            <span style="font-size: var(--text-sm); color: var(--text-secondary);">{{ latestReport.name }}</span>
          </div>
          <div class="card-body">
            <pre style="white-space: pre-wrap; font-size: var(--text-sm); line-height: 1.6; max-height: 400px; overflow-y: auto; background: var(--bg-secondary); padding: var(--space-4); border-radius: var(--radius-md);">{{ latestReport.content }}</pre>
          </div>
        </div>

        <!-- Report list -->
        <div class="card">
          <div class="card-header"><h3>反思报告列表</h3></div>
          <div class="card-body">
            <div v-if="reports.length === 0" class="empty-state">
              <p>暂无反思报告，点击「触发反思」生成第一份报告</p>
            </div>
            <div v-else class="table-wrap">
              <table>
                <thead>
                  <tr>
                    <th>报告文件</th>
                    <th>日期</th>
                    <th>大小</th>
                    <th>修改时间</th>
                  </tr>
                </thead>
                <tbody>
                  <tr v-for="(r, idx) in reports" :key="idx">
                    <td style="font-weight: 500;">{{ r.name }}</td>
                    <td>{{ r.date || '--' }}</td>
                    <td>{{ r.size ? (r.size < 1024 ? r.size + ' B' : (r.size / 1024).toFixed(1) + ' KB') : '--' }}</td>
                    <td style="font-size: var(--text-xs); color: var(--text-secondary);">{{ formatDate(r.modified) }}</td>
                  </tr>
                </tbody>
              </table>
            </div>
          </div>
        </div>
      </div>

      <!-- ==================== Learning Cycles ==================== -->
      <div v-if="activeTab === 'cycles'">
        <div class="card">
          <div class="card-header">
            <h3>学习循环记录</h3>
            <span style="font-size: var(--text-sm); color: var(--text-secondary);">共 {{ cycles.length }} 条</span>
          </div>
          <div class="card-body">
            <div v-if="cycles.length === 0" class="empty-state">
              <p>暂无学习循环记录</p>
            </div>
            <div v-else class="table-wrap">
              <table>
                <thead>
                  <tr>
                    <th>ID</th>
                    <th>状态</th>
                    <th>发现模式</th>
                    <th>执行动作</th>
                    <th>开始时间</th>
                    <th>完成时间</th>
                  </tr>
                </thead>
                <tbody>
                  <tr v-for="(c, idx) in cycles" :key="idx">
                    <td style="font-family: monospace; font-size: var(--text-xs);">{{ (c.id || '').substring(0, 8) }}</td>
                    <td><span class="badge" :class="cycleStatusClass(c.status)">{{ cycleStatusLabel(c.status) }}</span></td>
                    <td>
                      <span v-if="c.patterns_found > 0" style="color: var(--accent); font-weight: 500;">{{ c.patterns_found }}</span>
                      <span v-else>0</span>
                    </td>
                    <td>{{ c.actions_taken }}</td>
                    <td style="font-size: var(--text-xs); color: var(--text-secondary);">{{ formatDate(c.started_at) }}</td>
                    <td style="font-size: var(--text-xs); color: var(--text-secondary);">{{ c.completed_at ? formatDate(c.completed_at) : '--' }}</td>
                  </tr>
                </tbody>
              </table>
            </div>
          </div>
        </div>
      </div>

      <!-- ==================== Artifacts ==================== -->
      <div v-if="activeTab === 'artifacts'">
        <!-- Registry artifacts -->
        <div class="card" style="margin-bottom: var(--space-4);">
          <div class="card-header">
            <h3>注册表</h3>
            <span style="font-size: var(--text-sm); color: var(--text-secondary);">共 {{ artifacts.length }} 个</span>
          </div>
          <div class="card-body">
            <div v-if="artifacts.length === 0" class="empty-state">
              <p>暂无注册的学习产物</p>
            </div>
            <div v-else class="table-wrap">
              <table>
                <thead>
                  <tr>
                    <th>名称</th>
                    <th>类型</th>
                    <th>状态</th>
                    <th>版本</th>
                    <th>使用次数</th>
                    <th>成功率</th>
                    <th>更新时间</th>
                    <th>操作</th>
                  </tr>
                </thead>
                <tbody>
                  <tr v-for="(a, idx) in artifacts" :key="idx">
                    <td style="font-weight: 500;">{{ a.name || '--' }}</td>
                    <td><span class="badge badge-info">{{ a.kind || '--' }}</span></td>
                    <td><span class="badge" :class="statusClass(a.status)">{{ statusLabel(a.status) }}</span></td>
                    <td>{{ a.version || '--' }}</td>
                    <td>{{ a.usage_count || 0 }}</td>
                    <td>{{ a.success_rate ? (a.success_rate * 100).toFixed(1) + '%' : '--' }}</td>
                    <td style="font-size: var(--text-xs); color: var(--text-secondary);">{{ formatDate(a.updated_at) }}</td>
                    <td>
                      <select
                        :value="a.status"
                        @change="(e: Event) => updateArtifactStatus(a.id, (e.target as HTMLSelectElement).value)"
                        style="font-size: var(--text-xs); padding: 2px 4px; border-radius: var(--radius-sm); border: 1px solid var(--border); background: var(--surface);"
                      >
                        <option value="Draft">草稿</option>
                        <option value="Active">活跃</option>
                        <option value="Observing">观察中</option>
                        <option value="Degraded">降级</option>
                        <option value="Negative">负面</option>
                        <option value="Archived">归档</option>
                      </select>
                    </td>
                  </tr>
                </tbody>
              </table>
            </div>
          </div>
        </div>

        <!-- Skill directories -->
        <div v-if="skillDirectories.length > 0" class="card">
          <div class="card-header"><h3>技能目录</h3></div>
          <div class="card-body">
            <div class="table-wrap">
              <table>
                <thead>
                  <tr><th>目录名</th><th>SKILL.md</th></tr>
                </thead>
                <tbody>
                  <tr v-for="(s, idx) in skillDirectories" :key="idx">
                    <td style="font-weight: 500;">{{ s.name }}</td>
                    <td>
                      <span class="badge" :class="s.has_skill_md ? 'badge-success' : 'badge-neutral'">
                        {{ s.has_skill_md ? '存在' : '缺失' }}
                      </span>
                    </td>
                  </tr>
                </tbody>
              </table>
            </div>
          </div>
        </div>
      </div>

      <!-- ==================== Configuration ==================== -->
      <div v-if="activeTab === 'config'">
        <div class="card">
          <div class="card-header"><h3>Forge 配置</h3></div>
          <div class="card-body">
            <div v-if="!configData" class="empty-state">
              <p>无法加载配置信息</p>
            </div>
            <div v-else>
              <div style="display: grid; grid-template-columns: 1fr 1fr; gap: var(--space-4);">
                <!-- Collection -->
                <div>
                  <h4 style="font-size: var(--text-sm); color: var(--text-muted); margin-bottom: var(--space-2);">采集</h4>
                  <div style="display: flex; flex-direction: column; gap: var(--space-2);">
                    <div style="display: flex; justify-content: space-between;">
                      <span style="color: var(--text-secondary);">刷新间隔</span>
                      <span>{{ formatDuration(configData.collection_flush_secs * 1000) }}</span>
                    </div>
                    <div style="display: flex; justify-content: space-between;">
                      <span style="color: var(--text-secondary);">经验保留天数</span>
                      <span>{{ configData.max_experience_age_days }} 天</span>
                    </div>
                  </div>
                </div>

                <!-- Reflection -->
                <div>
                  <h4 style="font-size: var(--text-sm); color: var(--text-muted); margin-bottom: var(--space-2);">反思</h4>
                  <div style="display: flex; flex-direction: column; gap: var(--space-2);">
                    <div style="display: flex; justify-content: space-between;">
                      <span style="color: var(--text-secondary);">反思间隔</span>
                      <span>{{ (configData.reflection_interval_secs / 3600).toFixed(1) }} 小时</span>
                    </div>
                    <div style="display: flex; justify-content: space-between;">
                      <span style="color: var(--text-secondary);">清理间隔</span>
                      <span>{{ (configData.cleanup_interval_secs / 3600).toFixed(1) }} 小时</span>
                    </div>
                  </div>
                </div>

                <!-- Learning -->
                <div>
                  <h4 style="font-size: var(--text-sm); color: var(--text-muted); margin-bottom: var(--space-2);">学习</h4>
                  <div style="display: flex; flex-direction: column; gap: var(--space-2);">
                    <div style="display: flex; justify-content: space-between;">
                      <span style="color: var(--text-secondary);">学习开关</span>
                      <span class="badge" :class="configData.learning_enabled ? 'badge-success' : 'badge-neutral'">{{ configData.learning_enabled ? '启用' : '禁用' }}</span>
                    </div>
                    <div style="display: flex; justify-content: space-between;">
                      <span style="color: var(--text-secondary);">最小模式频率</span>
                      <span>{{ configData.min_pattern_frequency }}</span>
                    </div>
                    <div style="display: flex; justify-content: space-between;">
                      <span style="color: var(--text-secondary);">单循环最大创建数</span>
                      <span>{{ configData.max_auto_creates }}</span>
                    </div>
                  </div>
                </div>
              </div>
            </div>
          </div>
        </div>
      </div>
    </template>
    </div>
  </div>
</template>

<style scoped>
.forge-status-card--active {
  background-color: var(--success-bg);
}
</style>
