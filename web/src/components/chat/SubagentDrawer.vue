<script setup lang="ts">
// 会话内嵌子代理抽屉（子代理可见性 D1c，2026-11-03）。
//
// 挂在 ChatPanel 交付物块之下：拉 subagents.list 后按父会话过滤出
// 当前会话派生的子代理行（detached/bg_spawn/delegate 全形态），零行隐藏。
// 「详情」深链到 /subagents?focus=<id> 独立页（台账+事件环时间线在彼处）。
// delegate 行诚实注记：本节点只记 A 端生命周期，过程环在 B 端节点。
import { ref, computed, watch, onMounted, onBeforeUnmount } from 'vue'
import { useRouter } from 'vue-router'
import { useWSAPI } from '../../composables/useWSAPI'
import { addMessageHandler, removeMessageHandler } from '../../composables/useWebSocket'

const props = defineProps<{ sessionSid: string }>()

const { request } = useWSAPI()
const router = useRouter()

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

const ORIGIN_LABEL: Record<string, string> = {
  detached: '内联',
  bg_spawn: '后台',
  delegate: '委派',
}
const STATUS_BADGE: Record<string, string> = {
  running: 'badge-info',
  done: 'badge-success',
  failed: 'badge-error',
  cancelled: 'badge-neutral',
  interrupted: 'badge-warning',
}
const STATUS_LABEL: Record<string, string> = {
  running: '运行中',
  done: '已完成',
  failed: '失败',
  cancelled: '已终止',
  interrupted: '已中断',
}

const mine = computed(() =>
  rows.value.filter(
    (r) =>
      r.parent_chat_id === props.sessionSid ||
      (props.sessionSid && r.parent_session_key.endsWith(props.sessionSid)),
  ),
)

function statusBadge(s: string): string {
  return STATUS_BADGE[s] ?? 'badge-neutral'
}
function statusLabel(s: string): string {
  return STATUS_LABEL[s] ?? s
}
function originLabel(o: string): string {
  return ORIGIN_LABEL[o] ?? o
}

async function load() {
  try {
    const r = await request('subagents', 'list', {})
    rows.value = (r?.tasks as SubagentRow[]) || []
  } catch { /* WS 未就绪——下轮再试 */ }
}

function openDetail(id: string) {
  router.push({ path: '/subagents', query: { focus: id } })
}

function handleWSMessage(frame: any) {
  // 子代理 push 帧（outer data.subagent_id）到达 = 新活动，即时刷新列表。
  if (frame?.type === 'push' && frame?.cmd === 'tool_event' && frame?.data?.subagent_id) load()
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
  <div v-if="mine.length" class="subagent-drawer">
    <div class="drawer-header" @click="openDetail(mine[0].id)">
      <span class="drawer-title">子代理（{{ mine.length }}）</span>
      <span v-if="mine.some((r) => r.running)" class="badge badge-info">● 运行中</span>
    </div>
    <div v-for="r in mine" :key="r.id" class="drawer-row" @click="openDetail(r.id)">
      <span class="badge" :class="statusBadge(r.status)">{{ statusLabel(r.status) }}</span>
      <span class="badge badge-neutral" :title="r.origin">{{ originLabel(r.origin) }}</span>
      <span class="drawer-task">{{ r.task || '（无任务文本）' }}</span>
      <span v-if="r.summary" class="drawer-summary" :title="r.summary">{{ r.summary }}</span>
      <span v-else-if="r.error" class="drawer-summary drawer-error" :title="r.error">{{ r.error }}</span>
    </div>
    <div class="drawer-foot">详情见「子代理」页（台账 + 执行时间线）</div>
  </div>
</template>

<style scoped>
.subagent-drawer {
  margin: var(--space-2) 0;
  border: 1px solid var(--border-color);
  border-radius: var(--radius-md, 8px);
  background: var(--bg-secondary);
  font-size: var(--text-sm);
}
.drawer-header {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  padding: var(--space-2) var(--space-3);
  cursor: pointer;
  border-bottom: 1px solid var(--border-color);
  font-weight: 600;
}
.drawer-row {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  padding: var(--space-1) var(--space-3);
  cursor: pointer;
  min-width: 0;
}
.drawer-row:hover {
  background: var(--bg-hover, rgba(128, 128, 128, 0.08));
}
.drawer-task {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.drawer-summary {
  margin-left: auto;
  flex: 0 1 40%;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--text-muted);
  font-size: var(--text-xs);
}
.drawer-error {
  color: var(--error, #e74c3c);
}
.drawer-foot {
  padding: 2px var(--space-3) var(--space-2);
  font-size: var(--text-xs);
  color: var(--text-muted);
}
</style>
