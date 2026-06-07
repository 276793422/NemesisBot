<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { useWSAPI } from '../../composables/useWSAPI'
import { useToast } from '../../composables/useToast'

const { request } = useWSAPI()
const toast = useToast()

const loading = ref(true)
const saving = ref(false)

const masterEnabled = ref(false)
const enabled = ref(false)
const port = ref(11949)
const rpcPort = ref(21949)
const broadcastInterval = ref(30)
const llmTimeout = ref(7200)
const authToken = ref('')

const snapshots = ref<any[]>([])
const snapshotsLoading = ref(false)

async function loadConfig() {
  try {
    const data = await request('cluster', 'config.get')
    if (!data) return
    masterEnabled.value = data.master_enabled ?? false
    enabled.value = data.enabled ?? false
    port.value = data.port ?? 11949
    rpcPort.value = data.rpc_port ?? 21949
    broadcastInterval.value = data.broadcast_interval ?? 30
    llmTimeout.value = data.llm_timeout_secs ?? 7200
    authToken.value = data.token ?? ''
  } catch { /* ignore */ }
}

async function loadSnapshots() {
  snapshotsLoading.value = true
  try {
    const data = await request('cluster', 'snapshots.list')
    if (data?.snapshots) snapshots.value = data.snapshots
  } catch { /* ignore */ }
  snapshotsLoading.value = false
}

async function toggleMasterEnabled() {
  const newVal = !masterEnabled.value
  try {
    await request('cluster', 'config.set_master_enabled', { enabled: newVal })
    masterEnabled.value = newVal
    toast.success(newVal ? '集群功能已启用' : '集群功能已禁用')
  } catch (e: any) {
    toast.error('操作失败: ' + (e || '未知错误'))
  }
}

async function saveConfig() {
  saving.value = true
  try {
    await request('cluster', 'config.save', {
      enabled: enabled.value,
      port: port.value,
      rpc_port: rpcPort.value,
      broadcast_interval: broadcastInterval.value,
      llm_timeout_secs: llmTimeout.value,
      token: authToken.value,
    })
    toast.success('配置已保存')
  } catch (e: any) {
    toast.error('保存失败: ' + (e || '未知错误'))
  }
  saving.value = false
}

function resetConfig() {
  loadConfig()
}

async function cleanupSnapshots() {
  try {
    await request('cluster', 'snapshots.cleanup')
    toast.success('快照已清理')
    await loadSnapshots()
  } catch (e: any) {
    toast.error('清理失败: ' + (e || '未知错误'))
  }
}

onMounted(async () => {
  await Promise.all([loadConfig(), loadSnapshots()])
  loading.value = false
})
</script>

<template>
  <div v-if="loading" style="text-align:center;padding:var(--space-8)">
    <div class="spinner spinner-lg" style="margin:0 auto" />
  </div>

  <div v-if="!loading">
    <div class="card" style="margin-bottom:var(--space-4)">
      <div class="card-header"><h3>基础设置</h3></div>
      <div class="card-body">
        <div class="form-group">
          <label class="form-label">
            启用集群
            <span class="form-hint" title="总开关：启用后，集群功能可在概览页启动。重启 Gateway 后也会自动生效。">ⓘ</span>
          </label>
          <div
            class="toggle"
            :class="{ active: masterEnabled }"
            @click="toggleMasterEnabled"
            title="总开关：启用后，集群功能可在概览页启动。重启 Gateway 后也会自动生效。"
            style="cursor:pointer"
          ></div>
        </div>
        <div class="form-group">
          <label class="form-label">发现端口</label>
          <input class="form-input" type="number" v-model.number="port" style="width:120px" />
        </div>
        <div class="form-group">
          <label class="form-label">RPC 端口</label>
          <input class="form-input" type="number" v-model.number="rpcPort" style="width:120px" />
        </div>
        <div class="form-group">
          <label class="form-label">广播间隔 (秒)</label>
          <input class="form-input" type="number" v-model.number="broadcastInterval" style="width:120px" />
        </div>
        <div class="form-group">
          <label class="form-label">LLM 超时 (秒)</label>
          <input class="form-input" type="number" v-model.number="llmTimeout" style="width:120px" />
        </div>
        <div class="form-group">
          <label class="form-label">认证 Token</label>
          <div style="display:flex;gap:var(--space-2);align-items:center">
            <input class="form-input" type="password" :value="authToken" readonly style="width:240px" />
            <button class="btn btn-sm" @click="authToken = crypto.randomUUID()">重新生成</button>
          </div>
        </div>
        <div style="display:flex;gap:var(--space-2);margin-top:var(--space-4)">
          <button class="btn btn-primary" :disabled="saving" @click="saveConfig">
            {{ saving ? '保存中...' : '保存' }}
          </button>
          <button class="btn" @click="resetConfig">重置</button>
        </div>
      </div>
    </div>

    <div class="card">
      <div class="card-header">
        <h3>续行快照管理</h3>
        <button class="btn btn-sm btn-danger" :disabled="!snapshots.length" @click="cleanupSnapshots">清理全部</button>
      </div>
      <div class="card-body">
        <div v-if="snapshotsLoading" style="text-align:center;padding:var(--space-4)">
          <div class="spinner" style="margin:0 auto" />
        </div>
        <div v-else-if="!snapshots.length" class="empty-state" style="padding:var(--space-4)">
          <p>暂无快照文件</p>
        </div>
        <div v-else class="table-wrap">
          <table>
            <thead>
              <tr><th>文件名</th><th>大小</th><th>创建时间</th></tr>
            </thead>
            <tbody>
              <tr v-for="snap in snapshots" :key="snap.name">
                <td style="font-family:var(--font-mono);font-size:var(--text-xs)">{{ snap.name }}</td>
                <td>{{ snap.size }}</td>
                <td>{{ snap.created }}</td>
              </tr>
            </tbody>
          </table>
        </div>
      </div>
    </div>
  </div>
</template>
