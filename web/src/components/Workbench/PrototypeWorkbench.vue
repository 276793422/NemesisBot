<script setup lang="ts">
// 原型评审工作台（人工触点体系 P2）：机器闸徽标 + 摘要 + 报告 markdown +
// 可玩预览（仅当载荷带 play_url——后端今日不产该字段 = 诚实缺席，渲染面
// 已就绪）+ 粘性操作条（确认方向 / 局部返工 / 别再问我 / 取消）。
// 载荷形态（后端 prototype payload）：{version, machine_gate:"pass", report,
// summary, recheck_review?}。
// 局部返工 = 展开否决面板（粘性条上方，不遮正文）：必填原因（原因需写明
// 范围——后端载荷只收 reason+recheck，范围是 reason 文本的一部分）+
// 「改完再看」（recheck——机器闸复验时要求再挂人工卡）。
// 后端语义：原型否决不重挂本卡（落账后恢复钩子证据回灌重派，每次机器
// 周期挂新卡）；confirm = 按当前版继续。
import { computed, ref } from 'vue'
import { renderMarkdownHtml } from '../../utils/markdown'
import { safeExternalUrl } from '../../utils/url'
import { parsePayload } from './registry'

const props = defineProps<{
  data: any
  /** 触点是否 pending（false = 超时已默认继续，操作条隐藏）。 */
  pending: boolean
  /** 散单（无项目归属）不能静音。 */
  canMute: boolean
}>()
const emit = defineEmits<{ respond: [req: { action: string; payload?: any; label?: string }] }>()

const payload = computed(() => parsePayload(props.data?.touchpoint?.payload))
const reportHtml = computed(() => renderMarkdownHtml(String(payload.value.report || '（原型报告为空）')))
const summary = computed(() => (typeof payload.value.summary === 'string' && payload.value.summary) || '')
const machineGate = computed(() => String(payload.value.machine_gate || ''))
const machineGatePass = computed(() => machineGate.value === 'pass')
const recheckReview = computed(() => payload.value.recheck_review != null)
const playUrl = computed(() => {
  const u = payload.value.play_url
  // iframe 的 src 是 URL 属性绑定：payload 源自触点载荷（模型/流水线产物，
  // 提示词注入可携带 javascript:/data: 伪 URL——iframe src 的 javascript:
  // 在 about:blank 继承父 origin 执行 = Dashboard 域内脚本执行，持全量
  // WSAPI 权限）。过 safeExternalUrl scheme 闸（BUG-20261101-274 同款约定
  // 「href/src 属性绑定必过本闸」——Deliveries/Mcp 同源先例）；非 http(s)
  // 返 '' → v-if 诚实缺席（与「后端不产该字段 = 不渲染」同一语义面）。
  return typeof u === 'string' && u ? safeExternalUrl(u) : ''
})

// 否决面板（粘性条上方展开）
const vetoOpen = ref(false)
const vetoReason = ref('')
const vetoRecheck = ref(false)
const vetoErr = ref('')

function openVeto() {
  vetoOpen.value = true
  vetoErr.value = ''
}
function submitVeto() {
  const reason = vetoReason.value.trim()
  if (!reason) {
    vetoErr.value = '否决必须给出原因（写明范围：哪里不对、期望什么）'
    return
  }
  vetoErr.value = ''
  emit('respond', {
    action: 'rework_local',
    payload: { reason, recheck: vetoRecheck.value },
    label: '局部返工',
  })
  vetoOpen.value = false
  vetoReason.value = ''
  vetoRecheck.value = false
}
</script>

<template>
  <div>
    <!-- 机器闸 + 复验徽标 + 摘要 -->
    <div class="wb-card">
      <div class="wb-badges">
        <span
          class="badge"
          :class="machineGatePass ? 'badge-success' : 'badge-warning'"
          :title="`机器闸：${machineGate || '未声明'}`"
          data-test="proto-machine-gate"
        >{{ machineGatePass ? '✓ 机器闸通过' : `机器闸：${machineGate || '未声明'}` }}</span>
        <span
          v-if="recheckReview"
          class="badge badge-info"
          title="本轮是「改完再看」复验（上一轮否决后机器闸复验重挂）"
          data-test="proto-recheck-badge"
        >🔁 复验轮</span>
      </div>
      <p v-if="summary" class="wb-summary" data-test="proto-summary">{{ summary }}</p>
    </div>

    <!-- 原型报告（markdown 全宽） -->
    <div class="wb-card">
      <h3>原型报告</h3>
      <div class="wb-report-html" v-html="reportHtml"></div>
    </div>

    <!-- 可玩预览（仅当载荷带 play_url；后端今日不产 = 不渲染，诚实缺席） -->
    <div v-if="playUrl" class="wb-card">
      <h3>可玩预览</h3>
      <iframe :src="playUrl" class="wb-play-frame" allow="fullscreen" data-test="proto-play"></iframe>
    </div>

    <!-- 否决面板（粘性条上方展开，不遮正文） -->
    <div v-if="pending && vetoOpen" class="wb-veto-panel" data-test="proto-veto-panel">
      <h4>↩️ 局部返工</h4>
      <div class="form-group">
        <label class="form-label">否决原因（必填；写明范围：哪里不对、期望什么）</label>
        <textarea
          v-model="vetoReason"
          class="form-textarea"
          rows="3"
          placeholder="例：移动端布局重叠——左下资源面板挡住建造按钮，期望小屏折叠为抽屉"
          data-test="proto-veto-reason"
        ></textarea>
        <p v-if="vetoErr" class="wb-veto-err">{{ vetoErr }}</p>
      </div>
      <label class="wb-recheck-row">
        <input v-model="vetoRecheck" type="checkbox" data-test="proto-veto-recheck" />
        改完再看（机器闸复验通过后重新挂人工卡）
      </label>
      <div class="wb-veto-actions">
        <button class="btn btn-primary btn-sm" data-test="proto-veto-submit" @click="submitVeto">提交否决</button>
        <button class="btn btn-sm" @click="vetoOpen = false">放弃</button>
        <span class="muted">否决后本卡结案，机器侧证据回灌后重派新卡（本卡不重挂）</span>
      </div>
    </div>

    <!-- 粘性操作条（仅 pending） -->
    <div v-if="pending" class="wb-sticky" data-test="proto-actions">
      <button
        class="btn btn-primary"
        data-test="proto-confirm"
        title="按当前版本继续（触点结案 confirmed）"
        @click="emit('respond', { action: 'confirm', label: '确认方向' })"
      >✅ 确认方向</button>
      <button v-if="!vetoOpen" class="btn" data-test="proto-veto" @click="openVeto">↩️ 局部返工</button>
      <button
        class="btn"
        data-test="proto-mute"
        :disabled="!canMute"
        :title="canMute ? '本项目后续触点不再询问，倒计时到点按默认动作继续' : '散单（无项目归属）不支持静音'"
        @click="emit('respond', { action: 'mute', label: '别再问我' })"
      >🔇 别再问我</button>
      <button
        class="btn btn-danger"
        data-test="proto-cancel"
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
.wb-badges {
  display: flex;
  gap: var(--space-2);
  align-items: center;
  flex-wrap: wrap;
}
.wb-summary {
  margin: var(--space-2) 0 0;
  font-size: var(--text-sm);
  line-height: 1.6;
}
.wb-report-html {
  word-break: break-word;
  font-size: var(--text-sm);
  line-height: 1.65;
}
.wb-play-frame {
  width: 100%;
  height: 520px;
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
  background: #000;
}
.wb-veto-panel {
  background: color-mix(in srgb, var(--warning) 8%, var(--surface));
  border: 1px solid var(--warning);
  border-radius: var(--radius-md);
  padding: var(--space-3) var(--space-4);
  margin-bottom: var(--space-3);
}
.wb-veto-panel h4 {
  margin: 0 0 var(--space-2);
}
.wb-veto-err {
  color: var(--error, var(--warning));
  font-size: var(--text-xs);
  margin: var(--space-1) 0 0;
}
.wb-recheck-row {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  font-size: var(--text-sm);
  margin-bottom: var(--space-2);
}
.wb-veto-actions {
  display: flex;
  align-items: center;
  gap: var(--space-2);
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
