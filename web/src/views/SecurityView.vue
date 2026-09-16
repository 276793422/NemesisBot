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

// D1/D2（2026-09-16 横扫存量加固）：exec/spawn 未知命令 + Guardian 失败
// 姿态下拉。走 security.config.save 整体写回（后端 typed SecurityConfig
// 校验），失败回滚本地显示。JSON 编辑模式下由编辑器接管，下拉禁用。
// guardian_mode（同日无上下文 LLM 命令审计）：覆盖面开关，默认 off。
const POLICY_KEYS = ['exec_unknown_policy', 'guardian_failure_policy', 'guardian_mode'] as const
type PolicyKey = (typeof POLICY_KEYS)[number]
const isPolicyKey = (key: string) => (POLICY_KEYS as readonly string[]).includes(key)

const POLICY_CHOICES: Record<PolicyKey, { value: string; label: string }[]> = {
  exec_unknown_policy: [
    { value: 'allow', label: 'allow · 放行（默认）' },
    { value: 'ask', label: 'ask · 送审批卡' },
    { value: 'deny', label: 'deny · 硬拦' },
  ],
  guardian_failure_policy: [
    { value: 'ask', label: 'ask · 送审批卡（默认）' },
    { value: 'allow', label: 'allow · 放行' },
    { value: 'deny', label: 'deny · 硬拦' },
  ],
  guardian_mode: [
    { value: 'off', label: 'off · 关闭（默认）' },
    { value: 'critical', label: 'critical · CRITICAL 级全审' },
    { value: 'high', label: 'high · HIGH+CRITICAL 破坏形态预筛' },
  ],
}

const POLICY_HINTS: Record<PolicyKey, string> = {
  exec_unknown_policy:
    'exec / spawn 命令未命中任何规则时的姿态。allow = 放行（旧行为）；ask = 弹审批卡；deny = 硬拦。保存后重启网关生效。',
  guardian_failure_policy:
    'Guardian（LLM 语义二审）异常或不可用时的姿态。ask = 弹审批卡（不静默放行也不误伤）；allow = 放行并记录 WARN；deny = 硬拦。保存后重启网关生效。',
  guardian_mode:
    '无上下文 LLM 命令审计（guardian）覆盖面。off = 不审（默认，零 LLM 成本）；critical = CRITICAL 级工具全审；high = HIGH+CRITICAL 级先过破坏形态词表再进 LLM。审计模型走 agents.small_model（未配置用主模型）。保存后重启网关生效。',
}

function policyValue(key: PolicyKey): string {
  // 与后端 serde 默认对齐：缺键时显示默认值而非空白选项。
  if (key === 'guardian_mode') return config.value[key] || 'off'
  return config.value[key] || (key === 'exec_unknown_policy' ? 'allow' : 'ask')
}

const policySaving = ref('')

async function savePolicy(key: PolicyKey, value: string) {
  const prev = config.value[key]
  if (prev === value) return
  policySaving.value = key
  config.value[key] = value
  try {
    await request('security', 'config.save', config.value)
    toast.success('已保存')
  } catch (e: any) {
    config.value[key] = prev
    toast.error('保存失败: ' + e)
  } finally {
    policySaving.value = ''
  }
}

function onPolicyChange(key: PolicyKey, ev: Event) {
  savePolicy(key, (ev.target as HTMLSelectElement).value)
}

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
          <!-- D1/D2 策略开关 -->
          <div class="card" style="margin-bottom: var(--space-4);">
            <div class="card-header"><h3>策略开关</h3></div>
            <div class="card-body">
              <div v-if="!editing" class="settings-grid" data-test="policy-switches">
                <template v-for="key in POLICY_KEYS" :key="key">
                  <span class="settings-key">{{ key }}</span>
                  <span class="settings-value">
                    <select
                      class="form-select"
                      style="max-width: 260px;"
                      :data-test="key"
                      :value="policyValue(key)"
                      :disabled="policySaving !== ''"
                      @change="onPolicyChange(key, $event)"
                    >
                      <option v-for="c in POLICY_CHOICES[key]" :key="c.value" :value="c.value">{{ c.label }}</option>
                    </select>
                    <div style="font-size: var(--text-xs); color: var(--text-muted, #888); margin-top: var(--space-1); max-width: 520px;">
                      {{ POLICY_HINTS[key] }}
                    </div>
                  </span>
                </template>
              </div>
              <div v-else style="font-size: var(--text-xs); color: var(--text-muted, #888);">
                JSON 编辑模式下，策略开关由下方编辑器直接控制。
              </div>
            </div>
          </div>

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
                <textarea class="form-textarea" style="min-height: 60vh; font-family: var(--font-mono); font-size: var(--text-xs);" v-model="editConfig"></textarea>
              </div>
              <div v-else>
                <div class="settings-grid">
                  <template v-for="(value, key) in config" :key="key">
                    <template v-if="typeof value !== 'object' && !isPolicyKey(String(key))">
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
