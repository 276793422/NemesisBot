<script setup lang="ts">
import { computed } from 'vue'

const props = defineProps<{
  node: {
    id: string
    name: string
    role: string
    address: string
    category?: string
    tags?: string[]
    capabilities?: string[]
    online?: boolean
    lastSeen?: string
    taskCount?: number
    successRate?: number
    uptime?: string
  }
}>()

const emit = defineEmits<{
  (e: 'ping', id: string): void
  (e: 'remove', id: string): void
}>()

const roleLabel = computed(() => {
  const map: Record<string, string> = {
    manager: '管理节点',
    coordinator: '协调节点',
    worker: '工作节点',
    observer: '观察节点',
    standby: '备用节点',
  }
  return map[props.node.role] || props.node.role
})
</script>

<template>
  <div class="node-detail">
    <div class="node-detail-header">
      <span class="node-name">{{ node.name }}</span>
      <span class="badge" :class="node.role === 'manager' ? 'badge-info' : 'badge-neutral'">{{ roleLabel }}</span>
      <div style="flex:1" />
      <button class="btn btn-sm" @click="emit('ping', node.id)">Ping</button>
      <button class="btn btn-sm btn-danger" @click="emit('remove', node.id)">移除</button>
    </div>
    <div class="node-detail-body">
      <div class="node-detail-row"><span class="label">地址</span><span>{{ node.address || '--' }}</span></div>
      <div class="node-detail-row"><span class="label">分类</span><span>{{ node.category || '--' }}</span></div>
      <div class="node-detail-row">
        <span class="label">标签</span>
        <span v-if="node.tags?.length">{{ node.tags.join(', ') }}</span>
        <span v-else>--</span>
      </div>
      <div class="node-detail-row">
        <span class="label">能力</span>
        <span v-if="node.capabilities?.length">
          <span v-for="cap in node.capabilities" :key="cap" class="badge badge-neutral" style="margin-right:var(--space-1);font-size:var(--text-xs)">{{ cap }}</span>
        </span>
        <span v-else>--</span>
      </div>
      <div class="node-detail-stats">
        <div class="stat"><span class="stat-num">{{ node.taskCount ?? '--' }}</span><span class="stat-lbl">累计任务</span></div>
        <div class="stat"><span class="stat-num">{{ node.successRate != null ? (node.successRate * 100).toFixed(1) + '%' : '--' }}</span><span class="stat-lbl">成功率</span></div>
        <div class="stat"><span class="stat-num">{{ node.uptime || '--' }}</span><span class="stat-lbl">在线时长</span></div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.node-detail {
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
  padding: var(--space-3) var(--space-4);
  margin-bottom: var(--space-2);
}
.node-detail-header {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  margin-bottom: var(--space-2);
}
.node-name { font-weight: 600; }
.node-detail-body { font-size: var(--text-sm); }
.node-detail-row {
  display: flex;
  gap: var(--space-3);
  padding: var(--space-1) 0;
}
.node-detail-row .label {
  width: 48px;
  color: var(--text-muted);
  flex-shrink: 0;
}
.node-detail-stats {
  display: flex;
  gap: var(--space-6);
  margin-top: var(--space-2);
  padding-top: var(--space-2);
  border-top: 1px solid var(--border);
}
.stat { display: flex; flex-direction: column; align-items: center; }
.stat-num { font-weight: 700; font-size: var(--text-lg); }
.stat-lbl { font-size: var(--text-xs); color: var(--text-muted); }
</style>
