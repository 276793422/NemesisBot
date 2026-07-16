<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'
import { usePageTab } from '../lib/pageTab'
import ToolsView from './ToolsView.vue'
import TasksView from './TasksView.vue'

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
          <button v-for="t in tabs" :key="t.id" class="tab" :class="{ active: activeTab === t.id }" @click="setTab(t.id)">{{ t.label }}</button>
        </div>

        <div v-if="activeTab === 'tools-md'">
          <ToolsView embedded />
        </div>
        <div v-if="activeTab === 'tasks'">
          <TasksView embedded />
        </div>

        <!-- Agent config: presets instead of raw temperature numbers -->
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
            <div class="form-group">
              <label class="form-label">回复风格</label>
              <div style="display: flex; flex-wrap: wrap; gap: var(--space-2);">
                <button
                  type="button"
                  class="btn btn-sm"
                  :class="{ 'btn-primary': Math.abs((config.agents?.defaults?.temperature ?? 0.7) - 0.2) < 0.05 }"
                  @click="saveField('agents.defaults.temperature', 0.2)"
                >严谨</button>
                <button
                  type="button"
                  class="btn btn-sm"
                  :class="{ 'btn-primary': Math.abs((config.agents?.defaults?.temperature ?? 0.7) - 0.7) < 0.05 }"
                  @click="saveField('agents.defaults.temperature', 0.7)"
                >均衡</button>
                <button
                  type="button"
                  class="btn btn-sm"
                  :class="{ 'btn-primary': Math.abs((config.agents?.defaults?.temperature ?? 0.7) - 1.2) < 0.05 }"
                  @click="saveField('agents.defaults.temperature', 1.2)"
                >创意</button>
              </div>
              <span class="form-hint">对应温度约 0.2 / 0.7 / 1.2，无需手填小数</span>
            </div>
            <div class="form-group">
              <label class="form-label">回复长度上限</label>
              <div style="display: flex; flex-wrap: wrap; gap: var(--space-2);">
                <button type="button" class="btn btn-sm" :class="{ 'btn-primary': (config.agents?.defaults?.max_tokens ?? 4096) <= 2048 }" @click="saveField('agents.defaults.max_tokens', 2048)">短</button>
                <button type="button" class="btn btn-sm" :class="{ 'btn-primary': (config.agents?.defaults?.max_tokens ?? 4096) > 2048 && (config.agents?.defaults?.max_tokens ?? 4096) <= 4096 }" @click="saveField('agents.defaults.max_tokens', 4096)">中</button>
                <button type="button" class="btn btn-sm" :class="{ 'btn-primary': (config.agents?.defaults?.max_tokens ?? 4096) > 4096 }" @click="saveField('agents.defaults.max_tokens', 8192)">长</button>
              </div>
            </div>
            <div class="form-group">
              <label class="form-label">限制在工作空间内操作</label>
              <div class="toggle" :class="{ active: config.agents?.defaults?.restrict_to_workspace !== false }"
                @click="toggleService('agents.defaults.restrict_to_workspace', config.agents?.defaults?.restrict_to_workspace !== false)"></div>
              <span class="form-hint" style="margin-left: var(--space-2);">{{ config.agents?.defaults?.restrict_to_workspace !== false ? '已启用（推荐）' : '已关闭' }}</span>
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
              <label class="form-label">Brave 搜索</label>
              <div class="toggle" :class="{ active: config.tools?.web?.brave?.enabled === true }"
                @click="toggleService('tools.web.brave.enabled', config.tools?.web?.brave?.enabled === true)"></div>
              <span class="form-hint" style="margin-left: var(--space-2);">{{ config.tools?.web?.brave?.enabled === true ? '已启用' : '已禁用' }}</span>
            </div>
            <div class="form-group">
              <label class="form-label">DuckDuckGo 搜索</label>
              <div class="toggle" :class="{ active: config.tools?.web?.duckduckgo?.enabled === true }"
                @click="toggleService('tools.web.duckduckgo.enabled', config.tools?.web?.duckduckgo?.enabled === true)"></div>
              <span class="form-hint" style="margin-left: var(--space-2);">{{ config.tools?.web?.duckduckgo?.enabled === true ? '已启用' : '已禁用' }}</span>
            </div>
            <div class="form-group">
              <label class="form-label">Cron 执行超时（分钟）</label>
              <input class="form-input" type="number" :value="config.tools?.cron?.exec_timeout_minutes ?? 60"
                @change="(e: any) => saveField('tools.cron.exec_timeout_minutes', parseInt(e.target.value))" style="max-width: 200px;">
            </div>
          </div>
        </div>

        <!-- Services toggles -->
        <div v-if="activeTab === 'services'" class="card">
          <div class="card-header"><h3>系统服务开关</h3></div>
          <div class="card-body">
            <div v-for="svc in [
              { label: 'Heartbeat', path: 'heartbeat.enabled', value: config.heartbeat?.enabled },
              { label: 'USB 监控', path: 'devices.monitor_usb', value: config.devices?.monitor_usb },
              { label: 'Security', path: 'security.enabled', value: config.security?.enabled },
              { label: 'Forge', path: 'forge.enabled', value: config.forge?.enabled },
              { label: 'MCP', path: 'mcp.enabled', value: config.mcp?.enabled },
            ]" :key="svc.path"
              style="display: flex; align-items: center; justify-content: space-between; padding: var(--space-3) 0; border-bottom: 1px solid var(--border-light);">
              <span style="font-size: var(--text-sm); font-weight: 500;">{{ svc.label }}</span>
              <div class="toggle" :class="{ active: svc.value !== false }" @click="toggleService(svc.path, svc.value !== false)"></div>
            </div>
          </div>
        </div>

        <!-- Logging -->
        <div v-if="activeTab === 'logging'" class="card">
          <div class="card-header"><h3>日志配置</h3></div>
          <div class="card-body">
            <div class="form-group">
              <label class="form-label">通用日志</label>
              <div class="toggle" :class="{ active: config.logging?.general?.enabled !== false }"
                @click="toggleService('logging.general.enabled', config.logging?.general?.enabled !== false)"></div>
              <span class="form-hint" style="margin-left: var(--space-2);">{{ config.logging?.general?.enabled !== false ? '已启用' : '已禁用' }}</span>
            </div>
            <div class="form-group">
              <label class="form-label">控制台输出</label>
              <div class="toggle" :class="{ active: config.logging?.general?.enable_console !== false }"
                @click="toggleService('logging.general.enable_console', config.logging?.general?.enable_console !== false)"></div>
              <span class="form-hint" style="margin-left: var(--space-2);">{{ config.logging?.general?.enable_console !== false ? '已启用' : '已禁用' }}</span>
            </div>
            <div class="form-group">
              <label class="form-label">日志级别</label>
              <select class="form-select" style="max-width: 200px;"
                :value="config.logging?.general?.level || 'info'"
                @change="(e: any) => saveField('logging.general.level', e.target.value)">
                <option value="debug">DEBUG</option>
                <option value="info">INFO</option>
                <option value="warn">WARN</option>
                <option value="error">ERROR</option>
              </select>
            </div>
            <div class="form-group">
              <label class="form-label">LLM 通信日志</label>
              <div class="toggle" :class="{ active: config.logging?.llm?.enabled === true }"
                @click="toggleService('logging.llm.enabled', config.logging?.llm?.enabled === true)"></div>
              <span class="form-hint" style="margin-left: var(--space-2);">{{ config.logging?.llm?.enabled === true ? '已启用' : '已禁用' }}</span>
            </div>
          </div>
        </div>

        <!-- CORS: honest CLI-only guidance — no half-disabled fake controls -->
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

        <!-- Raw JSON (advanced only — discouraged for normal use) -->
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
                <div style="padding: var(--space-3); margin-bottom: var(--space-3); background: var(--warning-bg, #fef3cd); border: 1px solid var(--warning, #e5a00d); border-radius: var(--radius-md); font-size: var(--text-sm); color: var(--text-secondary);">
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
