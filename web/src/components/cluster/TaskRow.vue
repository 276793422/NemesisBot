<script setup lang="ts">
import { ref, watch } from 'vue'
import { useWSAPI } from '../../composables/useWSAPI'

const props = defineProps<{
  task: {
    id: string
    status: 'queued' | 'running' | 'completed' | 'failed'
    source?: string
    target?: string
    input?: string
    duration?: string
    rounds?: number
    toolCalls?: number
    toolChain?: string[]
  }
}>()

const emit = defineEmits<{
  (e: 'cancel', id: string): void
}>()

const { request } = useWSAPI()
const expanded = ref(false)
const detailLoading = ref(false)
const detail = ref<any>(null)

async function fetchDetail() {
  if (!props.task.id) return
  detailLoading.value = true
  try {
    const data = await request('cluster', 'tasks.detail', { task_id: props.task.id })
    detail.value = data
  } catch {
    detail.value = null
  } finally {
    detailLoading.value = false
  }
}

watch(expanded, (val) => {
  if (val) {
    fetchDetail()
  } else {
    detail.value = null
  }
})

function statusLabel(s: string) {
  const map: Record<string, string> = { queued: '排队', running: '运行', completed: '完成', failed: '失败' }
  return map[s] || s
}

function statusBadge(s: string) {
  const map: Record<string, string> = { queued: 'badge-warning', running: 'badge-info', completed: 'badge-success', failed: 'badge-error' }
  return map[s] || 'badge-neutral'
}
</script>

<template>
  <div class="task-row" :class="{ expanded }">
    <div class="task-summary" @click="expanded = !expanded">
      <span class="task-id">#{{ task.id }}</span>
      <span class="badge" :class="statusBadge(task.status)">{{ statusLabel(task.status) }}</span>
      <span class="task-route">{{ task.source || '?' }} → {{ task.target || '?' }}</span>
      <span class="task-duration">{{ task.duration || '--' }}</span>
      <span class="task-expand">{{ expanded ? '▲' : '▼' }}</span>
    </div>
    <div v-if="expanded" class="task-detail">
      <div v-if="detailLoading" style="text-align:center;padding:var(--space-2)">
        <div class="spinner" style="margin:0 auto" />
      </div>
      <template v-else>
        <div v-if="(detail?.input || task.input)" class="task-detail-row">
          <span class="label">输入</span>
          <span class="value">{{ detail?.input || task.input }}</span>
        </div>
        <div class="task-detail-row">
          <span class="label">来源</span>
          <span class="value">{{ detail?.source || task.source || '--' }} → {{ detail?.target || task.target || '--' }}</span>
        </div>
        <div class="task-detail-row">
          <span class="label">轮次</span>
          <span class="value">LLM: {{ detail?.rounds ?? task.rounds ?? '--' }} / 工具: {{ detail?.toolCalls ?? task.toolCalls ?? '--' }}</span>
        </div>
        <div v-if="(detail?.toolChain?.length || task.toolChain?.length)" class="task-detail-row">
          <span class="label">工具链</span>
          <span class="value">{{ (detail?.toolChain || task.toolChain || []).join(' → ') }}</span>
        </div>
        <div v-if="task.status === 'queued' || task.status === 'running'" class="task-actions">
          <button class="btn btn-sm btn-danger" @click.stop="emit('cancel', task.id)">取消任务</button>
        </div>
      </template>
    </div>
  </div>
</template>

<style scoped>
.task-row {
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
  margin-bottom: var(--space-2);
  overflow: hidden;
}
.task-row.expanded { border-color: var(--accent); }
.task-summary {
  display: flex;
  align-items: center;
  gap: var(--space-3);
  padding: var(--space-2) var(--space-3);
  cursor: pointer;
  font-size: var(--text-sm);
}
.task-summary:hover { background: var(--surface-hover); }
.task-id { font-family: var(--font-mono); font-weight: 600; min-width: 40px; }
.task-route { flex: 1; color: var(--text-muted); }
.task-duration { font-family: var(--font-mono); color: var(--text-muted); min-width: 50px; text-align: right; }
.task-expand { color: var(--text-muted); font-size: 10px; }
.task-detail {
  padding: var(--space-3);
  border-top: 1px solid var(--border);
  background: var(--bg-secondary);
  font-size: var(--text-sm);
}
.task-detail-row {
  display: flex;
  gap: var(--space-3);
  padding: var(--space-1) 0;
}
.task-detail-row .label { width: 48px; color: var(--text-muted); flex-shrink: 0; }
.task-detail-row .value { flex: 1; word-break: break-all; }
.task-actions { margin-top: var(--space-2); }
</style>
