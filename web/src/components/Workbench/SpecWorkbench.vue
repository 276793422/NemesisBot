<script setup lang="ts">
// 规格确认工作台（人工触点体系 P2）：规格草案 markdown 全宽渲染 +
// 假设/验收检查清单 + 粘性操作条（确认/修改/别再问我/取消）。
// 载荷形态（后端 spec payload，touchpoints.rs）：{version, spec(markdown),
// assumptions[], acceptance_checks[], distilled}。
// 修改 = 页内编辑模式（textarea 预填当前草案）→ revise_spec {spec 必填非空}，
// 否决不重挂卡（同载荷继续，veto_rounds 累计）；取消可带 reason。
import { computed, ref } from 'vue'
import { renderMarkdownHtml } from '../../utils/markdown'
import { parsePayload } from './registry'

const props = defineProps<{
  /** workbench.get 返回（touchpoint + issue + default_action…）。 */
  data: any
  /** 触点是否 pending（false = 超时已默认继续，操作条隐藏）。 */
  pending: boolean
  /** 散单（无项目归属）不能静音——后端诚实报错，前端先一步禁用。 */
  canMute: boolean
}>()
const emit = defineEmits<{ respond: [req: { action: string; payload?: any; label?: string }] }>()

const payload = computed(() => parsePayload(props.data?.touchpoint?.payload))
const specHtml = computed(() => renderMarkdownHtml(String(payload.value.spec || '（规格正文为空）')))
const assumptions = computed(() => (Array.isArray(payload.value.assumptions) ? payload.value.assumptions : []))
const checks = computed(() => (Array.isArray(payload.value.acceptance_checks) ? payload.value.acceptance_checks : []))
const version = computed(() => payload.value.version ?? '—')

// 页内编辑模式（✏️修改）：textarea 预填当前草案 → revise_spec。
const editing = ref(false)
const draft = ref('')

function startEdit() {
  draft.value = String(payload.value.spec || '')
  editing.value = true
}
function submitRevise() {
  const spec = draft.value.trim()
  if (!spec) return // 后端必填非空，前端先一步拦
  emit('respond', { action: 'revise_spec', payload: { spec }, label: '修改规格' })
  editing.value = false
}
</script>

<template>
  <div>
    <!-- 规格草案（markdown 全宽，单一渲染入口 renderMarkdownHtml） -->
    <div class="wb-card">
      <div class="wb-card-head">
        <h3>规格草案 <span class="muted">v{{ version }}</span></h3>
      </div>
      <div v-if="!editing" class="wb-spec-html" v-html="specHtml"></div>
      <template v-else>
        <textarea v-model="draft" class="form-textarea wb-edit-area" rows="18" data-test="spec-edit-area"></textarea>
        <div class="wb-edit-actions">
          <button class="btn btn-primary btn-sm" data-test="spec-edit-submit" @click="submitRevise">保存并提交修订</button>
          <button class="btn btn-sm" @click="editing = false">放弃修改</button>
          <span class="muted">提交后触点保持 pending（同一草案继续等待确认，否决轮次累计）</span>
        </div>
      </template>
    </div>

    <!-- 假设 / 验收检查 -->
    <div class="wb-two-col">
      <div class="wb-card">
        <h3>关键假设 <span class="muted">{{ assumptions.length }}</span></h3>
        <p v-if="assumptions.length === 0" class="muted">无假设记录。</p>
        <ul v-else class="wb-list">
          <li v-for="(a, i) in assumptions" :key="i">{{ a }}</li>
        </ul>
      </div>
      <div class="wb-card">
        <h3>验收检查 <span class="muted">{{ checks.length }}</span></h3>
        <p v-if="checks.length === 0" class="muted">无验收检查项。</p>
        <ul v-else class="wb-list">
          <li v-for="(c, i) in checks" :key="i">{{ c }}</li>
        </ul>
      </div>
    </div>

    <!-- 粘性操作条（仅 pending） -->
    <div v-if="pending" class="wb-sticky" data-test="spec-actions">
      <button
        class="btn btn-primary"
        data-test="spec-confirm"
        title="按当前草案开工（触点结案 confirmed）"
        @click="emit('respond', { action: 'confirm', label: '确认规格' })"
      >✅ 确认，按此开工</button>
      <button v-if="!editing" class="btn" data-test="spec-edit" @click="startEdit">✏️ 修改</button>
      <button
        class="btn"
        data-test="spec-mute"
        :disabled="!canMute"
        :title="canMute ? '本项目后续触点不再询问，倒计时到点按默认动作继续' : '散单（无项目归属）不支持静音'"
        @click="emit('respond', { action: 'mute', label: '别再问我' })"
      >🔇 别再问我</button>
      <button
        class="btn btn-danger"
        data-test="spec-cancel"
        title="取消该触点（可注明原因；单据本身不受影响）"
        @click="emit('respond', { action: 'cancel', payload: { reason: '人工在工作台取消' }, label: '取消触点' })"
      >✕ 取消</button>
    </div>
  </div>
</template>

<style scoped>
.muted {
  color: var(--text-muted);
  font-size: var(--text-xs);
  font-weight: 400;
}
.wb-card {
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
  padding: var(--space-4);
  margin-bottom: var(--space-3);
}
.wb-card h3 {
  margin: 0 0 var(--space-2);
  font-size: var(--text-base);
}
.wb-card-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
}
.wb-spec-html {
  word-break: break-word;
  font-size: var(--text-sm);
  line-height: 1.65;
}
.wb-two-col {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: var(--space-3);
}
@media (max-width: 800px) {
  .wb-two-col {
    grid-template-columns: 1fr;
  }
}
.wb-list {
  margin: 0;
  padding-left: var(--space-4);
  font-size: var(--text-sm);
  line-height: 1.7;
}
.wb-edit-area {
  width: 100%;
  font-family: var(--font-mono, monospace);
  font-size: var(--text-xs);
}
.wb-edit-actions {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  margin-top: var(--space-2);
  flex-wrap: wrap;
}
.wb-sticky {
  position: sticky;
  bottom: 0;
  display: flex;
  gap: var(--space-2);
  padding: var(--space-3);
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
  box-shadow: 0 -2px 8px rgba(0, 0, 0, 0.08);
  flex-wrap: wrap;
}
</style>
