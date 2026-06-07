<script setup lang="ts">
import { ref, watch, onMounted, onUnmounted } from 'vue'
import { useSSE } from '../../composables/useSSE'

const props = defineProps<{
  events: Array<{
    time: string
    type: 'task_start' | 'task_complete' | 'task_fail' | 'node_online' | 'node_offline'
    message: string
  }>
}>()

const sse = useSSE()
const mergedEvents = ref<any[]>([])

// Sync prop events as base
watch(() => props.events, (val) => {
  // Only replace if SSE hasn't added events on top
  if (mergedEvents.value.length <= val.length) {
    mergedEvents.value = [...val]
  } else {
    // Prepend any new prop events that aren't already in merged
    const existingTimes = new Set(mergedEvents.value.map(e => e.time + e.message))
    const newFromProp = val.filter(e => !existingTimes.has(e.time + e.message))
    if (newFromProp.length > 0) {
      mergedEvents.value = [...newFromProp, ...mergedEvents.value]
    }
  }
}, { immediate: true })

function handleClusterEvent(data: any) {
  const formatted = formatSSEEvent(data)
  if (formatted) {
    mergedEvents.value.unshift(formatted)
  }
}

function formatSSEEvent(data: any) {
  const event = data?.event || ''
  const payload = data?.data || {}
  const now = new Date().toLocaleTimeString()

  const map: Record<string, { type: string; messageFn: (p: any) => string }> = {
    task_started: { type: 'task_start', messageFn: (p) => `任务 ${p.task_id || ''} 已启动` },
    task_completed: { type: 'task_complete', messageFn: (p) => `任务 ${p.task_id || ''} 已完成` },
    task_failed: { type: 'task_fail', messageFn: (p) => `任务 ${p.task_id || ''} 失败: ${p.error || ''}` },
    node_online: { type: 'node_online', messageFn: (p) => `节点 ${p.node_id || p.name || ''} 上线` },
    node_offline: { type: 'node_offline', messageFn: (p) => `节点 ${p.node_id || p.name || ''} 离线` },
  }

  const entry = map[event]
  if (!entry) return null

  return { time: now, type: entry.type, message: entry.messageFn(payload) }
}

onMounted(() => {
  sse.on('cluster-event', handleClusterEvent)
})

onUnmounted(() => {
  sse.off('cluster-event', handleClusterEvent)
})

function icon(type: string) {
  const map: Record<string, string> = {
    task_complete: '✓',
    task_start: '⟳',
    task_fail: '✗',
    node_online: '●',
    node_offline: '○',
  }
  return map[type] || '•'
}

function iconClass(type: string) {
  const map: Record<string, string> = {
    task_complete: 'badge-success',
    task_start: 'badge-info',
    task_fail: 'badge-error',
    node_online: 'badge-success',
    node_offline: 'badge-neutral',
  }
  return map[type] || 'badge-neutral'
}
</script>

<template>
  <div class="activity-feed">
    <div v-if="!mergedEvents.length" class="empty-state" style="padding: var(--space-6);">
      <p>暂无活动</p>
    </div>
    <div v-for="(evt, i) in mergedEvents" :key="i" class="activity-item">
      <span class="badge" :class="iconClass(evt.type)" style="width:20px;height:20px;display:inline-flex;align-items:center;justify-content:center;border-radius:50%;padding:0;font-size:11px;">{{ icon(evt.type) }}</span>
      <span class="activity-time">{{ evt.time }}</span>
      <span class="activity-msg">{{ evt.message }}</span>
    </div>
  </div>
</template>

<style scoped>
.activity-feed {
  max-height: 320px;
  overflow-y: auto;
}
.activity-item {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  padding: var(--space-2) 0;
  border-bottom: 1px solid var(--border);
  font-size: var(--text-sm);
}
.activity-item:last-child { border-bottom: none; }
.activity-time {
  color: var(--text-muted);
  font-family: var(--font-mono);
  font-size: var(--text-xs);
  white-space: nowrap;
}
.activity-msg {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
</style>
