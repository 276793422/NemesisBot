<script setup lang="ts">
import { ref, watch } from 'vue'
import { useWSAPI } from '../../composables/useWSAPI'

// L14（2026-11-01）：会话执行统计面板——8 字段只读视图（工具调用/失败/
// 重试/校验失败/压缩/审批总数·批准·拒绝）。数据源 sessions.stats（后端
// flush 后读盘，重启不归零）；聚合器未装配（headless 形态/老 gateway）
// = 诚实提示不渲染表格。

const props = defineProps<{ session: string }>()

const { request } = useWSAPI()

const expanded = ref(false)
const loading = ref(false)
const stats = ref<Record<string, number> | null>(null)
const unavailable = ref(false)
// 会话级缓存：切换回来时不重复拉取（统计是单调累加，详情生命周期内
// 够用；重新进入页签可刷新）。
const cache = new Map<string, Record<string, number>>()

async function load() {
  if (cache.has(props.session)) {
    stats.value = cache.get(props.session)!
    return
  }
  const sess = props.session
  loading.value = true
  try {
    const data = await request('sessions', 'stats', { session_id: sess })
    if (sess !== props.session) return // 会话已切换：在途响应丢弃
    const s = (data?.stats ?? {}) as Record<string, number>
    stats.value = s
    cache.set(sess, s)
  } catch {
    if (sess !== props.session) return
    stats.value = null
    unavailable.value = true
  } finally {
    if (sess === props.session) loading.value = false
  }
}

watch(expanded, v => { if (v) void load() })
watch(
  () => props.session,
  () => {
    stats.value = null
    unavailable.value = false
    if (expanded.value) void load()
  },
)

const ROWS: { key: string; label: string; hint: string }[] = [
  { key: 'tool_calls', label: '工具调用', hint: '安全闸通过进入执行的工具次数' },
  { key: 'tool_failures', label: '工具失败', hint: '执行失败（ok=false）的工具次数' },
  { key: 'retries', label: 'LLM 重试', hint: '瞬时错误/限流/上下文错误三类重试发起次数' },
  { key: 'validation_failures', label: '参数校验失败', hint: '工具入参 schema 校验失败次数' },
  { key: 'compactions', label: '上下文压缩', hint: '手动与自动压缩成功次数' },
  { key: 'approvals_total', label: '审批请求', hint: '产生审批卡的高危操作次数' },
  { key: 'approvals_approved', label: '审批批准', hint: '被人工批准的次数' },
  { key: 'approvals_denied', label: '审批拒绝', hint: '被拒绝/超时自动拒绝的次数' },
]
</script>

<template>
  <div class="execstats-panel">
    <button class="panel-toggle" @click="expanded = !expanded">
      <span class="toggle-icon">{{ expanded ? '▾' : '▸' }}</span>
      📊 执行统计
      <span v-if="stats" class="toggle-count">
        {{ stats.tool_calls ?? 0 }} 次工具调用 · {{ stats.approvals_total ?? 0 }} 次审批
      </span>
    </button>

    <div v-if="expanded" class="panel-body">
      <div v-if="loading" class="panel-hint">加载中…</div>

      <table v-else-if="stats" class="stats-table">
        <tbody>
          <tr v-for="row in ROWS" :key="row.key">
            <td class="stats-label" :title="row.hint">{{ row.label }}</td>
            <td class="stats-value">{{ stats[row.key] ?? 0 }}</td>
          </tr>
        </tbody>
      </table>

      <div v-else-if="unavailable" class="panel-hint">
        执行统计不可用（聚合器未装配：headless 形态或旧版 gateway）
      </div>
      <div v-else class="panel-hint">暂无统计数据</div>
    </div>
  </div>
</template>

<style scoped>
.execstats-panel {
  border-bottom: 1px solid var(--border-light);
  background: var(--bg-secondary);
}

.panel-toggle {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  width: 100%;
  padding: var(--space-2) var(--space-4);
  background: transparent;
  border: none;
  cursor: pointer;
  color: var(--text-secondary);
  font-size: var(--text-sm);
  text-align: left;
}

.panel-toggle:hover { color: var(--text-primary); background: var(--bg-hover); }

.toggle-icon { font-size: var(--text-xs); width: 12px; }

.toggle-count {
  margin-left: auto;
  color: var(--text-muted);
  font-size: var(--text-xs);
}

.panel-body {
  padding: var(--space-2) var(--space-4) var(--space-3);
  max-height: 240px;
  overflow-y: auto;
}

.panel-hint {
  color: var(--text-muted);
  font-size: var(--text-xs);
  padding: var(--space-1) 0;
}

.stats-table {
  width: 100%;
  border-collapse: collapse;
  font-size: var(--text-xs);
}

.stats-table td {
  padding: 3px 0;
  border-bottom: 1px dashed var(--border-light);
}

.stats-table tr:last-child td { border-bottom: none; }

.stats-label { color: var(--text-muted); }

.stats-value {
  text-align: right;
  font-family: monospace;
  color: var(--text-primary);
  font-weight: 600;
}
</style>
