<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
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

// ── §2 免打扰（2026-10-11 无人值守计划 Part 2）：审批自动批准 ──────────
// config.approvals.auto_approve（持久真相源，typed save 热翻运行态投影）。
// 老配置缺 approvals 段 = 关（skip_serializing_if 不落盘）。运行态计数从
// security.approvals.status 读（count 重启清零，诚实标「本次会话」）。
const autoApprove = computed(() => !!config.value.approvals?.auto_approve)

const autoApproveStatus = ref<{ injected: boolean; enabled?: boolean; auto_approved_count?: number }>({
  injected: false,
})

async function loadApprovalStatus() {
  try {
    const r = await request('security', 'approvals.status', {})
    autoApproveStatus.value = r || { injected: false }
  } catch {
    /* 状态读取失败不阻塞页面 */
  }
}

async function onAutoApproveToggle(ev: Event) {
  const on = (ev.target as HTMLInputElement).checked
  const prevApprovals = config.value.approvals
  policySaving.value = 'auto_approve'
  config.value.approvals = { ...(prevApprovals || {}), auto_approve: on }
  try {
    await request('security', 'config.save', config.value)
    toast.success(on ? '免打扰已开启（审批将自动批准）' : '免打扰已关闭（恢复人工审批）')
    await loadApprovalStatus()
  } catch (e: any) {
    config.value.approvals = prevApprovals
    ;(ev.target as HTMLInputElement).checked = !on
    toast.error('保存失败: ' + e)
  } finally {
    policySaving.value = ''
  }
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

// ── 审批记忆规则（批次 3）：条目级管理（approvals.add/remove）──────────
// 「总是允许」规则表：审批卡「总是允许」自动写入 + 这里人工管理同表。
// 写入走 nemesis-security 同源闸（安装类拒 + CRITICAL 非 exec 拒 + 死
// pattern 拒）——闸拒绝的后端错误原样 toast，不加前端预判（单一裁决点）。
const approvalRules = ref<any[]>([])
const arForm = ref({ op: '', pattern: '' })
const AR_OP_SUGGESTIONS = [
  'process_exec',
  'process_spawn',
  'file_write',
  'file_edit',
  'file_delete',
  'dir_create',
  'dir_delete',
  'registry_read',
  'network_download',
  'network_request',
]

async function loadApprovalRules() {
  try {
    const r = await request('security', 'approvals.list', {})
    approvalRules.value = r?.rules || []
  } catch {
    /* 列表失败不阻塞页面 */
  }
}

async function addApprovalRule() {
  const op = arForm.value.op.trim()
  const pattern = arForm.value.pattern.trim()
  if (!op || !pattern) {
    toast.error('op 与 pattern 均必填')
    return
  }
  try {
    const r = await request('security', 'approvals.add', { op, pattern, action: 'allow' })
    toast.success(`规则已添加（${r?.pattern || pattern}）`)
    arForm.value = { op: '', pattern: '' }
    await loadApprovalRules()
  } catch (e: any) {
    toast.error('添加失败: ' + e)
  }
}

async function removeApprovalRule(r: any) {
  if (!confirm(`删除规则 ${r.op} / ${r.pattern}？`)) return
  try {
    await request('security', 'approvals.remove', { op: r.op, pattern: r.pattern })
    await loadApprovalRules()
  } catch (e: any) {
    toast.error('删除失败: ' + e)
  }
}

async function clearApprovalRules() {
  if (!confirm('清空全部审批记忆规则？（运行中网关下一次审批查询自动感知）')) return
  try {
    await request('security', 'approvals.clear', {})
    toast.success('已清空')
    await loadApprovalRules()
  } catch (e: any) {
    toast.error('清空失败: ' + e)
  }
}

// ── 评估规则（批次 3）：security.eval_rules.* ────────────────────────
// eval 沙盒安全评估（提示词/技能行为检测）的规则驱动三分类规则表。数据
// 半边与 CLI `eval rules *` 同源（nemesis-config::eval_rules 单一真相源）。
const evalRules = ref<any[]>([])
const evalVersion = ref<number | null>(null)

async function loadEvalRules() {
  try {
    const r = await request('security', 'eval_rules.list', {})
    evalRules.value = r?.rules || []
    evalVersion.value = r?.version ?? null
  } catch {
    /* 列表失败不阻塞页面 */
  }
}

// 增改模态（inline detail-card 形态）：conditions 编辑器行内增删。
const ER_LEVELS = ['critical', 'high', 'medium', 'low']
const ER_SOURCES = [
  { value: 'driver_events', label: 'driver_events · 行为驱动事件流' },
  { value: 'tool_trace', label: 'tool_trace · 工具调用轨迹' },
  { value: 'subject', label: 'subject · 提示词静态文本' },
]
const ER_OPS = [
  { value: 'equals', label: 'equals · 相等' },
  { value: 'contains', label: 'contains · 含子串' },
  { value: 'regex', label: 'regex · 正则' },
  { value: 'exists', label: 'exists · 字段存在' },
  { value: 'gt', label: 'gt · 大于（数字）' },
]
const erModal = ref({
  open: false,
  editing: false,
  id: '',
  description: '',
  level: 'high',
  enabled: true,
  source: 'tool_trace',
  minCount: 1,
  conditions: [{ field: '', op: 'contains', value: '' }] as { field: string; op: string; value: string }[],
})

function openEvalNew() {
  erModal.value = {
    open: true,
    editing: false,
    id: '',
    description: '',
    level: 'high',
    enabled: true,
    source: 'tool_trace',
    minCount: 1,
    conditions: [{ field: '', op: 'contains', value: '' }],
  }
}

function openEvalEdit(r: any) {
  erModal.value = {
    open: true,
    editing: true,
    id: r.id,
    description: r.description || '',
    level: r.level || 'high',
    enabled: r.enabled !== false,
    source: r.source || 'tool_trace',
    minCount: r.min_count ?? 1,
    conditions: (r.conditions || []).map((c: any) => ({
      field: c.field || '',
      op: c.op || 'contains',
      value: typeof c.value === 'string' ? c.value : JSON.stringify(c.value ?? ''),
    })),
  }
}

function erAddCondition() {
  erModal.value.conditions.push({ field: '', op: 'contains', value: '' })
}

function erRemoveCondition(i: number) {
  erModal.value.conditions.splice(i, 1)
}

async function submitEvalRule() {
  const m = erModal.value
  if (!m.id.trim()) {
    toast.error('规则 id 必填')
    return
  }
  if (!m.conditions.length) {
    toast.error('至少一条匹配条件')
    return
  }
  // value 归一：尝试 JSON.parse（equals 可命中非字符串 JSON 值），失败当
  // 字符串——exists 忽略 value（空串）。
  const conditions = m.conditions.map((c) => {
    let value: any = ''
    const raw = c.value.trim()
    if (raw) {
      try {
        value = JSON.parse(raw)
      } catch {
        value = raw
      }
    }
    return { field: c.field.trim(), op: c.op, value }
  })
  const rule = {
    id: m.id.trim(),
    description: m.description.trim() || m.id.trim(),
    level: m.level,
    enabled: m.enabled,
    source: m.source,
    conditions,
    min_count: Number(m.minCount) || 1,
  }
  try {
    await request('security', m.editing ? 'eval_rules.update' : 'eval_rules.add', { rule })
    toast.success(m.editing ? '规则已更新' : '规则已添加')
    erModal.value.open = false
    await loadEvalRules()
  } catch (e: any) {
    toast.error('保存失败: ' + e)
  }
}

async function toggleEvalRule(r: any, enabled: boolean) {
  try {
    await request('security', 'eval_rules.set_enabled', { id: r.id, enabled })
    r.enabled = enabled
  } catch (e: any) {
    toast.error('切换失败: ' + e)
    await loadEvalRules()
  }
}

async function removeEvalRule(r: any) {
  if (!confirm(`删除评估规则「${r.id}」？`)) return
  try {
    await request('security', 'eval_rules.remove', { id: r.id })
    await loadEvalRules()
  } catch (e: any) {
    toast.error('删除失败: ' + e)
  }
}

async function resetEvalRules() {
  if (!confirm('恢复内置默认规则集？当前全部自定义规则将被替换（可从 eval_rules.json.bak 找回）。')) return
  try {
    const r = await request('security', 'eval_rules.reset', {})
    toast.success(`已恢复默认集（替换 ${r?.replaced ?? 0} 条 → ${r?.count ?? 0} 条）`)
    await loadEvalRules()
  } catch (e: any) {
    toast.error('重置失败: ' + e)
  }
}

/// 条件摘要（镜像 CLI condition_summary 中文形态：列表里直接看到"匹配什么"）。
function condSummary(rule: any): string {
  const parts = (rule.conditions || []).map((c: any) => {
    const val = typeof c.value === 'string' ? c.value : JSON.stringify(c.value ?? '')
    const short = val.length > 48 ? val.slice(0, 48) + '…' : val
    switch (c.op) {
      case 'exists':
        return `${c.field} 存在`
      case 'equals':
        return `${c.field} == ${short}`
      case 'contains':
        return `${c.field} 含 '${short}'`
      case 'regex':
        return `${c.field} 匹配 /${short}/`
      case 'gt':
        return `${c.field} > ${short}`
      default:
        return `${c.field} ${c.op} ${short}`
    }
  })
  const body = parts.join(' 且 ')
  return rule.min_count > 1 ? `${body}（≥${rule.min_count} 条记录）` : body
}

function evalLevelBadge(level: string): string {
  if (level === 'critical') return 'badge-error'
  if (level === 'high') return 'badge-warning'
  if (level === 'medium') return 'badge-info'
  return 'badge-neutral'
}

function formatDate(ts?: string): string {
  if (!ts) return '--'
  return new Date(ts).toLocaleString('zh-CN')
}

onMounted(async () => {
  await Promise.all([
    loadConfig(),
    loadAudit(),
    loadStats(),
    loadApprovalStatus(),
    loadApprovalRules(),
    loadEvalRules(),
  ])
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
          <button class="tab" :class="{ active: activeTab === 'rules' }" @click="activeTab = 'rules'">评估规则</button>
          <button class="tab" :class="{ active: activeTab === 'audit' }" @click="activeTab = 'audit'">审计日志</button>
          <button class="tab" :class="{ active: activeTab === 'stats' }" @click="activeTab = 'stats'">统计</button>
        </div>

        <!-- Config -->
        <div v-if="activeTab === 'config'">
          <!-- §2 免打扰：审批自动批准开关（无人值守） -->
          <div class="card" style="margin-bottom: var(--space-4);">
            <div class="card-header"><h3>无人值守免打扰</h3></div>
            <div class="card-body">
              <label class="auto-approve-row" data-test="auto-approve-block">
                <input
                  type="checkbox"
                  data-test="auto-approve-toggle"
                  :checked="autoApprove"
                  :disabled="policySaving !== '' || editing"
                  @change="onAutoApproveToggle($event)"
                />
                <span>
                  <strong>审批自动批准（免打扰模式）</strong>
                  <div class="hint">
                    开启后所有审批卡（本地操作 / 看板派发远程审批 / agent 提问 / doom loop 停轮）一律自动批准，不再等待人工——适合无人值守运行；安全 8 层其余检查照常，每次自动批准照常落审计。配置持久化，重启后保持。
                  </div>
                </span>
              </label>
              <div
                v-if="autoApproveStatus.enabled"
                class="auto-approve-status"
                data-test="auto-approve-status"
              >✅ 免打扰中：本次会话已自动批准 {{ autoApproveStatus.auto_approved_count || 0 }} 条</div>
              <div v-else-if="!autoApproveStatus.injected" class="hint">
                免打扰运行态未装配（开发/降级形态）——开关只落配置，运行期投影以网关重启后为准。
              </div>
            </div>
          </div>

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

          <!-- 审批记忆规则（批次 3 条目级管理） -->
          <div class="card" style="margin-bottom: var(--space-4);" data-test="approval-rules-card">
            <div class="card-header">
              <h3>审批记忆规则</h3>
              <button class="btn btn-sm" @click="clearApprovalRules" :disabled="!approvalRules.length">清空</button>
            </div>
            <div class="card-body">
              <div class="hint" style="margin: 0 0 var(--space-2);">
                审批卡点「总是允许」自动写入的规则表，这里可人工补充/删除。写入走与自动写入同一套闸：安装类操作永不入表；CRITICAL 非 exec 操作规则不生效（永远人工）；exec 类 pattern 建议带 <code>*</code> 后缀表示前缀放行（如 <code>cargo test *</code>）。规则修改即时生效（下一次审批查询现读）。
              </div>
              <div class="ar-form">
                <input
                  v-model="arForm.op"
                  list="ar-op-list"
                  placeholder="操作类型（如 process_exec）"
                  style="width: 180px;"
                />
                <datalist id="ar-op-list">
                  <option v-for="op in AR_OP_SUGGESTIONS" :key="op" :value="op" />
                </datalist>
                <input
                  v-model="arForm.pattern"
                  placeholder="匹配 pattern（如 cargo test * 或完整路径）"
                  style="flex: 1; min-width: 220px;"
                  @keyup.enter="addApprovalRule"
                />
                <span class="ar-action-chip" title="v1 规则动作恒为 allow（deny 没有记忆意义）">allow</span>
                <button class="btn btn-sm btn-primary" @click="addApprovalRule">添加</button>
              </div>
              <div v-if="approvalRules.length === 0" class="empty-state" style="padding: var(--space-4);">
                <p style="margin: 0;">暂无规则。审批卡选「总是允许」或用上方表单添加。</p>
              </div>
              <div v-else class="table-wrap">
                <table data-test="approval-rules-table">
                  <thead><tr><th>操作</th><th>匹配 pattern</th><th>动作</th><th>写入时间</th><th></th></tr></thead>
                  <tbody>
                    <tr v-for="(r, i) in approvalRules" :key="i">
                      <td class="mono">{{ r.op }}</td>
                      <td class="mono">{{ r.pattern }}</td>
                      <td>{{ r.action }}</td>
                      <td style="font-size: var(--text-xs);">{{ formatDate(r.created_at) }}</td>
                      <td><button class="btn btn-sm danger" @click="removeApprovalRule(r)">删除</button></td>
                    </tr>
                  </tbody>
                </table>
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

        <!-- Eval Rules（批次 3：eval 沙盒安全评估规则管理） -->
        <div v-if="activeTab === 'rules'" data-test="eval-rules-tab">
          <div class="card">
            <div class="card-header">
              <h3>评估规则 <span v-if="evalVersion != null" class="mono" style="font-size: var(--text-xs); color: var(--text-muted, #888);">v{{ evalVersion }}</span></h3>
              <div style="display: flex; gap: var(--space-2);">
                <button class="btn btn-sm" data-test="eval-rule-new" @click="openEvalNew">新增规则</button>
                <button class="btn btn-sm" @click="resetEvalRules">恢复默认集</button>
              </div>
            </div>
            <div class="card-body">
              <div class="hint" style="margin: 0 0 var(--space-2);">
                eval 沙盒安全评估（<code>eval prompt</code> / <code>eval skill</code>）的规则驱动三分类依据：每条规则声明「抓什么行为」，评估时命中即计入分级。与 CLI <code>eval rules *</code> 管理同一份规则表（workspace/config/eval_rules.json），修改即时生效（下次评估现读）。
              </div>

              <!-- 增改表单（inline detail-card 形态） -->
              <div v-if="erModal.open" class="er-modal" data-test="eval-rule-modal">
                <div class="er-modal-head">
                  <strong>{{ erModal.editing ? `编辑规则 ${erModal.id}` : '新增评估规则' }}</strong>
                  <button class="btn btn-sm" @click="erModal.open = false">取消</button>
                </div>
                <div class="er-grid">
                  <label>id <input v-model="erModal.id" class="mono" :disabled="erModal.editing" placeholder="probe-aws-cli" /></label>
                  <label>描述 <input v-model="erModal.description" placeholder="一句话描述（命中时展示）" style="flex: 1;" /></label>
                  <label>严重度
                    <select v-model="erModal.level">
                      <option v-for="l in ER_LEVELS" :key="l" :value="l">{{ l }}</option>
                    </select>
                  </label>
                  <label>来源
                    <select v-model="erModal.source">
                      <option v-for="s in ER_SOURCES" :key="s.value" :value="s.value">{{ s.label }}</option>
                    </select>
                  </label>
                  <label>触发条数 ≥ <input v-model.number="erModal.minCount" type="number" min="1" style="width: 80px;" /></label>
                  <label class="er-enabled"><input v-model="erModal.enabled" type="checkbox" /> 启用</label>
                </div>
                <div class="er-conds">
                  <div class="er-conds-title">
                    匹配条件（同一条记录内全部满足才计一次命中）
                    <button class="btn btn-sm" @click="erAddCondition">+ 加条件</button>
                  </div>
                  <div v-for="(c, i) in erModal.conditions" :key="i" class="er-cond-row">
                    <input v-model="c.field" class="mono" placeholder="字段路径（如 arguments.command）" style="flex: 1; min-width: 180px;" />
                    <select v-model="c.op" style="width: 160px;">
                      <option v-for="o in ER_OPS" :key="o.value" :value="o.value">{{ o.label }}</option>
                    </select>
                    <input
                      v-if="c.op !== 'exists'"
                      v-model="c.value"
                      class="mono"
                      :placeholder="c.op === 'gt' ? '数字（如 3）' : c.op === 'regex' ? '正则（如 (?i)\\.aws' : '值（字符串或 JSON）'"
                      style="flex: 1; min-width: 160px;"
                    />
                    <button class="btn btn-sm danger" :disabled="erModal.conditions.length <= 1" @click="erRemoveCondition(i)">删</button>
                  </div>
                </div>
                <div class="er-actions">
                  <button class="btn btn-sm btn-primary" data-test="eval-rule-save" @click="submitEvalRule">{{ erModal.editing ? '保存修改' : '添加规则' }}</button>
                </div>
              </div>

              <div v-if="evalRules.length === 0" class="empty-state">
                <h3>暂无规则</h3>
                <p>新增一条，或恢复内置默认集</p>
              </div>
              <div v-else class="table-wrap">
                <table data-test="eval-rules-table">
                  <thead><tr><th>启用</th><th>ID</th><th>严重度</th><th>来源</th><th>描述</th><th>匹配条件</th><th></th></tr></thead>
                  <tbody>
                    <tr v-for="r in evalRules" :key="r.id" :style="r.enabled ? '' : 'opacity: 0.55;'">
                      <td><input type="checkbox" :checked="r.enabled" @change="toggleEvalRule(r, ($event.target as HTMLInputElement).checked)" /></td>
                      <td class="mono">{{ r.id }}</td>
                      <td><span class="badge" :class="evalLevelBadge(r.level)">{{ r.level }}</span></td>
                      <td style="font-size: var(--text-xs);">{{ r.source }}</td>
                      <td style="max-width: 220px;">{{ r.description }}</td>
                      <td class="er-cond-summary">{{ condSummary(r) }}</td>
                      <td style="white-space: nowrap;">
                        <button class="btn btn-sm" @click="openEvalEdit(r)">编辑</button>
                        <button class="btn btn-sm danger" @click="removeEvalRule(r)">删除</button>
                      </td>
                    </tr>
                  </tbody>
                </table>
              </div>
              <div class="hint" style="margin-top: var(--space-2);">
                共 {{ evalRules.length }} 条，{{ evalRules.filter((r: any) => r.enabled).length }} 条启用。评估按 critical &gt; high &gt; medium &gt; low 取最高命中分级。
              </div>
            </div>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
/* §2 免打扰开关块 */
.auto-approve-row {
  display: flex;
  align-items: flex-start;
  gap: var(--space-2);
  cursor: pointer;
  user-select: none;
}
.auto-approve-row input[type='checkbox'] {
  margin-top: 3px;
}
.auto-approve-row .hint {
  font-weight: 400;
}
.hint {
  font-size: var(--text-xs);
  color: var(--text-muted, #888);
  margin-top: var(--space-1);
  max-width: 640px;
}
.auto-approve-status {
  margin-top: var(--space-2);
  font-size: var(--text-sm);
  color: var(--success, #2da44e);
}

/* 审批记忆规则（批次 3） */
.mono {
  font-family: var(--font-mono, 'Consolas', monospace);
  font-size: var(--text-xs);
}
.ar-form {
  display: flex;
  gap: var(--space-2);
  align-items: center;
  flex-wrap: wrap;
  margin: var(--space-2) 0;
}
.ar-form input {
  flex: 1;
  min-width: 140px;
}
.ar-action-chip {
  font-size: var(--text-xs);
  padding: 2px 10px;
  border-radius: 999px;
  background: var(--success-bg, rgba(45, 164, 78, 0.15));
  color: var(--success, #2da44e);
  border: 1px solid var(--success, #2da44e);
}

/* 评估规则模态与表格（批次 3） */
.er-modal {
  border: 1px solid var(--border, #333);
  border-radius: var(--radius-md, 8px);
  padding: var(--space-3);
  margin-bottom: var(--space-3);
  background: var(--bg-elev, rgba(255, 255, 255, 0.03));
}
.er-modal-head {
  display: flex;
  justify-content: space-between;
  align-items: center;
  margin-bottom: var(--space-2);
}
.er-grid {
  display: flex;
  gap: var(--space-3);
  flex-wrap: wrap;
  align-items: flex-end;
  margin-bottom: var(--space-3);
}
.er-grid label {
  display: flex;
  flex-direction: column;
  gap: 4px;
  font-size: var(--text-xs);
  color: var(--text-muted, #888);
}
.er-grid label.er-enabled {
  flex-direction: row;
  align-items: center;
  gap: 6px;
  color: var(--text, #ddd);
}
.er-conds {
  margin-bottom: var(--space-3);
}
.er-conds-title {
  font-size: var(--text-xs);
  color: var(--text-muted, #888);
  display: flex;
  justify-content: space-between;
  align-items: center;
  margin-bottom: var(--space-1);
}
.er-cond-row {
  display: flex;
  gap: var(--space-2);
  margin-bottom: var(--space-1);
  align-items: center;
}
.er-cond-summary {
  font-size: var(--text-xs);
  color: var(--text-muted, #888);
  max-width: 260px;
}
.er-actions {
  display: flex;
  justify-content: flex-end;
}
</style>
