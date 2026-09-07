<script setup lang="ts">
import { computed, reactive } from 'vue'
import { useApprovals, type ApprovalRequest } from '../composables/useApprovals'

/**
 * M7（devtool-upgrade 阶段 5）：安全审批卡（模态）。
 *
 * auditor "ask" 规则命中 → SSE `approval-requested` → 本组件弹出模态，
 * 展示操作/目标/风险/理由 + 倒计时；批准/拒绝/总是允许经 WSAPI
 * `approval.respond` 送回 WebApprovalManager（超时服务端自动拒绝）。
 * F3：「总是允许」按钮仅在 pattern 适用时显示（有 pattern 且层级安全门
 * 通过 = 非 CRITICAL，或 CRITICAL 但 exec 类——与后端 rule_permitted_for
 * 同语义；真值判定在后端，这里只是 UX 镜像），并回显将记住的 pattern。
 * F6：拒绝可携带备注（输入框 → respond note → auditor 拼进拒绝消息回灌
 * 给模型，帮它下一轮纠正）；裁决广播 `approval-resolved` 到达时 composable
 * 统一摘卡（竞速败方窗口无感关闭）。
 * 挂载于 AppLayout 根层 —— 任意页面都能响应，与 ToastContainer 同层。
 */

const { pendingApprovals, now, respondTo } = useApprovals()

const visible = computed(() => pendingApprovals.length > 0)

/** F6: 每张卡的拒绝备注草稿（request_id → 输入内容；批准路径忽略）。 */
const noteDrafts = reactive<Record<string, string>>({})

function deny(req: ApprovalRequest) {
  const note = (noteDrafts[req.request_id] ?? '').trim()
  respondTo(req.request_id, false, false, note || undefined)
  delete noteDrafts[req.request_id]
}

function remaining(req: ApprovalRequest): number {
  return Math.max(0, Math.ceil((req.expiresAt - now.value) / 1000))
}

const RISK_LABELS: Record<string, string> = {
  LOW: '低风险',
  MEDIUM: '中风险',
  HIGH: '高风险',
  CRITICAL: '严重',
}

function riskLabel(level: string): string {
  return RISK_LABELS[level.toUpperCase()] ?? level
}

function riskBadgeClass(level: string): string {
  switch (level.toUpperCase()) {
    case 'CRITICAL':
    case 'HIGH':
      return 'badge badge-error'
    case 'MEDIUM':
      return 'badge badge-warning'
    default:
      return 'badge'
  }
}

/** 层级安全门 UX 镜像（后端 rule_permitted_for 同语义）：CRITICAL 只有
 * exec 类操作可记忆；pattern 空 = 后端派生不了规则，一律隐藏按钮。 */
function canAlways(req: ApprovalRequest): boolean {
  if (!req.pattern) return false
  if (req.risk_level.toUpperCase() !== 'CRITICAL') return true
  const op = req.operation.toLowerCase()
  return op === 'process_exec' || op === 'process_spawn'
}
</script>

<template>
  <div v-if="visible" class="modal-backdrop approval-backdrop">
    <div
      v-for="req in pendingApprovals"
      :key="req.request_id"
      class="modal approval-modal"
    >
      <div class="approval-card" :class="req.risk_level.toLowerCase()">
        <div class="approval-header">
          <h3 style="margin: 0;">🔐 安全审批</h3>
          <span :class="riskBadgeClass(req.risk_level)">{{ riskLabel(req.risk_level) }}</span>
        </div>

        <div class="approval-body">
          <div class="approval-row">
            <span class="approval-label">操作</span>
            <code class="approval-value">{{ req.operation }}</code>
          </div>
          <div class="approval-row">
            <span class="approval-label">目标</span>
            <code class="approval-value approval-target">{{ req.target }}</code>
          </div>
          <div v-if="req.reason" class="approval-row">
            <span class="approval-label">理由</span>
            <span class="approval-value">{{ req.reason }}</span>
          </div>
          <!-- F3：总是允许前的确认串回显——用户看到的就是将写入规则的 pattern。 -->
          <div v-if="canAlways(req)" class="approval-row approval-always-scope">
            <span class="approval-label">记住</span>
            <code class="approval-value approval-target">{{ req.pattern }}</code>
          </div>
          <!-- F6：拒绝备注（可选）——回灌给模型帮它下一轮纠正。 -->
          <input
            v-model="noteDrafts[req.request_id]"
            class="input approval-note-input"
            type="text"
            maxlength="500"
            placeholder="拒绝备注（可选，将告知模型原因）"
            @keydown.enter.prevent="deny(req)"
          />
        </div>

        <div class="approval-footer">
          <span class="approval-countdown">⏱ {{ remaining(req) }}s 后自动拒绝</span>
          <div class="approval-actions">
            <button class="btn btn-danger" @click="deny(req)">拒绝</button>
            <button
              v-if="canAlways(req)"
              class="btn"
              title="批准本次，并记住该 pattern（同类操作不再询问）"
              @click="respondTo(req.request_id, true, true)"
            >
              总是允许
            </button>
            <button class="btn btn-success" @click="respondTo(req.request_id, true)">批准</button>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
/* 模态壳复用全局 .modal-backdrop / .modal；这里只补审批卡专属的
   布局细节（全局 .approval-card 家族在 components.css 已有基线）。 */
.approval-backdrop {
  display: flex;
  align-items: center;
  justify-content: center;
}

.approval-modal {
  width: min(520px, calc(100vw - 32px));
  padding: 0;
  background: transparent;
  border: none;
  box-shadow: none;
}

.approval-card {
  margin-bottom: 0;
}

.approval-body {
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
  margin-bottom: var(--space-3);
}

.approval-row {
  display: flex;
  gap: var(--space-2);
  align-items: baseline;
}

.approval-label {
  flex-shrink: 0;
  width: 3em;
  font-size: var(--text-xs);
  color: var(--text-muted);
}

.approval-value {
  font-size: var(--text-sm);
  word-break: break-all;
}

.approval-target {
  font-family: var(--font-mono);
}

.approval-always-scope {
  padding-top: var(--space-1);
  border-top: 1px dashed var(--border);
}

.approval-note-input {
  width: 100%;
  margin-top: var(--space-1);
  font-size: var(--text-sm);
}

.approval-footer {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--space-2);
}

.approval-countdown {
  font-size: var(--text-xs);
  color: var(--text-muted);
}
</style>
