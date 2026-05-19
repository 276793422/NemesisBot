<script setup lang="ts">
import { ref, onMounted, onUnmounted } from 'vue'
import { httpGet } from '../composables/useWebSocket'
import { on as sseOn, off as sseOff } from '../composables/useSSE'

interface ScannerEngine {
  name?: string
  state?: string
  version?: string
  description?: string
  progress?: any
}

const engines = ref<ScannerEngine[]>([])
const loading = ref(true)
let _onProgress: ((data: any) => void) | null = null

const stateBadgeMap: Record<string, string> = {
  pending: '待安装',
  installed: '已安装',
  failed: '安装失败',
  ready: '就绪',
  stale: '需更新',
  running: '运行中',
}

const stateBadgeClassMap: Record<string, string> = {
  pending: 'badge-neutral',
  installed: 'badge-info',
  failed: 'badge-error',
  ready: 'badge-success',
  stale: 'badge-warning',
  running: 'badge-success',
}

function getStateBadge(state?: string): string {
  return stateBadgeMap[state || ''] || state || ''
}

function getStateBadgeClass(state?: string): string {
  return stateBadgeClassMap[state || ''] || 'badge-neutral'
}

onMounted(async () => {
  try {
    const data = await httpGet<{ engines: ScannerEngine[] }>('/api/scanner/status')
    engines.value = data.engines || []
  } catch (err) {
    console.error('[Scanner] Failed to load:', err)
  }
  loading.value = false

  _onProgress = (data: any) => {
    for (const engine of engines.value) {
      if (engine.name === data.engine) {
        engine.progress = data
        break
      }
    }
  }
  sseOn('scanner-progress', _onProgress)
})

onUnmounted(() => {
  if (_onProgress) sseOff('scanner-progress', _onProgress)
})
</script>

<template>
  <div class="page-scanner">
    <div class="page-header">
      <h2>扫描器管理</h2>
    </div>
    <div class="page-body">
      <div v-if="loading" style="text-align: center; padding: var(--space-8);">
        <div class="spinner spinner-lg" style="margin: 0 auto;"></div>
      </div>

      <div v-if="!loading && engines.length === 0" class="empty-state">
        <div class="empty-state-icon">\u{1F6E1}\uFE0F</div>
        <h3>未检测到扫描引擎</h3>
        <p>请先配置扫描器引擎后重试</p>
      </div>

      <div v-if="!loading && engines.length > 0" class="engines-grid">
        <div v-for="(engine, idx) in engines" :key="idx" class="card">
          <div class="card-header">
            <h3>{{ engine.name || 'Engine' }}</h3>
            <span class="badge" :class="getStateBadgeClass(engine.state)">{{ getStateBadge(engine.state) }}</span>
          </div>
          <div class="card-body">
            <p v-if="engine.version" style="font-size: var(--text-sm); color: var(--text-muted); margin-bottom: var(--space-2);">
              版本: <span>{{ engine.version }}</span>
            </p>
            <p v-if="engine.description" style="font-size: var(--text-sm); color: var(--text-secondary);">{{ engine.description }}</p>
            <p v-else style="font-size: var(--text-sm); color: var(--text-muted);">病毒扫描引擎</p>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>
