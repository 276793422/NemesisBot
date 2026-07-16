<script setup lang="ts">
import { onMounted, watch } from 'vue'
import { storeToRefs } from 'pinia'
import { useWorkflowStore } from '../stores/workflow'
import WorkflowTabs from '../components/workflow/WorkflowTabs.vue'
import WorkflowList from '../components/workflow/WorkflowList.vue'
import WorkflowCanvas from '../components/workflow/WorkflowCanvas.vue'
import WorkflowHistory from '../components/workflow/WorkflowHistory.vue'
import WorkflowYaml from '../components/workflow/WorkflowYaml.vue'

const store = useWorkflowStore()
const { activeTab, listLoading } = storeToRefs(store)

defineProps<{ embedded?: boolean }>()

onMounted(() => {
  store.fetchList()
})

watch(activeTab, (tab) => {
  if (tab === 'list') store.fetchList()
  if (tab === 'history') store.fetchRuns({})
})
</script>

<template>
  <div :class="embedded ? 'workflow-embed' : 'page-workflow'">
    <div v-if="!embedded" class="page-header">
      <h2>工作流</h2>
      <span v-if="listLoading" class="loading-hint">⟳ 加载中...</span>
    </div>
    <div v-else-if="listLoading" class="loading-hint" style="margin-bottom: var(--space-2);">⟳ 加载中...</div>

    <div :class="embedded ? 'page-workflow-body' : 'page-body page-workflow-body'">
      <WorkflowTabs v-model="activeTab" />

      <div class="workflow-content">
        <WorkflowList v-if="activeTab === 'list'" />
        <WorkflowCanvas v-else-if="activeTab === 'canvas'" />
        <WorkflowHistory v-else-if="activeTab === 'history'" />
        <WorkflowYaml v-else-if="activeTab === 'yaml'" />
      </div>
    </div>
  </div>
</template>

<style scoped>
.page-workflow {
  display: flex;
  flex-direction: column;
  height: 100%;
  background: var(--bg-primary);
}

.page-header {
  display: flex;
  align-items: center;
  gap: var(--space-3);
}

.loading-hint {
  font-size: var(--text-xs);
  color: var(--text-muted);
}

.page-workflow-body {
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

.workflow-content {
  flex: 1;
  overflow: hidden;
}
</style>
