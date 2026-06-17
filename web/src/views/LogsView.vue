<script setup lang="ts">
import { ref, computed, watch } from 'vue'
import LogsTabs from '../components/logs/LogsTabs.vue'
import EventStream from '../components/logs/EventStream.vue'
import SessionExplorer from '../components/logs/SessionExplorer.vue'
import SecurityAudit from '../components/logs/SecurityAudit.vue'
import IntegrityChain from '../components/logs/IntegrityChain.vue'
import { useWSAPI } from '../composables/useWSAPI'
import type {
  SessionEntry, LlmRequestEntry, ClusterTaskEntry,
  AuditEntry, ChainSegment,
} from '../components/logs/mockData'

const { request } = useWSAPI()

const activeTab = ref('events')

// Per-tab data, loaded lazily on first tab activation.
const sessions = ref<SessionEntry[]>([])
const requests = ref<LlmRequestEntry[]>([])
const tasks = ref<ClusterTaskEntry[]>([])
const auditEntries = ref<AuditEntry[]>([])
const chainSegments = ref<ChainSegment[]>([])

const loadedTabs = new Set<string>()
const loadingTab = ref<string | null>(null)
const chainVerifyResult = ref<{ valid: boolean; first_broken_index: number | null; broken_count: number; total_segments: number } | null>(null)

const counts = computed(() => ({
  events: 0,
  sessions: sessions.value.length + requests.value.length + tasks.value.length,
  audit: auditEntries.value.length,
  chain: chainSegments.value.length,
}))

async function loadSessionsTab() {
  loadingTab.value = 'sessions'
  try {
    const [sessRes, reqRes, taskRes] = await Promise.allSettled([
      request('logs', 'session_list', { limit: 50, offset: 0 }),
      request('logs', 'requests', { limit: 50, offset: 0 }),
      request('logs', 'cluster_task_list', { limit: 50, offset: 0 }),
    ])
    if (sessRes.status === 'fulfilled') sessions.value = sessRes.value?.sessions ?? []
    if (reqRes.status === 'fulfilled') requests.value = reqRes.value?.entries ?? []
    if (taskRes.status === 'fulfilled') tasks.value = taskRes.value?.entries ?? []
    loadedTabs.add('sessions')
  } catch (e) {
    console.error('[LogsView] loadSessionsTab failed', e)
  } finally {
    loadingTab.value = null
  }
}

async function loadAuditTab() {
  loadingTab.value = 'audit'
  try {
    const res = await request('logs', 'security', { limit: 200, offset: 0 })
    auditEntries.value = res?.entries ?? []
    loadedTabs.add('audit')
  } catch (e) {
    console.error('[LogsView] loadAuditTab failed', e)
  } finally {
    loadingTab.value = null
  }
}

async function loadChainTab() {
  loadingTab.value = 'chain'
  try {
    const res = await request('logs', 'chain_list', { limit: 200, offset: 0 })
    chainSegments.value = res?.segments ?? []
    loadedTabs.add('chain')
  } catch (e) {
    console.error('[LogsView] loadChainTab failed', e)
  } finally {
    loadingTab.value = null
  }
}

async function verifyChain() {
  loadingTab.value = 'chain-verify'
  try {
    const res = await request('logs', 'chain_verify', {})
    chainVerifyResult.value = res
    // After verify, reload chain_list so per-segment breakReason shows up consistently.
    await loadChainTab()
  } catch (e) {
    console.error('[LogsView] verifyChain failed', e)
  } finally {
    loadingTab.value = null
  }
}

watch(activeTab, (tab) => {
  if (tab === 'sessions' && !loadedTabs.has('sessions')) loadSessionsTab()
  else if (tab === 'audit' && !loadedTabs.has('audit')) loadAuditTab()
  else if (tab === 'chain' && !loadedTabs.has('chain')) loadChainTab()
}, { immediate: true })

function onNavigate(target: { type: string; id: string }) {
  console.log('[LogsView] navigate', target)
}
</script>

<template>
  <div class="page-logs">
    <div class="page-header">
      <h2>日志管理</h2>
      <span v-if="loadingTab" class="loading-hint">⟳ 加载中...</span>
    </div>

    <div class="page-body page-logs-body">
      <LogsTabs v-model="activeTab" :counts="counts" />

      <div class="logs-content">
        <EventStream v-if="activeTab === 'events'" />
        <SessionExplorer
          v-else-if="activeTab === 'sessions'"
          :sessions="sessions"
          :requests="requests"
          :tasks="tasks"
          @navigate="onNavigate"
          @reload="loadSessionsTab"
        />
        <SecurityAudit
          v-else-if="activeTab === 'audit'"
          :entries="auditEntries"
        />
        <IntegrityChain
          v-else-if="activeTab === 'chain'"
          :segments="chainSegments"
          :verify-result="chainVerifyResult"
          @verify="verifyChain"
          @reload="loadChainTab"
        />
      </div>
    </div>
  </div>
</template>

<style scoped>
.page-logs {
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

.page-logs-body {
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

.logs-content {
  flex: 1;
  overflow: hidden;
}
</style>
