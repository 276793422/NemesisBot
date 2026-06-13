<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted } from 'vue'
import { useWSAPI } from '../../composables/useWSAPI'
import { useToast } from '../../composables/useToast'

const { request } = useWSAPI()
const toast = useToast()

const nodes = ref<any[]>([])
const selectedNodeId = ref<string | null>(null)
const diagResults = ref<Record<string, any>>({})
const runningDiag = ref<string | null>(null)
const expandedSections = ref<Record<string, boolean>>({
  get_info: true,
  'diagnostics.system': true,
  'diagnostics.network': true,
  'diagnostics.cluster_state': true,
  ping: true,
})

// Remote command (peer_chat)
const commandInput = ref('')
const commandResult = ref('')
const commandRunning = ref(false)

let refreshTimer: ReturnType<typeof setInterval> | null = null

const selectedNode = computed(() =>
  nodes.value.find(n => n.id === selectedNodeId.value)
)

const diagActions = [
  { id: 'get_info', label: '基本信息' },
  { id: 'diagnostics.system', label: '系统信息' },
  { id: 'diagnostics.network', label: '网络接口' },
  { id: 'diagnostics.cluster_state', label: '集群视角' },
]

async function loadNodes() {
  try {
    const data = await request('cluster', 'nodes.list')
    if (data?.nodes) nodes.value = data.nodes
  } catch { /* ignore */ }
}

function selectNode(id: string) {
  selectedNodeId.value = id
  diagResults.value = {}
  commandResult.value = ''
}

async function runDiagnostic(action: string) {
  if (!selectedNodeId.value) return
  runningDiag.value = action
  try {
    const data = await request('cluster', 'diagnostics.run', {
      node_id: selectedNodeId.value,
      action,
    })
    diagResults.value[action] = data
    expandedSections.value[action] = true
  } catch (e) {
    diagResults.value[action] = { error: String(e) }
    expandedSections.value[action] = true
  }
  runningDiag.value = null
}

async function runPing() {
  if (!selectedNodeId.value) return
  runningDiag.value = 'ping'
  try {
    const data = await request('cluster', 'nodes.ping', {
      node_id: selectedNodeId.value,
    })
    diagResults.value['ping'] = data
    expandedSections.value['ping'] = true
  } catch (e) {
    diagResults.value['ping'] = { error: String(e) }
    expandedSections.value['ping'] = true
  }
  runningDiag.value = null
}

async function runAllDiagnostics() {
  if (!selectedNodeId.value) return
  for (const action of ['get_info', 'diagnostics.system', 'diagnostics.network', 'diagnostics.cluster_state']) {
    await runDiagnostic(action)
  }
  await runPing()
  toast.success('全部诊断完成')
}

async function sendCommand() {
  commandRunning.value = true
  commandResult.value = '正在发送...'
  const input = commandInput.value
  commandInput.value = ''
  try {
    const data = await request('cluster', 'tasks.submit', {
      target_node_id: selectedNodeId.value,
      content: input,
    })
    const taskId = data.task_id
    commandResult.value = `任务已提交 (${taskId})，等待远程节点响应...`

    const maxAttempts = 60
    for (let i = 0; i < maxAttempts; i++) {
      await new Promise(r => setTimeout(r, 2000))
      try {
        const detail = await request('cluster', 'tasks.detail', { task_id: taskId })
        if (detail.status === 'completed') {
          const r = detail.result
          commandResult.value = typeof r === 'string' ? r : (r?.response || r?.error || JSON.stringify(r) || '(无输出)')
          break
        } else if (detail.status === 'failed') {
          const r = detail.result
          commandResult.value = `任务失败: ${typeof r === 'string' ? r : (r?.error || r?.response || JSON.stringify(r) || '未知错误')}`
          break
        }
        commandResult.value = `等待中... (${(i + 1) * 2}s)`
      } catch { /* poll error, continue */ }
    }
    if (commandResult.value.startsWith('等待中')) {
      commandResult.value = '任务超时（120s），请到「任务」页查看'
    }
  } catch (e: any) {
    commandResult.value = `错误: ${e || '未知'}`
  }
  commandRunning.value = false
}

function toggleSection(key: string) {
  expandedSections.value[key] = !expandedSections.value[key]
}

function formatBytes(bytes: number): string {
  if (bytes === 0) return '0 B'
  const units = ['B', 'KB', 'MB', 'GB', 'TB']
  const i = Math.floor(Math.log(bytes) / Math.log(1024))
  return (bytes / Math.pow(1024, i)).toFixed(1) + ' ' + units[i]
}

function formatUptime(secs: number): string {
  const d = Math.floor(secs / 86400)
  const h = Math.floor((secs % 86400) / 3600)
  const m = Math.floor((secs % 3600) / 60)
  if (d > 0) return `${d}d ${h}h ${m}m`
  if (h > 0) return `${h}h ${m}m`
  return `${m}m`
}

function hasResult(key: string): boolean {
  return key in diagResults.value
}

function hasError(key: string): boolean {
  return !!diagResults.value[key]?.error
}

onMounted(() => {
  loadNodes()
  refreshTimer = setInterval(loadNodes, 10000)
})
onUnmounted(() => {
  if (refreshTimer) clearInterval(refreshTimer)
})
</script>

<template>
  <div>
    <!-- Node selector -->
    <div class="card">
      <div class="card-header">
        <h3>选择节点</h3>
        <span class="text-muted text-sm">{{ nodes.length }} 个节点</span>
      </div>
      <div class="card-body">
        <div v-if="!nodes.length" class="empty-state">
          <p>暂无节点</p>
        </div>
        <div v-else class="node-list">
          <div
            v-for="node in nodes"
            :key="node.id"
            class="node-chip"
            :class="{
              selected: selectedNodeId === node.id,
              offline: !node.online,
            }"
            @click="selectNode(node.id)"
          >
            <span class="status-dot" :class="node.online ? 'online' : 'offline'" />
            <span class="node-name">{{ node.name || node.id.slice(0, 8) }}</span>
            <span v-if="node.isLocal" class="local-badge">本机</span>
          </div>
        </div>
      </div>
    </div>

    <!-- Action bar -->
    <div v-if="selectedNodeId" class="card action-bar">
      <div class="card-body" style="display:flex;gap:var(--space-2);flex-wrap:wrap;align-items:center">
        <button class="btn btn-primary" :disabled="!!runningDiag" @click="runAllDiagnostics">
          {{ runningDiag ? '诊断中...' : '全部诊断' }}
        </button>
        <button
          v-for="act in diagActions"
          :key="act.id"
          class="btn btn-sm"
          :disabled="!!runningDiag"
          @click="runDiagnostic(act.id)"
        >
          {{ act.label }}
        </button>
        <button class="btn btn-sm" :disabled="!!runningDiag" @click="runPing">
          Ping
        </button>
        <span v-if="runningDiag" class="text-muted text-sm">正在执行: {{ runningDiag }}</span>
      </div>
    </div>

    <!-- Results + Command panels -->
    <div v-if="selectedNodeId" class="diag-panels">
      <!-- Left: Results -->
      <div class="card">
        <div class="card-header"><h3>诊断结果</h3></div>
        <div class="card-body">
          <div v-if="Object.keys(diagResults).length === 0" class="empty-state">
            <p>点击上方按钮开始诊断</p>
          </div>

          <!-- Ping -->
          <div v-if="hasResult('ping')" class="diag-section">
            <div class="diag-section-header" @click="toggleSection('ping')">
              <span class="toggle-icon">{{ expandedSections['ping'] ? '▾' : '▸' }}</span>
              <span class="section-title">Ping</span>
              <span v-if="hasError('ping')" class="badge badge-error">失败</span>
              <span v-else-if="diagResults['ping']?.latency != null" class="badge badge-success">
                {{ diagResults['ping'].latency }}ms
              </span>
            </div>
            <div v-if="expandedSections['ping']" class="diag-section-body">
              <div v-if="hasError('ping')" class="diag-error">{{ diagResults['ping'].error }}</div>
              <div v-else class="diag-kv">
                <div class="diag-row">
                  <span class="diag-key">延迟</span>
                  <span class="diag-value">{{ diagResults['ping'].latency }}ms</span>
                </div>
              </div>
            </div>
          </div>

          <!-- get_info -->
          <div v-if="hasResult('get_info')" class="diag-section">
            <div class="diag-section-header" @click="toggleSection('get_info')">
              <span class="toggle-icon">{{ expandedSections['get_info'] ? '▾' : '▸' }}</span>
              <span class="section-title">基本信息</span>
              <span v-if="hasError('get_info')" class="badge badge-error">失败</span>
            </div>
            <div v-if="expandedSections['get_info']" class="diag-section-body">
              <div v-if="hasError('get_info')" class="diag-error">{{ diagResults['get_info'].error }}</div>
              <div v-else class="diag-kv">
                <div class="diag-row" v-for="key in ['name', 'role', 'category', 'status']" :key="key">
                  <span class="diag-key">{{ key }}</span>
                  <span class="diag-value">{{ diagResults['get_info'][key] }}</span>
                </div>
                <div class="diag-row">
                  <span class="diag-key">addresses</span>
                  <span class="diag-value">{{ (diagResults['get_info'].addresses || []).join(', ') }}</span>
                </div>
                <div class="diag-row">
                  <span class="diag-key">capabilities</span>
                  <span class="diag-value">{{ (diagResults['get_info'].capabilities || []).join(', ') || '(none)' }}</span>
                </div>
              </div>
            </div>
          </div>

          <!-- diagnostics.system -->
          <div v-if="hasResult('diagnostics.system')" class="diag-section">
            <div class="diag-section-header" @click="toggleSection('diagnostics.system')">
              <span class="toggle-icon">{{ expandedSections['diagnostics.system'] ? '▾' : '▸' }}</span>
              <span class="section-title">系统信息</span>
              <span v-if="hasError('diagnostics.system')" class="badge badge-error">失败</span>
            </div>
            <div v-if="expandedSections['diagnostics.system']" class="diag-section-body">
              <div v-if="hasError('diagnostics.system')" class="diag-error">{{ diagResults['diagnostics.system'].error }}</div>
              <div v-else class="diag-kv">
                <div class="diag-row">
                  <span class="diag-key">OS</span>
                  <span class="diag-value">{{ diagResults['diagnostics.system'].os_version }}</span>
                </div>
                <div class="diag-row">
                  <span class="diag-key">架构</span>
                  <span class="diag-value">{{ diagResults['diagnostics.system'].arch }}</span>
                </div>
                <div class="diag-row">
                  <span class="diag-key">主机名</span>
                  <span class="diag-value">{{ diagResults['diagnostics.system'].hostname }}</span>
                </div>
                <div class="diag-row">
                  <span class="diag-key">内存</span>
                  <span class="diag-value">
                    {{ formatBytes(diagResults['diagnostics.system'].memory_used_bytes) }} /
                    {{ formatBytes(diagResults['diagnostics.system'].memory_total_bytes) }}
                  </span>
                </div>
                <div class="diag-row">
                  <span class="diag-key">运行时间</span>
                  <span class="diag-value">{{ formatUptime(diagResults['diagnostics.system'].uptime_secs) }}</span>
                </div>
              </div>
            </div>
          </div>

          <!-- diagnostics.network -->
          <div v-if="hasResult('diagnostics.network')" class="diag-section">
            <div class="diag-section-header" @click="toggleSection('diagnostics.network')">
              <span class="toggle-icon">{{ expandedSections['diagnostics.network'] ? '▾' : '▸' }}</span>
              <span class="section-title">网络接口</span>
              <span v-if="hasError('diagnostics.network')" class="badge badge-error">失败</span>
            </div>
            <div v-if="expandedSections['diagnostics.network']" class="diag-section-body">
              <div v-if="hasError('diagnostics.network')" class="diag-error">{{ diagResults['diagnostics.network'].error }}</div>
              <div v-else>
                <table class="diag-table" v-if="diagResults['diagnostics.network'].interfaces?.length">
                  <thead>
                    <tr><th>IP</th><th>掩码</th><th>网段</th></tr>
                  </thead>
                  <tbody>
                    <tr v-for="iface in diagResults['diagnostics.network'].interfaces" :key="iface.ip">
                      <td style="font-family:var(--font-mono)">{{ iface.ip }}</td>
                      <td style="font-family:var(--font-mono)">{{ iface.mask }}</td>
                      <td style="font-family:var(--font-mono)">{{ iface.network_ip }}</td>
                    </tr>
                  </tbody>
                </table>
                <div class="diag-row" style="margin-top:var(--space-2)">
                  <span class="diag-key">所有 IP</span>
                  <span class="diag-value">{{ (diagResults['diagnostics.network'].all_ips || []).join(', ') }}</span>
                </div>
              </div>
            </div>
          </div>

          <!-- diagnostics.cluster_state -->
          <div v-if="hasResult('diagnostics.cluster_state')" class="diag-section">
            <div class="diag-section-header" @click="toggleSection('diagnostics.cluster_state')">
              <span class="toggle-icon">{{ expandedSections['diagnostics.cluster_state'] ? '▾' : '▸' }}</span>
              <span class="section-title">集群视角</span>
              <span v-if="hasError('diagnostics.cluster_state')" class="badge badge-error">失败</span>
              <span v-else class="badge badge-info">
                {{ diagResults['diagnostics.cluster_state']?.online_count }}/{{ diagResults['diagnostics.cluster_state']?.node_count }}
              </span>
            </div>
            <div v-if="expandedSections['diagnostics.cluster_state']" class="diag-section-body">
              <div v-if="hasError('diagnostics.cluster_state')" class="diag-error">{{ diagResults['diagnostics.cluster_state'].error }}</div>
              <div v-else>
                <table class="diag-table" v-if="diagResults['diagnostics.cluster_state'].nodes?.length">
                  <thead>
                    <tr><th>名称</th><th>地址</th><th>角色</th><th>最后可见</th></tr>
                  </thead>
                  <tbody>
                    <tr v-for="n in diagResults['diagnostics.cluster_state'].nodes" :key="n.id">
                      <td>{{ n.name }}</td>
                      <td style="font-family:var(--font-mono)">{{ n.address }}</td>
                      <td>{{ n.role }}</td>
                      <td>{{ n.last_seen }}</td>
                    </tr>
                  </tbody>
                </table>
                <div v-else class="text-muted text-sm">无在线节点</div>
              </div>
            </div>
          </div>
        </div>
      </div>

      <!-- Right: Remote command -->
      <div class="card">
        <div class="card-header"><h3>远程命令</h3></div>
        <div class="card-body">
          <p class="text-muted text-sm" style="margin-bottom:var(--space-3)">
            通过 Agent 向 {{ selectedNode?.name || selectedNodeId?.slice(0, 8) }} 发送自然语言指令
          </p>
          <div style="display:flex;gap:var(--space-2);margin-bottom:var(--space-3)">
            <input
              class="form-input"
              v-model="commandInput"
              placeholder="如: 查看 /etc/resolv.conf"
              :disabled="commandRunning"
              @keydown.enter="sendCommand"
              style="flex:1"
            />
            <button class="btn btn-primary" :disabled="commandRunning || !commandInput" @click="sendCommand">
              {{ commandRunning ? '发送中...' : '发送' }}
            </button>
          </div>
          <div v-if="commandResult" class="command-output">
            <pre>{{ commandResult }}</pre>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.node-list {
  display: flex;
  gap: var(--space-2);
  flex-wrap: wrap;
}

.node-chip {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  padding: var(--space-2) var(--space-3);
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
  cursor: pointer;
  transition: all 0.15s;
  user-select: none;
}

.node-chip:hover {
  border-color: var(--primary);
}

.node-chip.selected {
  border-color: var(--primary);
  background: color-mix(in srgb, var(--primary) 10%, transparent);
}

.node-chip.offline {
  opacity: 0.5;
}

.status-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  flex-shrink: 0;
}

.status-dot.online { background: var(--success); }
.status-dot.offline { background: var(--text-muted); }

.node-name {
  font-size: var(--text-sm);
  font-weight: 500;
}

.local-badge {
  font-size: var(--text-xs);
  padding: 1px 6px;
  border-radius: var(--radius-sm);
  background: color-mix(in srgb, var(--primary) 15%, transparent);
  color: var(--primary);
}

.action-bar {
  margin-top: var(--space-3);
}

.diag-panels {
  display: grid;
  grid-template-columns: 3fr 2fr;
  gap: var(--space-4);
  margin-top: var(--space-3);
}

@media (max-width: 900px) {
  .diag-panels {
    grid-template-columns: 1fr;
  }
}

/* Diag sections */
.diag-section {
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
  margin-bottom: var(--space-2);
}

.diag-section-header {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  padding: var(--space-2) var(--space-3);
  cursor: pointer;
  user-select: none;
}

.diag-section-header:hover {
  background: var(--surface-alt);
}

.toggle-icon {
  font-size: var(--text-sm);
  color: var(--text-muted);
  width: 14px;
}

.section-title {
  font-weight: 500;
  flex: 1;
}

.diag-section-body {
  padding: 0 var(--space-3) var(--space-3);
}

.diag-kv {
  display: flex;
  flex-direction: column;
  gap: var(--space-1);
}

.diag-row {
  display: flex;
  gap: var(--space-3);
  font-size: var(--text-sm);
}

.diag-key {
  color: var(--text-muted);
  min-width: 80px;
  flex-shrink: 0;
}

.diag-value {
  word-break: break-all;
}

.diag-error {
  color: var(--error);
  font-size: var(--text-sm);
}

.diag-table {
  width: 100%;
  font-size: var(--text-sm);
}

.diag-table th {
  text-align: left;
  padding: var(--space-1) var(--space-2);
  border-bottom: 1px solid var(--border);
  color: var(--text-muted);
  font-weight: 500;
}

.diag-table td {
  padding: var(--space-1) var(--space-2);
}

.badge {
  font-size: var(--text-xs);
  padding: 1px 6px;
  border-radius: var(--radius-sm);
}

.badge-error {
  background: color-mix(in srgb, var(--error) 15%, transparent);
  color: var(--error);
}

.badge-success {
  background: color-mix(in srgb, var(--success) 15%, transparent);
  color: var(--success);
}

.badge-info {
  background: color-mix(in srgb, var(--primary) 15%, transparent);
  color: var(--primary);
}

.command-output {
  background: var(--surface-alt);
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
  padding: var(--space-3);
  max-height: 400px;
  overflow-y: auto;
}

.command-output pre {
  white-space: pre-wrap;
  word-break: break-word;
  font-family: var(--font-mono);
  font-size: var(--text-sm);
  margin: 0;
}
</style>
