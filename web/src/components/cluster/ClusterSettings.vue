<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
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
// Snapshot of the token as last loaded/saved. "Regenerate" / editing only
// mutates `authToken`; nothing is persisted until the user clicks 保存. This
// drives the "modified, not yet saved" hint.
const savedAuthToken = ref('')
const authTokenDirty = computed(() => authToken.value !== savedAuthToken.value)

const snapshots = ref<any[]>([])
const snapshotsLoading = ref(false)

// Firewall diagnostics
const checking = ref(false)
const addingRules = ref(false)
const firewallResults = ref<any>(null)
const ruleResult = ref<any>(null)

const testNames: Record<string, string> = {
  udp_bind: 'UDP 端口绑定',
  broadcast_flag: '广播标志',
  broadcast_loopback: '广播回环',
  tcp_bind: 'TCP 端口绑定',
  firewall_status: '防火墙状态',
}

function regenerateAuthToken() {
  authToken.value = crypto.randomUUID()
}

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
    savedAuthToken.value = authToken.value
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
    savedAuthToken.value = authToken.value
    toast.success('配置已保存（Token 需重启 Gateway 后生效）')
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

async function checkFirewall() {
  checking.value = true
  firewallResults.value = null
  ruleResult.value = null
  try {
    const data = await request('cluster', 'firewall.check')
    firewallResults.value = data
    if (!data.all_pass) {
      toast.warn('网络检测发现问题，请查看详情')
    } else {
      toast.success('网络检测通过')
    }
  } catch (e: any) {
    toast.error('检测失败: ' + (e || '未知错误'))
  }
  checking.value = false
}

async function addFirewallRules() {
  addingRules.value = true
  ruleResult.value = null
  try {
    const data = await request('cluster', 'firewall.add_rules', {
      udp_port: port.value,
      tcp_port: rpcPort.value,
    })
    ruleResult.value = data
    if (data.success) {
      toast.success('防火墙规则已添加')
    } else if (data.uac_triggered) {
      toast.info('请确认 UAC 弹窗以添加防火墙规则')
    } else if (data.permission_denied) {
      toast.warn('权限不足，请查看手动命令')
    } else {
      toast.error(data.message || '添加失败')
    }
  } catch (e: any) {
    toast.error('添加失败: ' + (e || '未知错误'))
  }
  addingRules.value = false
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
    <div class="settings-two-col">
      <!-- Left: Basic Settings -->
      <div class="card">
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
            <label class="form-label">
              认证 Token
              <span class="form-hint" title="集群握手与 RPC 通信的鉴权密钥，所有节点必须一致。重新生成或手动修改后，需点击「保存」并重启 Gateway 才生效。">ⓘ</span>
            </label>
            <div style="display:flex;gap:var(--space-2);align-items:center">
              <input
                class="form-input"
                type="text"
                v-model="authToken"
                spellcheck="false"
                autocomplete="off"
                style="width:300px;font-family:var(--font-mono);font-size:var(--text-sm)"
              />
              <button class="btn btn-sm" @click="regenerateAuthToken()">重新生成</button>
            </div>
            <div v-if="authTokenDirty" class="auth-token-dirty">
              ⚠ Token 已修改但未保存，点击下方「保存」生效；保存后需重启 Gateway。
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

      <!-- Right: Network Diagnostics -->
      <div class="card">
        <div class="card-header"><h3>网络诊断</h3></div>
        <div class="card-body">
          <div class="form-group">
            <label class="form-label">发现端口 (UDP)</label>
            <span style="font-family:var(--font-mono);font-size:var(--text-sm)">{{ port }}</span>
          </div>
          <div class="form-group">
            <label class="form-label">RPC 端口 (TCP)</label>
            <span style="font-family:var(--font-mono);font-size:var(--text-sm)">{{ rpcPort }}</span>
          </div>
          <div style="display:flex;gap:var(--space-2);margin-bottom:var(--space-3)">
            <button class="btn btn-primary btn-sm" :disabled="checking" @click="checkFirewall">
              {{ checking ? '检测中...' : '检测网络' }}
            </button>
            <button class="btn btn-sm" :disabled="addingRules" @click="addFirewallRules">
              {{ addingRules ? '添加中...' : '添加防火墙规则' }}
            </button>
          </div>

          <!-- Test results -->
          <div v-if="firewallResults" class="fw-results">
            <div v-for="test in firewallResults.tests" :key="test.name" class="fw-test-row">
              <span class="fw-icon" :class="test.pass ? 'pass' : 'fail'">{{ test.pass ? '✓' : '✗' }}</span>
              <span class="fw-label">{{ testNames[test.name] || test.name }}</span>
              <span class="fw-detail">{{ test.detail }}</span>
            </div>
            <div class="fw-summary" :class="firewallResults.all_pass ? 'pass' : 'fail'">
              {{ firewallResults.all_pass ? '网络正常，集群通信就绪' : '存在问题，可能影响集群通信' }}
            </div>
          </div>

          <!-- Rule add result -->
          <div v-if="ruleResult" class="fw-results" style="margin-top:var(--space-2)">
            <div v-if="ruleResult.success" class="fw-summary pass">{{ ruleResult.message }}</div>
            <template v-else-if="ruleResult.uac_triggered">
              <div class="fw-summary uac">{{ ruleResult.message }}</div>
              <div style="margin-top:var(--space-2)">
                <div style="font-size:var(--text-sm);color:var(--text-muted);margin-bottom:var(--space-1)">如果未看到 UAC 弹窗，手动执行：</div>
                <pre v-for="cmd in ruleResult.manual_commands" :key="cmd" class="fw-cmd">{{ cmd }}</pre>
              </div>
            </template>
            <template v-else>
              <div class="fw-summary fail">{{ ruleResult.message }}</div>
              <div v-if="ruleResult.permission_denied" style="margin-top:var(--space-2)">
                <div style="font-size:var(--text-sm);color:var(--text-muted);margin-bottom:var(--space-1)">{{ ruleResult.platform_hint }}</div>
                <div style="font-size:var(--text-sm);color:var(--text-muted);margin-bottom:var(--space-1)">手动执行：</div>
                <pre v-for="cmd in ruleResult.manual_commands" :key="cmd" class="fw-cmd">{{ cmd }}</pre>
              </div>
            </template>
          </div>
        </div>
      </div>
    </div>

    <!-- Full-width snapshots card -->
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

<style scoped>
.settings-two-col {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: var(--space-4);
  margin-bottom: var(--space-4);
}
@media (max-width: 900px) {
  .settings-two-col {
    grid-template-columns: 1fr;
  }
}

.auth-token-dirty {
  margin-top: 4px;
  font-size: var(--text-xs);
  color: var(--warning, #f39c12);
}

.fw-results {
  border-top: 1px solid var(--border);
  padding-top: var(--space-3);
}

.fw-test-row {
  display: flex;
  align-items: baseline;
  gap: var(--space-2);
  padding: var(--space-1) 0;
  font-size: var(--text-sm);
}

.fw-icon {
  font-weight: 700;
  width: 16px;
  text-align: center;
  flex-shrink: 0;
}
.fw-icon.pass { color: var(--success); }
.fw-icon.fail { color: var(--error); }

.fw-label {
  font-weight: 500;
  white-space: nowrap;
  min-width: 100px;
}

.fw-detail {
  color: var(--text-muted);
  flex: 1;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.fw-summary {
  margin-top: var(--space-2);
  padding: var(--space-2);
  border-radius: var(--radius-md);
  font-size: var(--text-sm);
  font-weight: 500;
  text-align: center;
}
.fw-summary.pass {
  background: color-mix(in srgb, var(--success) 10%, transparent);
  color: var(--success);
}
.fw-summary.fail {
  background: color-mix(in srgb, var(--error) 10%, transparent);
  color: var(--error);
}

.fw-summary.uac {
  background: color-mix(in srgb, var(--primary) 10%, transparent);
  color: var(--primary);
}

.fw-cmd {
  background: var(--surface-alt);
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
  padding: var(--space-2) var(--space-3);
  font-family: var(--font-mono);
  font-size: var(--text-xs);
  overflow-x: auto;
  margin-bottom: var(--space-1);
}
</style>
