<script setup lang="ts">
import { ref, onMounted, onUnmounted } from 'vue'
import { useWSAPI } from '../composables/useWSAPI'
import { on as sseOn, off as sseOff } from '../composables/useSSE'
import { useToast } from '../composables/useToast'

defineProps<{ embedded?: boolean }>()

const { request } = useWSAPI()
const toast = useToast()

interface EngineState {
  install_status: string
  install_error: string
  last_install_attempt: string
  db_status: string
  last_db_update: string
}

interface ScannerEngine {
  name: string
  enabled: boolean
  url: string
  clamav_path: string
  address: string
  data_dir: string
  scan_on_write: boolean
  scan_on_download: boolean
  scan_on_exec: boolean
  update_interval: string
  max_file_size: number
  scan_extensions: string[]
  skip_extensions: string[]
  state: EngineState
  // frontend operation state
  operation: string
  progress: number
  progressMsg: string
}

const activeTab = ref('engines')
const engines = ref<ScannerEngine[]>([])
const loading = ref(true)
const scannerConfig = ref<any>({})
const editing = ref(false)
const editConfig = ref('')
let _onProgress: ((data: any) => void) | null = null

// --- Inline edit state ---
const editingKey = ref('')       // "engineName:fieldName"
const editOriginal = ref('')
const editValue = ref('')

function isEditing(engineName: string, field: string): boolean {
  return editingKey.value === `${engineName}:${field}`
}

function startEdit(engineName: string, field: string, currentVal: string) {
  editingKey.value = `${engineName}:${field}`
  editOriginal.value = currentVal || ''
  editValue.value = currentVal || ''
}

function cancelEdit() {
  editingKey.value = ''
}

function isDirty(): boolean {
  return editValue.value !== editOriginal.value
}

async function saveEdit(engineName: string, field: string) {
  const config: Record<string, string> = {}
  config[field] = editValue.value
  try {
    await request('scanner', 'engine.update_config', { name: engineName, config })
    toast.success('配置已保存')
    cancelEdit()
    await loadEngines()
  } catch (err: any) {
    toast.error('保存失败: ' + err)
  }
}

function handleClickOutside(e: MouseEvent) {
  if (!editingKey.value) return
  const target = e.target as HTMLElement
  if (target.closest('.inline-edit') || target.closest('.editable-val')) return
  cancelEdit()
}

// --- Display helpers ---

function getInstallBadge(s?: string): { text: string; cls: string } {
  switch (s) {
    case 'installed': return { text: '已安装', cls: 'badge-success' }
    case 'pending': return { text: '待安装', cls: 'badge-warning' }
    case 'failed': return { text: '安装失败', cls: 'badge-error' }
    default: return { text: '未配置', cls: 'badge-neutral' }
  }
}

function getDbBadge(s?: string): { text: string; cls: string } {
  switch (s) {
    case 'ready': return { text: '就绪', cls: 'badge-success' }
    case 'missing': return { text: '缺失', cls: 'badge-error' }
    case 'stale': return { text: '需更新', cls: 'badge-warning' }
    default: return { text: '--', cls: 'badge-neutral' }
  }
}

function formatTime(ts?: string): string {
  if (!ts) return '--'
  try {
    return new Date(ts).toLocaleString('zh-CN')
  } catch {
    return ts
  }
}

function getOpLabel(op: string): string {
  const map: Record<string, string> = {
    checking: '检查中...',
    installing: '安装中...',
    'updating-db': '更新数据库...',
    downloading: '下载中...',
    extracting: '解压中...',
    configuring: '配置中...',
    'downloading-db': '下载病毒库...',
  }
  return map[op] || op
}

function isBusy(e: ScannerEngine): boolean {
  return !!e.operation
}

// --- Data loading ---

async function loadEngines() {
  loading.value = true
  try {
    const data = await request('scanner', 'status')
    engines.value = (data?.engines || []).map((e: any) => ({
      ...e,
      state: e.state || { install_status: '', install_error: '', last_install_attempt: '', db_status: '', last_db_update: '' },
      operation: '',
      progress: 0,
      progressMsg: '',
    }))
  } catch (err) {
    console.error('[Scanner] Failed to load:', err)
  } finally {
    loading.value = false
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
    await loadEngines()
  } catch (e: any) {
    toast.error('保存失败: ' + e)
  }
}

// --- Engine operations ---

async function checkEngine(name: string) {
  const e = engines.value.find(x => x.name === name)
  if (!e) return
  e.operation = 'checking'
  e.progress = 0
  try {
    const data = await request('scanner', 'check', { name })
    if (data?.engines) {
      engines.value = data.engines.map((eng: any) => ({
        ...eng,
        state: eng.state || { install_status: '', install_error: '', last_install_attempt: '', db_status: '', last_db_update: '' },
        operation: '',
        progress: 0,
        progressMsg: '',
      }))
    } else if (data?.name) {
      updateSingleEngine(data)
    }
    toast.success(`${name} 状态检查完成`)
  } catch (err: any) {
    toast.error(`检查失败: ${err}`)
  } finally {
    clearOp(name)
  }
}

async function installEngine(name: string) {
  const e = engines.value.find(x => x.name === name)
  if (!e) return
  e.operation = 'installing'
  e.progress = 5
  e.progressMsg = '准备安装...'
  try {
    await request('scanner', 'install', { name })
    toast.info(`${name} 安装已在后台启动`)
  } catch (err: any) {
    toast.error(`安装失败: ${err}`)
    clearOp(name)
  }
}

async function updateDb(name: string) {
  const e = engines.value.find(x => x.name === name)
  if (!e) return
  e.operation = 'updating-db'
  e.progress = 5
  e.progressMsg = '准备更新...'
  try {
    await request('scanner', 'update_db', { name })
    toast.info(`${name} 数据库更新已在后台启动`)
  } catch (err: any) {
    toast.error(`更新失败: ${err}`)
    clearOp(name)
  }
}

async function testEngine(name: string) {
  const testPath = prompt('请输入要扫描的文件路径:')
  if (!testPath) return
  try {
    const result = await request('scanner', 'test', { name, path: testPath })
    if (result?.infected) {
      toast.warn(`检测到威胁: ${result.virus || '未知病毒'}`, 6000)
    } else {
      toast.success('扫描完成: 文件安全')
    }
  } catch (err: any) {
    toast.error(`测试失败: ${err}`)
  }
}

async function toggleEngine(name: string, enable: boolean) {
  try {
    const data = await request('scanner', enable ? 'enable' : 'disable', { name })
    if (data?.engines) {
      engines.value = data.engines.map((eng: any) => ({
        ...eng,
        state: eng.state || { install_status: '', install_error: '', last_install_attempt: '', db_status: '', last_db_update: '' },
        operation: '',
        progress: 0,
        progressMsg: '',
      }))
    }
    toast.success(enable ? `${name} 已启用` : `${name} 已禁用`)
  } catch (err: any) {
    toast.error(`操作失败: ${err}`)
  }
}

async function addClamav() {
  try {
    await request('scanner', 'add', { name: 'clamav' })
    toast.success('ClamAV 引擎已添加')
    await loadEngines()
  } catch (err: any) {
    toast.error(`添加失败: ${err}`)
  }
}

async function cancelOp(name: string) {
  try {
    await request('scanner', 'cancel', { name })
    toast.info(`${name} 操作已取消`)
    setTimeout(() => loadEngines(), 500)
  } catch (err: any) {
    toast.error(`取消失败: ${err}`)
  }
}

function clearOp(name: string) {
  const e = engines.value.find(x => x.name === name)
  if (e) {
    e.operation = ''
    e.progress = 0
    e.progressMsg = ''
  }
}

function updateSingleEngine(data: any) {
  const idx = engines.value.findIndex(e => e.name === data.name)
  if (idx >= 0) {
    engines.value[idx] = {
      ...engines.value[idx],
      ...data,
      state: data.state || engines.value[idx].state,
      operation: '',
      progress: 0,
      progressMsg: '',
    }
  }
}

// --- SSE progress handler ---

function handleProgress(data: any) {
  const e = engines.value.find(x => x.name === data.engine)
  if (!e) return
  if (data.phase === 'complete') {
    toast.success(data.message || `${e.name} 操作完成`)
    setTimeout(() => loadEngines(), 500)
    return
  }
  if (data.phase === 'error') {
    toast.error(data.message || `${e.name} 操作失败`)
    setTimeout(() => loadEngines(), 500)
    return
  }
  if (data.phase === 'cancelled') {
    toast.info(data.message || `${e.name} 操作已取消`)
    setTimeout(() => loadEngines(), 500)
    return
  }
  e.operation = data.phase || e.operation
  e.progress = data.progress || e.progress
  e.progressMsg = data.message || ''
}

// --- Lifecycle ---

onMounted(async () => {
  await Promise.all([loadEngines(), loadConfig()])
  loading.value = false

  _onProgress = handleProgress
  sseOn('scanner-progress', _onProgress)
  document.addEventListener('click', handleClickOutside)
})

onUnmounted(() => {
  if (_onProgress) sseOff('scanner-progress', _onProgress)
  document.removeEventListener('click', handleClickOutside)
})
</script>

<template>
  <div :class="embedded ? 'scanner-embed' : 'page-scanner'">
    <div v-if="!embedded" class="page-header"><h2>扫描器管理</h2></div>
    <div :class="embedded ? '' : 'page-body'">
      <div v-if="loading" style="text-align: center; padding: var(--space-8);">
        <div class="spinner spinner-lg" style="margin: 0 auto;"></div>
      </div>

      <div v-if="!loading">
        <div class="tabs">
          <button class="tab" :class="{ active: activeTab === 'engines' }" @click="activeTab = 'engines'">引擎状态</button>
          <button class="tab" :class="{ active: activeTab === 'config' }" @click="activeTab = 'config'">配置</button>
        </div>

        <!-- Engines Tab -->
        <div v-if="activeTab === 'engines'">
          <!-- Empty state -->
          <div v-if="engines.length === 0" class="empty-state">
            <div class="empty-state-icon">&#x1F6E1;&#xFE0F;</div>
            <h3>未检测到扫描引擎</h3>
            <p>点击下方按钮添加默认扫描引擎</p>
            <button class="btn btn-primary" @click="addClamav" style="margin-top: var(--space-4);">添加 ClamAV</button>
          </div>

          <!-- Engine cards -->
          <div v-if="engines.length > 0" class="engines-grid">
            <div v-for="engine in engines" :key="engine.name" class="card">
              <!-- Header -->
              <div class="card-header">
                <div style="display: flex; align-items: center; gap: var(--space-3);">
                  <h3>{{ engine.name.toUpperCase() }}</h3>
                  <span class="badge" :class="getInstallBadge(engine.state?.install_status).cls">
                    {{ getInstallBadge(engine.state?.install_status).text }}
                  </span>
                </div>
                <div style="display: flex; align-items: center; gap: var(--space-3);">
                  <span class="badge" :class="engine.enabled ? 'badge-success' : 'badge-neutral'">
                    {{ engine.enabled ? '已启用' : '已禁用' }}
                  </span>
                  <div class="toggle" :class="{ active: engine.enabled }"
                       @click="toggleEngine(engine.name, !engine.enabled)"></div>
                </div>
              </div>

              <div class="card-body">
                <!-- Error display -->
                <div v-if="engine.state?.install_error" class="engine-error">
                  {{ engine.state.install_error }}
                </div>

                <!-- Status grid -->
                <div class="engine-status-grid" style="margin-bottom: var(--space-4);">
                  <!-- 安装路径 (editable) -->
                  <span class="engine-status-key">安装路径</span>
                  <div class="engine-status-val">
                    <template v-if="!isEditing(engine.name, 'clamav_path')">
                      <span class="editable-val" @click.stop="startEdit(engine.name, 'clamav_path', engine.clamav_path)">
                        <template v-if="engine.clamav_path">{{ engine.clamav_path }}</template>
                        <template v-else><span class="editable-placeholder">点击设置</span></template>
                      </span>
                    </template>
                    <div v-else class="inline-edit" @click.stop>
                      <input class="form-input inline-edit-input" v-model="editValue"
                             placeholder="留空使用默认路径"
                             @keyup.escape="cancelEdit">
                      <button class="btn btn-sm" :class="isDirty() ? 'btn-primary' : ''"
                              @click="saveEdit(engine.name, 'clamav_path')">保存</button>
                      <button class="btn btn-sm" @click="cancelEdit">取消</button>
                    </div>
                  </div>

                  <!-- 监听地址 (editable) -->
                  <span class="engine-status-key">监听地址</span>
                  <div class="engine-status-val">
                    <template v-if="!isEditing(engine.name, 'address')">
                      <span class="editable-val" @click.stop="startEdit(engine.name, 'address', engine.address)">
                        <template v-if="engine.address">{{ engine.address }}</template>
                        <template v-else><span class="editable-placeholder">点击设置</span></template>
                      </span>
                    </template>
                    <div v-else class="inline-edit" @click.stop>
                      <input class="form-input inline-edit-input" v-model="editValue"
                             placeholder="例: 127.0.0.1:3310"
                             @keyup.escape="cancelEdit">
                      <button class="btn btn-sm" :class="isDirty() ? 'btn-primary' : ''"
                              @click="saveEdit(engine.name, 'address')">保存</button>
                      <button class="btn btn-sm" @click="cancelEdit">取消</button>
                    </div>
                  </div>

                  <!-- 数据库 (read-only) -->
                  <span class="engine-status-key">数据库</span>
                  <span class="engine-status-val">
                    <span class="badge" :class="getDbBadge(engine.state?.db_status).cls">
                      {{ getDbBadge(engine.state?.db_status).text }}
                    </span>
                    <span v-if="engine.state?.last_db_update" style="font-size: var(--text-xs); color: var(--text-muted);">
                      {{ formatTime(engine.state.last_db_update) }}
                    </span>
                  </span>

                  <!-- 下载地址 (editable) -->
                  <span class="engine-status-key">下载地址</span>
                  <div class="engine-status-val">
                    <template v-if="!isEditing(engine.name, 'url')">
                      <span class="editable-val" @click.stop="startEdit(engine.name, 'url', engine.url)">
                        <template v-if="engine.url">{{ engine.url }}</template>
                        <template v-else><span class="editable-placeholder">点击设置</span></template>
                      </span>
                    </template>
                    <div v-else class="inline-edit" @click.stop>
                      <input class="form-input inline-edit-input" v-model="editValue"
                             placeholder="留空使用默认下载源"
                             @keyup.escape="cancelEdit">
                      <button class="btn btn-sm" :class="isDirty() ? 'btn-primary' : ''"
                              @click="saveEdit(engine.name, 'url')">保存</button>
                      <button class="btn btn-sm" @click="cancelEdit">取消</button>
                    </div>
                  </div>

                  <!-- 数据目录 (editable) -->
                  <span class="engine-status-key">数据目录</span>
                  <div class="engine-status-val">
                    <template v-if="!isEditing(engine.name, 'data_dir')">
                      <span class="editable-val" @click.stop="startEdit(engine.name, 'data_dir', engine.data_dir)">
                        <template v-if="engine.data_dir">{{ engine.data_dir }}</template>
                        <template v-else><span class="editable-placeholder">点击设置</span></template>
                      </span>
                    </template>
                    <div v-else class="inline-edit" @click.stop>
                      <input class="form-input inline-edit-input" v-model="editValue"
                             placeholder="留空使用安装路径下的 database 目录"
                             @keyup.escape="cancelEdit">
                      <button class="btn btn-sm" :class="isDirty() ? 'btn-primary' : ''"
                              @click="saveEdit(engine.name, 'data_dir')">保存</button>
                      <button class="btn btn-sm" @click="cancelEdit">取消</button>
                    </div>
                  </div>

                  <!-- 上次安装 (read-only) -->
                  <template v-if="engine.state?.last_install_attempt">
                    <span class="engine-status-key">上次安装</span>
                    <span class="engine-status-val">{{ formatTime(engine.state.last_install_attempt) }}</span>
                  </template>
                </div>

                <!-- Progress bar -->
                <div v-if="engine.operation" class="engine-progress">
                  <div class="engine-progress-info">
                    <span>{{ engine.progressMsg || getOpLabel(engine.operation) }}</span>
                    <span v-if="engine.progress > 0">{{ engine.progress }}%</span>
                  </div>
                  <div class="progress">
                    <div class="progress-bar" :style="{ width: Math.max(engine.progress, 5) + '%' }"></div>
                  </div>
                </div>

                <!-- Action buttons -->
                <div class="engine-actions">
                  <button v-if="engine.operation" class="btn btn-sm btn-danger"
                          @click="cancelOp(engine.name)">停止</button>
                  <button class="btn btn-sm" @click="checkEngine(engine.name)" :disabled="isBusy(engine)">检查状态</button>
                  <button v-if="engine.state?.install_status !== 'installed'" class="btn btn-sm btn-primary"
                          @click="installEngine(engine.name)" :disabled="isBusy(engine)">安装</button>
                  <button v-if="engine.state?.install_status === 'installed' && (engine.state?.db_status === 'missing' || engine.state?.db_status === 'stale')"
                          class="btn btn-sm btn-primary" @click="updateDb(engine.name)" :disabled="isBusy(engine)">更新数据库</button>
                  <button v-if="engine.state?.install_status === 'installed' && engine.state?.db_status === 'ready'"
                          class="btn btn-sm btn-success" @click="testEngine(engine.name)" :disabled="isBusy(engine)">测试扫描</button>
                </div>
              </div>
            </div>
          </div>
        </div>

        <!-- Config Tab -->
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
                <textarea class="form-textarea" style="min-height: 60vh; font-family: var(--font-mono); font-size: var(--text-xs);" v-model="editConfig"></textarea>
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
