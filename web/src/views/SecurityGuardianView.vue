<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'
import SecurityLayerHeader from '../components/security/SecurityLayerHeader.vue'

// Guardian 二审专页（十层页面化 2026-11-10）：guardian_failure_policy /
// guardian_mode 两下拉（自旧 SecurityView 策略开关迁移）+ LLM judge 机制
// 说明 + guardian 审计事件过滤展示（approved/denied verdict）。
//
// 两键均为构造期键（guardian 装配在 security plugin 构造时读定）——保存
// 重启网关生效，不热翻。审计模型走 agents.small_model（未配置回落主模型）。

const { request } = useWSAPI()
const toast = useToast()

const loading = ref(true)
const saving = ref(false)
const config = ref<any>({})
const entries = ref<any[]>([])

const FAILURE_CHOICES = [
  { value: 'ask', label: 'ask · 送审批卡（默认）' },
  { value: 'allow', label: 'allow · 放行并记 WARN' },
  { value: 'deny', label: 'deny · 硬拦' },
]
const MODE_CHOICES = [
  { value: 'off', label: 'off · 关闭（默认，零 LLM 成本）' },
  { value: 'critical', label: 'critical · CRITICAL 级工具全审' },
  { value: 'high', label: 'high · HIGH+CRITICAL 破坏形态预筛' },
]

const failurePolicy = computed(() => config.value.guardian_failure_policy || 'ask')
const guardianMode = computed(() => config.value.guardian_mode || 'off')

async function saveKey(key: 'guardian_failure_policy' | 'guardian_mode', value: string) {
  const prev = config.value[key]
  if (prev === value) return
  saving.value = true
  config.value[key] = value
  try {
    await request('security', 'config.save', config.value)
    toast.success('已保存（重启网关生效）')
  } catch (e: any) {
    config.value[key] = prev
    toast.error('保存失败: ' + e)
  } finally {
    saving.value = false
  }
}

// guardian 审计事件：policy_rule 以 guardian_ 前缀（guardian_approved /
// guardian_denied / guardian_error 等 verdict 形态）。
async function loadAudit() {
  try {
    const data = await request('security', 'audit', { limit: 100 })
    const all: any[] = data?.entries || []
    entries.value = all.filter((e) => (e.policy || '').startsWith('guardian'))
  } catch { /* ignore */ }
}

async function loadConfig() {
  try {
    config.value = (await request('security', 'config.get')) || {}
  } catch { /* ignore */ }
}

function verdictTone(policy?: string): 'on' | 'off' | 'warn' {
  if ((policy || '').includes('approved')) return 'on'
  if ((policy || '').includes('denied')) return 'off'
  return 'warn'
}

function formatDate(ts?: string): string {
  if (!ts) return '--'
  const d = new Date(ts)
  return Number.isNaN(d.getTime()) ? ts : d.toLocaleString('zh-CN')
}

onMounted(async () => {
  await Promise.all([loadConfig(), loadAudit()])
  loading.value = false
})
</script>

<template>
  <div class="page-security-guardian">
    <SecurityLayerHeader
      num="⑪"
      title="Guardian 二审"
      duty="第⑪道闸 Guardian 二审：高危操作过规则层后由无上下文 LLM 做语义二审，防提示词注入绕过规则表；execute 同步返回 allow 后 agent loop 异步等待裁决。"
      :status-label="guardianMode === 'off' ? '未开启' : guardianMode === 'critical' ? 'CRITICAL 全审' : 'HIGH+ 破坏形态预筛'"
      :status-tone="guardianMode === 'off' ? 'na' : 'on'"
      effective="restart"
    />
    <div class="page-body">
      <div v-if="loading" style="text-align: center; padding: var(--space-8);">
        <div class="spinner spinner-lg" style="margin: 0 auto;"></div>
      </div>

      <div v-if="!loading">
        <!-- 两策略下拉 -->
        <div class="card" style="margin-bottom: var(--space-4);">
          <div class="card-header"><h3>策略</h3></div>
          <div class="card-body">
            <div class="guardian-param" data-test="guardian-failure-block">
              <span class="param-key">失败姿态</span>
              <select
                class="form-select"
                style="max-width: 280px;"
                data-test="guardian-failure-policy"
                :value="failurePolicy"
                :disabled="saving"
                @change="saveKey('guardian_failure_policy', ($event.target as HTMLSelectElement).value)"
              >
                <option v-for="c in FAILURE_CHOICES" :key="c.value" :value="c.value">{{ c.label }}</option>
              </select>
              <div class="hint">Guardian（LLM 语义二审）异常或不可用时的姿态。ask = 弹审批卡（不静默放行也不误伤）；allow = 放行并记录 WARN；deny = 硬拦。</div>
            </div>
            <div class="guardian-param" data-test="guardian-mode-block">
              <span class="param-key">审计覆盖面</span>
              <select
                class="form-select"
                style="max-width: 280px;"
                data-test="guardian-mode"
                :value="guardianMode"
                :disabled="saving"
                @change="saveKey('guardian_mode', ($event.target as HTMLSelectElement).value)"
              >
                <option v-for="c in MODE_CHOICES" :key="c.value" :value="c.value">{{ c.label }}</option>
              </select>
              <div class="hint">无上下文 LLM 命令审计覆盖面。off = 不审（默认，零 LLM 成本）；critical = CRITICAL 级工具全审；high = HIGH+CRITICAL 级先过破坏形态词表再进 LLM。审计模型走 agents.small_model（未配置用主模型）。</div>
            </div>
          </div>
        </div>

        <!-- 机制说明 -->
        <div class="card" style="margin-bottom: var(--space-4);" data-test="guardian-explain">
          <div class="card-header"><h3>工作机制</h3></div>
          <div class="card-body">
            <div class="hint" style="margin: 0;">
              规则引擎（③ ABAC）判定 CRITICAL 操作需要放行时，Guardian 用一次<strong>无上下文</strong> LLM 调用对操作语义做独立二审——模型只看操作本身（工具/入参/风险级别），不带对话历史，防「对话内注入话术骗过规则表」。裁决 approved = 放行（agent loop 继续执行）、denied = 拦截；Guardian 自身异常走上方「失败姿态」。审计事件按 policy_rule <code>guardian_*</code> 记录，下方列表即其过滤视图。
            </div>
          </div>
        </div>

        <!-- Guardian 审计事件 -->
        <div class="card">
          <div class="card-header"><h3>Guardian 裁决记录</h3></div>
          <div class="card-body">
            <div v-if="entries.length === 0" class="empty-state" style="padding: var(--space-4);">
              <p style="margin: 0;">暂无 guardian 审计事件（未开启或尚无 CRITICAL 操作触发）。</p>
            </div>
            <div v-else class="table-wrap">
              <table data-test="guardian-audit-table">
                <thead><tr><th>时间</th><th>裁决</th><th>操作</th><th>目标</th><th>原因</th></tr></thead>
                <tbody>
                  <tr v-for="(e, idx) in entries" :key="idx">
                    <td style="font-size: var(--text-xs);">{{ formatDate(e.timestamp) }}</td>
                    <td>
                      <span class="layer-status" :class="'tone-' + verdictTone(e.policy)" :data-test="'guardian-verdict-' + idx">{{ e.policy }}</span>
                    </td>
                    <td>{{ e.operation || '--' }}</td>
                    <td style="max-width: 220px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap;">{{ e.target || '--' }}</td>
                    <td style="max-width: 240px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: var(--text-xs); color: var(--text-muted, #888);">{{ e.reason || '--' }}</td>
                  </tr>
                </tbody>
              </table>
            </div>
            <div class="hint" style="margin-top: var(--space-2);">显示最近 {{ entries.length }} 条（policy_rule 以 guardian 开头的事件）。</div>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.hint {
  font-size: var(--text-xs);
  color: var(--text-muted, #888);
  margin-top: var(--space-1);
  max-width: 640px;
}
.guardian-param {
  display: flex;
  align-items: baseline;
  gap: var(--space-2);
  flex-wrap: wrap;
  margin: var(--space-2) 0;
}
.param-key {
  font-size: var(--text-sm);
  min-width: 96px;
}
.layer-status {
  font-size: var(--text-xs);
  padding: 1px 10px;
  border-radius: 999px;
  border: 1px solid currentColor;
  white-space: nowrap;
}
.layer-status.tone-on {
  color: var(--success, #2da44e);
  background: color-mix(in srgb, var(--success, #2da44e) 12%, transparent);
}
.layer-status.tone-off {
  color: var(--error, #cf222e);
  background: color-mix(in srgb, var(--error, #cf222e) 12%, transparent);
}
.layer-status.tone-warn {
  color: var(--warning, #e5a00d);
  background: color-mix(in srgb, var(--warning, #e5a00d) 12%, transparent);
}
</style>
