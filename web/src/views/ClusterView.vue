<script setup lang="ts">
import { ref } from 'vue'
import ClusterTabs from '../components/cluster/ClusterTabs.vue'
import ClusterOverview from '../components/cluster/ClusterOverview.vue'
import ClusterNodes from '../components/cluster/ClusterNodes.vue'
import ClusterTasks from '../components/cluster/ClusterTasks.vue'
import ClusterTopology from '../components/cluster/ClusterTopology.vue'
import ClusterIdentity from '../components/cluster/ClusterIdentity.vue'
import ClusterPersona from '../components/cluster/ClusterPersona.vue'
import ClusterSettings from '../components/cluster/ClusterSettings.vue'
import ClusterDiagnostics from '../components/cluster/ClusterDiagnostics.vue'

const activeTab = ref('overview')

const tabMap: Record<string, any> = {
  overview: ClusterOverview,
  nodes: ClusterNodes,
  tasks: ClusterTasks,
  topology: ClusterTopology,
  identity: ClusterIdentity,
  persona: ClusterPersona,
  settings: ClusterSettings,
  diagnostics: ClusterDiagnostics,
}
</script>

<template>
  <div class="page-cluster">
    <div class="page-header"><h2>集群管理</h2></div>
    <div class="page-body">
      <ClusterTabs v-model="activeTab" />
      <div style="margin-top:var(--space-4)">
        <component :is="tabMap[activeTab]" />
      </div>
    </div>
  </div>
</template>
