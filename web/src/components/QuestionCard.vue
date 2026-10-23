<script setup lang="ts">
import { computed, reactive } from 'vue'
import {
  useQuestions,
  QUESTION_DROPDOWN_THRESHOLD,
  isTerminalQuestion,
  type PendingQuestion,
} from '../composables/useQuestions'

/**
 * F7（devtool-upgrade 阶段 5）：结构化提问卡（模态）。
 *
 * agent 的 `question` 工具发起提问 → SSE `question-asked` → 本组件弹出
 * 模态 + 倒计时；提交经 WSAPI `question.respond` 送回 WebQuestionBroker
 * （超时服务端放行，模型按最佳判断继续）。裁决广播 `question-resolved`
 * 到达时 composable 统一摘卡（竞速败方窗口无感关闭）。
 * 挂载于 AppLayout 根层（与 ApprovalCard 同层同理：任意页面都能响应，
 * agent 提问不挑用户正在看的页面）。
 *
 * B8（提问两形态）：
 * - 选项卡（fields 空）：单选 radio / 多选 checkbox；单选选项超过
 *   QUESTION_DROPDOWN_THRESHOLD 收纳为下拉 select（工具侧上限 12）。
 * - 表单卡（fields 非空）：每字段一行——text 单行输入 / select 下拉；
 *   required 字段标红星，提交按钮在必填齐活前置灰。options/multi 不参与
 *   语义。
 */

const { pendingQuestions, now, respondTo, respondForm } = useQuestions()

const visible = computed(() => pendingQuestions.length > 0)

/**
 * 渲染序：终级卡（timeout_secs=0，永不超时）置顶，其余保持到达序。
 * 稳定排序——同档内不重排（避免并发卡抖动）。
 */
const orderedQuestions = computed(() => {
  const normal: PendingQuestion[] = []
  const terminal: PendingQuestion[] = []
  for (const q of pendingQuestions) {
    ;(isTerminalQuestion(q) ? terminal : normal).push(q)
  }
  return [...terminal, ...normal]
})

/** 每张卡的选项草稿（question_id → 已勾选选项；提交后随卡清除）。 */
const drafts = reactive<Record<string, string[]>>({})

/** B8：每张卡的表单草稿（question_id → 字段 key → 值）。 */
const formDrafts = reactive<Record<string, Record<string, string>>>({})

/** 表单卡判定：fields 非空 = 表单形态（后端两形态互斥）。 */
function isForm(q: PendingQuestion): boolean {
  return q.fields.length > 0
}

function draftFor(q: PendingQuestion): string[] {
  return drafts[q.question_id] ?? []
}

function formDraftFor(q: PendingQuestion): Record<string, string> {
  if (!formDrafts[q.question_id]) {
    formDrafts[q.question_id] = Object.fromEntries(q.fields.map(f => [f.key, '']))
  }
  return formDrafts[q.question_id]
}

/** 表单卡提交可用性：所有 required 字段 trim 后非空。 */
function formReady(q: PendingQuestion): boolean {
  const draft = formDraftFor(q)
  return q.fields.filter(f => f.required).every(f => (draft[f.key] ?? '').trim() !== '')
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
  if (isForm(q)) {
    if (!formReady(q)) return
    // 只提交非空字段（可选空值后端剔除；空对象 = 全部可选未填，合法）。
    const draft = formDraftFor(q)
    const values: Record<string, string> = {}
    for (const [k, v] of Object.entries(draft)) {
      if (v.trim() !== '') values[k] = v
    }
    respondForm(q.question_id, values)
    delete formDrafts[q.question_id]
  } else {
    respondTo(q.question_id, [...draftFor(q)])
    delete drafts[q.question_id]
  }
}

/** 单选选项卡是否收纳为下拉（B8：>6 项；多选恒 checkbox 列表）。 */
function asDropdown(q: PendingQuestion): boolean {
  return !q.multi && q.options.length > QUESTION_DROPDOWN_THRESHOLD
}

function remaining(q: PendingQuestion): number {
  return Math.max(0, Math.ceil((q.expiresAt - now.value) / 1000))
}
</script>

<template>
  <div v-if="visible" class="modal-backdrop question-backdrop">
    <div
      v-for="q in orderedQuestions"
      :key="q.question_id"
      class="modal question-modal"
    >
      <div class="approval-card question-card" :class="{ 'question-terminal': isTerminalQuestion(q) }">
        <div class="approval-header">
          <h3 style="margin: 0;">{{ isTerminalQuestion(q) ? '🚨 循环无法打破——需要你决定' : '❓ Agent 提问' }}</h3>
          <span class="badge" :class="{ 'badge-danger': isTerminalQuestion(q) }">{{ isTerminalQuestion(q) ? '必答·无超时' : isForm(q) ? '表单' : q.multi ? '多选' : '单选' }}</span>
        </div>

        <div class="question-body">
          <p class="question-text">{{ q.question }}</p>

          <!-- B8 表单卡：字段逐行渲染（text 输入框 / select 下拉）。 -->
          <template v-if="isForm(q)">
            <div v-for="f in q.fields" :key="f.key" class="question-field">
              <label class="question-field-label">
                <span>{{ f.label }}<template v-if="f.required"> <span class="required-mark">*</span></template></span>
                <input
                  v-if="f.type === 'text'"
                  type="text"
                  class="question-field-input"
                  :value="formDraftFor(q)[f.key] ?? ''"
                  @input="formDraftFor(q)[f.key] = ($event.target as HTMLInputElement).value"
                />
                <select
                  v-else-if="f.type === 'select'"
                  class="question-field-input"
                  :value="formDraftFor(q)[f.key] ?? ''"
                  @change="formDraftFor(q)[f.key] = ($event.target as HTMLSelectElement).value"
                >
                  <option value="" disabled>{{ f.required ? '请选择' : '（不选）' }}</option>
                  <option v-for="opt in f.options" :key="opt" :value="opt">{{ opt }}</option>
                </select>
              </label>
            </div>
          </template>

          <!-- B8 单选下拉收纳：选项 >6 时 select 代替 radio 列表。 -->
          <template v-else-if="asDropdown(q)">
            <select
              class="question-field-input question-dropdown"
              :value="draftFor(q)[0] ?? ''"
              @change="toggle(q, ($event.target as HTMLSelectElement).value)"
            >
              <option value="" disabled>请选择</option>
              <option v-for="opt in q.options" :key="opt" :value="opt">{{ opt }}</option>
            </select>
          </template>

          <!-- 选项卡（≤6 radio / 多选 checkbox）。 -->
          <template v-else>
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
          </template>
        </div>

        <div class="approval-footer">
          <!-- 终级卡无超时：不倒计时不自动关——必须人来决断（烟雾报警器
               必须响）；选「停止」即停车（deny / 关窗都不回答 = 停车）。 -->
          <span v-if="isTerminalQuestion(q)" class="approval-countdown terminal-countdown">
            ⏹ 无超时——agent 循环已暂停，等待你的决定（任何自动化放行对此卡无效）
          </span>
          <span v-else class="approval-countdown">⏱ {{ remaining(q) }}s 后超时（模型将自行判断）</span>
          <div class="approval-actions">
            <button
              class="btn btn-success"
              :disabled="isForm(q) ? !formReady(q) : draftFor(q).length === 0"
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

/* B8：表单字段行 + 下拉（表单 select 与单选下拉共用输入样式）。 */
.question-field {
  display: flex;
  flex-direction: column;
  gap: var(--space-1);
}

.question-field-label {
  display: flex;
  flex-direction: column;
  gap: var(--space-1);
  font-size: var(--text-sm);
}

.required-mark {
  color: var(--danger, #e5484d);
}

.question-field-input {
  padding: var(--space-2);
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
  background: var(--bg-input, transparent);
  color: inherit;
  font-size: var(--text-sm);
}

.question-field-input:focus {
  border-color: var(--accent);
  outline: none;
}

.question-dropdown {
  width: 100%;
  cursor: pointer;
}

/* 终级卡（doom-loop ≥24 次，2026-10-24 循环干预阶梯）：红色高危渲染。
 * 左侧色条走 .approval-card 的边框色变量；标题与提示用危险色强调。 */
.question-terminal {
  border-left-color: var(--danger, #e5484d) !important;
}

.question-terminal .badge-danger {
  background: var(--danger, #e5484d);
  color: #fff;
}

.terminal-countdown {
  color: var(--danger, #e5484d);
  font-weight: 600;
}
</style>
