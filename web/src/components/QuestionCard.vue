<script setup lang="ts">
import { computed, reactive } from 'vue'
import { useQuestions, type PendingQuestion } from '../composables/useQuestions'

/**
 * F7（devtool-upgrade 阶段 5）：结构化提问卡（模态）。
 *
 * agent 的 `question` 工具发起提问 → SSE `question-asked` → 本组件弹出
 * 模态：问题 + 候选选项（单选 radio / 多选 checkbox）+ 倒计时；提交经
 * WSAPI `question.respond` 送回 WebQuestionBroker（超时服务端放行，模型
 * 按最佳判断继续）。裁决广播 `question-resolved` 到达时 composable 统一
 * 摘卡（竞速败方窗口无感关闭）。
 * 挂载于 AppLayout 根层（与 ApprovalCard 同层同理：任意页面都能响应，
 * agent 提问不挑用户正在看的页面）。
 */

const { pendingQuestions, now, respondTo } = useQuestions()

const visible = computed(() => pendingQuestions.length > 0)

/** 每张卡的选项草稿（question_id → 已勾选选项；提交后随卡清除）。 */
const drafts = reactive<Record<string, string[]>>({})

function draftFor(q: PendingQuestion): string[] {
  return drafts[q.question_id] ?? []
}

function toggle(q: PendingQuestion, option: string) {
  const cur = drafts[q.question_id] ?? []
  drafts[q.question_id] = q.multi
    ? cur.includes(option)
      ? cur.filter(o => o !== option)
      : [...cur, option]
    : [option]
}

function submit(q: PendingQuestion) {
  respondTo(q.question_id, [...draftFor(q)])
  delete drafts[q.question_id]
}

function remaining(q: PendingQuestion): number {
  return Math.max(0, Math.ceil((q.expiresAt - now.value) / 1000))
}
</script>

<template>
  <div v-if="visible" class="modal-backdrop question-backdrop">
    <div
      v-for="q in pendingQuestions"
      :key="q.question_id"
      class="modal question-modal"
    >
      <div class="approval-card question-card">
        <div class="approval-header">
          <h3 style="margin: 0;">❓ Agent 提问</h3>
          <span class="badge">{{ q.multi ? '多选' : '单选' }}</span>
        </div>

        <div class="question-body">
          <p class="question-text">{{ q.question }}</p>
          <label
            v-for="opt in q.options"
            :key="opt"
            class="question-option"
          >
            <input
              :type="q.multi ? 'checkbox' : 'radio'"
              :name="q.question_id"
              :checked="draftFor(q).includes(opt)"
              @change="toggle(q, opt)"
            />
            <span>{{ opt }}</span>
          </label>
        </div>

        <div class="approval-footer">
          <span class="approval-countdown">⏱ {{ remaining(q) }}s 后超时（模型将自行判断）</span>
          <div class="approval-actions">
            <button
              class="btn btn-success"
              :disabled="draftFor(q).length === 0"
              @click="submit(q)"
            >
              提交
            </button>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
/* 模态壳复用全局 .modal-backdrop / .modal；卡片复用全局 .approval-card
   家族基线（信息型提问同样走左侧色条卡片，无风险语义 → 默认 warning 色条）。 */
.question-backdrop {
  display: flex;
  align-items: center;
  justify-content: center;
}

.question-modal {
  width: min(520px, calc(100vw - 32px));
  padding: 0;
  background: transparent;
  border: none;
  box-shadow: none;
}

.question-body {
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
  margin-bottom: var(--space-3);
}

.question-text {
  margin: 0;
  font-size: var(--text-sm);
  font-weight: 600;
  white-space: pre-wrap;
  word-break: break-word;
}

.question-option {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  padding: var(--space-2);
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
  cursor: pointer;
  font-size: var(--text-sm);
  transition: border-color var(--duration-fast), background var(--duration-fast);
}

.question-option:hover {
  border-color: var(--accent);
}

.question-option input {
  flex-shrink: 0;
  cursor: pointer;
}
</style>
