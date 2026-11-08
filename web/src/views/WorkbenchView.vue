<script setup lang="ts">
// 统一工作台（人工触点体系 P2，2026-11-07 规划）：/workbench/touchpoint-{kind}/{id}。
// 触点详情/裁决重页——看板轻卡点进来，这里承载重量（规格全文/原型报告 markdown +
// 决定史 + 粘性操作条）。骨架只管：装载（workbench.get）、超时态横条、无人值守
// 提示、共享侧栏（承载单 meta + 决定史）、respond 管道；正文与操作按钮按 kind
// 分派给注册表组件（registry.ts，与后端 dispatch 同构）。
// 后端契约（crates/nemesis-web/src/handlers/board/touchpoints.rs）：
//   workbench.get  {kind, id} → {touchpoint, issue, overdue, default_action,
//                                unattended, timeout_secs}
//   workbench.respond {kind, id, action, payload}；非 pending 拒绝（迟到意见
//   走单据评论）。
import { computed, ref, onMounted, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'
import { resolveKind, parsePayload } from '../components/Workbench/registry'
import { touchpointKindLabel, touchpointCountdown, touchpointDefaultActionLabel, type TouchpointRow } from '../components/board/boardMeta'
import { fmtTime } from '../components/board/boardMeta'

const route = useRoute()
const router = useRouter()
const { request } = useWSAPI()
const toast = useToast()

const loading = ref(true)
const loadError = ref('')
// workbench.get 返回（touchpoint 形态与 touchpoint.list 行内同构）。
const data = ref<any>(null)

const kindDef = computed(() => resolveKind(String(route.params.kind || '')))
const tpId = computed(() => Number(route.params.id) || 0)

const touchpoint = computed(() => data.value?.touchpoint || null)
const issue = computed(() => data.value?.issue || null)
const isPending = computed(() => touchpoint.value?.status === 'pending')
const isDefaulted = computed(() => touchpoint.value?.status === 'timeout_defaulted')
// mute（别再问我）要求项目归属——散单后端诚实报错，前端禁用+提示先一步。
const canMute = computed(() => touchpoint.value?.project_id != null)
// 决定史 = 承载单 System 评论（确认/超时/否决/取消均落系统评论）。
const decisions = computed(() => {
  const cs = Array.isArray(issue.value?.comments) ? issue.value.comments : []
  return cs.filter((c: any) => (c.ctype || c.type) === 'system')
})

const countdown = computed(() => touchpointCountdown(touchpoint.value?.deadline_ms || 0))

async function load() {
  if (!kindDef.value || !tpId.value) {
    loadError.value = `未知工作台类型或编号：${route.params.kind}/${route.params.id}`
    loading.value = false
    return
  }
  loading.value = true
  loadError.value = ''
  try {
    data.value = await request('workbench', 'get', { kind: kindDef.value.kind, id: tpId.value })
  } catch (e: any) {
    loadError.value = String(e)
  } finally {
    loading.value = false
  }
}

/** respond 管道：kind 组件 emit('respond', {action, payload}) → 后端 → 重载。 */
async function onRespond(req: { action: string; payload?: any; label?: string }) {
  if (!kindDef.value) return
  try {
    await request('workbench', 'respond', {
      kind: kindDef.value.kind,
      id: tpId.value,
      action: req.action,
      payload: req.payload || {},
    })
    toast.success(req.label ? `${req.label}：已提交` : '已提交')
    await load()
  } catch (e: any) {
    toast.error(String(e))
  }
}

function goBoard() {
  router.push('/board')
}

onMounted(load)
watch(() => [route.params.kind, route.params.id], load)
</script>

<template>
  <div class="workbench">
    <div v-if="loading" style="text-align: center; padding: var(--space-8);">
      <div class="spinner spinner-lg" style="margin: 0 auto;"></div>
    </div>

    <div v-else-if="loadError" class="wb-error">
      <p>⚠ {{ loadError }}</p>
      <button class="btn" @click="goBoard">← 返回看板</button>
    </div>

    <template v-else-if="data && touchpoint && kindDef">
      <!-- 头部：返回 + kind 徽标 + 承载单标题/编号 + 倒计时 -->
      <div class="wb-header">
        <button class="btn btn-sm" @click="goBoard">← 返回看板</button>
        <span class="badge badge-warning">{{ kindDef.icon }} {{ touchpointKindLabel(touchpoint.kind) }}</span>
        <span class="wb-title">{{ issue?.title || `#${tpId}` }}</span>
        <code v-if="issue?.number">{{ issue.number }}</code>
        <span
          class="wb-countdown"
          :class="{ urgent: countdown.urgent }"
        >{{ countdown.text }}</span>
        <span v-if="data.overdue" class="badge badge-error">已过期</span>
      </div>

      <!-- 超时态横条：已按默认继续，人工只剩补意见（走单据评论进变更管理） -->
      <div v-if="isDefaulted" class="wb-banner wb-banner-timeout" data-test="wb-timeout-banner">
        ⏱ 已按默认继续：<b>{{ touchpointDefaultActionLabel(data.default_action || kindDef.defaultAction) }}</b>。
        本触点已结案，不再接受确认/否决；迟到意见请到承载单评论区补充（进变更管理）。
      </div>
      <!-- 无人值守横幅：项目/单子挂 unattended 时该触点只是展示面 -->
      <div v-if="data.unattended" class="wb-banner wb-banner-unattended" data-test="wb-unattended-banner">
        🤖 无人值守模式：此单子不等待人工裁决，倒计时到点按默认动作继续。下面的操作仍可提前介入。
      </div>

      <div class="wb-layout">
        <!-- 正文：kind 组件（规格全文 / 原型报告） -->
        <div class="wb-main">
          <component
            :is="kindDef.component"
            :data="data"
            :pending="isPending"
            :can-mute="canMute"
            @respond="onRespond"
          />
        </div>

        <!-- 共享侧栏：承载单 meta + 决定史 -->
        <aside class="wb-side">
          <div class="wb-side-card">
            <h4>触点信息</h4>
            <dl class="wb-meta">
              <dt>状态</dt><dd><span class="badge badge-neutral">{{ touchpoint.status }}</span></dd>
              <dt>截止</dt><dd>{{ touchpoint.deadline_ms ? new Date(touchpoint.deadline_ms).toLocaleString() : '—' }}</dd>
              <dt>否决轮次</dt><dd>{{ touchpoint.veto_rounds }}</dd>
              <dt>超时预算</dt><dd>{{ data.timeout_secs ? `${data.timeout_secs}s` : '—' }}</dd>
              <dt>默认动作</dt><dd>{{ touchpointDefaultActionLabel(data.default_action || kindDef.defaultAction) }}</dd>
            </dl>
          </div>
          <div class="wb-side-card">
            <h4>决定史</h4>
            <p v-if="decisions.length === 0" class="muted">暂无系统决定记录。</p>
            <ol v-else class="wb-decisions">
              <li v-for="c in decisions" :key="c.id">
                <div class="wb-decision-time muted">{{ fmtTime(c.created_at) }}</div>
                <div class="wb-decision-content">{{ c.content }}</div>
              </li>
            </ol>
          </div>
        </aside>
      </div>
    </template>
  </div>
</template>

<style scoped>
.workbench {
  max-width: 1200px;
  margin: 0 auto;
}
.muted {
  color: var(--text-muted);
  font-size: var(--text-xs);
}
.wb-error {
  text-align: center;
  padding: var(--space-8);
}
.wb-header {
  display: flex;
  align-items: center;
  gap: var(--space-3);
  margin-bottom: var(--space-4);
  flex-wrap: wrap;
}
.wb-title {
  font-size: var(--text-lg);
  font-weight: 600;
  word-break: break-word;
}
.wb-countdown {
  font-size: var(--text-sm);
  color: var(--text-muted);
  white-space: nowrap;
}
.wb-countdown.urgent {
  color: var(--warning);
  font-weight: 600;
}
.wb-banner {
  border-radius: var(--radius-md);
  padding: var(--space-3) var(--space-4);
  margin-bottom: var(--space-4);
  font-size: var(--text-sm);
}
.wb-banner-timeout {
  background: color-mix(in srgb, var(--warning) 12%, transparent);
  border: 1px solid var(--warning);
}
.wb-banner-unattended {
  background: color-mix(in srgb, var(--info) 10%, transparent);
  border: 1px solid var(--info);
}
.wb-layout {
  display: grid;
  grid-template-columns: minmax(0, 1fr) 300px;
  gap: var(--space-4);
  align-items: start;
}
@media (max-width: 900px) {
  .wb-layout {
    grid-template-columns: 1fr;
  }
}
.wb-main {
  min-width: 0;
}
.wb-side {
  display: flex;
  flex-direction: column;
  gap: var(--space-3);
}
.wb-side-card {
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
  padding: var(--space-3);
}
.wb-side-card h4 {
  margin: 0 0 var(--space-2);
  font-size: var(--text-sm);
}
.wb-meta {
  display: grid;
  grid-template-columns: auto 1fr;
  gap: var(--space-1) var(--space-2);
  font-size: var(--text-xs);
  margin: 0;
}
.wb-meta dt {
  color: var(--text-muted);
}
.wb-meta dd {
  margin: 0;
  word-break: break-all;
}
.wb-decisions {
  margin: 0;
  padding-left: var(--space-4);
  font-size: var(--text-xs);
}
.wb-decision-time {
  font-size: var(--text-xs);
}
.wb-decision-content {
  word-break: break-word;
  white-space: pre-wrap;
}
/* 粘性操作条（kind 组件内渲染，样式共享类放全局层面：组件用 :deep 或自带） */
</style>
