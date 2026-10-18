<script setup lang="ts">
import { computed, reactive } from 'vue'
import { useCredentials, type PendingCredential } from '../composables/useCredentials'

/**
 * B4（2026-10-18 凭据 vault §7）：凭据索要卡（模态）。
 *
 * 模型 API key 401/403 等触发点发卡 → SSE `credential-requested` → 本组件
 * 弹出模态：服务/别名/原因 + 密码输入 + 倒计时；提交经 WSAPI
 * `credential.respond` 送回 WebCredentialBroker（先落 vault 再放行等待侧，
 * agent 原地重解析续行）。忽略 = 只摘本地卡（服务端等待窗口自然超时，
 * 调用方走原错误路径）。`credential-resolved` 广播到达时 composable 统一
 * 摘卡（竞速败方窗口无感关闭）。
 * 挂载于 AppLayout 根层（与 ApprovalCard/QuestionCard 同层同理：任意页面
 * 都能响应）。
 */

const { pendingCredentials, now, submitCredential, dismissCredential } = useCredentials()

const visible = computed(() => pendingCredentials.length > 0)

/** 每张卡的值草稿（request_id → 输入内容；提交后随卡清除）。 */
const drafts = reactive<Record<string, string>>({})

function submit(c: PendingCredential) {
  const value = drafts[c.request_id] ?? ''
  if (!value.trim()) return
  submitCredential(c.request_id, value)
  delete drafts[c.request_id]
}

function dismiss(c: PendingCredential) {
  dismissCredential(c.request_id)
  delete drafts[c.request_id]
}

function remaining(c: PendingCredential): number {
  return Math.max(0, Math.ceil((c.expiresAt - now.value) / 1000))
}
</script>

<template>
  <div v-if="visible" class="modal-backdrop credential-backdrop">
    <div
      v-for="c in pendingCredentials"
      :key="c.request_id"
      class="modal credential-modal"
    >
      <div class="approval-card credential-card">
        <div class="approval-header">
          <h3 style="margin: 0;">🔑 需要凭据</h3>
          <span class="badge">vault</span>
        </div>

        <div class="credential-body">
          <p class="credential-meta">
            服务：<code>{{ c.service || '—' }}</code>
            · 写入别名：<code>{{ c.key_name }}</code>
          </p>
          <p v-if="c.reason" class="credential-reason">{{ c.reason }}</p>
          <input
            v-model="drafts[c.request_id]"
            type="password"
            class="credential-input"
            autocomplete="off"
            placeholder="粘贴 API key / 凭据值（提交后存入 vault，不再明文流转）"
            @keyup.enter="submit(c)"
          />
        </div>

        <div class="approval-footer">
          <span class="approval-countdown">⏱ {{ remaining(c) }}s 后超时（任务将按原错误路径收尾）</span>
          <div class="approval-actions">
            <button class="btn" @click="dismiss(c)">忽略</button>
            <button
              class="btn btn-success"
              :disabled="!(drafts[c.request_id] ?? '').trim()"
              @click="submit(c)"
            >
              保存并继续
            </button>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
/* 模态壳复用全局 .modal-backdrop / .modal；卡片复用全局 .approval-card
   家族基线（凭据卡无风险语义 → 默认色条）。 */
.credential-backdrop {
  display: flex;
  align-items: center;
  justify-content: center;
}

.credential-modal {
  width: min(560px, calc(100vw - 32px));
  padding: 0;
  background: transparent;
  border: none;
  box-shadow: none;
}

.credential-body {
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
  margin-bottom: var(--space-3);
}

.credential-meta {
  margin: 0;
  font-size: var(--text-sm);
}

.credential-meta code {
  font-weight: 600;
}

.credential-reason {
  margin: 0;
  font-size: var(--text-xs);
  color: var(--text-secondary, var(--text-muted, #888));
  white-space: pre-wrap;
  word-break: break-word;
}

.credential-input {
  width: 100%;
  padding: var(--space-2);
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
  font-family: monospace;
  font-size: var(--text-sm);
  background: var(--bg-input, transparent);
  color: inherit;
}

.credential-input:focus {
  outline: none;
  border-color: var(--accent);
}
</style>
