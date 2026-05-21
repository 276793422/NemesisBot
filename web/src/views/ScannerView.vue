<script setup lang="ts">
import { ref, onMounted, onUnmounted } from 'vue'
import { httpGet } from '../composables/useWebSocket'
import { useWSAPI } from '../composables/useWSAPI'
import { on as sseOn, off as sseOff } from '../composables/useSSE'
import { useToast } from '../composables/useToast'

const { request } = useWSAPI()
const toast = useToast()

interface ScannerEngine { name?: string; state?: string; version?: string; description?: string; progress?: any }

const activeTab = ref('engines')
const engines = ref<ScannerEngine[]>([])
const loading = ref(true)
const scannerConfig = ref<any>({})
const editing = ref(false)
const editConfig = ref('')
let _onProgress: ((data: any) => void) | null = null

const stateBadgeMap: Record<string, string> = {
  pending: '待安装', installed: '已安装', failed: '安装失败',
  ready: '就绪', stale: '需更新', running: '运行中',
}
const stateBadgeClassMap: Record<string, string> = {
  pending: 'badge-neutral', installed: 'badge-info', failed: 'badge-error',
  ready: 'badge-success', stale: 'badge-warning', running: 'badge-success',
}

function getStateBadge(state?: string): string { return stateBadgeMap[state || ''] || state || '' }
function getStateBadgeClass(state?: string): string { return stateBadgeClassMap[state || ''] || 'badge-neutral' }

async function loadEngines() {
  try {
    const data = await httpGet<{ engines: ScannerEngine[] }>('/api/scanner/status')
    engines.value = data.engines || []
  } catch (err) {
    console.error('[Scanner] Failed to load:', err)
  }
}

async function loadConfig() {
  try {
    const data = await request('scanner', 'config.get')
    scannerConfig.value = data || {}
    editConfig.value = JSON.stringify(data, null, 2)
  } catch { /* ignore */ }
}

async function saveConfig() {
  try {
    const parsed = JSON.parse(editConfig.value)
    await request('scanner', 'config.save', parsed)
    toast.success('配置已保存')
    editing.value = false
    await loadConfig()
  } catch (e: any) {
    toast.error('保存失败: ' + e)
  }
}

onMounted(async () => {
  await Promise.all([loadEngines(), loadConfig()])
  loading.value = false

  _onProgress = (data: any) => {
    for (const engine of engines.value) {
      if (engine.name === data.engine) { engine.progress = data; break }
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
    <div class="page-header"><h2>扫描器管理</h2></div>
    <div class="page-body">
      <div v-if="loading" style="text-align: center; padding: var(--space-8);">
        <div class="spinner spinner-lg" style="margin: 0 auto;"></div>
      </div>

      <div v-if="!loading">
        <div class="tabs">
          <button class="tab" :class="{ active: activeTab === 'engines' }" @click="activeTab = 'engines'">引擎状态</button>
          <button class="tab" :class="{ active: activeTab === 'config' }" @click="activeTab = 'config'">配置</button>
        </div>

        <!-- Engines -->
        <div v-if="activeTab === 'engines'">
          <div v-if="engines.length === 0" class="empty-state">
            <div class="empty-state-icon">&#x1F6E1;&#xFE0F;</div>
            <h3>未检测到扫描引擎</h3>
            <p>请先配置扫描器引擎后重试</p>
          </div>
          <div v-if="engines.length > 0" class="engines-grid">
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

        <!-- Config -->
        <div v-if="activeTab === 'config'">
          <div class="card">
            <div class="card-header">
              <h3>Scanner 配置</h3>
              <div style="display: flex; gap: var(--space-2);">
                <template v-if="!editing">
                  <button class="btn btn-sm" @click="editing = true">编辑</button>
                </template>
                <template v-else>
                  <button class="btn btn-sm" @click="editing = false">取消</button>
                  <button class="btn btn-sm btn-primary" @click="saveConfig">保存</button>
                </template>
              </div>
            </div>
            <div class="card-body">
              <div v-if="editing">
                <textarea class="form-textarea" style="min-height: 400px; font-family: var(--font-mono); font-size: var(--text-xs);" v-model="editConfig"></textarea>
              </div>
              <div v-else>
                <div class="settings-grid">
                  <template v-for="(value, key) in scannerConfig" :key="key">
                    <template v-if="typeof value !== 'object'">
                      <span class="settings-key">{{ key }}</span>
                      <span class="settings-value">{{ typeof value === 'boolean' ? (value ? '是' : '否') : String(value) }}</span>
                    </template>
                  </template>
                </div>
              </div>
            </div>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>
