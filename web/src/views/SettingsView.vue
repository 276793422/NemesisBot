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

// Hooks state (P4 — hooks.json CC dialect editor)
const hooksContent = ref('')
const hooksExists = ref(false)
const hooksValid = ref(true)
const hooksError = ref<string | null>(null)
const hooksSummary = ref<Record<string, number> | null>(null)
const hooksSaving = ref(false)
const hooksRestarting = ref(false)

const tabs = [
  { id: 'hooks', label: 'Hooks' },
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

// Hooks functions (P4)
async function loadHooks() {
  try {
    const data = await request('hooks', 'get')
    hooksContent.value = data?.content ?? ''
    hooksExists.value = !!data?.exists
    hooksValid.value = data?.valid !== false
    hooksError.value = data?.error || null
    hooksSummary.value = data?.summary || null
  } catch (e: any) {
    toast.error('加载 hooks.json 失败: ' + e)
  }
}

/** 保存：前端先做 JSON 语法自检（语义校验由后端 parse_cc_hooks 把关，
 * 校验失败不落盘）。 */
async function saveHooks() {
  try {
    JSON.parse(hooksContent.value)
  } catch (e: any) {
    toast.error('JSON 语法错误，未保存: ' + e?.message || e)
    return
  }
  hooksSaving.value = true
  try {
    const data = await request('hooks', 'set', { content: hooksContent.value })
    hooksSummary.value = data?.summary || null
    hooksExists.value = true
    hooksValid.value = true
    hooksError.value = null
    toast.success(`已保存（${data?.summary?.total ?? 0} 个脚本）。重启 Agent 后生效`)
  } catch (e: any) {
    // 后端语义校验拒绝 —— 错误串就是 parse 详情，文件未动。
    toast.error('保存被拒（文件未写入）: ' + e)
  }
  hooksSaving.value = false
}

/** hooks.json 只在 Agent 启动时加载（agent_factory），保存后一键重启生效。 */
async function restartAgentForHooks() {
  hooksRestarting.value = true
  try {
    await request('agent', 'stop')
    await new Promise(r => setTimeout(r, 1000))
    await request('agent', 'start')
    toast.success('Agent 已重启，hooks 配置已生效')
  } catch (e: any) {
    toast.error('重启 Agent 失败: ' + (e?.message || e))
  }
  hooksRestarting.value = false
}

onMounted(async () => {
  await Promise.all([loadConfig(), loadCors(), loadHooks()])
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

        <!-- Hooks (P4: hooks.json CC dialect editor) -->
        <div v-if="activeTab === 'hooks'">
          <div class="card" style="margin-bottom: var(--space-4);">
            <div class="card-header"><h3>Hooks 钩子说明</h3></div>
            <div class="card-body">
              <p style="font-size: var(--text-sm); margin: 0 0 var(--space-3);">
                hooks.json 采用 Claude Code 方言：每个 hook 是一条子进程脚本（stdin 收单行 JSON 事件、
                env <code>CLAUDE_PROJECT_DIR</code>、cwd 为 workspace），五个事件映射到 Agent 内核钩点：
              </p>
              <table class="hooks-table">
                <thead><tr><th>CC 事件</th><th>触发时机</th><th>阻断语义（exit 2）</th></tr></thead>
                <tbody>
                  <tr><td>SessionStart</td><td>每会话首条 prompt 到达时</td><td>观察型（不阻断）</td></tr>
                  <tr><td>UserPromptSubmit</td><td>用户消息进入历史之前</td><td>拦下 prompt，模型永远看不到</td></tr>
                  <tr><td>PreToolUse</td><td>工具执行前（安全 8 层闸之后）</td><td>拦截工具调用，stderr 作理由回灌模型</td></tr>
                  <tr><td>PostToolUse</td><td>工具执行后（Forge 记录前）</td><td>stderr 追加到结果反馈模型（不撤销已执行操作）</td></tr>
                  <tr><td>Stop</td><td>最终答案被接受后、轮次结束前</td><td>阻止收尾，stderr 作反馈再答一轮（每轮封顶 2 次）</td></tr>
                </tbody>
              </table>
              <p style="font-size: var(--text-sm); margin: var(--space-3) 0 0; color: var(--text-muted);">
                脚本协议：退出码 0 = 放行（PreToolUse/Stop 还认 stdout JSON <code>{"decision":"block","reason":…}</code>）；
                2 = 阻断；其他/超时/启动失败 = 非阻断错误（fail-open）。matcher 用 CC 工具名
                （Bash/Edit/Write/Read/Grep，本系统工具名自动映射别名），省略 = 全命中。每脚本可设
                <code>timeout</code> 秒（默认 60）。LLM 调用级钩子（K1b）无 CC 对应事件，不在此文件配置。
              </p>
            </div>
          </div>
          <div class="card">
            <div class="card-header">
              <h3>hooks.json<span v-if="hooksSummary" style="font-weight: 400; font-size: var(--text-sm); color: var(--text-muted);">　当前 {{ hooksSummary.total }} 个脚本</span></h3>
              <div style="display: flex; gap: var(--space-2);">
                <button class="btn btn-sm" @click="loadHooks" :disabled="hooksSaving">重载</button>
                <button class="btn btn-sm btn-primary" @click="saveHooks" :disabled="hooksSaving">
                  {{ hooksSaving ? '保存中…' : '校验并保存' }}
                </button>
                <button class="btn btn-sm" @click="restartAgentForHooks" :disabled="hooksRestarting">
                  {{ hooksRestarting ? '重启中…' : '重启 Agent 生效' }}
                </button>
              </div>
            </div>
            <div class="card-body">
              <div v-if="!hooksValid" style="padding: var(--space-3); margin-bottom: var(--space-3); background: var(--danger-bg, #fdecea); border: 1px solid var(--danger, #dc3545); border-radius: var(--radius-md); font-size: var(--text-sm);">
                磁盘上的文件解析失败（Agent 启动时已跳过加载，fail-open）：{{ hooksError }}<br>
                在下方修正后保存即可恢复。
              </div>
              <div v-else-if="!hooksExists" style="padding: var(--space-3); margin-bottom: var(--space-3); background: var(--accent-muted, #e8f0fe); border: 1px solid var(--border-light); border-radius: var(--radius-md); font-size: var(--text-sm);">
                尚未配置 hooks.json —— 下方为空模板（五个事件全空，可直接编辑）。保存后创建文件。
              </div>
              <textarea class="form-textarea" style="min-height: 50vh; font-family: var(--font-mono); font-size: var(--text-xs);" v-model="hooksContent"></textarea>
              <p class="form-hint" style="margin-top: var(--space-2);">
                保存时后端做 CC 方言语义校验，校验失败不落盘。hooks.json 在 Agent 启动时加载——保存后点「重启 Agent 生效」。
              </p>
            </div>
          </div>
        </div>

        <!-- Agent config -->
        <div v-if="activeTab === 'agent'" class="card">
          <div class="card-header"><h3>Agent 配置</h3></div>
          <div class="card-body">
            <div class="form-group">
              <label class="form-label">默认模型</label>
              <input class="form-input" :value="config.agents?.defaults?.llm || '--'" disabled style="max-width: 300px;">
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
              <div class="toggle" :class="{ active: config.agents?.defaults?.restrict_to_workspace !== false }"
                @click="toggleService('agents.defaults.restrict_to_workspace', config.agents?.defaults?.restrict_to_workspace !== false)"></div>
              <span class="form-hint" style="margin-left: var(--space-2);">{{ config.agents?.defaults?.restrict_to_workspace !== false ? '已启用' : '已禁用' }}</span>
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

        <!-- CORS -->
        <div v-if="activeTab === 'cors'">
          <div class="card" style="margin-bottom: var(--space-4);">
            <div class="card-header">
              <h3>CORS 管理</h3>
              <div class="toggle" :class="{ active: corsEnabled }" @click="toggleCors(!corsEnabled)"></div>
            </div>
            <div class="card-body">
              <div style="padding: var(--space-3); margin-bottom: var(--space-4); background: var(--accent-muted, #e8f0fe); border: 1px solid var(--border-light); border-radius: var(--radius-md); font-size: var(--text-sm); color: var(--text-secondary);">
                CORS 管理功能当前通过 WebSocket API 不可用，请使用 CLI 命令 <code>nemesisbot cors</code> 进行管理。
              </div>
              <div style="display: flex; gap: var(--space-2); margin-bottom: var(--space-4);">
                <input class="form-input" v-model="newOrigin" placeholder="例如: http://localhost:3000" style="max-width: 400px;" disabled>
                <button class="btn btn-primary" @click="addCorsOrigin" disabled>添加</button>
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

<style scoped>
/* P4 Hooks tab: CC 5-event mapping table */
.hooks-table {
  width: 100%;
  border-collapse: collapse;
  font-size: var(--text-sm);
}
.hooks-table th,
.hooks-table td {
  padding: 6px 10px;
  border: 1px solid var(--border-light);
  text-align: left;
  vertical-align: top;
}
.hooks-table th {
  background: var(--bg-secondary);
  font-weight: 600;
}
.hooks-table td:first-child {
  white-space: nowrap;
  font-family: var(--font-mono);
}
</style>
