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
import ClusterPersonaGen from '../components/cluster/ClusterPersonaGen.vue'

const activeTab = ref('overview')

// 跨网漫游卡 2026-10-23 自独立 TAB 收编进【配置】页右列（ClusterRoaming 组件
// 保留，卡片形态）——QUIC 与集群解耦后「跨网开关」是集群配置的一部分。
const tabMap: Record<string, any> = {
  overview: ClusterOverview,
  nodes: ClusterNodes,
  tasks: ClusterTasks,
  topology: ClusterTopology,
  identity: ClusterIdentity,
  persona: ClusterPersona,
  settings: ClusterSettings,
  diagnostics: ClusterDiagnostics,
  'persona-gen': ClusterPersonaGen,
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
