<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'
import { usePageTab } from '../lib/pageTab'
import ScannerView from './ScannerView.vue'
import SandboxView from './SandboxView.vue'
import SimpleFieldForm from '../components/SimpleFieldForm.vue'
import { SECURITY_FIELD_META } from '../lib/friendlyFields'

const { request } = useWSAPI()
const toast = useToast()

const securityFeature = import.meta.env.VITE_FEATURE_SECURITY !== 'false'
const sandboxFeature = import.meta.env.VITE_FEATURE_SANDBOX !== 'false'

const activeTab = ref('config')
const { setTab } = usePageTab(activeTab, ['config', 'audit', 'stats', 'scanner', 'sandbox'] as const, 'config')
const config = ref<any>({})
const formModel = ref<Record<string, any>>({})
const auditEntries = ref<any[]>([])
const stats = ref<any>({})
const loading = ref(true)
const editing = ref(false)
const showRaw = ref(false)
const editConfig = ref('')

async function loadConfig() {
  try {
    const data = await request('security', 'config.get')
    config.value = data || {}
    formModel.value = { ...(data || {}) }
    editConfig.value = JSON.stringify(data, null, 2)
  } catch { /* ignore */ }
}

async function loadAudit() {
  try {
    const data = await request('security', 'audit', { limit: 100 })
    auditEntries.value = data?.entries || []
  } catch { /* ignore */ }
}

async function loadStats() {
  try {
    const data = await request('security', 'stats')
    stats.value = data || {}
  } catch { /* ignore */ }
}

async function saveConfig() {
  try {
    let parsed: any
    if (showRaw.value) {
      parsed = JSON.parse(editConfig.value)
    } else {
      parsed = { ...config.value, ...formModel.value }
    }
    await request('security', 'config.save', parsed)
    toast.success('已保存')
    editing.value = false
    showRaw.value = false
    await loadConfig()
  } catch (e: any) {
    toast.error('保存失败: ' + e)
  }
}

function startEdit() {
  formModel.value = { ...config.value }
  editConfig.value = JSON.stringify(config.value, null, 2)
  editing.value = true
  showRaw.value = false
}

function formatDate(ts?: string): string {
  if (!ts) return '--'
  return new Date(ts).toLocaleString('zh-CN')
}

onMounted(async () => {
  await Promise.all([loadConfig(), loadAudit(), loadStats()])
  loading.value = false
})
</script>

<template>
  <div class="page-security">
    <div class="page-header"><h2>安全管理</h2></div>
    <div class="page-body">
      <div v-if="loading" style="text-align: center; padding: var(--space-8);">
        <div class="spinner spinner-lg" style="margin: 0 auto;"></div>
      </div>

      <div v-if="!loading">
        <div class="tabs">
          <button class="tab" :class="{ active: activeTab === 'config' }" @click="setTab('config')">策略配置</button>
          <button class="tab" :class="{ active: activeTab === 'audit' }" @click="setTab('audit')">审计日志</button>
          <button class="tab" :class="{ active: activeTab === 'stats' }" @click="setTab('stats')">统计</button>
          <button v-if="securityFeature" class="tab" :class="{ active: activeTab === 'scanner' }" @click="setTab('scanner')">扫描器</button>
          <button v-if="sandboxFeature" class="tab" :class="{ active: activeTab === 'sandbox' }" @click="setTab('sandbox')">沙盒</button>
        </div>

        <div v-if="activeTab === 'scanner'">
          <ScannerView embedded />
        </div>
        <div v-if="activeTab === 'sandbox'">
          <SandboxView embedded />
        </div>

        <!-- Config: friendly fields first, raw JSON only as advanced -->
        <div v-if="activeTab === 'config'">
          <div class="card">
            <div class="card-header">
              <h3>安全策略</h3>
              <div style="display: flex; gap: var(--space-2);">
                <template v-if="!editing">
                  <button class="btn btn-sm btn-primary" @click="startEdit">修改策略</button>
                </template>
                <template v-else>
                  <button class="btn btn-sm" @click="editing = false; showRaw = false">取消</button>
                  <button class="btn btn-sm btn-primary" @click="saveConfig">保存</button>
                </template>
              </div>
            </div>
            <div class="card-body">
              <div v-if="editing">
                <p class="form-hint" style="margin-bottom: var(--space-4);">用开关与数字调整即可，无需手写 JSON。</p>
                <SimpleFieldForm v-model="formModel" :meta-table="SECURITY_FIELD_META" />
                <div style="margin-top: var(--space-4);">
                  <button type="button" class="btn btn-sm" @click="showRaw = !showRaw">{{ showRaw ? '隐藏 JSON' : '高级：原始 JSON' }}</button>
                </div>
                <textarea v-if="showRaw" class="form-textarea" style="min-height: 240px; margin-top: var(--space-2); font-family: var(--font-mono); font-size: var(--text-xs);" v-model="editConfig"></textarea>
              </div>
              <div v-else>
                <div class="settings-grid">
                  <template v-for="(value, key) in config" :key="key">
                    <template v-if="typeof value !== 'object'">
                      <span class="settings-key">{{ SECURITY_FIELD_META[key as string]?.label || key }}</span>
                      <span class="settings-value">{{ typeof value === 'boolean' ? (value ? '是' : '否') : String(value) }}</span>
                    </template>
                  </template>
                </div>
              </div>
            </div>
          </div>
        </div>

        <!-- Audit -->
        <div v-if="activeTab === 'audit'">
          <div v-if="auditEntries.length === 0" class="empty-state">
            <h3>暂无审计记录</h3>
            <p>安全事件将自动记录在此</p>
          </div>
          <div v-if="auditEntries.length > 0" class="table-wrap">
            <table>
              <thead><tr><th>时间</th><th>操作</th><th>风险级别</th><th>目标</th><th>结果</th></tr></thead>
              <tbody>
                <tr v-for="(e, idx) in auditEntries" :key="idx">
                  <td style="font-size: var(--text-xs);">{{ formatDate(e.timestamp) }}</td>
                  <td>{{ e.action || e.operation || '--' }}</td>
                  <td>
                    <span class="badge" :class="{
                      'badge-error': e.risk_level === 'CRITICAL',
                      'badge-warning': e.risk_level === 'HIGH',
                      'badge-info': e.risk_level === 'MEDIUM',
                      'badge-neutral': e.risk_level === 'LOW',
                    }">{{ e.risk_level || '--' }}</span>
                  </td>
                  <td style="max-width: 200px; overflow: hidden; text-overflow: ellipsis;">{{ e.target || '--' }}</td>
                  <td>{{ e.result || '--' }}</td>
                </tr>
              </tbody>
            </table>
          </div>
        </div>

        <!-- Stats -->
        <div v-if="activeTab === 'stats'">
          <div class="stats-grid">
            <div class="stat-card">
              <div class="stat-label">总事件数</div>
              <div class="stat-value">{{ stats.total_events || 0 }}</div>
            </div>
            <div class="stat-card">
              <div class="stat-label">CRITICAL</div>
              <div class="stat-value" style="color: var(--error);">{{ stats.by_level?.CRITICAL || 0 }}</div>
            </div>
            <div class="stat-card">
              <div class="stat-label">HIGH</div>
              <div class="stat-value" style="color: var(--warning, #e5a00d);">{{ stats.by_level?.HIGH || 0 }}</div>
            </div>
            <div class="stat-card">
              <div class="stat-label">MEDIUM</div>
              <div class="stat-value">{{ stats.by_level?.MEDIUM || 0 }}</div>
            </div>
            <div class="stat-card">
              <div class="stat-label">LOW</div>
              <div class="stat-value">{{ stats.by_level?.LOW || 0 }}</div>
            </div>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>
