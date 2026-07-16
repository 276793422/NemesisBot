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
import ForgeView from './ForgeView.vue'
import { usePageTab } from '../lib/pageTab'

const forgeOn = import.meta.env.VITE_FEATURE_FORGE !== 'false'
const clusterOn = import.meta.env.VITE_FEATURE_CLUSTER !== 'false'

/** Outer hub: 集群 | Forge */
const hubTab = ref(clusterOn ? 'cluster' : 'forge')
const { setTab: setHubTab } = usePageTab(
  hubTab,
  ['cluster', 'forge'] as const,
  clusterOn ? 'cluster' : 'forge',
)

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
  'persona-gen': ClusterPersonaGen,
}
</script>

<template>
  <div class="page-cluster page-advanced">
    <div class="page-header"><h2>高级</h2></div>
    <div class="page-body">
      <div class="tabs" style="margin-bottom: var(--space-4);">
        <button
          v-if="clusterOn"
          class="tab"
          :class="{ active: hubTab === 'cluster' }"
          @click="setHubTab('cluster')"
        >集群</button>
        <button
          v-if="forgeOn"
          class="tab"
          :class="{ active: hubTab === 'forge' }"
          @click="setHubTab('forge')"
        >Forge</button>
      </div>

      <div v-if="hubTab === 'forge' && forgeOn">
        <ForgeView embedded />
      </div>

      <template v-if="hubTab === 'cluster' && clusterOn">
        <ClusterTabs v-model="activeTab" />
        <div style="margin-top:var(--space-4)">
          <component :is="tabMap[activeTab]" />
        </div>
      </template>
    </div>
  </div>
</template>
