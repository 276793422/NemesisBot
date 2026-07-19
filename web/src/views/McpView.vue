<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'
import { MCP_PRESETS, type McpPreset } from '../lib/mcpPresets'

defineProps<{ embedded?: boolean }>()

const { request } = useWSAPI()
const toast = useToast()

interface McpServer {
  name: string
  transport_type: string
  url: string
  description: string
  headers: string[]
  args: string[]
  env: string[]
  timeout: number
  provider_name: string
  provider_url: string
  tags: string[]
}

const servers = ref<McpServer[]>([])
const enabled = ref(false)
const loading = ref(true)
const showAddDialog = ref(false)
const activeTab = ref('servers')
const showDetailDialog = ref(false)
const editingServer = ref<string | null>(null)
const detailServer = ref<McpServer | null>(null)
const confirmDeleteName = ref('')

const TRANSPORT_TYPES = [
  { id: 'stdio', name: 'STDIO', desc: '本地命令行进程' },
  { id: 'http', name: 'HTTP', desc: 'HTTP POST 接口' },
  { id: 'sse', name: 'SSE', desc: 'Server-Sent Events' },
]

const defaultForm = () => ({
  name: '',
  transport_type: 'stdio' as string,
  url: '',
  description: '',
  headersText: '',
  argsText: '',
  envText: '',
  timeout: 30,
  provider_name: '',
  provider_url: '',
  tagsText: '',
})
const form = ref(defaultForm())
const presetId = ref('filesystem')
const envValues = ref<Record<string, string>>({})
const showAdvanced = ref(false)

const isStdio = computed(() => form.value.transport_type === 'stdio')
const currentPreset = computed(() => MCP_PRESETS.find((p) => p.id === presetId.value))

function applyPreset(p: McpPreset) {
  presetId.value = p.id
  if (p.id === 'custom') {
    showAdvanced.value = true
    form.value = defaultForm()
    envValues.value = {}
    return
  }
  showAdvanced.value = false
  form.value = {
    ...defaultForm(),
    name: p.id,
    transport_type: p.transport_type,
    url: p.url,
    description: p.description,
    argsText: (p.args || []).join(' '),
    tagsText: (p.tags || []).join(', '),
  }
  const ev: Record<string, string> = {}
  for (const k of p.envKeys || []) ev[k] = ''
  envValues.value = ev
}

async function loadStatus() {
  try {
    const data = await request('mcp', 'status')
    enabled.value = data?.enabled || false
  } catch { /* ignore */ }
}

async function loadServers() {
  try {
    const data = await request('mcp', 'servers')
    servers.value = data?.servers || []
  } catch (e: any) {
    toast.error('加载失败: ' + e)
  }
  loading.value = false
}

function openAdd() {
  editingServer.value = null
  applyPreset(MCP_PRESETS[0]!)
  showAddDialog.value = true
}

function openEdit(s: McpServer) {
  editingServer.value = s.name
  form.value = {
    name: s.name,
    transport_type: s.transport_type || 'stdio',
    url: s.url || '',
    description: s.description || '',
    headersText: (s.headers || []).join('\n'),
    argsText: (s.args || []).join(' '),
    envText: (s.env || []).join('\n'),
    timeout: s.timeout || 30,
    provider_name: s.provider_name || '',
    provider_url: s.provider_url || '',
    tagsText: (s.tags || []).join(', '),
  }
  showAddDialog.value = true
}

function showDetail(s: McpServer) {
  detailServer.value = s
  showDetailDialog.value = true
}

async function saveServer() {
  // Merge envValues from preset keys into envText
  const envLines = Object.entries(envValues.value)
    .filter(([, v]) => v.trim())
    .map(([k, v]) => `${k}=${v.trim()}`)
  if (envLines.length) {
    form.value.envText = [form.value.envText, ...envLines].filter(Boolean).join('\n')
  }
  if (!form.value.name || !form.value.url) {
    toast.warn('请选择模板或填写名称与命令')
    return
  }
  const missingEnv = (currentPreset.value?.envKeys || []).filter((k) => !envValues.value[k]?.trim())
  if (missingEnv.length && presetId.value !== 'custom') {
    toast.warn('请填写：' + missingEnv.join(', '))
    return
  }
  const payload: any = {
    name: form.value.name,
    transport_type: form.value.transport_type,
    url: form.value.url,
    description: form.value.description,
    timeout: form.value.timeout,
    provider_name: form.value.provider_name,
    provider_url: form.value.provider_url,
  }
  if (isStdio.value) {
    payload.args = form.value.argsText ? form.value.argsText.split(/\s+/).filter(Boolean) : []
    payload.env = form.value.envText ? form.value.envText.split('\n').map((l: string) => l.trim()).filter(Boolean) : []
  } else {
    payload.headers = form.value.headersText ? form.value.headersText.split('\n').map((l: string) => l.trim()).filter(Boolean) : []
  }
  if (form.value.tagsText) {
    payload.tags = form.value.tagsText.split(',').map((t: string) => t.trim()).filter(Boolean)
  }

  try {
    if (editingServer.value) {
      await request('mcp', 'server.update', payload)
      toast.success('已更新')
    } else {
      await request('mcp', 'server.add', payload)
      toast.success('已添加')
    }
    showAddDialog.value = false
    await loadServers()
  } catch (e: any) {
    toast.error((editingServer.value ? '更新' : '添加') + '失败: ' + e)
  }
}

async function deleteServer(name: string) {
  try {
    await request('mcp', 'server.delete', { name })
    toast.success('已删除')
    confirmDeleteName.value = ''
    showDetailDialog.value = false
    await loadServers()
  } catch (e: any) {
    toast.error('删除失败: ' + e)
  }
}

function transportBadge(type: string) {
  switch (type) {
    case 'http': return 'badge-success'
    case 'sse': return 'badge-warning'
    default: return 'badge-info'
  }
}

function transportColor(type: string) {
  switch (type) {
    case 'http': return 'var(--success)'
    case 'sse': return 'var(--warning)'
    default: return 'var(--info)'
  }
}

onMounted(async () => {
  await Promise.all([loadStatus(), loadServers()])
})
</script>

<template>
  <div :class="embedded ? 'mcp-embed' : 'page-mcp'">
    <div v-if="!embedded" class="page-header">
      <h2>MCP 管理</h2>
      <div class="page-header-actions">
        <span class="badge" :class="enabled ? 'badge-success' : 'badge-neutral'">{{ enabled ? '已启用' : '未启用' }}</span>
      </div>
    </div>
    <div v-else style="display: flex; justify-content: flex-end; margin-bottom: var(--space-3);">
      <span class="badge" :class="enabled ? 'badge-success' : 'badge-neutral'">{{ enabled ? '已启用' : '未启用' }}</span>
    </div>
    <div :class="embedded ? '' : 'page-body'">
      <!-- ==================== 服务器 ==================== -->
      <div>
      <!-- Server cards -->
      <div v-if="loading" style="text-align: center; padding: var(--space-8);">
        <div class="spinner spinner-lg" style="margin: 0 auto;"></div>
      </div>
      <div v-if="!loading && servers.length === 0" class="empty-state">
        <h3>暂无 MCP 服务器</h3>
        <p>点击下方按钮添加第一个 MCP 服务器</p>
        <button class="btn btn-primary" style="margin-top: var(--space-3);" @click="openAdd">添加服务器</button>
      </div>
      <div v-if="!loading && servers.length > 0">
        <div style="display: grid; grid-template-columns: repeat(auto-fill, minmax(320px, 1fr)); gap: var(--space-4);">
          <div v-for="s in servers" :key="s.name" class="skill-card" style="cursor: pointer;" @click="showDetail(s)">
            <div class="skill-card-header">
              <div class="skill-name">{{ s.name }}</div>
              <span class="badge" :class="transportBadge(s.transport_type)">{{ (s.transport_type || 'stdio').toUpperCase() }}</span>
            </div>
            <div class="skill-description" style="display: -webkit-box; -webkit-line-clamp: 2; -webkit-box-orient: vertical; overflow: hidden;">
              {{ s.description || (s.transport_type === 'stdio' ? s.url + ' ' + (s.args || []).join(' ') : s.url) }}
            </div>
            <div style="display: flex; gap: var(--space-2); align-items: center; margin-top: var(--space-3);">
              <code style="font-size: var(--text-xs); color: var(--text-muted); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; flex: 1;">{{ s.url || '--' }}</code>
            </div>
            <div style="display: flex; gap: var(--space-2); align-items: center; margin-top: var(--space-2);">
              <span v-if="s.timeout" class="badge badge-neutral" style="font-size: 0.65rem;">⏱ {{ s.timeout }}s</span>
              <span v-if="s.provider_name" style="font-size: var(--text-xs); color: var(--text-muted);">{{ s.provider_name }}</span>
            </div>
            <div v-if="s.tags && s.tags.length" style="display: flex; gap: 4px; flex-wrap: wrap; margin-top: var(--space-2);">
              <span v-for="t in s.tags" :key="t" class="badge badge-neutral" style="font-size: 0.6rem;">{{ t }}</span>
            </div>
          </div>
        </div>
        <div style="margin-top: var(--space-4); text-align: center;">
          <button class="btn btn-primary" @click="openAdd">添加服务器</button>
        </div>
      </div>

      <!-- Add/Edit dialog -->
      <div v-if="showAddDialog" class="modal-backdrop" @click.self="showAddDialog = false">
        <div class="modal" style="max-width: 540px;">
          <div class="modal-header">
            <h3>{{ editingServer ? '编辑' : '添加' }} MCP 服务器</h3>
            <button class="modal-close" @click="showAddDialog = false">&times;</button>
          </div>
          <div class="modal-body">
            <template v-if="!editingServer">
              <div class="form-group">
                <label class="form-label">选择模板</label>
                <div class="preset-list">
                  <button
                    v-for="p in MCP_PRESETS"
                    :key="p.id"
                    type="button"
                    class="preset-card"
                    :class="{ active: presetId === p.id }"
                    @click="applyPreset(p)"
                  >
                    <div class="preset-card-header">
                      <strong class="preset-card-name">{{ p.label }}</strong>
                      <span v-if="presetId === p.id" class="preset-card-check">✓</span>
                    </div>
                    <span class="preset-card-desc">{{ p.description }}</span>
                  </button>
                </div>
              </div>
              <div v-for="k in (currentPreset?.envKeys || [])" :key="k" class="form-group">
                <label class="form-label">{{ k }}</label>
                <input class="form-input" type="password" v-model="envValues[k]" :placeholder="'粘贴 ' + k" style="width: 100%;" autocomplete="off">
              </div>
            </template>
            <button type="button" class="btn btn-sm" style="margin-bottom: var(--space-3);" @click="showAdvanced = !showAdvanced">
              {{ showAdvanced ? '收起技术选项' : '显示技术选项（进阶）' }}
            </button>
            <div v-if="showAdvanced || editingServer">
              <div class="form-group">
                <label class="form-label">名称 *</label>
                <input class="form-input" v-model="form.name" placeholder="例如: filesystem" :disabled="!!editingServer" style="width: 100%;">
              </div>
              <div class="form-group">
                <label class="form-label">类型 *</label>
                <div style="display: flex; gap: var(--space-2);">
                  <button v-for="t in TRANSPORT_TYPES" :key="t.id" type="button" class="transport-btn" :class="{ active: form.transport_type === t.id }" @click="form.transport_type = t.id" :title="t.desc">{{ t.name }}</button>
                </div>
              </div>
              <div class="form-group">
                <label class="form-label">{{ isStdio ? '命令 *' : 'URL *' }}</label>
                <input class="form-input" v-model="form.url" :placeholder="isStdio ? 'npx' : 'https://…'" style="width: 100%;">
              </div>
              <div v-if="isStdio" class="form-group">
                <label class="form-label">参数</label>
                <input class="form-input" v-model="form.argsText" style="width: 100%;">
              </div>
              <div v-if="isStdio" class="form-group">
                <label class="form-label">环境变量</label>
                <textarea class="form-textarea" v-model="form.envText" rows="2" style="width: 100%;" placeholder="KEY=value，每行一个"></textarea>
              </div>
              <div class="form-group">
                <label class="form-label">超时</label>
                <div class="timeout-slider">
                  <input type="range" class="nice-slider" min="5" max="120" step="5" v-model.number="form.timeout" style="flex: 1;" />
                  <span class="slider-value">{{ form.timeout }}s</span>
                </div>
              </div>
            </div>
          </div>
          <div class="modal-footer">
            <button class="btn btn-sm" @click="showAddDialog = false">取消</button>
            <button class="btn btn-sm btn-primary" @click="saveServer" :disabled="!form.name || !form.url">{{ editingServer ? '保存' : '添加' }}</button>
          </div>
        </div>
      </div>

      <!-- Detail dialog -->
      <div v-if="showDetailDialog && detailServer" class="modal-backdrop" @click.self="showDetailDialog = false">
        <div class="modal" style="max-width: 560px;">
          <div class="modal-header">
            <h3>{{ detailServer.name }}</h3>
            <button class="modal-close" @click="showDetailDialog = false">&times;</button>
          </div>
          <div class="modal-body">
            <div style="display: flex; gap: var(--space-2); align-items: center; flex-wrap: wrap; margin-bottom: var(--space-3);">
              <span class="badge" :class="transportBadge(detailServer.transport_type)">{{ (detailServer.transport_type || 'stdio').toUpperCase() }}</span>
              <span v-if="detailServer.timeout" style="font-size: var(--text-xs); color: var(--text-muted);">超时 {{ detailServer.timeout }}s</span>
            </div>
            <div v-if="detailServer.description" style="margin-bottom: var(--space-3);">
              <p>{{ detailServer.description }}</p>
            </div>
            <div style="margin-bottom: var(--space-3);">
              <div style="font-size: var(--text-xs); color: var(--text-muted); margin-bottom: 4px;">{{ detailServer.transport_type === 'stdio' ? '命令' : 'URL' }}</div>
              <code style="word-break: break-all; font-size: var(--text-sm);">{{ detailServer.url }}</code>
            </div>
            <div v-if="detailServer.args && detailServer.args.length" style="margin-bottom: var(--space-3);">
              <div style="font-size: var(--text-xs); color: var(--text-muted); margin-bottom: 4px;">参数</div>
              <code style="font-size: var(--text-sm);">{{ detailServer.args.join(' ') }}</code>
            </div>
            <div v-if="detailServer.env && detailServer.env.length" style="margin-bottom: var(--space-3);">
              <div style="font-size: var(--text-xs); color: var(--text-muted); margin-bottom: 4px;">环境变量</div>
              <code style="font-size: var(--text-xs); display: block; white-space: pre-wrap;">{{ detailServer.env.join('\n') }}</code>
            </div>
            <div v-if="detailServer.headers && detailServer.headers.length" style="margin-bottom: var(--space-3);">
              <div style="font-size: var(--text-xs); color: var(--text-muted); margin-bottom: 4px;">请求头</div>
              <code style="font-size: var(--text-xs); display: block; white-space: pre-wrap;">{{ detailServer.headers.join('\n') }}</code>
            </div>
            <div v-if="detailServer.provider_name || detailServer.provider_url" style="margin-bottom: var(--space-3);">
              <div style="font-size: var(--text-xs); color: var(--text-muted); margin-bottom: 4px;">供应商</div>
              <span>{{ detailServer.provider_name }}</span>
              <a v-if="detailServer.provider_url" :href="detailServer.provider_url" target="_blank" style="font-size: var(--text-xs); margin-left: var(--space-2);">{{ detailServer.provider_url }}</a>
            </div>
            <div v-if="detailServer.tags && detailServer.tags.length" style="margin-bottom: var(--space-3);">
              <div style="display: flex; gap: 4px; flex-wrap: wrap;">
                <span v-for="t in detailServer.tags" :key="t" class="badge badge-neutral" style="font-size: 0.7rem;">{{ t }}</span>
              </div>
            </div>
            <div style="display: flex; gap: var(--space-2); margin-top: var(--space-4);">
              <button class="btn btn-primary" style="flex: 1;" @click="showDetailDialog = false; openEdit(detailServer!)">编辑</button>
              <template v-if="confirmDeleteName === detailServer.name">
                <span style="font-size: var(--text-xs); color: var(--error); align-self: center;">确定？</span>
                <button class="btn btn-sm btn-danger" @click="deleteServer(detailServer!.name)">确认</button>
                <button class="btn btn-sm" @click="confirmDeleteName = ''">取消</button>
              </template>
              <button v-else class="btn btn-danger" @click="confirmDeleteName = detailServer!.name">删除</button>
            </div>
          </div>
        </div>
      </div>
      </div>

    </div>
  </div>
</template>

<style scoped>
.form-group {
  margin-bottom: var(--space-3);
}
.form-label {
  display: block;
  font-size: var(--text-sm);
  font-weight: 500;
  color: var(--text-secondary);
  margin-bottom: var(--space-1);
}
.modal-footer {
  display: flex;
  justify-content: flex-end;
  gap: var(--space-2);
  padding: var(--space-3) var(--space-4);
  border-top: 1px solid var(--border);
}

/* ===== Preset List ===== */
.preset-list {
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
}

.preset-card {
  display: flex;
  flex-direction: column;
  gap: 2px;
  align-items: flex-start;
  text-align: left;
  width: 100%;
  padding: var(--space-3) var(--space-4);
  border-radius: var(--radius-md);
  border: 1px solid var(--border);
  background: var(--surface);
  color: var(--text-secondary);
  cursor: pointer;
  transition: all var(--duration-fast);
  font-family: var(--font-sans);
}

.preset-card:hover {
  border-color: var(--text-muted);
  background: var(--surface-hover);
}

.preset-card.active {
  border-color: var(--accent);
  background: var(--accent-muted);
  color: var(--accent);
  box-shadow: 0 0 0 1px rgba(232, 112, 90, 0.15);
}

.preset-card-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  width: 100%;
}

.preset-card-name {
  font-size: var(--text-sm);
  font-weight: 600;
  color: inherit;
}

.preset-card-check {
  font-size: var(--text-sm);
  font-weight: 700;
  color: var(--accent);
}

.preset-card-desc {
  font-size: var(--text-xs);
  opacity: 0.75;
  font-weight: 400;
}

/* ===== Transport Buttons ===== */
.transport-btn {
  padding: 6px 16px;
  border-radius: var(--radius-md);
  font-size: var(--text-xs);
  font-weight: 600;
  cursor: pointer;
  background: var(--surface);
  border: 1px solid var(--border);
  color: var(--text-muted);
  transition: all var(--duration-fast);
  font-family: var(--font-sans);
}

.transport-btn:hover {
  border-color: var(--accent);
  color: var(--text);
}

.transport-btn.active {
  background: var(--accent);
  border-color: var(--accent);
  color: #fff;
}

/* ===== Timeout Slider ===== */
.timeout-slider {
  display: flex;
  align-items: center;
  gap: var(--space-3);
}

.nice-slider {
  -webkit-appearance: none;
  appearance: none;
  height: 6px;
  background: var(--border);
  border-radius: var(--radius-full);
  outline: none;
  cursor: pointer;
}

.nice-slider::-webkit-slider-thumb {
  -webkit-appearance: none;
  appearance: none;
  width: 20px;
  height: 20px;
  background: var(--accent);
  border-radius: 50%;
  cursor: pointer;
  box-shadow: var(--shadow-sm);
  transition: transform var(--duration-fast), box-shadow var(--duration-fast);
}

.nice-slider::-webkit-slider-thumb:hover {
  transform: scale(1.15);
  box-shadow: 0 0 0 4px var(--accent-muted);
}

.nice-slider::-moz-range-thumb {
  width: 20px;
  height: 20px;
  background: var(--accent);
  border-radius: 50%;
  cursor: pointer;
  border: none;
  box-shadow: var(--shadow-sm);
}

.slider-value {
  font-size: var(--text-sm);
  font-weight: 600;
  color: var(--accent);
  background: var(--accent-muted);
  padding: 2px 10px;
  border-radius: var(--radius-sm);
  min-width: 50px;
  text-align: center;
}
</style>
