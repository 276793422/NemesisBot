<script setup lang="ts">
import { ref, onMounted, computed } from 'vue'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'
import { usePageTab } from '../lib/pageTab'
import ToolsView from './ToolsView.vue'
import TasksView from './TasksView.vue'
import SmartFieldForm from '../components/SmartFieldForm.vue'
import { SETTINGS_FIELD_META } from '../lib/friendlyFields'

const { request } = useWSAPI()
const toast = useToast()

const activeTab = ref('agent')
const config = ref<any>({})
const loading = ref(true)
const editing = ref(false)
const editConfig = ref('')

// CORS state
const corsOrigins = ref<string[]>([])
const newOrigin = ref('')

const tabs = [
  { id: 'agent', label: 'Agent' },
  { id: 'gateway', label: 'Gateway' },
  { id: 'tools', label: '工具开关' },
  { id: 'tools-md', label: '工具笔记' },
  { id: 'tasks', label: '任务' },
  { id: 'services', label: '服务开关' },
  { id: 'logging', label: '日志' },
  { id: 'cors', label: 'CORS' },
  { id: 'raw', label: '进阶 JSON' },
]
const { setTab } = usePageTab(activeTab, tabs.map(t => t.id), 'agent')

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

function toggleService(path: string, current: boolean) {
  saveField(path, !current)
}

// Computed form models for smart fields
const agentFormModel = computed(() => ({
  'agents.defaults.temperature': config.value?.agents?.defaults?.temperature ?? 0.7,
  'agents.defaults.max_tokens': config.value?.agents?.defaults?.max_tokens ?? 4096,
  'agents.defaults.restrict_to_workspace': config.value?.agents?.defaults?.restrict_to_workspace !== false,
}))

const toolsFormModel = computed(() => ({
  'tools.web.brave.enabled': config.value?.tools?.web?.brave?.enabled === true,
  'tools.web.duckduckgo.enabled': config.value?.tools?.web?.duckduckgo?.enabled === true,
  'tools.cron.exec_timeout_minutes': config.value?.tools?.cron?.exec_timeout_minutes ?? 60,
}))

const loggingFormModel = computed(() => ({
  'logging.general.enabled': config.value?.logging?.general?.enabled !== false,
  'logging.general.enable_console': config.value?.logging?.general?.enable_console !== false,
  'logging.general.level': config.value?.logging?.general?.level || 'info',
  'logging.llm.enabled': config.value?.logging?.llm?.enabled === true,
}))

const servicesFormModel = computed(() => ({
  'heartbeat.enabled': config.value?.heartbeat?.enabled,
  'devices.monitor_usb': config.value?.devices?.monitor_usb,
  'security.enabled': config.value?.security?.enabled,
  'forge.enabled': config.value?.forge?.enabled,
  'mcp.enabled': config.value?.mcp?.enabled,
}))

function updateAgentForm(updated: Record<string, any>) {
  if ('agents.defaults.temperature' in updated) {
    saveField('agents.defaults.temperature', updated['agents.defaults.temperature'])
  }
  if ('agents.defaults.max_tokens' in updated) {
    saveField('agents.defaults.max_tokens', updated['agents.defaults.max_tokens'])
  }
  if ('agents.defaults.restrict_to_workspace' in updated) {
    saveField('agents.defaults.restrict_to_workspace', updated['agents.defaults.restrict_to_workspace'])
  }
}

function updateToolsForm(updated: Record<string, any>) {
  if ('tools.web.brave.enabled' in updated) {
    saveField('tools.web.brave.enabled', updated['tools.web.brave.enabled'])
  }
  if ('tools.web.duckduckgo.enabled' in updated) {
    saveField('tools.web.duckduckgo.enabled', updated['tools.web.duckduckgo.enabled'])
  }
  if ('tools.cron.exec_timeout_minutes' in updated) {
    saveField('tools.cron.exec_timeout_minutes', updated['tools.cron.exec_timeout_minutes'])
  }
}

function updateLoggingForm(updated: Record<string, any>) {
  if ('logging.general.enabled' in updated) {
    saveField('logging.general.enabled', updated['logging.general.enabled'])
  }
  if ('logging.general.enable_console' in updated) {
    saveField('logging.general.enable_console', updated['logging.general.enable_console'])
  }
  if ('logging.general.level' in updated) {
    saveField('logging.general.level', updated['logging.general.level'])
  }
  if ('logging.llm.enabled' in updated) {
    saveField('logging.llm.enabled', updated['logging.llm.enabled'])
  }
}

function updateServicesForm(updated: Record<string, any>) {
  if ('heartbeat.enabled' in updated) saveField('heartbeat.enabled', updated['heartbeat.enabled'])
  if ('devices.monitor_usb' in updated) saveField('devices.monitor_usb', updated['devices.monitor_usb'])
  if ('security.enabled' in updated) saveField('security.enabled', updated['security.enabled'])
  if ('forge.enabled' in updated) saveField('forge.enabled', updated['forge.enabled'])
  if ('mcp.enabled' in updated) saveField('mcp.enabled', updated['mcp.enabled'])
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
          <button v-for="t in tabs" :key="t.id" class="tab" :class="{ active: activeTab === t.id }" @click="setTab(t.id)">{{ t.label }}</button>
        </div>

        <div v-if="activeTab === 'tools-md'">
          <ToolsView embedded />
        </div>
        <div v-if="activeTab === 'tasks'">
          <TasksView embedded />
        </div>

        <!-- Agent config -->
        <div v-if="activeTab === 'agent'" class="card">
          <div class="card-header"><h3>Agent 配置</h3></div>
          <div class="card-body">
            <div class="form-group">
              <label class="form-label">默认模型</label>
              <div style="display: flex; align-items: center; gap: var(--space-2); flex-wrap: wrap;">
                <code style="font-size: var(--text-sm);">{{ config.agents?.defaults?.llm || '未配置' }}</code>
                <router-link class="btn btn-sm" to="/models">去模型页更换</router-link>
              </div>
            </div>
            <SmartFieldForm
              :model-value="agentFormModel"
              :meta-table="SETTINGS_FIELD_META"
              @update:model-value="updateAgentForm"
            />
          </div>
        </div>

        <!-- Gateway config -->
        <div v-if="activeTab === 'gateway'" class="card">
          <div class="card-header"><h3>Gateway 配置</h3></div>
          <div class="card-body">
            <div class="settings-readonly">
              <div class="readonly-row">
                <span class="readonly-label">主机</span>
                <span class="readonly-value">{{ config.gateway?.host || '0.0.0.0' }}</span>
              </div>
              <div class="readonly-row">
                <span class="readonly-label">端口</span>
                <span class="readonly-value">{{ config.gateway?.port || 49000 }}</span>
              </div>
            </div>
          </div>
        </div>

        <!-- Tools config -->
        <div v-if="activeTab === 'tools'" class="card">
          <div class="card-header"><h3>工具配置</h3></div>
          <div class="card-body">
            <SmartFieldForm
              :model-value="toolsFormModel"
              :meta-table="SETTINGS_FIELD_META"
              @update:model-value="updateToolsForm"
            />
          </div>
        </div>

        <!-- Services toggles -->
        <div v-if="activeTab === 'services'" class="card">
          <div class="card-header"><h3>系统服务开关</h3></div>
          <div class="card-body">
            <SmartFieldForm
              :model-value="servicesFormModel"
              :meta-table="SETTINGS_FIELD_META"
              @update:model-value="updateServicesForm"
            />
          </div>
        </div>

        <!-- Logging -->
        <div v-if="activeTab === 'logging'" class="card">
          <div class="card-header"><h3>日志配置</h3></div>
          <div class="card-body">
            <SmartFieldForm
              :model-value="loggingFormModel"
              :meta-table="SETTINGS_FIELD_META"
              @update:model-value="updateLoggingForm"
            />
          </div>
        </div>

        <!-- CORS -->
        <div v-if="activeTab === 'cors'">
          <div class="card" style="margin-bottom: var(--space-4);">
            <div class="card-header">
              <h3>CORS 管理</h3>
              <span class="badge badge-neutral">仅 CLI</span>
            </div>
            <div class="card-body">
              <div class="empty-state" style="padding: var(--space-6);">
                <h3>请在终端中管理 CORS</h3>
                <p style="margin-top: var(--space-2);">
                  当前 Dashboard 无法通过 WebSocket 修改 CORS。请使用：
                </p>
                <pre style="margin-top: var(--space-3); text-align: left; padding: var(--space-3); background: var(--bg-secondary); border-radius: var(--radius-md); font-family: var(--font-mono); font-size: var(--text-sm); overflow-x: auto;">nemesisbot cors</pre>
              </div>
            </div>
          </div>
        </div>

        <!-- Raw JSON (advanced only) -->
        <div v-if="activeTab === 'raw'">
          <div class="card">
            <div class="card-header">
              <h3>原始配置 (进阶)</h3>
              <div style="display: flex; gap: var(--space-2);">
                <template v-if="!editing">
                  <button class="btn btn-sm" @click="editing = true; editConfig = JSON.stringify(config, null, 2)">编辑 JSON</button>
                </template>
                <template v-else>
                  <button class="btn btn-sm" @click="editing = false">取消</button>
                  <button class="btn btn-sm btn-primary" @click="saveFullConfig">保存</button>
                </template>
              </div>
            </div>
            <div class="card-body" style="padding-bottom: 0;">
              <p class="form-hint">日常请用上方各 Tab 与开关。仅在排查问题时直接改 JSON。</p>
            </div>
            <div class="card-body">
              <div v-if="editing">
                <div style="padding: var(--space-3); margin-bottom: var(--space-3); background: var(--warning-bg); border: 1px solid var(--warning); border-radius: var(--radius-md); font-size: var(--text-sm); color: var(--text-secondary);">
                  注意：敏感字段（如 API Key、Token）已被遮蔽显示（含 **** ）。如需修改，请将遮蔽值替换为真实值；如保持遮蔽值不变，保存后该字段将被覆盖为遮蔽值。
                </div>
                <textarea class="form-textarea" style="min-height: 60vh; font-family: var(--font-mono); font-size: var(--text-xs);" v-model="editConfig"></textarea>
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

<style scoped>
.settings-readonly {
  display: flex;
  flex-direction: column;
  gap: var(--space-3);
}

.readonly-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: var(--space-3) 0;
  border-bottom: 1px solid var(--border);
}

.readonly-row:last-child {
  border-bottom: none;
}

.readonly-label {
  font-size: var(--text-sm);
  font-weight: 600;
  color: var(--text);
}

.readonly-value {
  font-size: var(--text-sm);
  color: var(--text-muted);
  font-family: var(--font-mono);
}
</style>
