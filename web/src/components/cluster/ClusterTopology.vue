<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { useWSAPI } from '../../composables/useWSAPI'
import TopologyCanvas from './TopologyCanvas.vue'

const { request } = useWSAPI()

const loading = ref(true)
const nodes = ref<any[]>([])
const connections = ref<any[]>([])
const traces = ref<any[]>([])

async function loadTopology() {
  try {
    const data = await request('cluster', 'topology')
    if (data?.nodes) nodes.value = data.nodes
    if (data?.connections) connections.value = data.connections
    if (data?.traces) traces.value = data.traces
  } catch { /* backend not ready */ }
}

function onSelectNode(id: string) {
  // Future: switch to nodes tab and expand this node
  console.log('[ClusterTopology] Selected node:', id)
}

onMounted(async () => {
  await loadTopology()
  loading.value = false
})
</script>

<template>
  <div v-if="loading" style="text-align:center;padding:var(--space-8)">
    <div class="spinner spinner-lg" style="margin:0 auto" />
  </div>

  <div v-if="!loading">
    <div class="card">
      <div class="card-header">
        <h3>集群拓扑</h3>
        <button class="btn btn-sm" @click="loadTopology">刷新</button>
      </div>
      <div class="card-body">
        <TopologyCanvas
          :nodes="nodes"
          :connections="connections"
          :traces="traces"
          @select-node="onSelectNode"
        />
      </div>
    </div>
  </div>
</template>
