<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted, watch } from 'vue'
import { useWSAPI } from '../composables/useWSAPI'
import { on as sseOn, off as sseOff } from '../composables/useSSE'
import { useToast } from '../composables/useToast'

const { request } = useWSAPI()
const toast = useToast()

interface DocEntry { path: string; size?: number; modified?: string }

// --- Document tab state ---
const activeTab = ref('documents')
const documents = ref<DocEntry[]>([])
const docContent = ref('')
const docPath = ref('')
const editing = ref(false)
const editContent = ref('')

// --- Enhanced memory: environment ---
const envStatus = ref<any>(null)
const setupProgress = ref('')
const showEmbeddingConfig = ref(false)
const embeddingConfigContent = ref('')

// --- Enhanced memory: configuration ---
const mainEnabled = ref(false)
const subEnabled = ref(false)
const activeTier = ref('medium')
const similarityThreshold = ref(0.7)
const maxResults = ref(10)
const _configInitialized = ref(false)

// --- Enhanced memory: auto-inject (P1-1, 2026-08-24) ---
// 区别于记忆库（有问才答：agent 主动 memory_search）——自动注入是不问自答：
// 每轮 LLM 调用前自动把与最新消息最相关的 top_k 条记忆注入上下文。
// 开关在 agent_factory 构建 AgentLoop 时读取，保存后需重启 Agent 生效。
const autoInject = ref(false)
const autoInjectTopK = ref(3)
const restartingAgent = ref(false)

// --- Enhanced memory: content ---
const memoryStats = ref<any>(null)
const entriesList = ref<any[]>([])
const entriesSearchQuery = ref('')
const entriesSearchResults = ref<any[]>([])

// --- Enhanced memory: test ---
const testInputText = ref('')
const testSearchQuery = ref('')
const testResults = ref<any[]>([])
const testStoring = ref(false)

const loading = ref(true)

// SSE handler ref
let _onSetupProgress: ((data: any) => void) | null = null
let _saveTimer: ReturnType<typeof setTimeout> | null = null

// ---------------------------------------------------------------------------
// Formatting
// ---------------------------------------------------------------------------

function formatSize(bytes?: number): string {
  if (!bytes) return '--'
  if (bytes < 1024) return bytes + ' B'
  if (bytes < 1024 * 1024) return (bytes / 1024).toFixed(1) + ' KB'
  return (bytes / (1024 * 1024)).toFixed(1) + ' MB'
}

// ---------------------------------------------------------------------------
// Document tab (original)
// ---------------------------------------------------------------------------

async function loadStatus() {
  try {
    const data = await request('memory', 'status')
    mainEnabled.value = data?.vector_memory?.main_enabled ?? false
  } catch { /* ignore */ }
}

async function loadDocuments() {
  try {
    const data = await request('memory', 'documents')
    documents.value = data?.documents || []
  } catch (e: any) {
    toast.error('加载失败: ' + e)
  }
}

async function openDocument(path: string) {
  try {
    const data = await request('memory', 'document.get', { path })
    docContent.value = data?.content || ''
    docPath.value = path
  } catch (e: any) {
    toast.error('读取失败: ' + e)
  }
}

function startEdit() {
  editContent.value = docContent.value
  editing.value = true
}

async function saveDocument() {
  try {
    await request('memory', 'document.save', { path: docPath.value, content: editContent.value })
    toast.success('已保存')
    docContent.value = editContent.value
    editing.value = false
    await loadDocuments()
  } catch (e: any) {
    toast.error('保存失败: ' + e)
  }
}

// ---------------------------------------------------------------------------
// Enhanced memory: data loading
// ---------------------------------------------------------------------------

async function loadEnvStatus() {
  try {
    envStatus.value = await request('memory', 'env.check')
  } catch (e: any) {
    toast.error('环境检测失败: ' + e)
  }
}

async function loadConfig() {
  try {
    const data = await request('memory', 'config.get')
    mainEnabled.value = data?.main_enabled ?? false
    subEnabled.value = data?.sub_enabled ?? false
    activeTier.value = data?.active_tier ?? 'medium'
    similarityThreshold.value = data?.similarity_threshold ?? 0.7
    maxResults.value = data?.max_results ?? 10
    autoInject.value = data?.auto_inject ?? false
    autoInjectTopK.value = data?.auto_inject_top_k ?? 3
    if (data?.embedding_config_content) {
      embeddingConfigContent.value = data.embedding_config_content
    }
  } catch (e: any) {
    toast.error('加载配置失败: ' + e)
  }
}

async function loadStats() {
  try {
    memoryStats.value = await request('memory', 'stats')
  } catch { /* non-critical */ }
}

async function loadEntries() {
  try {
    const data = await request('memory', 'entries.list')
    entriesList.value = data?.entries || []
  } catch { /* non-critical */ }
}

// ---------------------------------------------------------------------------
// Enhanced memory: actions
// ---------------------------------------------------------------------------

async function checkEnv() {
  try {
    envStatus.value = await request('memory', 'env.check')
    toast.success('环境检查完成')
  } catch (e: any) {
    toast.error('检查失败: ' + e)
  }
}

async function oneClickSetup() {
  setupProgress.value = '正在安装...'
  try {
    await request('memory', 'env.setup', undefined, 0)
    toast.success('一键安装完成')
    setupProgress.value = ''
    await Promise.all([loadEnvStatus(), loadConfig()])
  } catch (e: any) {
    toast.error('安装失败: ' + e)
    setupProgress.value = ''
  }
}

async function installModelTier(tier: string, label: string) {
  setupProgress.value = `正在安装${label}模型...`
  try {
    await request('memory', 'model.install', { tier }, 0)
    toast.success(`${label}模型安装完成`)
    setupProgress.value = ''
    await loadEnvStatus()
  } catch (e: any) {
    toast.error(`${label}模型安装失败: ` + e)
    setupProgress.value = ''
  }
}

async function searchEntries() {
  if (!entriesSearchQuery.value.trim()) return
  try {
    const data = await request('memory', 'entries.search', {
      query: entriesSearchQuery.value,
      limit: 20,
    })
    entriesSearchResults.value = data?.results || []
  } catch (e: any) {
    toast.error('搜索失败: ' + e)
  }
}

async function storeTestEntry() {
  if (!testInputText.value.trim()) {
    toast.error('请输入测试文本')
    return
  }
  testStoring.value = true
  try {
    const data = await request('memory', 'entries.store', {
      content: testInputText.value,
    })
    toast.success('测试条目已存储: ' + (data?.id || '').substring(0, 8))
    testInputText.value = ''
    await Promise.all([loadStats(), loadEntries()])
  } catch (e: any) {
    toast.error('存储失败: ' + e)
  }
  testStoring.value = false
}

async function runTestSearch() {
  if (!testSearchQuery.value.trim()) return
  try {
    const data = await request('memory', 'entries.search', {
      query: testSearchQuery.value,
      limit: 5,
    })
    testResults.value = data?.results || []
  } catch (e: any) {
    toast.error('测试搜索失败: ' + e)
  }
}

async function toggleEmbeddingConfig() {
  showEmbeddingConfig.value = !showEmbeddingConfig.value
  if (showEmbeddingConfig.value) {
    try {
      const data = await request('memory', 'config.get')
      if (data?.embedding_config_content) {
        embeddingConfigContent.value = data.embedding_config_content
      }
    } catch (e: any) {
      toast.error('加载配置失败: ' + e)
    }
  }
}

async function saveEmbeddingConfig() {
  try {
    await request('memory', 'config.set', {
      embedding_config_content: embeddingConfigContent.value,
    })
    toast.success('配置已保存')
    await loadEnvStatus()
  } catch (e: any) {
    toast.error('保存失败: ' + e)
  }
}

// ---------------------------------------------------------------------------
// Config auto-save (debounce)
// ---------------------------------------------------------------------------

function saveConfigDebounced() {
  if (!_configInitialized.value) return
  if (_saveTimer) clearTimeout(_saveTimer)
  _saveTimer = setTimeout(async () => {
    try {
      await request('memory', 'config.set', {
        main_enabled: mainEnabled.value,
        sub_enabled: subEnabled.value,
        active_tier: activeTier.value,
        similarity_threshold: similarityThreshold.value,
        max_results: maxResults.value,
        auto_inject: autoInject.value,
        auto_inject_top_k: Math.min(10, Math.max(1, autoInjectTopK.value || 3)),
      })
      // Show restart hint when enabling enhanced memory
      if (subEnabled.value) {
        toast.warn('配置已保存，需要重启 Bot 后强化记忆功能才能生效')
      }
    } catch (e: any) {
      toast.error('保存失败: ' + (e?.message || e))
    }
  }, 500)
}

// 自动注入开关是 Agent 启动时读取的——保存后一键重启 Agent（agent.stop →
// agent.start 按磁盘 config 重建 AgentLoop，无需重启进程）。
async function restartAgent() {
  restartingAgent.value = true
  try {
    await request('agent', 'stop')
    // 给 stop 的清理动作（spawned 任务 abort、session 落盘）一点时间。
    await new Promise(r => setTimeout(r, 1000))
    await request('agent', 'start')
    toast.success('Agent 已重启，自动注入设置已生效')
  } catch (e: any) {
    toast.error('重启 Agent 失败: ' + (e?.message || e))
  }
  restartingAgent.value = false
}

// 开关级联：自动注入 = 强化记忆子系统在「不问自答」方向上的应用——检索走
// 向量语义匹配（关键词回退分数恒 0，过不了 0.35 阈值），强化记忆关着时注入
// 永远为空。所以打开自动注入时自动带上强化记忆；反过来关强化记忆时连带关
// 自动注入（留着也是死的）。用户只操作一个开关，依赖由界面兜住。
// 模型未装时后端会拒绝 sub_enabled=true（开强化记忆强制要求模型），所以级联
// 前先查模型：未装则回滚自动注入并引导去环境准备卡，避免 UI 与磁盘状态分叉。
watch(autoInject, on => {
  if (!on) return
  if (!subEnabled.value) {
    if (!envStatus.value?.models?.[activeTier.value]?.model_ready) {
      autoInject.value = false
      toast.error('注入模型尚未安装：请先在上方「环境准备」卡点对应档位的「安装」按钮，装好后再打开自动注入')
      return
    }
    subEnabled.value = true
    toast.info('已同步开启「强化记忆」——自动注入的语义检索依赖它')
  }
})
watch(subEnabled, on => {
  if (!on && autoInject.value) {
    autoInject.value = false
    toast.warn('「强化记忆」已关闭，自动注入随之关闭')
  }
})

watch([mainEnabled, subEnabled, activeTier, similarityThreshold, maxResults, autoInject, autoInjectTopK], () => {
  saveConfigDebounced()
})

// ---------------------------------------------------------------------------
// 自动记忆注入 TAB：记忆条目管理（查看/搜索/新增/编辑/删除）
// ---------------------------------------------------------------------------

const mgmtEntries = ref<any[]>([])
const mgmtTotal = ref(0)
const mgmtLimit = 50
const mgmtSearchQuery = ref('')
const mgmtResults = ref<any[] | null>(null)
const mgmtNewContent = ref('')
const mgmtAdding = ref(false)
const mgmtEditingId = ref('')
const mgmtEditContent = ref('')
const mgmtSaving = ref(false)

async function loadMgmtEntries() {
  try {
    const data = await request('memory', 'entries.list', { offset: 0, limit: mgmtLimit })
    mgmtEntries.value = data?.entries || []
    mgmtTotal.value = data?.total || 0
    mgmtResults.value = null
  } catch (e: any) {
    toast.error('加载记忆条目失败: ' + e)
  }
}

async function loadMoreMgmtEntries() {
  try {
    // 按已加载条数续页（entries.list 内容有 200 字符显示截断，编辑走 entries.get 全量）。
    const data = await request('memory', 'entries.list', {
      offset: mgmtEntries.value.length,
      limit: mgmtLimit,
    })
    mgmtEntries.value.push(...(data?.entries || []))
    mgmtTotal.value = data?.total || 0
  } catch (e: any) {
    toast.error('加载更多失败: ' + e)
  }
}

async function addMgmtEntry() {
  if (!mgmtNewContent.value.trim()) return
  mgmtAdding.value = true
  try {
    await request('memory', 'entries.store', { content: mgmtNewContent.value })
    toast.success('记忆条目已添加')
    mgmtNewContent.value = ''
    await loadMgmtEntries()
  } catch (e: any) {
    toast.error('添加失败: ' + e)
  }
  mgmtAdding.value = false
}

async function searchMgmtEntries() {
  if (!mgmtSearchQuery.value.trim()) {
    mgmtResults.value = null
    return
  }
  try {
    const data = await request('memory', 'entries.search', {
      query: mgmtSearchQuery.value,
      limit: 20,
    })
    mgmtResults.value = data?.results || []
  } catch (e: any) {
    toast.error('搜索失败: ' + e)
  }
}

async function startMgmtEdit(entry: any) {
  try {
    // entries.list 的 content 截断到 200 字符——编辑必须取全量原文。
    const data = await request('memory', 'entries.get', { id: entry.id })
    mgmtEditContent.value = data?.entry?.content ?? entry.content ?? ''
    mgmtEditingId.value = entry.id
  } catch (e: any) {
    toast.error('读取条目失败: ' + e)
  }
}

function cancelMgmtEdit() {
  mgmtEditingId.value = ''
  mgmtEditContent.value = ''
}

async function saveMgmtEdit() {
  if (!mgmtEditingId.value || !mgmtEditContent.value.trim()) return
  mgmtSaving.value = true
  try {
    // update = 删除旧条目 + 重新嵌入存储 → 返回新 id。
    await request('memory', 'entries.update', {
      id: mgmtEditingId.value,
      content: mgmtEditContent.value,
    })
    toast.success('记忆条目已更新')
    cancelMgmtEdit()
    await loadMgmtEntries()
  } catch (e: any) {
    toast.error('更新失败: ' + e)
  }
  mgmtSaving.value = false
}

async function deleteMgmtEntry(entry: any) {
  if (!window.confirm('确定删除这条记忆吗？删除后不可恢复。')) return
  try {
    await request('memory', 'entries.delete', { id: entry.id })
    toast.success('记忆条目已删除')
    if (mgmtEditingId.value === entry.id) cancelMgmtEdit()
    await loadMgmtEntries()
  } catch (e: any) {
    toast.error('删除失败: ' + e)
  }
}

// 切到自动注入 TAB 时刷新条目列表（外部/agent 侧可能新增过）。
watch(activeTab, t => {
  if (t === 'autoinject') void loadMgmtEntries()
})

// ---------------------------------------------------------------------------
// SSE progress handler
// ---------------------------------------------------------------------------

_onSetupProgress = (data: any) => {
  if (data?.message) setupProgress.value = data.message
  if (data?.status === 'complete' || data?.status === 'error') {
    setTimeout(() => { setupProgress.value = '' }, 2000)
  }
}
sseOn('memory-setup', _onSetupProgress)

// ---------------------------------------------------------------------------
// Lifecycle
// ---------------------------------------------------------------------------

onMounted(async () => {
  loading.value = true
  await Promise.all([loadStatus(), loadDocuments(), loadEnvStatus(), loadConfig(), loadStats(), loadEntries()])
  loading.value = false
  _configInitialized.value = true
})

onUnmounted(() => {
  if (_onSetupProgress) sseOff('memory-setup', _onSetupProgress)
  if (_saveTimer) clearTimeout(_saveTimer)
})
</script>

<template>
  <div class="page-memory">
    <div class="page-header"><h2>记忆管理</h2></div>
    <div class="page-body">
      <div class="tabs">
        <button class="tab" :class="{ active: activeTab === 'documents' }" @click="activeTab = 'documents'">文档记忆</button>
        <button class="tab" :class="{ active: activeTab === 'vector' }" @click="activeTab = 'vector'">强化记忆</button>
        <button class="tab" :class="{ active: activeTab === 'autoinject' }" @click="activeTab = 'autoinject'">自动记忆注入</button>
      </div>

      <!-- Documents tab (unchanged) -->
      <div v-if="activeTab === 'documents'">
        <div v-if="loading" style="text-align: center; padding: var(--space-8);">
          <div class="spinner spinner-lg" style="margin: 0 auto;"></div>
        </div>

        <div v-if="!loading" style="display: grid; grid-template-columns: 280px 1fr; gap: var(--space-4); min-height: 400px;">
          <!-- File list -->
          <div class="card" style="overflow-y: auto;">
            <div class="card-header"><h3>文件列表</h3></div>
            <div style="padding: var(--space-2);">
              <div v-for="doc in documents" :key="doc.path"
                style="padding: var(--space-2) var(--space-3); cursor: pointer; border-radius: var(--radius-md); font-size: var(--text-sm); transition: background 0.1s;"
                :style="{ background: docPath === doc.path ? 'var(--accent-muted)' : '' }"
                @click="openDocument(doc.path)">
                <div style="font-weight: 500;">{{ doc.path }}</div>
                <div style="font-size: var(--text-xs); color: var(--text-muted);">{{ formatSize(doc.size) }}</div>
              </div>
              <div v-if="documents.length === 0" style="padding: var(--space-4); text-align: center; color: var(--text-muted); font-size: var(--text-sm);">
                暂无记忆文件
              </div>
            </div>
          </div>

          <!-- Content viewer/editor -->
          <div class="card">
            <div class="card-header">
              <h3>{{ docPath || '请选择文件' }}</h3>
              <div v-if="docPath" style="display: flex; gap: var(--space-2);">
                <template v-if="!editing">
                  <button class="btn btn-sm" @click="startEdit">编辑</button>
                </template>
                <template v-else>
                  <button class="btn btn-sm" @click="editing = false">取消</button>
                  <button class="btn btn-sm btn-primary" @click="saveDocument">保存</button>
                </template>
              </div>
            </div>
            <div class="card-body">
              <div v-if="!docPath" class="empty-state" style="padding: var(--space-6);">
                <p>从左侧选择一个文件查看内容</p>
              </div>
              <div v-else-if="editing">
                <textarea class="form-textarea" style="min-height: 55vh; font-family: var(--font-mono); font-size: var(--text-sm);" v-model="editContent"></textarea>
              </div>
              <div v-else class="markdown-body" style="max-height: 60vh; overflow-y: auto;">
                <pre style="white-space: pre-wrap; word-break: break-word;">{{ docContent }}</pre>
              </div>
            </div>
          </div>
        </div>
      </div>

      <!-- Enhanced memory tab: 2x2 grid -->
      <div v-if="activeTab === 'vector'">

        <!-- Setup progress bar -->
        <div v-if="setupProgress" class="card" style="padding: var(--space-3) var(--space-4); background: var(--accent-bg, rgba(59,130,246,0.08)); border-color: var(--accent);">
          <div style="display: flex; align-items: center; gap: var(--space-3);">
            <div class="spinner spinner-sm"></div>
            <span style="font-size: var(--text-sm); color: var(--accent);">{{ setupProgress }}</span>
          </div>
        </div>

        <!-- Row 1: Environment + Configuration -->
        <div style="display: grid; grid-template-columns: 1fr 1fr; gap: var(--space-4); margin-top: var(--space-4);">

          <!-- Section 1: 环境管理 -->
          <div class="card">
            <div class="card-header" style="display: flex; justify-content: space-between; align-items: center;">
              <h3 style="margin: 0;">环境管理</h3>
              <div style="display: flex; gap: var(--space-2);">
                <button class="btn btn-sm" @click="toggleEmbeddingConfig">{{ showEmbeddingConfig ? '隐藏配置' : '查看配置' }}</button>
                <button class="btn btn-sm" @click="checkEnv">检查环境</button>
                <button class="btn btn-sm btn-primary" @click="oneClickSetup" :disabled="!!setupProgress">一键安装</button>
              </div>
            </div>
            <div class="card-body">
              <!-- Plugin status -->
              <div style="margin-bottom: var(--space-3);">
                <div style="font-weight: 500; margin-bottom: var(--space-2);">插件状态</div>
                <div style="padding-left: var(--space-4);">
                  <div style="display: flex; align-items: center; gap: var(--space-2); font-size: var(--text-sm);">
                    <span :style="{ color: envStatus?.plugin?.found ? 'var(--success)' : 'var(--text-secondary)' }">{{ envStatus?.plugin?.found ? '●' : '○' }}</span>
                    <span>plugin_onnx.dll</span>
                    <span v-if="envStatus?.plugin?.found" style="color: var(--text-secondary);">(已找到)</span>
                    <span v-else style="color: var(--danger);">未找到</span>
                  </div>
                </div>
              </div>

              <!-- Models -->
              <div>
                <div style="font-weight: 500; margin-bottom: var(--space-2);">模型文件</div>
                <div style="display: flex; flex-direction: column; gap: var(--space-2); padding-left: var(--space-4);">
                  <div v-for="tier in ['large', 'medium', 'small']" :key="tier" style="display: flex; justify-content: space-between; align-items: center;">
                    <span style="display: flex; align-items: center; gap: var(--space-2); font-size: var(--text-sm);">
                      <span :style="{ color: envStatus?.models?.[tier]?.model_ready ? 'var(--success)' : 'var(--text-secondary)' }">{{ envStatus?.models?.[tier]?.model_ready ? '●' : '○' }}</span>
                      <span>{{ tier === 'large' ? '大模型' : tier === 'medium' ? '中模型' : '小模型' }} ({{ envStatus?.models?.[tier]?.dimension || '?' }}d)</span>
                      <span v-if="envStatus?.models?.[tier]?.model_ready && envStatus?.models?.[tier]?.model_size" style="color: var(--text-secondary);">({{ formatSize(envStatus.models[tier].model_size) }})</span>
                    </span>
                    <button class="btn btn-sm" @click="installModelTier(tier, tier === 'large' ? '大模型' : tier === 'medium' ? '中模型' : '小模型')" :disabled="!!setupProgress || envStatus?.models?.[tier]?.model_ready">安装</button>
                  </div>
                </div>
              </div>

              <!-- Config editor (toggle) -->
              <div v-if="showEmbeddingConfig" style="margin-top: var(--space-4); border-top: 1px solid var(--border); padding-top: var(--space-4);">
                <textarea class="form-textarea" style="min-height: 200px; font-family: var(--font-mono); font-size: var(--text-xs);" v-model="embeddingConfigContent"></textarea>
                <div style="margin-top: var(--space-2); display: flex; justify-content: flex-end;">
                  <button class="btn btn-sm btn-primary" @click="saveEmbeddingConfig">保存</button>
                </div>
              </div>
            </div>
          </div>

          <!-- Section 2: 记忆配置 -->
          <div class="card">
            <div class="card-header"><h3 style="margin: 0;">记忆配置</h3></div>
            <div class="card-body">
              <div class="settings-grid">
                <!-- Main switch -->
                <span class="settings-key">主开关</span>
                <label class="toggle-switch">
                  <input type="checkbox" v-model="mainEnabled" />
                  <span class="toggle-slider"></span>
                  <span class="toggle-label">{{ mainEnabled ? '启用' : '停用' }}</span>
                </label>

                <!-- Sub switch -->
                <span class="settings-key">强化记忆</span>
                <label class="toggle-switch">
                  <input type="checkbox" v-model="subEnabled" :disabled="!mainEnabled" />
                  <span class="toggle-slider"></span>
                  <span class="toggle-label">{{ subEnabled ? '启用' : '停用' }}</span>
                </label>

                <!-- Active tier -->
                <span class="settings-key">模型规格</span>
                <select class="form-select" v-model="activeTier" style="width: 100%;" :disabled="!subEnabled">
                  <option value="large">大模型 (768d)</option>
                  <option value="medium">中模型 (384d)</option>
                  <option value="small">小模型 (256d)</option>
                </select>

                <!-- Similarity threshold -->
                <span class="settings-key">相似度阈值</span>
                <div style="display: flex; align-items: center; gap: var(--space-3);">
                  <input type="range" min="0.1" max="1.0" step="0.05" v-model.number="similarityThreshold" style="flex: 1;" :disabled="!subEnabled" />
                  <span style="font-size: var(--text-sm); min-width: 36px; text-align: right;">{{ similarityThreshold.toFixed(2) }}</span>
                </div>

                <!-- Max results -->
                <span class="settings-key">最大结果数</span>
                <div style="display: flex; align-items: center; gap: var(--space-2);">
                  <input type="number" v-model.number="maxResults" min="1" max="50" style="width: 70px; text-align: center;" :disabled="!subEnabled" />
                </div>

                <!-- Overall status -->
                <span class="settings-key">整体状态</span>
                <span>
                  <span class="badge" :class="envStatus?.overall === 'ready' ? 'badge-success' : envStatus?.overall === 'degraded' ? 'badge-warning' : 'badge-neutral'">
                    {{ envStatus?.overall === 'ready' ? '就绪' : envStatus?.overall === 'degraded' ? '降级' : '未启用' }}
                  </span>
                </span>
              </div>
            </div>
          </div>

        </div><!-- End Row 1 -->

        <!-- Row 2: Content + Test -->
        <div style="display: grid; grid-template-columns: 1fr 1fr; gap: var(--space-4); margin-top: var(--space-4);">

          <!-- Section 3: 强化记忆内容 -->
          <div class="card">
            <div class="card-header" style="display: flex; justify-content: space-between; align-items: center;">
              <h3 style="margin: 0;">强化记忆内容</h3>
              <button class="btn btn-sm" @click="loadStats(); loadEntries()">刷新</button>
            </div>
            <div class="card-body">
              <!-- Stats row -->
              <div style="display: grid; grid-template-columns: repeat(3, 1fr); gap: var(--space-2); margin-bottom: var(--space-3);">
                <div class="stat-card">
                  <div class="stat-label">向量条目</div>
                  <div class="stat-value">{{ memoryStats?.vector_entries ?? 0 }}</div>
                </div>
                <div class="stat-card">
                  <div class="stat-label">对话段</div>
                  <div class="stat-value">{{ memoryStats?.episodic_episodes ?? 0 }}</div>
                </div>
                <div class="stat-card">
                  <div class="stat-label">图谱三元组</div>
                  <div class="stat-value">{{ memoryStats?.graph_triples ?? 0 }}</div>
                </div>
              </div>

              <!-- Search -->
              <div style="display: flex; gap: var(--space-2); margin-bottom: var(--space-3);">
                <input class="form-input" style="flex: 1;" v-model="entriesSearchQuery" placeholder="搜索记忆条目..." @keydown.enter="searchEntries" />
                <button class="btn btn-sm btn-primary" @click="searchEntries" :disabled="!entriesSearchQuery.trim()">搜索</button>
                <button v-if="entriesSearchResults.length > 0" class="btn btn-sm" @click="entriesSearchResults = []; entriesSearchQuery = ''">清除</button>
              </div>

              <!-- Results / entries list -->
              <div style="border: 1px solid var(--border); border-radius: var(--radius-md); max-height: 300px; overflow-y: auto;">
                <div v-if="entriesSearchResults.length > 0">
                  <div v-for="entry in entriesSearchResults" :key="entry.id" style="padding: var(--space-2) var(--space-3); border-bottom: 1px solid var(--border); font-size: var(--text-sm);">
                    <div style="display: flex; justify-content: space-between;">
                      <span style="font-weight: 500;">{{ entry.content }}</span>
                      <span v-if="entry.type" style="color: var(--text-secondary); font-size: var(--text-xs);">{{ entry.type }}</span>
                    </div>
                  </div>
                </div>
                <div v-else-if="entriesList.length > 0">
                  <div v-for="entry in entriesList.slice(0, 50)" :key="entry.id" style="padding: var(--space-2) var(--space-3); border-bottom: 1px solid var(--border); font-size: var(--text-sm);">
                    <div style="display: flex; justify-content: space-between;">
                      <span>{{ entry.content }}</span>
                      <span v-if="entry.type" style="color: var(--text-secondary); font-size: var(--text-xs);">{{ entry.type }}</span>
                    </div>
                  </div>
                </div>
                <div v-else class="empty-state" style="padding: var(--space-4);">
                  <p>暂无记忆条目</p>
                </div>
              </div>
            </div>
          </div>

          <!-- Section 4: 强化记忆测试 -->
          <div class="card">
            <div class="card-header"><h3 style="margin: 0;">强化记忆测试</h3></div>
            <div class="card-body">
              <!-- Store test entry -->
              <div style="margin-bottom: var(--space-4);">
                <div style="font-weight: 500; margin-bottom: var(--space-2);">存储测试条目</div>
                <div style="display: flex; gap: var(--space-2);">
                  <textarea class="form-textarea" style="flex: 1; min-height: 80px; resize: vertical;" v-model="testInputText" placeholder="输入文本存储到记忆中..." @keydown.ctrl.enter="storeTestEntry"></textarea>
                  <button class="btn btn-primary" @click="storeTestEntry" :disabled="testStoring || !testInputText.trim()">
                    {{ testStoring ? '存储中...' : '存储' }}
                  </button>
                </div>
              </div>

              <!-- Keyword search test -->
              <div>
                <div style="font-weight: 500; margin-bottom: var(--space-2);">关键词搜索测试</div>
                <div style="display: flex; gap: var(--space-2); margin-bottom: var(--space-3);">
                  <input class="form-input" style="flex: 1;" v-model="testSearchQuery" placeholder="输入搜索查询..." @keydown.enter="runTestSearch" />
                  <button class="btn btn-primary" @click="runTestSearch" :disabled="!testSearchQuery.trim()">搜索</button>
                </div>
                <div v-if="testResults.length > 0" style="border: 1px solid var(--border); border-radius: var(--radius-md); padding: var(--space-3); max-height: 250px; overflow-y: auto;">
                  <div v-for="(r, i) in testResults" :key="i" style="display: flex; align-items: center; gap: var(--space-2); padding: var(--space-1) 0; font-size: var(--text-sm);">
                    <span style="flex: 1;">{{ r.content }}</span>
                    <span v-if="r.type" style="color: var(--text-secondary); font-size: var(--text-xs);">{{ r.type }}</span>
                  </div>
                </div>
                <div v-else style="color: var(--text-secondary); font-size: var(--text-sm);">
                  输入查询文字进行搜索测试
                </div>
              </div>
            </div>
          </div>

        </div><!-- End Row 2 -->

      </div><!-- End vector tab -->

      <!-- 自动记忆注入 TAB：环境准备（模型下载）+ 注入配置 + 记忆条目管理 -->
      <div v-if="activeTab === 'autoinject'">

        <!-- Setup progress bar -->
        <div v-if="setupProgress" class="card" style="padding: var(--space-3) var(--space-4); background: var(--accent-bg, rgba(59,130,246,0.08)); border-color: var(--accent); margin-top: var(--space-4);">
          <div style="display: flex; align-items: center; gap: var(--space-3);">
            <div class="spinner spinner-sm"></div>
            <span style="font-size: var(--text-sm); color: var(--accent);">{{ setupProgress }}</span>
          </div>
        </div>

        <!-- 卡 1：环境准备（模型下载）—— 检查环境 → 查看配置 → 手动点安装 -->
        <div class="card" style="margin-top: var(--space-4);">
          <div class="card-header" style="display: flex; justify-content: space-between; align-items: center;">
            <h3 style="margin: 0;">环境准备（注入模型）</h3>
            <div style="display: flex; gap: var(--space-2);">
              <button class="btn btn-sm" @click="toggleEmbeddingConfig">{{ showEmbeddingConfig ? '隐藏配置' : '查看配置' }}</button>
              <button class="btn btn-sm" @click="checkEnv">检查环境</button>
              <button class="btn btn-sm btn-primary" @click="oneClickSetup" :disabled="!!setupProgress">一键安装</button>
            </div>
          </div>
          <div class="card-body">
            <p style="color: var(--text-secondary); font-size: var(--text-xs); margin: 0 0 var(--space-3);">
              自动注入依赖本地 ONNX 嵌入模型。流程：先「检查环境」确认插件与模型状态 →
              需要时「查看配置」→ 再点对应档位的「安装」从 hf-mirror 国内镜像自动下载
              （小 ~60MB / 中 ~90MB / 大 ~430MB），无需手动找文件。
            </p>
            <!-- Plugin status -->
            <div style="margin-bottom: var(--space-3);">
              <div style="font-weight: 500; margin-bottom: var(--space-2);">插件状态</div>
              <div style="padding-left: var(--space-4);">
                <div style="display: flex; align-items: center; gap: var(--space-2); font-size: var(--text-sm);">
                  <span :style="{ color: envStatus?.plugin?.found ? 'var(--success)' : 'var(--text-secondary)' }">{{ envStatus?.plugin?.found ? '●' : '○' }}</span>
                  <span>plugin_onnx.dll</span>
                  <span v-if="envStatus?.plugin?.found" style="color: var(--text-secondary);">(已找到)</span>
                  <span v-else style="color: var(--danger);">未找到</span>
                </div>
              </div>
            </div>

            <!-- Models -->
            <div>
              <div style="font-weight: 500; margin-bottom: var(--space-2);">模型文件</div>
              <div style="display: flex; flex-direction: column; gap: var(--space-2); padding-left: var(--space-4);">
                <div v-for="tier in ['large', 'medium', 'small']" :key="tier" style="display: flex; justify-content: space-between; align-items: center;">
                  <span style="display: flex; align-items: center; gap: var(--space-2); font-size: var(--text-sm);">
                    <span :style="{ color: envStatus?.models?.[tier]?.model_ready ? 'var(--success)' : 'var(--text-secondary)' }">{{ envStatus?.models?.[tier]?.model_ready ? '●' : '○' }}</span>
                    <span>{{ tier === 'large' ? '大模型' : tier === 'medium' ? '中模型' : '小模型' }} ({{ envStatus?.models?.[tier]?.dimension || '?' }}d)</span>
                    <span v-if="envStatus?.models?.[tier]?.model_ready && envStatus?.models?.[tier]?.model_size" style="color: var(--text-secondary);">({{ formatSize(envStatus.models[tier].model_size) }})</span>
                    <span v-if="tier === activeTier" class="badge badge-neutral">当前档</span>
                  </span>
                  <button class="btn btn-sm" @click="installModelTier(tier, tier === 'large' ? '大模型' : tier === 'medium' ? '中模型' : '小模型')" :disabled="!!setupProgress || envStatus?.models?.[tier]?.model_ready">安装</button>
                </div>
              </div>
            </div>

            <!-- Config editor (toggle) -->
            <div v-if="showEmbeddingConfig" style="margin-top: var(--space-4); border-top: 1px solid var(--border); padding-top: var(--space-4);">
              <textarea class="form-textarea" style="min-height: 200px; font-family: var(--font-mono); font-size: var(--text-xs);" v-model="embeddingConfigContent"></textarea>
              <div style="margin-top: var(--space-2); display: flex; justify-content: flex-end;">
                <button class="btn btn-sm btn-primary" @click="saveEmbeddingConfig">保存</button>
              </div>
            </div>
          </div>
        </div>

        <!-- 卡 2：注入配置（自强化记忆 TAB 底部卡片迁移至此） -->
        <div class="card" style="margin-top: var(--space-4);">
          <div class="card-header" style="display: flex; justify-content: space-between; align-items: center;">
            <h3 style="margin: 0;">注入配置（每轮自动想起）</h3>
            <button class="btn btn-sm" :disabled="restartingAgent" @click="restartAgent">
              {{ restartingAgent ? '重启中...' : '重启 Agent 生效' }}
            </button>
          </div>
          <div class="card-body">
            <p style="color: var(--text-secondary); font-size: var(--text-sm); margin: 0 0 var(--space-3);">
              记忆库是<b>有问才答</b>（agent 觉得需要时主动查 memory_search）；自动注入是<b>不问自答</b>——
              每轮 LLM 调用前，自动把与最新消息最相关的记忆注入上下文，模型不用自己开口问就能想起来。
              开关在 Agent 启动时读取，保存后需点击右上角「重启 Agent 生效」。
            </p>
            <div class="settings-grid">
              <!-- 总开关（与「强化记忆」TAB 的记忆配置卡共享同一份状态/配置） -->
              <span class="settings-key">主开关</span>
              <label class="toggle-switch">
                <input type="checkbox" v-model="mainEnabled" />
                <span class="toggle-slider"></span>
                <span class="toggle-label">{{ mainEnabled ? '启用' : '停用' }}</span>
              </label>

              <span class="settings-key">强化记忆</span>
              <label class="toggle-switch">
                <input type="checkbox" v-model="subEnabled" :disabled="!mainEnabled" />
                <span class="toggle-slider"></span>
                <span class="toggle-label">{{ subEnabled ? '启用' : '停用' }}</span>
              </label>

              <!-- Prerequisite status -->
              <span class="settings-key">前置状态</span>
              <span style="display: flex; align-items: center; gap: var(--space-2); font-size: var(--text-sm);">
                <template v-if="!mainEnabled">
                  <span class="badge badge-neutral">主开关未启用</span>
                  <span style="color: var(--text-muted);">先打开上方「主开关」</span>
                </template>
                <template v-else-if="!subEnabled">
                  <span class="badge badge-neutral">强化记忆未启用</span>
                  <span style="color: var(--text-muted);">打开「自动注入」会自动带上它</span>
                </template>
                <template v-else-if="!envStatus?.models?.[activeTier]?.model_ready">
                  <span class="badge badge-error">模型未安装</span>
                  <span style="color: var(--text-muted);">在上方「环境准备」卡点对应档位的「安装」按钮下载</span>
                </template>
                <template v-else>
                  <span class="badge badge-success">就绪</span>
                  <span style="color: var(--text-muted);">{{ activeTier }} 档模型已安装，注入可用</span>
                </template>
              </span>

              <!-- Active tier -->
              <span class="settings-key">模型规格</span>
              <select class="form-select" v-model="activeTier" style="width: 100%;" :disabled="!subEnabled">
                <option value="large">大模型 (768d)</option>
                <option value="medium">中模型 (384d)</option>
                <option value="small">小模型 (256d)</option>
              </select>

              <!-- Auto-inject switch（级联：打开时自动带上强化记忆，不再禁用） -->
              <span class="settings-key">自动注入</span>
              <label class="toggle-switch">
                <input type="checkbox" v-model="autoInject" />
                <span class="toggle-slider"></span>
                <span class="toggle-label">{{ autoInject ? '启用' : '停用' }}</span>
              </label>

              <!-- Top-K -->
              <span class="settings-key">注入条数</span>
              <div style="display: flex; align-items: center; gap: var(--space-3);">
                <input type="number" v-model.number="autoInjectTopK" min="1" max="10" style="width: 70px; text-align: center;" :disabled="!subEnabled || !autoInject" />
                <span style="color: var(--text-muted); font-size: var(--text-xs);">每轮最多注入的相关记忆条数（1-10，默认 3）</span>
              </div>
            </div>
          </div>
        </div>

        <!-- 卡 3：记忆条目管理（自动注入的内容来源） -->
        <div class="card" style="margin-top: var(--space-4);">
          <div class="card-header" style="display: flex; justify-content: space-between; align-items: center;">
            <h3 style="margin: 0;">记忆条目管理</h3>
            <button class="btn btn-sm" @click="loadMgmtEntries">刷新</button>
          </div>
          <div class="card-body">
            <p style="color: var(--text-secondary); font-size: var(--text-xs); margin: 0 0 var(--space-3);">
              这里就是会被自动注入的记忆内容——新增、编辑、删除直接生效（编辑会重新生成向量）。
            </p>

            <!-- 新增 -->
            <div style="margin-bottom: var(--space-4);">
              <div style="font-weight: 500; margin-bottom: var(--space-2);">新增条目</div>
              <div style="display: flex; gap: var(--space-2);">
                <textarea class="form-textarea" style="flex: 1; min-height: 70px; resize: vertical;" v-model="mgmtNewContent" placeholder="输入要记住的内容，之后每轮会自动想起..." @keydown.ctrl.enter="addMgmtEntry"></textarea>
                <button class="btn btn-primary" @click="addMgmtEntry" :disabled="mgmtAdding || !mgmtNewContent.trim()">
                  {{ mgmtAdding ? '添加中...' : '添加' }}
                </button>
              </div>
            </div>

            <!-- 搜索 -->
            <div style="margin-bottom: var(--space-3);">
              <div style="font-weight: 500; margin-bottom: var(--space-2);">搜索</div>
              <div style="display: flex; gap: var(--space-2);">
                <input class="form-input" style="flex: 1;" v-model="mgmtSearchQuery" placeholder="关键词搜索记忆条目..." @keydown.enter="searchMgmtEntries" />
                <button class="btn" @click="searchMgmtEntries">搜索</button>
                <button class="btn" v-if="mgmtResults !== null" @click="mgmtSearchQuery = ''; mgmtResults = null">清除</button>
              </div>
            </div>

            <!-- 列表 -->
            <div style="border: 1px solid var(--border); border-radius: var(--radius-md); max-height: 400px; overflow-y: auto;">
              <template v-if="mgmtResults !== null">
                <div v-if="mgmtResults.length === 0" class="empty-state" style="padding: var(--space-4);"><p>无匹配结果</p></div>
                <div v-for="entry in mgmtResults" :key="entry.id" style="padding: var(--space-2) var(--space-3); border-bottom: 1px solid var(--border); font-size: var(--text-sm);">
                  <div style="display: flex; justify-content: space-between; align-items: center; gap: var(--space-2);">
                    <span style="flex: 1;">{{ entry.content }}</span>
                    <span style="color: var(--text-muted); font-size: var(--text-xs);">搜索结果</span>
                  </div>
                </div>
              </template>
              <template v-else>
                <div v-if="mgmtEntries.length === 0" class="empty-state" style="padding: var(--space-4);"><p>暂无记忆条目</p></div>
                <div v-for="entry in mgmtEntries" :key="entry.id" style="padding: var(--space-2) var(--space-3); border-bottom: 1px solid var(--border); font-size: var(--text-sm);">
                  <template v-if="mgmtEditingId === entry.id">
                    <textarea class="form-textarea" aria-label="编辑条目" style="width: 100%; min-height: 70px; resize: vertical; margin-bottom: var(--space-2);" v-model="mgmtEditContent"></textarea>
                    <div style="display: flex; gap: var(--space-2); justify-content: flex-end;">
                      <button class="btn btn-sm" @click="cancelMgmtEdit">取消</button>
                      <button class="btn btn-sm btn-primary" @click="saveMgmtEdit" :disabled="mgmtSaving || !mgmtEditContent.trim()">
                        {{ mgmtSaving ? '保存中...' : '保存（重新生成向量）' }}
                      </button>
                    </div>
                  </template>
                  <template v-else>
                    <div style="display: flex; justify-content: space-between; align-items: center; gap: var(--space-2);">
                      <span style="flex: 1;">{{ entry.content }}</span>
                      <span style="display: flex; gap: var(--space-1); flex-shrink: 0;">
                        <button class="btn btn-sm" @click="startMgmtEdit(entry)">编辑</button>
                        <button class="btn btn-sm" style="color: var(--danger);" @click="deleteMgmtEntry(entry)">删除</button>
                      </span>
                    </div>
                    <div style="color: var(--text-muted); font-size: var(--text-xs); margin-top: 2px;">{{ (entry.id || '').substring(0, 8) }}<template v-if="entry.created_at"> · {{ entry.created_at }}</template></div>
                  </template>
                </div>
              </template>
            </div>
            <div v-if="mgmtResults === null && mgmtEntries.length > 0" style="display: flex; align-items: center; justify-content: space-between; padding: var(--space-2) var(--space-3);">
              <span style="color: var(--text-muted); font-size: var(--text-xs);">已显示 {{ mgmtEntries.length }} / 共 {{ mgmtTotal }} 条</span>
              <button v-if="mgmtTotal > mgmtEntries.length" class="btn btn-sm" @click="loadMoreMgmtEntries">加载更多</button>
            </div>
          </div>
        </div>

      </div><!-- End autoinject tab -->
    </div>
  </div>
</template>

<style scoped>
.settings-grid {
  display: grid;
  grid-template-columns: 120px 1fr;
  gap: var(--space-3) var(--space-4);
  align-items: center;
}
.settings-key {
  font-size: var(--text-sm);
  color: var(--text-secondary);
  font-weight: 500;
}

/* Range slider styling */
input[type="range"] {
  height: 6px;
  appearance: none;
  background: var(--border);
  border-radius: 3px;
  outline: none;
}
input[type="range"]::-webkit-slider-thumb {
  appearance: none;
  width: 16px;
  height: 16px;
  background: var(--accent);
  border-radius: 50%;
  cursor: pointer;
}

.btn-danger {
  background: var(--danger, #ef4444);
  color: white;
  border-color: var(--danger, #ef4444);
}
.btn-danger:hover {
  opacity: 0.9;
}

/* Toggle switch */
.toggle-switch {
  display: inline-flex;
  align-items: center;
  gap: var(--space-2);
  cursor: pointer;
  position: relative;
}
.toggle-switch input {
  position: absolute;
  opacity: 0;
  width: 0;
  height: 0;
}
.toggle-slider {
  width: 36px;
  height: 20px;
  background: var(--border, #d1d5db);
  border-radius: 10px;
  position: relative;
  transition: background 0.2s;
  flex-shrink: 0;
}
.toggle-slider::after {
  content: '';
  position: absolute;
  width: 16px;
  height: 16px;
  background: white;
  border-radius: 50%;
  top: 2px;
  left: 2px;
  transition: transform 0.2s;
  box-shadow: 0 1px 3px rgba(0,0,0,0.15);
}
.toggle-switch input:checked + .toggle-slider {
  background: var(--accent, #3b82f6);
}
.toggle-switch input:checked + .toggle-slider::after {
  transform: translateX(16px);
}
.toggle-label {
  font-size: var(--text-sm);
  color: var(--text-secondary);
  user-select: none;
}
.toggle-switch input:disabled + .toggle-slider {
  opacity: 0.4;
  cursor: not-allowed;
}
.toggle-switch input:disabled ~ .toggle-label {
  opacity: 0.5;
}
</style>
