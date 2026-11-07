<script setup lang="ts">
/**
 * ProcessOutputTail（2026-11-06 P1 后台进程实时输出 UI）。
 *
 * 后台进程 stdout/stderr 合流 tail 的统一渲染块，compact/full 两态共用：
 * - compact=true：聊天 WorkTimeline 内嵌迷你 tail（~160px 钳高、4KB 窗口、
 *   「完整输出」深链 /executors?tab=processes&focus=<id>；无 steer/停止——
 *   操作面归执行单元页后台任务 TAB）。
 * - full：执行单元页「后台任务」TAB 详情模态 tail（~360px、32KB 窗口 +
 *   停止按钮 + steer 注入输入（steerable 且运行中））。
 *
 * 数据面 = background.output 增量轮询（v1 poll 制，PLAN §0）：
 * 两步初始化——offset:0 拿元数据（total/dropped/running）→ initialTailOffset
 * 选尾块起点 → 之后 next_offset 单调追帧；自调度循环（await 完再排下一发，
 * 不用 setInterval——慢响应不堆积）。running=false 且追平即停轮。
 * 上滚暂停跟随、滚回底部恢复；丢弃线前移显示「已丢弃 N 字节」徽标；
 * 任务被逐出（no such job_id）显示「输出已过期」诚实停轮。
 */
import { computed, nextTick, onBeforeUnmount, ref, watch } from 'vue'
import { useRouter } from 'vue-router'
import { useWSAPI } from '../../composables/useWSAPI'
import { useToast } from '../../composables/useToast'
import { initialTailOffset, stripAnsi } from './tailUtil'

const props = withDefaults(
  defineProps<{
    jobId: number
    /** compact=聊天内嵌迷你态（无操作面 + 完整输出深链）。 */
    compact?: boolean
  }>(),
  { compact: false },
)

const { request } = useWSAPI()
const toast = useToast()
const router = useRouter()

const TAIL_WINDOW = props.compact ? 4 * 1024 : 32 * 1024
const POLL_MS = 500

// --- tail 状态 ---
const text = ref('')
const meta = ref<{
  command: string
  running: boolean
  steerable: boolean
  total_bytes: number
  dropped_bytes: number
  started_at_unix: number
  exit_code?: number
  killed?: boolean
} | null>(null)
/** idle=未起轮 / live=追帧中 / done=终态停轮 / expired=任务已逐出 / error=装配无注册表等 */
const phase = ref<'idle' | 'live' | 'done' | 'expired' | 'error'>('idle')
const errMsg = ref('')
const droppedSeen = ref(0)

let stopped = false
let generation = 0

// --- 时长（运行中每秒走表；终态冻结）---
const nowTick = ref(Date.now())
let tickTimer: number | null = null

function ensureTick(run: boolean) {
  if (run && tickTimer === null) {
    tickTimer = window.setInterval(() => (nowTick.value = Date.now()), 1000)
  } else if (!run && tickTimer !== null) {
    window.clearInterval(tickTimer)
    tickTimer = null
  }
}

const durationMs = computed(() => {
  if (!meta.value) return 0
  // 终态后 ensureTick(false) 已停走表，nowTick 冻结在最后一次 tick——
  // 时长自然定格，不再继续增长。
  return Math.max(0, nowTick.value - meta.value.started_at_unix * 1000)
})

function formatDuration(ms: number): string {
  const s = Math.floor(ms / 1000)
  if (s < 60) return `${s}秒`
  const m = Math.floor(s / 60)
  if (m < 60) return `${m}分${s % 60}秒`
  return `${Math.floor(m / 60)}时${m % 60}分`
}

function formatBytes(n: number): string {
  if (n < 1024) return `${n}B`
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)}KB`
  return `${(n / 1024 / 1024).toFixed(2)}MB`
}

// --- 跟随 / 上滚暂停 ---
const tailEl = ref<HTMLElement | null>(null)
const followPaused = ref(false)

function onScroll() {
  const el = tailEl.value
  if (!el) return
  const atBottom = el.scrollTop + el.clientHeight >= el.scrollHeight - 24
  followPaused.value = !atBottom
}

function scrollToBottom() {
  const el = tailEl.value
  if (el) el.scrollTop = el.scrollHeight
}

function resumeFollow() {
  followPaused.value = false
  scrollToBottom()
}

// --- 取数循环（自调度：await 完成再排下一发，慢响应不堆积）---
async function runLoop() {
  const gen = ++generation
  stopped = false
  phase.value = 'idle'
  text.value = ''
  meta.value = null
  droppedSeen.value = 0
  errMsg.value = ''
  followPaused.value = false

  // 第一步：offset 0 只为拿元数据（total/dropped/running；chunk 会被第二步
  // 的尾块取代，不渲染）。
  let offset = 0
  let started = false
  while (!stopped && gen === generation) {
    let r: any = null
    try {
      r = await request('background', 'output', { id: props.jobId, offset }, 10000)
    } catch (e: any) {
      if (stopped || gen !== generation) return
      const msg = String(e?.message ?? e)
      if (msg.includes('no such job_id')) {
        phase.value = 'expired'
        ensureTick(false)
        return
      }
      if (msg.includes('registry not injected')) {
        phase.value = 'error'
        errMsg.value = '当前装配没有后台进程注册表（headless/精简版无此子系统）'
        return
      }
      // 断线窗口：500ms 后重试（保活 loop，不自杀）。
      await sleep(POLL_MS)
      continue
    }
    if (stopped || gen !== generation) return
    if (!started) {
      // 尾块起点：丢弃线优先，否则总量-窗口。
      started = true
      meta.value = {
        command: r.command ?? '',
        running: !!r.running,
        steerable: !!r.steerable,
        total_bytes: r.total_bytes ?? 0,
        dropped_bytes: r.dropped_bytes ?? 0,
        started_at_unix: r.started_at_unix ?? 0,
        exit_code: r.exit_code,
        killed: r.killed,
      }
      droppedSeen.value = r.dropped_bytes ?? 0
      offset = initialTailOffset(r.total_bytes ?? 0, r.dropped_bytes ?? 0, TAIL_WINDOW)
      ensureTick(!!r.running)
      phase.value = 'live'
    }
    const chunk: string = r.chunk ?? ''
    if (chunk) {
      // 贴底去竞态（2026-11-07 根修）：text 的 DOM flush 是异步的，append 后
      // 同步贴底读到的是旧 scrollHeight——scrollTop 被钳到「旧高-视口高」的
      // 中部位置（差一个 chunk 的高度），延迟派发的 scroll 事件读到非贴底 →
      // followPaused 误翻 true：组件在无用户滚动时自己暂停跟随，视图悬停
      // 在任意中间位置（快载荷 ~264px/tick 下实测实录，e2e 边角 spec 抓出）。
      // 先快照「是否贴底」（tailEl 未挂载视作贴底，保首轮即贴底），等
      // nextTick flush 后再贴底。
      const el = tailEl.value
      const wasAtBottom = !el || el.scrollTop + el.clientHeight >= el.scrollHeight - 24
      text.value += stripAnsi(chunk)
      if (wasAtBottom && !followPaused.value) {
        await nextTick()
        if (stopped || gen !== generation) return
        if (!followPaused.value) scrollToBottom()
      }
    }
    if ((r.dropped_bytes ?? 0) > droppedSeen.value) {
      droppedSeen.value = r.dropped_bytes ?? 0
    }
    offset = r.next_offset ?? offset + chunk.length
    const done = !r.running && (r.next_offset ?? 0) >= (r.total_bytes ?? 0)
    if (done) {
      // 终态平铺字段收进 meta（exit_code/killed 展示）。
      meta.value = {
        ...meta.value!,
        running: false,
        exit_code: r.exit_code,
        killed: r.killed,
      }
      phase.value = 'done'
      ensureTick(false)
      // 终态定格后再贴底一次（收尾块通常还在涌出）；同上等 flush 再读高度。
      if (!followPaused.value) {
        await nextTick()
        if (!followPaused.value) scrollToBottom()
      }
      return
    }
    if (!followPaused.value) scrollToBottom()
    await sleep(POLL_MS)
  }
}

function sleep(ms: number): Promise<void> {
  return new Promise((res) => setTimeout(res, ms))
}

watch(
  () => props.jobId,
  () => {
    generation++ // 作废旧循环（其 gen 检查会退出）
    runLoop()
  },
  { immediate: true },
)

onBeforeUnmount(() => {
  stopped = true
  generation++
  ensureTick(false)
})

// --- 操作面（full 态）---
async function stopJob() {
  if (!meta.value) return
  if (!confirm(`停止后台进程 #${props.jobId}（${excerptCmd(meta.value.command)}）？进程树将被终止。`))
    return
  try {
    await request('background', 'kill', { id: props.jobId })
    toast.success('已发送停止信号')
  } catch (e: any) {
    toast.error('停止失败: ' + (e?.message ?? e))
  }
}

const steerText = ref('')
const steering = ref(false)

async function sendSteer() {
  const t = steerText.value.trim()
  if (!t || steering.value) return
  steering.value = true
  try {
    await request('background', 'steer', { id: props.jobId, text: t })
    steerText.value = ''
    toast.success('已注入 stdin')
  } catch (e: any) {
    toast.error('注入失败: ' + (e?.message ?? e))
  } finally {
    steering.value = false
  }
}

// --- 深链（compact 态）---
function openFull() {
  router.push({ path: '/executors', query: { tab: 'processes', focus: String(props.jobId) } })
}

function excerptCmd(cmd: string, n = 40): string {
  const t = (cmd ?? '').replace(/\s+/g, ' ').trim()
  return t.length > n ? t.slice(0, n) + '…' : t
}
</script>

<template>
  <div class="pot" :class="{ compact }">
    <!-- 状态行：id · 命令 · 时长 · 字节（compact 同款单行） -->
    <div class="pot-status">
      <span class="pot-dot" :class="phase === 'live' && meta?.running ? 'is-run' : meta?.running ? 'is-run' : 'is-end'"></span>
      <span class="pot-id">#{{ jobId }}</span>
      <span class="pot-cmd" :title="meta?.command">{{ meta ? excerptCmd(meta.command) : '…' }}</span>
      <span class="pot-dur">{{ formatDuration(durationMs) }}</span>
      <span v-if="meta" class="pot-bytes">{{ formatBytes(meta.total_bytes) }}</span>
      <span v-if="phase === 'done' && meta?.exit_code !== undefined" class="pot-exit" :class="meta.exit_code === 0 ? 'is-ok' : 'is-fail'">
        {{ meta.killed ? '⛔ 已停止' : meta.exit_code === 0 ? '✓ 退出 0' : `✗ 退出 ${meta.exit_code}` }}
      </span>
      <span style="flex: 1"></span>
      <button v-if="!compact" class="btn btn-sm pot-stop" :disabled="!meta?.running" @click="stopJob">停止进程</button>
      <button v-if="compact" class="pot-link" @click="openFull">完整输出 →</button>
    </div>

    <!-- 丢弃徽标：环形缓冲头部被淘汰（超出 256KB 上限） -->
    <div v-if="droppedSeen > 0" class="pot-dropped">
      已丢弃开头 {{ formatBytes(droppedSeen) }}（超出输出缓冲上限，早期内容不可回看）
    </div>

    <!-- 主体 -->
    <div v-if="phase === 'idle'" class="pot-empty">连接中…</div>
    <div v-else-if="phase === 'expired'" class="pot-empty">输出已过期（任务已被逐出注册表）</div>
    <div v-else-if="phase === 'error'" class="pot-empty">{{ errMsg }}</div>
    <div v-else class="pot-tail-wrap">
      <div ref="tailEl" class="pot-tail" :class="{ 'is-paused': followPaused }" @scroll="onScroll"><pre>{{ text || '（暂无输出，等待进程产出…）' }}</pre></div>
      <button v-if="followPaused" class="pot-follow" @click="resumeFollow">↓ 跟随最新</button>
    </div>

    <!-- steer 注入（full 态 + steerable + 运行中；非 steerable 进程 stdin
         是 null 管道，注入框不渲染——不给必败操作摆入口） -->
    <div v-if="!compact && meta?.running && meta?.steerable" class="pot-steer">
      <input
        v-model="steerText"
        class="form-input"
        placeholder="注入 stdin（回车发送，内容作为一行写入进程标准输入）"
        @keydown.enter="sendSteer"
      />
      <button class="btn btn-sm" :disabled="!steerText.trim() || steering" @click="sendSteer">注入</button>
    </div>
  </div>
</template>

<style scoped>
.pot {
  border: 1px solid var(--border-color);
  border-radius: 6px;
  overflow: hidden;
  background: var(--bg-primary);
  font-size: var(--text-xs, 12px);
}
.pot.compact {
  margin-top: var(--space-2);
}
.pot-status {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  padding: var(--space-1) var(--space-2);
  background: var(--bg-secondary);
  border-bottom: 1px solid var(--border-color);
  min-width: 0;
}
.pot-dot {
  flex: none;
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: var(--text-muted);
}
.pot-dot.is-run {
  background: var(--accent, #3b82f6);
  animation: pot-pulse 1.2s ease-in-out infinite;
}
.pot-dot.is-end {
  background: var(--success, #2ecc71);
}
@keyframes pot-pulse {
  0%,
  100% {
    opacity: 1;
  }
  50% {
    opacity: 0.3;
  }
}
.pot-id {
  flex: none;
  font-family: var(--font-mono);
  color: var(--text-secondary);
}
.pot-cmd {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--text);
}
.pot-dur,
.pot-bytes {
  flex: none;
  color: var(--text-muted);
  font-size: 11px;
}
.pot-exit {
  flex: none;
  font-size: 11px;
}
.pot-exit.is-ok {
  color: var(--success, #2ecc71);
}
.pot-exit.is-fail {
  color: var(--error, #e74c3c);
}
.pot-stop {
  flex: none;
}
.pot-link {
  flex: none;
  border: none;
  background: none;
  color: var(--accent, #3b82f6);
  cursor: pointer;
  font-size: var(--text-xs, 12px);
  padding: 0;
}
.pot-link:hover {
  text-decoration: underline;
}
.pot-dropped {
  padding: var(--space-1) var(--space-2);
  background: rgba(201, 138, 43, 0.12);
  color: #c98a2b;
  font-size: 11px;
  border-bottom: 1px dashed var(--border-color);
}
.pot-empty {
  padding: var(--space-3) var(--space-2);
  color: var(--text-muted);
  text-align: center;
}
.pot-tail-wrap {
  position: relative;
}
.pot-tail {
  height: 360px;
  overflow-y: auto;
  padding: var(--space-2);
}
.pot.compact .pot-tail {
  height: 160px;
}
.pot-tail.is-paused {
  opacity: 0.92;
}
.pot-tail pre {
  margin: 0;
  font-family: var(--font-mono);
  font-size: 11px;
  line-height: 1.5;
  white-space: pre-wrap;
  word-break: break-all;
  color: var(--text);
}
.pot-follow {
  position: absolute;
  right: var(--space-3);
  bottom: var(--space-3);
  border: 1px solid var(--accent, #3b82f6);
  border-radius: 999px;
  background: var(--bg-secondary);
  color: var(--accent, #3b82f6);
  font-size: 11px;
  padding: 2px 10px;
  cursor: pointer;
}
.pot-steer {
  display: flex;
  gap: var(--space-2);
  padding: var(--space-2);
  border-top: 1px solid var(--border-color);
}
.pot-steer .form-input {
  flex: 1;
  font-size: var(--text-xs, 12px);
}
</style>
