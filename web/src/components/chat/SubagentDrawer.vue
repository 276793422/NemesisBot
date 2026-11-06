<script setup lang="ts">
// 子代理活动胶囊（2026-11-06 胶囊化改造，取代旧「台账抽屉」形态）。
//
// 三方调研确立的公共不变量（opencode/gemini-cli，见
// docs/PLAN/2026-11-06_子代理胶囊化改造计划.md）：
// - **live 容器只放 live 数据**：硬闸 `status === 'running'`——完成/失败/
//   取消即从胶囊消失（无宽限窗）。BUG 根因（2026-11-06 截图）正是旧版
//   只按会话过滤、终态台账行每轮在 live 容器重放。终态历史锚定在消息流
//   WorkTimelineBlock（spawn 行 ✓/✗/用时已持久化），不在此处。
// - 形态：单行胶囊（spinner + 任务名纯文本 ellipsis + 用时走表 + 悬停
//   显现停止钮）。无徽标、无 markdown 渲染、无页脚、无点击跳转——
//   live 态只是状态指示，钻取入口在历史工作块 spawn 行。
// - 静止约束（opencode「stays at rest」）：首拉已在场的行不做入场动画，
//   仅会话打开期间新出现的行淡入。
//
// 挂载点在 composer strip lane（宿主 ChatPanel 负责），非 turn 级瞬态块。
import { ref, computed, watch, onMounted, onBeforeUnmount } from 'vue'
import { useWSAPI } from '../../composables/useWSAPI'
import { addMessageHandler, removeMessageHandler } from '../../composables/useWebSocket'

const props = defineProps<{ sessionSid: string }>()

const { request } = useWSAPI()

interface SubagentRow {
  id: string
  parent_session_key: string
  parent_chat_id: string
  origin: string
  task: string
  status: string
  running: boolean
  summary?: string | null
  error?: string | null
  started_at_ms: number
}

const rows = ref<SubagentRow[]>([])
let pollTimer: ReturnType<typeof setInterval> | null = null

// A1 数据闸：会话过滤 **且** status === 'running'。台账注册表保留终态行
// （ledger 语义，容量 500），它们属于历史不属于 live。
const mine = computed(() =>
  rows.value.filter(
    (r) =>
      r.status === 'running' &&
      (r.parent_chat_id === props.sessionSid ||
        (!!props.sessionSid && r.parent_session_key.endsWith(props.sessionSid))),
  ),
)

// --- 用时走表（仅在场时每秒走，行清空即停表）---
const nowTick = ref(Date.now())
let tickTimer: number | null = null
watch(
  () => mine.value.length,
  (n) => {
    if (n > 0 && tickTimer === null) {
      nowTick.value = Date.now()
      tickTimer = window.setInterval(() => (nowTick.value = Date.now()), 1000)
    } else if (n === 0 && tickTimer !== null) {
      window.clearInterval(tickTimer)
      tickTimer = null
    }
  },
  { immediate: true },
)
onBeforeUnmount(() => {
  if (tickTimer !== null) window.clearInterval(tickTimer)
})

function elapsed(startedAtMs: number): string {
  const s = Math.max(0, Math.floor((nowTick.value - startedAtMs) / 1000))
  if (s < 60) return `${s}s`
  const m = Math.floor(s / 60)
  if (m < 60) return `${m}m ${s % 60}s`
  return `${Math.floor(m / 60)}h ${m % 60}m`
}

// --- 入场动画静止约束：首拉在场的行不动画，之后新出现的行才淡入 ---
const animIds = ref(new Set<string>())
let bootedIds = new Set<string>()
let firstLoadDone = false

async function load() {
  try {
    const r = await request('subagents', 'list', {})
    rows.value = (r?.tasks as SubagentRow[]) || []
  } catch { /* WS 未就绪——下轮再试 */ }
  const ids = new Set(mine.value.map((r) => r.id))
  if (!firstLoadDone) {
    bootedIds = ids
    firstLoadDone = true
  } else {
    const fresh = [...ids].filter((id) => !bootedIds.has(id) && !animIds.value.has(id))
    if (fresh.length) animIds.value = new Set([...animIds.value, ...fresh])
  }
}

async function stop(r: SubagentRow) {
  const excerpt = (r.task || '').slice(0, 40)
  if (!window.confirm(`终止「${excerpt}」？运行中的执行链将收到取消信号。`)) return
  try {
    await request('subagents', 'cancel', { id: r.id })
  } catch { /* WS 未就绪——行仍在，可重试 */ }
}

function handleWSMessage(frame: any) {
  // 子代理 push 帧（outer data.subagent_id）到达 = 新活动，即时刷新列表。
  if (frame?.type === 'push' && frame?.data?.subagent_id) {
    if (frame?.cmd === 'tool_event' || frame?.cmd === 'subagent_reply') load()
  }
}

watch(
  () => props.sessionSid,
  () => load(),
)

onMounted(() => {
  addMessageHandler(handleWSMessage)
  load()
  pollTimer = setInterval(load, 5000)
})

onBeforeUnmount(() => {
  if (pollTimer) clearInterval(pollTimer)
  removeMessageHandler(handleWSMessage)
})
</script>

<template>
  <div v-if="mine.length" class="subagent-capsules" role="status">
    <div
      v-for="r in mine"
      :key="r.id"
      class="subagent-capsule"
      :class="{ 'sc-anim': animIds.has(r.id) }"
    >
      <span class="sc-spinner"></span>
      <span class="sc-task" :title="r.task">{{ r.task || '（无任务文本）' }}</span>
      <span class="sc-elapsed">{{ elapsed(r.started_at_ms) }}</span>
      <button class="sc-stop" type="button" title="终止此子代理" @click="stop(r)">✕</button>
    </div>
  </div>
</template>

<style scoped>
/* 单行胶囊（opencode 视觉 DNA）：紧凑卡 padding 8px 12px / radius 6px /
   0.5px 边框 / 13px 文字；spinner + 单行任务名 + 走表用时 + 悬停停止钮。 */
.subagent-capsules {
  display: flex;
  flex-direction: column;
  gap: 4px;
  margin: 4px 0;
}
.subagent-capsule {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 6px 12px;
  border: 0.5px solid var(--border-color);
  border-radius: 6px;
  background: var(--bg-secondary);
  font-size: 13px;
  min-width: 0;
}
.sc-spinner {
  flex: none;
  width: 12px;
  height: 12px;
  border: 2px solid var(--accent, #3b82f6);
  border-top-color: transparent;
  border-radius: 50%;
  animation: sc-spin 0.8s linear infinite;
}
@keyframes sc-spin {
  to {
    transform: rotate(360deg);
  }
}
.sc-task {
  flex: 1 1 auto;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.sc-elapsed {
  flex: none;
  font-size: 11px;
  color: var(--text-muted);
  font-variant-numeric: tabular-nums;
}
.sc-stop {
  flex: none;
  border: none;
  background: none;
  color: var(--text-muted);
  font-size: 11px;
  line-height: 1;
  padding: 3px 5px;
  border-radius: 4px;
  cursor: pointer;
  opacity: 0;
  transition: opacity 150ms ease;
}
.subagent-capsule:hover .sc-stop {
  opacity: 1;
}
.sc-stop:hover {
  color: #e05252;
  background: rgba(224, 82, 82, 0.12);
}
/* 入场动画：仅会话打开期间新出现的行（首拉在场行静止——opencode
   「stays at rest」约束；动画类常驻无副作用，动画只播一次）。 */
.sc-anim {
  animation: sc-fade-in 150ms ease;
}
@keyframes sc-fade-in {
  from {
    opacity: 0;
    transform: translateY(-2px);
  }
  to {
    opacity: 1;
    transform: none;
  }
}
</style>
