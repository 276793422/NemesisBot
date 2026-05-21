<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'

const { request } = useWSAPI()
const toast = useToast()

const activeTab = ref('agent')
const config = ref<any>({})
const loading = ref(true)
const editing = ref(false)
const editConfig = ref('')

// CORS state
const corsOrigins = ref<string[]>([])
const corsEnabled = ref(false)
const newOrigin = ref('')

const tabs = [
  { id: 'agent', label: 'Agent' },
  { id: 'gateway', label: 'Gateway' },
  { id: 'tools', label: '工具' },
  { id: 'services', label: '服务开关' },
  { id: 'logging', label: '日志' },
  { id: 'cors', label: 'CORS' },
  { id: 'raw', label: '原始 JSON' },
]

async function loadConfig() {
  try {
    const data = await request('config', 'get')
    config.value = data || {}
  } catch (e: any) {
    toast.error('加载配置失败: ' + e)
  }
  loading.value = false
}

async function saveField(path: string, value: any) {
  try {
    await request('config', 'set_field', { path, value })
    toast.success(`已更新 ${path}`)
    await loadConfig()
  } catch (e: any) {
    toast.error('保存失败: ' + e)
  }
}

async function saveFullConfig() {
  try {
    const parsed = JSON.parse(editConfig.value)
    await request('config', 'save', parsed)
    toast.success('配置已保存')
    editing.value = false
    await loadConfig()
  } catch (e: any) {
    toast.error('保存失败: ' + e)
  }
}

// CORS functions
async function loadCors() {
  try {
    const data = await request('config', 'cors.list')
    corsOrigins.value = data?.origins || []
  } catch { /* ignore */ }
}

async function addCorsOrigin() {
  if (!newOrigin.value) return
  try {
    await request('config', 'cors.add', { origin: newOrigin.value })
    toast.success('已添加')
    newOrigin.value = ''
    await loadCors()
  } catch (e: any) {
    toast.error('添加失败: ' + e)
  }
}

async function removeCorsOrigin(origin: string) {
  try {
    await request('config', 'cors.remove', { origin })
    toast.success('已移除')
    await loadCors()
  } catch (e: any) {
    toast.error('移除失败: ' + e)
  }
}

async function toggleCors(enabled: boolean) {
  try {
    await request('config', 'cors.toggle', { enabled })
    corsEnabled.value = enabled
    toast.success(enabled ? '已启用' : '已禁用')
  } catch (e: any) {
    toast.error('操作失败: ' + e)
  }
}

function toggleService(path: string, current: boolean) {
  saveField(path, !current)
}

onMounted(async () => {
  await Promise.all([loadConfig(), loadCors()])
})
</script>

<template>
  <div class="page-settings">
    <div class="page-header"><h2>设置</h2></div>
    <div class="page-body">
      <div v-if="loading" style="text-align: center; padding: var(--space-8);">
        <div class="spinner spinner-lg" style="margin: 0 auto;"></div>
      </div>

      <div v-if="!loading">
        <div class="tabs">
          <button v-for="t in tabs" :key="t.id" class="tab" :class="{ active: activeTab === t.id }" @click="activeTab = t.id">{{ t.label }}</button>
        </div>

        <!-- Agent config -->
        <div v-if="activeTab === 'agent'" class="card">
          <div class="card-header"><h3>Agent 配置</h3></div>
          <div class="card-body">
            <div class="form-group">
              <label class="form-label">默认模型</label>
              <input class="form-input" :value="config.agents?.defaults?.model || '--'" disabled style="max-width: 300px;">
              <span class="form-hint">在模型页面修改</span>
            </div>
            <div class="form-group">
              <label class="form-label">温度</label>
              <input class="form-input" type="number" step="0.1" min="0" max="2" :value="config.agents?.defaults?.temperature ?? 0.7"
                @change="(e: any) => saveField('agents.defaults.temperature', parseFloat(e.target.value))" style="max-width: 200px;">
            </div>
            <div class="form-group">
              <label class="form-label">最大 Tokens</label>
              <input class="form-input" type="number" :value="config.agents?.defaults?.max_tokens ?? 4096"
                @change="(e: any) => saveField('agents.defaults.max_tokens', parseInt(e.target.value))" style="max-width: 200px;">
            </div>
            <div class="form-group">
              <label class="form-label">工作空间限制</label>
              <div class="toggle" :class="{ active: config.security?.restrict_to_workspace !== false }"
                @click="toggleService('security.restrict_to_workspace', config.security?.restrict_to_workspace === false)"></div>
              <span class="form-hint" style="margin-left: var(--space-2);">{{ config.security?.restrict_to_workspace !== false ? '已启用' : '已禁用' }}</span>
            </div>
          </div>
        </div>

        <!-- Gateway config -->
        <div v-if="activeTab === 'gateway'" class="card">
          <div class="card-header"><h3>Gateway 配置</h3></div>
          <div class="card-body">
            <div class="form-group">
              <label class="form-label">主机</label>
              <input class="form-input" :value="config.gateway?.host || '0.0.0.0'" disabled style="max-width: 300px;">
            </div>
            <div class="form-group">
              <label class="form-label">端口</label>
              <input class="form-input" :value="config.gateway?.port || 49000" disabled style="max-width: 200px;">
            </div>
          </div>
        </div>

        <!-- Tools config -->
        <div v-if="activeTab === 'tools'" class="card">
          <div class="card-header"><h3>工具配置</h3></div>
          <div class="card-body">
            <div class="form-group">
              <label class="form-label">Web 搜索引擎</label>
              <select class="form-select" style="max-width: 200px;"
                :value="config.tools?.web_search?.engine || 'none'"
                @change="(e: any) => saveField('tools.web_search.engine', e.target.value)">
                <option value="none">禁用</option>
                <option value="brave">Brave</option>
                <option value="duckduckgo">DuckDuckGo</option>
              </select>
            </div>
            <div class="form-group">
              <label class="form-label">Cron 超时（秒）</label>
              <input class="form-input" type="number" :value="config.cron?.timeout_secs ?? 60"
                @change="(e: any) => saveField('cron.timeout_secs', parseInt(e.target.value))" style="max-width: 200px;">
            </div>
          </div>
        </div>

        <!-- Services toggles -->
        <div v-if="activeTab === 'services'" class="card">
          <div class="card-header"><h3>系统服务开关</h3></div>
          <div class="card-body">
            <div v-for="(val, key) in { heartbeat: config.heartbeat?.enabled, 'device monitor': config.devices?.monitor_enabled, security: config.security?.enabled, forge: config.forge?.enabled, mcp: config.mcp?.enabled }" :key="key"
              style="display: flex; align-items: center; justify-content: space-between; padding: var(--space-3) 0; border-bottom: 1px solid var(--border-light);">
              <span style="font-size: var(--text-sm); font-weight: 500; text-transform: capitalize;">{{ key }}</span>
              <div class="toggle" :class="{ active: val !== false }" @click="toggleService(key + '.enabled', val === false)"></div>
            </div>
          </div>
        </div>

        <!-- Logging -->
        <div v-if="activeTab === 'logging'" class="card">
          <div class="card-header"><h3>日志配置</h3></div>
          <div class="card-body">
            <div class="form-group">
              <label class="form-label">控制台日志</label>
              <div class="toggle" :class="{ active: config.logging?.console?.enabled !== false }"
                @click="toggleService('logging.console.enabled', config.logging?.console?.enabled === false)"></div>
            </div>
            <div class="form-group">
              <label class="form-label">日志级别</label>
              <select class="form-select" style="max-width: 200px;"
                :value="config.logging?.console?.level || 'info'"
                @change="(e: any) => saveField('logging.console.level', e.target.value)">
                <option value="debug">DEBUG</option>
                <option value="info">INFO</option>
                <option value="warn">WARN</option>
                <option value="error">ERROR</option>
              </select>
            </div>
          </div>
        </div>

        <!-- CORS -->
        <div v-if="activeTab === 'cors'">
          <div class="card" style="margin-bottom: var(--space-4);">
            <div class="card-header">
              <h3>CORS 管理</h3>
              <div class="toggle" :class="{ active: corsEnabled }" @click="toggleCors(!corsEnabled)"></div>
            </div>
            <div class="card-body">
              <div style="display: flex; gap: var(--space-2); margin-bottom: var(--space-4);">
                <input class="form-input" v-model="newOrigin" placeholder="例如: http://localhost:3000" style="max-width: 400px;">
                <button class="btn btn-primary" @click="addCorsOrigin">添加</button>
              </div>
              <div v-if="corsOrigins.length === 0" style="color: var(--text-muted); font-size: var(--text-sm);">暂无 CORS 规则</div>
              <div v-for="origin in corsOrigins" :key="origin" style="display: flex; align-items: center; justify-content: space-between; padding: var(--space-2) var(--space-3); border: 1px solid var(--border-light); border-radius: var(--radius-md); margin-bottom: var(--space-2);">
                <code style="font-size: var(--text-sm);">{{ origin }}</code>
                <button class="btn btn-sm btn-danger" @click="removeCorsOrigin(origin)">移除</button>
              </div>
            </div>
          </div>
        </div>

        <!-- Raw JSON -->
        <div v-if="activeTab === 'raw'">
          <div class="card">
            <div class="card-header">
              <h3>原始配置 (config.json)</h3>
              <div style="display: flex; gap: var(--space-2);">
                <template v-if="!editing">
                  <button class="btn btn-sm" @click="editing = true; editConfig = JSON.stringify(config, null, 2)">编辑</button>
                </template>
                <template v-else>
                  <button class="btn btn-sm" @click="editing = false">取消</button>
                  <button class="btn btn-sm btn-primary" @click="saveFullConfig">保存</button>
                </template>
              </div>
            </div>
            <div class="card-body">
              <div v-if="editing">
                <textarea class="form-textarea" style="min-height: 500px; font-family: var(--font-mono); font-size: var(--text-xs);" v-model="editConfig"></textarea>
              </div>
              <div v-else>
                <div class="settings-section" v-for="(sectionData, section) in config" :key="section">
                  <h3>{{ section }}</h3>
                  <div class="settings-grid">
                    <template v-for="(value, key) in (sectionData as any)" :key="key">
                      <template v-if="typeof value !== 'object'">
                        <div class="settings-key">{{ key }}</div>
                        <div class="settings-value">{{ String(value) }}</div>
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
  </div>
</template>
