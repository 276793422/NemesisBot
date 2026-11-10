<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'
import SecurityLayerHeader from '../components/security/SecurityLayerHeader.vue'

// 人工审批专页（十层页面化 2026-11-10）：免打扰开关（§2 无人值守）+
// 审批记忆规则（「总是允许」规则表条目级管理）+ eval 评估规则节（用户
// 拍板 3：eval 先做出来放 Approval 页一节，位置后议）。三块机制自旧
// SecurityView config tab 原样迁移，数据面命令不变。

const { request } = useWSAPI()
const toast = useToast()

const loading = ref(true)
const config = ref<any>({})

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
  config.value.approvals = { ...(prevApprovals || {}), auto_approve: on }
  try {
    await request('security', 'config.save', config.value)
    toast.success(on ? '免打扰已开启（审批将自动批准）' : '免打扰已关闭（恢复人工审批）')
    await loadApprovalStatus()
  } catch (e: any) {
    config.value.approvals = prevApprovals
    ;(ev.target as HTMLInputElement).checked = !on
    toast.error('保存失败: ' + e)
  }
}

// ── 审批记忆规则：条目级管理（approvals.add/remove）──────────────────
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

// ── 评估规则：security.eval_rules.*（用户拍板 3：先挂本页，位置后议）──
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
  const d = new Date(ts)
  return Number.isNaN(d.getTime()) ? ts : d.toLocaleString('zh-CN')
}

async function loadConfig() {
  try {
    config.value = (await request('security', 'config.get')) || {}
  } catch { /* ignore */ }
}

onMounted(async () => {
  await Promise.all([
    loadConfig(),
    loadApprovalStatus(),
    loadApprovalRules(),
    loadEvalRules(),
  ])
  loading.value = false
})
</script>

<template>
  <div class="page-security-approval">
    <SecurityLayerHeader
      title="人工审批"
      duty="附加机制（不在八层编号内）：高危操作规则层放行后再过人工审批卡（Dashboard 模态 / IM 通道回源文本卡）；「总是允许」可把规则记入下方记忆表免重复确认。"
      status-label="运行态"
      status-tone="na"
    />
    <div class="page-body">
      <div v-if="loading" style="text-align: center; padding: var(--space-8);">
        <div class="spinner spinner-lg" style="margin: 0 auto;"></div>
      </div>

      <div v-if="!loading">
        <!-- §2 免打扰：审批自动批准开关（无人值守） -->
        <div class="card" style="margin-bottom: var(--space-4);">
          <div class="card-header"><h3>无人值守免打扰</h3></div>
          <div class="card-body">
            <label class="auto-approve-row" data-test="auto-approve-block">
              <input
                type="checkbox"
                data-test="auto-approve-toggle"
                :checked="autoApprove"
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

        <!-- 审批记忆规则 -->
        <div class="card" style="margin-bottom: var(--space-4);" data-test="approval-rules-card">
          <div class="card-header">
            <h3>审批记忆规则</h3>
            <button class="btn btn-sm" data-test="approval-rules-clear" @click="clearApprovalRules" :disabled="!approvalRules.length">清空</button>
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
              <button class="btn btn-sm btn-primary" data-test="approval-rule-add" @click="addApprovalRule">添加</button>
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
                    <td><button class="btn btn-sm danger" :data-test="'approval-rule-remove-' + i" @click="removeApprovalRule(r)">删除</button></td>
                  </tr>
                </tbody>
              </table>
            </div>
          </div>
        </div>

        <!-- eval 评估规则（用户拍板 3：先挂本页一节，位置后议） -->
        <div class="card" data-test="eval-rules-card">
          <div class="card-header">
            <h3>eval 评估规则 <span v-if="evalVersion != null" class="mono" style="font-size: var(--text-xs); color: var(--text-muted, #888);">v{{ evalVersion }}</span></h3>
            <div style="display: flex; gap: var(--space-2);">
              <button class="btn btn-sm" data-test="eval-rule-new" @click="openEvalNew">新增规则</button>
              <button class="btn btn-sm" data-test="eval-rule-reset" @click="resetEvalRules">恢复默认集</button>
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
                      <button class="btn btn-sm" :data-test="'eval-rule-edit-' + r.id" @click="openEvalEdit(r)">编辑</button>
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

/* 审批记忆规则 */
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

/* 评估规则模态与表格 */
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
