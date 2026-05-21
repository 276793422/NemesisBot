<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'

const { request } = useWSAPI()
const toast = useToast()

const activeTab = ref('config')
const config = ref<any>({})
const auditEntries = ref<any[]>([])
const stats = ref<any>({})
const loading = ref(true)
const editing = ref(false)
const editConfig = ref('')

async function loadConfig() {
  try {
    const data = await request('security', 'config.get')
    config.value = data || {}
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
    const parsed = JSON.parse(editConfig.value)
    await request('security', 'config.save', parsed)
    toast.success('已保存')
    editing.value = false
    await loadConfig()
  } catch (e: any) {
    toast.error('保存失败: ' + e)
  }
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
          <button class="tab" :class="{ active: activeTab === 'config' }" @click="activeTab = 'config'">配置</button>
          <button class="tab" :class="{ active: activeTab === 'audit' }" @click="activeTab = 'audit'">审计日志</button>
          <button class="tab" :class="{ active: activeTab === 'stats' }" @click="activeTab = 'stats'">统计</button>
        </div>

        <!-- Config -->
        <div v-if="activeTab === 'config'">
          <div class="card">
            <div class="card-header">
              <h3>安全策略配置</h3>
              <div style="display: flex; gap: var(--space-2);">
                <template v-if="!editing">
                  <button class="btn btn-sm" @click="editing = true">编辑</button>
                </template>
                <template v-else>
                  <button class="btn btn-sm" @click="editing = false">取消</button>
                  <button class="btn btn-sm btn-primary" @click="saveConfig">保存</button>
                </template>
              </div>
            </div>
            <div class="card-body">
              <div v-if="editing">
                <textarea class="form-textarea" style="min-height: 500px; font-family: var(--font-mono); font-size: var(--text-xs);" v-model="editConfig"></textarea>
              </div>
              <div v-else>
                <div class="settings-grid">
                  <template v-for="(value, key) in config" :key="key">
                    <template v-if="typeof value !== 'object'">
                      <span class="settings-key">{{ key }}</span>
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
              <div class="stat-label">高危事件</div>
              <div class="stat-value" style="color: var(--error);">{{ stats.high_risk || 0 }}</div>
            </div>
            <div class="stat-card">
              <div class="stat-label">拦截次数</div>
              <div class="stat-value">{{ stats.blocked || 0 }}</div>
            </div>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>
