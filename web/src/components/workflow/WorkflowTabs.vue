<script setup lang="ts">
import { computed } from 'vue'
import { storeToRefs } from 'pinia'
import { useWorkflowStore } from '../../stores/workflow'

const props = defineProps<{
  modelValue: 'list' | 'canvas' | 'history' | 'yaml'
}>()

const emit = defineEmits<{
  (e: 'update:modelValue', tab: 'list' | 'canvas' | 'history' | 'yaml'): void
}>()

const store = useWorkflowStore()
const { workflows, runs, editingDirty } = storeToRefs(store)

const counts = computed(() => ({
  list: workflows.value.length,
  canvas: 0,
  history: runs.value.length,
  yaml: 0,
}))

const tabs: { id: 'list' | 'canvas' | 'history' | 'yaml'; label: string; icon: string }[] = [
  { id: 'list', label: '工作流列表', icon: '📋' },
  { id: 'canvas', label: '画布', icon: '🎯' },
  { id: 'history', label: '执行历史', icon: '📈' },
  { id: 'yaml', label: 'YAML', icon: '📝' },
]

function click(tab: 'list' | 'canvas' | 'history' | 'yaml') {
  if (tab === props.modelValue) return

  // Only guard when the user actually has unsaved edits. Blank-but-untouched
  // new workflows (editingDirty=false) don't trigger the prompt — this was
  // the bug where every tab change fired the dialog.
  if (editingDirty.value) {
    const choice = window.confirm(
      '当前编辑内容尚未保存。\n\n点击「确定」= 丢弃修改并切换\n点击「取消」= 留在当前 Tab',
    )
    if (choice) {
      store.discardEditing()
    } else {
      return
    }
  }
  emit('update:modelValue', tab)
}
</script>

<template>
  <div class="tabs">
    <button
      v-for="tab in tabs"
      :key="tab.id"
      class="tab"
      :class="{ active: modelValue === tab.id }"
      @click="click(tab.id)"
    >
      <span class="tab-icon">{{ tab.icon }}</span>
      <span class="tab-label">{{ tab.label }}</span>
      <span v-if="counts[tab.id]" class="tab-count">{{ counts[tab.id] }}</span>
      <span v-if="tab.id === 'canvas' && editingDirty" class="dirty-dot" title="未保存修改">●</span>
      <span v-if="tab.id === 'yaml' && editingDirty" class="dirty-dot" title="未保存修改">●</span>
    </button>
  </div>
</template>

<style scoped>
.tab-icon {
  margin-right: var(--space-1);
  opacity: 0.55;
  font-size: var(--text-sm);
  transition: opacity var(--duration-fast);
}

.tab:hover .tab-icon {
  opacity: 0.85;
}

.tab.active .tab-icon {
  opacity: 1;
}

.tab-count {
  margin-left: var(--space-2);
  font-size: var(--text-xs);
  color: var(--text-muted);
  font-weight: 400;
  font-variant-numeric: tabular-nums;
}

.tab.active .tab-count {
  color: var(--accent);
  font-weight: 500;
}

.dirty-dot {
  margin-left: var(--space-1);
  color: var(--warning, #f39c12);
  font-size: var(--text-xs);
}
</style>
