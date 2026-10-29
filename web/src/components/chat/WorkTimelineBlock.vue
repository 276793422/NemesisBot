<script setup lang="ts">
/**
 * WorkTimelineBlock（2026-10-30 ChatPanel 长任务时间线改造）。
 *
 * 「中间轮叙述+工具调用」交错时间线的统一渲染块，两个位置两态共用：
 * - 落地态（live=false）：assistant 消息上方，默认折叠成一条头部
 *   （用时 + 失败/截断徽标），点开回看——取代旧 toolEvents 卡片组 +
 *   roundTexts 折叠条两块并行。
 * - live 态（live=true）：typing 占位区内，默认展开，新内容到达时
 *   emit('grown') 供宿主贴底滚动。
 *
 * 数据形状 = store WorkTimeline（后端 work_timeline.rs serde 镜像，
 * snake_case 原样）；本组件纯渲染不改数据。
 */
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import type { WorkTimeline } from '../../stores/chat'
import { toolLabel } from './workLabels'

const props = withDefaults(
  defineProps<{
    work: WorkTimeline
    /** live=true：执行中（typing 区），默认展开 + 每秒走表。 */
    live?: boolean
  }>(),
  { live: false },
)

const emit = defineEmits<{ (e: 'grown'): void }>()

// 折叠态：落地默认收起（用户要看自己去点，2026-10-30 拍板）；执行中默认
// 展开实时滚动。两态是两个组件实例，各自的初值即终值（live 块随回复落地
// 整体卸载，由落地态实例接棒）。
const open = ref(props.live)

// 失败徽标：终态失败的工具数（running 占位 ok=false 不计）。
const failedCount = computed(
  () => props.work.rounds.flatMap(r => r.tools).filter(t => !t.running && !t.ok).length,
)
// 进行中工具数（live 头部「运行中 N」）。
const runningCount = computed(
  () => props.work.rounds.flatMap(r => r.tools).filter(t => t.running).length,
)

// --- 用时（live 每秒走表；落地取 ended_at_ms 定格）---
const nowTick = ref(Date.now())
let tickTimer: number | null = null
onMounted(() => {
  if (props.live) tickTimer = window.setInterval(() => (nowTick.value = Date.now()), 1000)
})
onBeforeUnmount(() => {
  if (tickTimer !== null) window.clearInterval(tickTimer)
})
const durationMs = computed(() => {
  const end = props.live ? Math.max(props.work.ended_at_ms, nowTick.value) : props.work.ended_at_ms
  return Math.max(0, end - props.work.started_at_ms)
})

function formatDuration(ms: number): string {
  const s = Math.floor(ms / 1000)
  if (s < 60) return `${s}秒`
  const m = Math.floor(s / 60)
  if (m < 60) return `${m}分${s % 60}秒`
  return `${Math.floor(m / 60)}时${m % 60}分`
}

function formatToolMs(ms: number): string {
  if (ms < 1000) return `${ms}ms`
  const s = ms / 1000
  return `${s % 1 === 0 ? s : s.toFixed(1)}s`
}

// 新内容到达（轮数或工具数增长）→ 通知宿主贴底（live 态才有意义）。
watch(
  () => props.work.rounds.length + props.work.rounds.reduce((n, r) => n + r.tools.length, 0),
  () => {
    if (props.live) emit('grown')
  },
)
</script>

<template>
  <div class="wt-block" :class="{ live }">
    <button class="wt-header" type="button" @click="open = !open">
      <span class="wt-icon">⚒</span>
      <span class="wt-title">用时 {{ formatDuration(durationMs) }}</span>
      <span v-if="live" class="wt-badge wt-live"><span class="wt-dot"></span>进行中</span>
      <span v-if="runningCount" class="wt-badge wt-running">运行中 {{ runningCount }}</span>
      <span v-if="failedCount" class="wt-badge wt-failed">失败 {{ failedCount }}</span>
      <span v-if="work.truncated" class="wt-badge wt-truncated">已截断</span>
      <span class="wt-caret">{{ open ? '▾' : '▸' }}</span>
    </button>
    <div v-if="open" class="wt-body">
      <template v-for="(r, ri) in work.rounds" :key="ri">
        <div v-if="r.text" class="wt-round-text">{{ r.text }}</div>
        <div v-for="t in r.tools" :key="t.call_id" class="wt-tool" :title="t.args_preview || t.tool">
          <span class="wt-tool-status" :class="t.running ? 'is-running' : t.ok ? 'is-ok' : 'is-fail'">
            <span v-if="t.running" class="wt-spinner"></span>
            <template v-else>{{ t.ok ? '✓' : '✗' }}</template>
          </span>
          <span class="wt-tool-label">{{ toolLabel(t.tool) }}</span>
          <span v-if="t.args_preview" class="wt-tool-args">{{ t.args_preview }}</span>
          <span class="wt-tool-dur">{{ formatToolMs(t.duration_ms) }}</span>
        </div>
      </template>
    </div>
  </div>
</template>

<style scoped>
/* 复用 ChatPanel 工具卡/折叠条同款视觉语言（胶囊头部 + 浅字叙述体）。 */
.wt-block {
  margin-bottom: 6px;
}
.wt-header {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 3px 10px;
  border: 1px solid var(--border);
  border-radius: 999px;
  background: var(--bg-elev, rgba(128, 128, 128, 0.06));
  color: var(--text-muted);
  font-size: var(--text-xs, 12px);
  cursor: pointer;
}
.wt-header:hover {
  color: var(--text);
  border-color: var(--accent);
}
.wt-icon {
  font-size: 11px;
}
.wt-caret {
  font-size: 10px;
}
.wt-badge {
  padding: 0 6px;
  border-radius: 999px;
  font-size: 10px;
  line-height: 16px;
}
.wt-live {
  color: var(--accent);
}
.wt-running {
  color: var(--accent);
  background: rgba(59, 130, 246, 0.12);
}
.wt-failed {
  color: #e05252;
  background: rgba(224, 82, 82, 0.12);
}
.wt-truncated {
  color: #c98a2b;
  background: rgba(201, 138, 43, 0.12);
}
.wt-dot {
  display: inline-block;
  width: 6px;
  height: 6px;
  margin-right: 4px;
  border-radius: 50%;
  background: var(--accent);
  animation: wt-pulse 1.2s ease-in-out infinite;
}
@keyframes wt-pulse {
  0%,
  100% {
    opacity: 1;
  }
  50% {
    opacity: 0.3;
  }
}
.wt-body {
  margin-top: 6px;
  padding: 6px 10px;
  border-left: 2px solid var(--border);
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.wt-round-text {
  color: var(--text-muted);
  font-size: var(--text-sm, 13px);
  line-height: 1.5;
  white-space: pre-wrap;
  word-break: break-word;
  padding: 2px 0;
}
.wt-round-text + .wt-tool,
.wt-tool + .wt-round-text {
  margin-top: 2px;
}
.wt-tool {
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
  font-size: var(--text-xs, 12px);
  color: var(--text-muted);
  padding: 1px 0;
}
.wt-tool-status {
  flex: none;
  width: 14px;
  text-align: center;
}
.wt-tool-status.is-ok {
  color: #4caf7d;
}
.wt-tool-status.is-fail {
  color: #e05252;
}
.wt-spinner {
  display: inline-block;
  width: 9px;
  height: 9px;
  border: 2px solid var(--accent);
  border-top-color: transparent;
  border-radius: 50%;
  animation: wt-spin 0.8s linear infinite;
  vertical-align: -1px;
}
@keyframes wt-spin {
  to {
    transform: rotate(360deg);
  }
}
.wt-tool-label {
  flex: none;
  color: var(--text);
}
.wt-tool-args {
  flex: 1 1 auto;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-family: var(--font-mono, monospace);
  font-size: 11px;
  opacity: 0.75;
}
.wt-tool-dur {
  flex: none;
  font-size: 10px;
  opacity: 0.7;
}
</style>
