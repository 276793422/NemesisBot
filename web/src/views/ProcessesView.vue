<script setup lang="ts">
// 后台任务页（2026-11-06 P1 后台进程实时输出 UI）。
//
// Agent 经 background_start 起的本机后台子进程台账 + 输出观察面。数据面
// 单一真相源 = 后端 background WSAPI（B4 注册表的投影，与 agent 工具通道
// 同一 Arc——这里看到的就是 agent 起的那些进程）：
// - 列表：background.list；有运行行 2s 轮询、纯终态 10s 慢拍（注册表
//   满员会淘汰最旧已完成任务，终态行会消失——这是容量语义不是丢数据）。
// - 详情：行字段 + ProcessOutputTail（full 态，组件内自轮增量 output；
//   steerable 运行中带 stdin 注入框）。本页详情打开是同步动作（从已加载
//   行取数，无 in-flight 首载请求），不需要 SubagentsView 那套代次围栏；
//   行级刷新由列表轮询统一推进（行被逐出 → 详情诚实关闭）。
// - 停止：background.kill（幂等树杀）；启动归 agent 工具通道，本页无 start。
import { ref, computed, onMounted, onBeforeUnmount } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'
import ProcessOutputTail from '../components/process/ProcessOutputTail.vue'

const { request } = useWSAPI()
const toast = useToast()
const route = useRoute()
const router = useRouter()

interface JobRow {
  id: number
  command: string
  cwd: string
  started_at_unix: number
  running: boolean
  steerable: boolean
  total_bytes: number
  dropped_bytes: number
  exit_code?: number
  success?: boolean
  killed?: boolean
  /** 深链落空时合成的占位行：不属注册表，列表同步永不触碰（防误关）。 */
  stale?: boolean
}

// --- 列表 ---
const jobs = ref<JobRow[]>([])
const loading = ref(true)
let listTimer: ReturnType<typeof setInterval> | null = null
// 卸载护栏：onMounted 的 async 续段（loadList → armListTimer → openFromQuery）
// 可能在组件卸载后才继续跑——route 是全局 reactive，死实例的续段会照常读到
// focus 并消费/剥离（模态开在已分离的 DOM 上永不显示，活实例却收不到深链，
// 即 ChatPanel 迷你 tail 深链静默丢失）；armListTimer 装的定时器也无人清理
// （onBeforeUnmount 已跑过，泄漏成永久轮询）。跨 await 的续段落点一律过闸。
let alive = true

async function loadList() {
  try {
    const r = await request('background', 'list')
    // id 倒序（最新在上）——后端已排，这里兜底再排一次防契约漂移。
    const list = ((r?.jobs as JobRow[]) || []).slice().sort((a, b) => b.id - a.id)
    jobs.value = list
    syncDetailWithList()
  } catch { /* WS 未就绪——下轮再试 */ }
  loading.value = false
}

const hasRunning = computed(() => jobs.value.some((j) => j.running))

function armListTimer() {
  if (listTimer) clearInterval(listTimer)
  if (!alive) return // 卸载后不得再装无人清理的定时器
  listTimer = setInterval(loadList, hasRunning.value ? 2000 : 10000)
}

// --- 详情 ---
const detail = ref<JobRow | null>(null)

function openDetail(id: number | string) {
  const nid = Number(id)
  const row = jobs.value.find((j) => j.id === nid)
  if (!row) {
    // 不在注册表（已被逐出/网关重启清空/另一实例）：仍开模态，让
    // ProcessOutputTail 自轮 background.output 收到 no such job_id →
    // 「输出已过期」诚实降级（与聊天侧历史 tail 同语义）。
    detail.value = {
      id: nid,
      command: '—',
      cwd: '—',
      started_at_unix: 0,
      running: false,
      steerable: false,
      total_bytes: 0,
      dropped_bytes: 0,
      stale: true,
    }
    return
  }
  detail.value = row
}

function closeDetail() {
  detail.value = null
}

/** 列表轮询推进详情行字段；行被逐出（容量淘汰）→ 详情诚实关闭。
 * stale 占位行（深链落空）不属注册表，永不触碰。 */
function syncDetailWithList() {
  if (!detail.value || detail.value.stale) return
  const row = jobs.value.find((j) => j.id === detail.value!.id)
  if (row) {
    detail.value = row
  } else {
    detail.value = null
    toast.warn('该后台任务已被注册表逐出（满员淘汰最旧已完成任务）')
  }
}

// --- 停止 ---
async function killJob(j: JobRow) {
  const label = excerpt(j.command, 48)
  if (!confirm(`停止后台进程 #${j.id}（${label}）？进程树将被终止。`)) return
  try {
    await request('background', 'kill', { id: j.id })
    toast.success('已发送停止信号')
    loadList()
    if (detail.value?.id === j.id) {
      // tail 组件自轮会看到终态平铺；这里立刻刷一次列表推进详情行。
      setTimeout(loadList, 300)
    }
  } catch (e: any) {
    toast.error('停止失败: ' + (e?.message ?? e))
  }
}

// --- 展示辅助 ---
function excerpt(s: string | null | undefined, n: number): string {
  const t = (s ?? '').replace(/\s+/g, ' ').trim()
  return t.length > n ? t.slice(0, n) + '…' : t
}

/** 三态徽标：●运行中 / ✓ 退出 N / ⛔ 已停止（killed）。 */
function statusInfo(j: JobRow): { cls: string; label: string } {
  if (j.stale) return { cls: 'badge-neutral', label: '已过期' }
  if (j.running) return { cls: 'badge-info', label: '● 运行中' }
  if (j.killed) return { cls: 'badge-neutral', label: '⛔ 已停止' }
  const code = j.exit_code
  if (code === undefined) return { cls: 'badge-neutral', label: '已退出' }
  return code === 0
    ? { cls: 'badge-success', label: '✓ 退出 0' }
    : { cls: 'badge-error', label: `✗ 退出 ${code}` }
}

function fmtBytes(n: number): string {
  if (n < 1024) return `${n}B`
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)}KB`
  return `${(n / 1024 / 1024).toFixed(2)}MB`
}

function fmtTime(unix?: number): string {
  if (!unix) return '—'
  return new Date(unix * 1000).toLocaleTimeString()
}

// ChatPanel 迷你 tail 深链：/executors?tab=processes&focus=<id> 自动开详情。
async function openFromQuery() {
  const focus = route.query.focus
  if (typeof focus === 'string' && focus) {
    if (!alive) return // 卸载后的续段不得消费/剥离 focus（死实例偷深链）
    if (!jobs.value.length) await loadList()
    if (!alive) return // loadList 在途期间可能已被卸载
    openDetail(focus)
    // 用完即清（刷新页面不再自动弹）
    router.replace({ query: { ...route.query, focus: undefined } })
  }
}

onMounted(async () => {
  await loadList()
  armListTimer()
  await openFromQuery()
})

onBeforeUnmount(() => {
  alive = false
  if (listTimer) clearInterval(listTimer)
})
</script>

<template>
  <div class="page-processes">
    <div class="page-header">
      <h2>后台任务</h2>
      <p class="page-desc">
        Agent 用 background_start 启动的本机后台子进程：实时输出、stdin 注入、进程树停止。
        输出缓冲有上限（超出丢弃头部）；注册表满员时自动淘汰最旧的已完成任务。
      </p>
    </div>
    <div class="page-body">
      <div class="proc-filters">
        <span v-if="hasRunning" class="badge badge-info">● {{ jobs.filter((j) => j.running).length }} 个运行中</span>
        <div style="flex: 1;"></div>
        <button class="btn btn-sm" @click="loadList">刷新</button>
      </div>

      <div v-if="loading" style="text-align: center; padding: var(--space-8);">
        <div class="spinner spinner-lg" style="margin: 0 auto;"></div>
      </div>

      <div v-else-if="jobs.length === 0" class="empty-state">
        <h3>暂无后台任务</h3>
        <p>Agent 启动后台进程（background_start）后会出现在这里；正在运行的进程可实时查看输出</p>
      </div>

      <div v-else class="table-wrap">
        <table>
          <thead>
            <tr>
              <th>ID</th><th>命令</th><th>状态</th><th>输出量</th><th>开始</th><th>操作</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="j in jobs" :key="j.id" class="proc-row" @click="openDetail(j.id)">
              <td style="font-family: var(--font-mono); color: var(--text-secondary);">#{{ j.id }}</td>
              <td style="max-width: 360px;">
                <div style="font-family: var(--font-mono); font-size: var(--text-xs); word-break: break-all;" :title="j.command">{{ excerpt(j.command, 90) }}</div>
                <div v-if="j.steerable && j.running" style="font-size: var(--text-xs); color: var(--info, #4a90e2);">⌨ 可注入 stdin</div>
              </td>
              <td><span class="badge" :class="statusInfo(j).cls">{{ statusInfo(j).label }}</span></td>
              <td style="font-size: var(--text-sm);">
                {{ fmtBytes(j.total_bytes) }}
                <span v-if="j.dropped_bytes > 0" class="proc-dropped" title="超出输出缓冲上限，头部已丢弃">已丢 {{ fmtBytes(j.dropped_bytes) }}</span>
              </td>
              <td style="font-size: var(--text-sm); color: var(--text-muted);">{{ fmtTime(j.started_at_unix) }}</td>
              <td @click.stop>
                <div style="display: flex; gap: var(--space-1);">
                  <button class="btn btn-sm" @click="openDetail(j.id)">详情</button>
                  <button v-if="j.running" class="btn btn-sm btn-danger" @click="killJob(j)">停止</button>
                </div>
              </td>
            </tr>
          </tbody>
        </table>
      </div>
    </div>

    <!-- 详情 modal：行字段 + 实时输出 tail（full 态：32KB 窗口 + 停止 + 注入） -->
    <div v-if="detail" class="modal-backdrop" @click.self="closeDetail()">
      <div class="modal modal-wide">
        <div class="modal-header">
          <h3>后台任务 #{{ detail.id }} · {{ statusInfo(detail).label }}</h3>
          <button class="modal-close" @click="closeDetail()">✕</button>
        </div>
        <div class="modal-body">
          <div class="detail-grid">
            <div class="kv-row"><span class="kv-key">命令</span><span class="kv-value" style="font-family: var(--font-mono); font-size: var(--text-xs);">{{ detail.command }}</span></div>
            <div class="kv-row"><span class="kv-key">工作目录</span><span class="kv-value" style="font-family: var(--font-mono); font-size: var(--text-xs);">{{ detail.cwd }}</span></div>
            <div class="kv-row"><span class="kv-key">状态</span><span class="kv-value"><span class="badge" :class="statusInfo(detail).cls">{{ statusInfo(detail).label }}</span></span></div>
            <div class="kv-row"><span class="kv-key">开始时间</span><span class="kv-value">{{ fmtTime(detail.started_at_unix) }}</span></div>
            <div class="kv-row" v-if="!detail.stale"><span class="kv-key">stdin 注入</span><span class="kv-value">{{ detail.steerable ? '支持（运行中可在下方注入）' : '不支持（启动时未开 steerable，stdin 已关闭）' }}</span></div>
            <div class="kv-row" v-if="detail.dropped_bytes > 0"><span class="kv-key">输出丢弃</span><span class="kv-value" style="color: #c98a2b;">头部 {{ fmtBytes(detail.dropped_bytes) }} 已被缓冲上限淘汰</span></div>
          </div>

          <div style="margin-top: var(--space-3);">
            <ProcessOutputTail :job-id="detail.id" />
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.page-desc {
  color: var(--text-muted);
  font-size: var(--text-sm);
  margin-top: var(--space-1);
}
.proc-filters {
  display: flex;
  align-items: center;
  gap: var(--space-3);
  margin-bottom: var(--space-4);
}
.proc-row {
  cursor: pointer;
}
.proc-dropped {
  display: inline-block;
  margin-left: var(--space-2);
  font-size: var(--text-xs);
  color: #c98a2b;
}
.detail-grid {
  display: flex;
  flex-direction: column;
  gap: var(--space-1);
}
.kv-row {
  display: flex;
  gap: var(--space-3);
  font-size: var(--text-sm);
  line-height: 1.6;
}
.kv-key {
  flex: 0 0 92px;
  color: var(--text-muted);
}
.kv-value {
  flex: 1;
  min-width: 0;
  word-break: break-all;
  white-space: pre-wrap;
}
.modal-wide {
  max-width: 760px;
  width: calc(100vw - 48px);
}
</style>
