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
import { useRouter } from 'vue-router'
import type { WorkTimeline, WorkToolEntry } from '../../stores/chat'
import { toolLabel, spawnTaskFromArgs, jobIdFromPreview } from './workLabels'
import { useWSAPI } from '../../composables/useWSAPI'
import ProcessOutputTail from '../process/ProcessOutputTail.vue'

const props = withDefaults(
  defineProps<{
    work: WorkTimeline
    /** live=true：执行中（typing 区），默认展开 + 每秒走表。 */
    live?: boolean
  }>(),
  { live: false },
)

const emit = defineEmits<{ (e: 'grown'): void }>()

// 折叠态：落地默认收起（用户要看自己去点，2026-10-30 决策）；执行中默认
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
// 思考链轮数（批次二 2026-10-31；头部 💭 N 徽标，有思考才显示）。
const thinkCount = computed(
  () => props.work.rounds.filter(r => r.reasoning).length,
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

// --- spawn 行钻取（2026-11-06 B 档）：历史/落地态里子代理调用的钻取入口 ---
// 显示任务文本（替代原始参数 JSON），点击按任务文本匹配台账行 → 深链
// /executors?tab=subagents&focus=<id>；台账缺席（过旧被逐出/WS 未就绪）回落
// 执行单元页子代理 TAB。
const router = useRouter()
const { request } = useWSAPI()

function spawnTask(t: WorkToolEntry): string {
  return t.tool === 'spawn' ? spawnTaskFromArgs(t.args_preview ?? '') : ''
}

async function openSpawn(t: WorkToolEntry) {
  if (t.tool !== 'spawn') return
  const task = spawnTaskFromArgs(t.args_preview ?? '')
  if (!task) return
  // 截断伪影剥离：后端预览截断以 … 收尾（tool_event_hook truncate_chars），
  // 匹配前剥掉才能作为前缀命中台账行（展示侧保留 … = 诚实截断信号）。
  const needle = task.endsWith('…') ? task.slice(0, -1) : task
  let focus = ''
  try {
    const r = await request('subagents', 'list', {})
    const tasks = (r?.tasks as Array<{ id: string; task: string }>) || []
    const hit =
      tasks.find((x) => x.task === needle) || tasks.find((x) => !!needle && x.task.startsWith(needle))
    if (hit) focus = hit.id
  } catch { /* WS 未就绪——落台账页 */ }
  router.push(
    focus ? { path: '/executors', query: { tab: 'subagents', focus } } : { path: '/executors', query: { tab: 'subagents' } },
  )
}

// --- background_start 行（2026-11-06 后台进程实时输出）：内嵌迷你 live
// tail + 深链 /executors?tab=processes&focus=<id>。job_id 直接从
// result_preview 抽取
//（start 返回值带 job_id），无需查台账——spawn 需要文本匹配而这里
// id 是结构化的，两个钻取路径的差别是刻意的。tail 挂在行外（group
// 包裹），点击 tail 区域不会误触发行级深链。 -1 哨兵 = 无 tail。
function bgJob(t: WorkToolEntry): number {
  return t.tool === 'background_start' ? (jobIdFromPreview(t.result_preview ?? '') ?? -1) : -1
}

function openBg(t: WorkToolEntry) {
  const id = bgJob(t)
  if (id >= 0) router.push({ path: '/executors', query: { tab: 'processes', focus: String(id) } })
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
      <span v-if="thinkCount" class="wt-badge wt-think">💭 {{ thinkCount }}</span>
      <span v-if="runningCount" class="wt-badge wt-running">运行中 {{ runningCount }}</span>
      <span v-if="failedCount" class="wt-badge wt-failed">失败 {{ failedCount }}</span>
      <span v-if="work.truncated" class="wt-badge wt-truncated">已截断</span>
      <span class="wt-caret">{{ open ? '▾' : '▸' }}</span>
    </button>
    <div v-if="open" class="wt-body">
      <template v-for="(r, ri) in work.rounds" :key="ri">
        <div v-if="r.text" class="wt-round-text">{{ r.text }}</div>
        <!-- 思考链折叠卡（批次二）：原生 details 默认收起（用户决策：思考
             是可看不可刷的次级内容，65k 级长文不默认展开）；正文 pre-wrap，
             高度钳制滚动——万级 token 思考不撑爆时间线。 -->
        <details v-if="r.reasoning" class="wt-think">
          <summary>💭 思考过程</summary>
          <div class="wt-think-body">{{ r.reasoning }}</div>
        </details>
        <div
          v-for="t in r.tools"
          :key="t.call_id"
          class="wt-tool-group"
        >
          <div
            class="wt-tool"
            :class="{ 'wt-tool-spawn': t.tool === 'spawn', 'wt-tool-bg': t.tool === 'background_start' }"
            :title="t.args_preview || t.tool"
            :data-task="spawnTask(t)"
            @click="t.tool === 'spawn' ? openSpawn(t) : t.tool === 'background_start' ? openBg(t) : undefined"
          >
            <span class="wt-tool-status" :class="t.running ? 'is-running' : t.ok ? 'is-ok' : 'is-fail'">
              <span v-if="t.running" class="wt-spinner"></span>
              <template v-else>{{ t.ok ? '✓' : '✗' }}</template>
            </span>
            <span class="wt-tool-label">{{ toolLabel(t.tool) }}</span>
            <span v-if="spawnTask(t)" class="wt-tool-args wt-tool-task">{{ spawnTask(t) }}</span>
            <span v-else-if="t.args_preview" class="wt-tool-args">{{ t.args_preview }}</span>
            <span class="wt-tool-dur">{{ formatToolMs(t.duration_ms) }}</span>
          </div>
          <!-- background_start 内嵌迷你 live tail（compact：4KB 窗口 +
               「完整输出」深链 /executors?tab=processes&focus=<id>）；挂在行外——点击
               tail 不触发行级深链。job_id 抽取失败（截断伪影）= 无 tail
               诚实降级，行点击仍可深链台账页。 -->
          <ProcessOutputTail v-if="bgJob(t) >= 0" :job-id="bgJob(t)" compact />
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
/* 思考链（批次二 2026-10-31）：头部计数徽标 + 正文折叠卡。元素限定选择
   器——徽标 span 与 details 共用 wt-think 类名但形态不同。紫色系 = 「思
   考/推理」次级语义，与工具（accent）/失败（红）区分。 */
span.wt-think {
  color: #9b7fd4;
  background: rgba(155, 127, 212, 0.12);
}
details.wt-think {
  border: 1px solid var(--border);
  border-radius: 6px;
  padding: 2px 8px;
  font-size: var(--text-xs, 12px);
  color: var(--text-muted);
}
details.wt-think > summary {
  cursor: pointer;
  user-select: none;
  line-height: 1.6;
}
details.wt-think > summary:hover {
  color: var(--text);
}
/* 折叠正文：pre-wrap 原样保留模型思考的换行/缩进；高度钳制 + 滚动——
   65k 级思考展开后不撑爆时间线，也不截断内容（滚动能看全）。 */
.wt-think-body {
  white-space: pre-wrap;
  word-break: break-word;
  max-height: 240px;
  overflow-y: auto;
  line-height: 1.5;
  margin-top: 2px;
  padding: 4px 0 2px;
  border-top: 1px dashed var(--border);
  opacity: 0.9;
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
/* spawn 行钻取（2026-11-06 B 档）：可点 + 悬停标签提亮（点击按任务文本
   匹配台账行深链 /executors?tab=subagents&focus=<id>）。 */
.wt-tool-spawn {
  cursor: pointer;
}
.wt-tool-spawn:hover .wt-tool-label {
  color: var(--accent, #3b82f6);
}
.wt-tool-task {
  font-family: inherit;
  opacity: 0.85;
}
/* background_start 行（2026-11-06 后台进程实时输出）：可点深链同 spawn；
   group 包裹行 + 内嵌迷你 tail（tail 自带 margin-top）。 */
.wt-tool-group {
  display: flex;
  flex-direction: column;
}
.wt-tool-bg {
  cursor: pointer;
}
.wt-tool-bg:hover .wt-tool-label {
  color: var(--accent, #3b82f6);
}
</style>
