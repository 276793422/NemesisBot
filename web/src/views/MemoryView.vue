<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted, watch } from 'vue'
import { useWSAPI } from '../composables/useWSAPI'
import { on as sseOn, off as sseOff } from '../composables/useSSE'
import { useToast } from '../composables/useToast'

const { request } = useWSAPI()
const toast = useToast()

defineProps<{ embedded?: boolean }>()

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

watch([mainEnabled, subEnabled, activeTier, similarityThreshold, maxResults], () => {
  saveConfigDebounced()
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
  <div :class="embedded ? 'memory-embed' : 'page-memory'">
    <div v-if="!embedded" class="page-header"><h2>记忆管理</h2></div>
    <div :class="embedded ? '' : 'page-body'">
      <div class="tabs">
        <button class="tab" :class="{ active: activeTab === 'documents' }" @click="activeTab = 'documents'">文档记忆</button>
        <button class="tab" :class="{ active: activeTab === 'vector' }" @click="activeTab = 'vector'">强化记忆</button>
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
              <div v-if="showEmbeddingConfig" class="config-editor-wrap">
                <div class="config-editor-header">
                  <span class="config-editor-title">原始配置（进阶）</span>
                  <button class="btn btn-sm btn-primary" @click="saveEmbeddingConfig">保存</button>
                </div>
                <p class="config-editor-hint">日常用右侧开关即可。仅在排查问题时直接改配置文本。</p>
                <textarea class="form-textarea config-editor" v-model="embeddingConfigContent"></textarea>
              </div>
            </div>
          </div>

          <!-- Section 2: 记忆配置 -->
          <div class="card">
            <div class="card-header"><h3 style="margin: 0;">记忆配置</h3></div>
            <div class="card-body">
              <div class="memory-config-grid">
                <!-- Main switch -->
                <div class="memory-field toggle-field">
                  <span class="memory-label">主开关</span>
                  <div class="nice-toggle" :class="{ active: mainEnabled }" @click="mainEnabled = !mainEnabled" role="switch" :aria-checked="mainEnabled">
                    <span class="toggle-track"><span class="toggle-thumb"></span></span>
                    <span class="toggle-text">{{ mainEnabled ? '启用' : '停用' }}</span>
                  </div>
                </div>

                <!-- Sub switch -->
                <div class="memory-field toggle-field">
                  <span class="memory-label">强化记忆</span>
                  <div class="nice-toggle" :class="{ active: subEnabled, disabled: !mainEnabled }" @click="mainEnabled && (subEnabled = !subEnabled)" role="switch" :aria-checked="subEnabled">
                    <span class="toggle-track"><span class="toggle-thumb"></span></span>
                    <span class="toggle-text">{{ subEnabled ? '启用' : '停用' }}</span>
                  </div>
                </div>

                <!-- Active tier -->
                <div class="memory-field">
                  <span class="memory-label">模型规格</span>
                  <div class="tier-chips">
                    <button
                      v-for="t in [{key:'large',label:'大模型 (768d)'},{key:'medium',label:'中模型 (384d)'},{key:'small',label:'小模型 (256d)'}]"
                      :key="t.key"
                      type="button"
                      class="tier-chip"
                      :class="{ active: activeTier === t.key, disabled: !subEnabled }"
                      :disabled="!subEnabled"
                      @click="activeTier = t.key"
                    >{{ t.label }}</button>
                  </div>
                </div>

                <!-- Similarity threshold -->
                <div class="memory-field">
                  <span class="memory-label">相似度阈值</span>
                  <div class="slider-row">
                    <input type="range" class="nice-slider" min="0.1" max="1.0" step="0.05" v-model.number="similarityThreshold" :disabled="!subEnabled" />
                    <span class="slider-value">{{ similarityThreshold.toFixed(2) }}</span>
                  </div>
                </div>

                <!-- Max results -->
                <div class="memory-field">
                  <span class="memory-label">最大结果数</span>
                  <div class="slider-row">
                    <input type="range" class="nice-slider" min="1" max="50" step="1" v-model.number="maxResults" :disabled="!subEnabled" />
                    <span class="slider-value">{{ maxResults }}</span>
                  </div>
                </div>

                <!-- Overall status -->
                <div class="memory-field toggle-field">
                  <span class="memory-label">整体状态</span>
                  <span class="badge" :class="envStatus?.overall === 'ready' ? 'badge-success' : envStatus?.overall === 'degraded' ? 'badge-warning' : 'badge-neutral'">
                    {{ envStatus?.overall === 'ready' ? '就绪' : envStatus?.overall === 'degraded' ? '降级' : '未启用' }}
                  </span>
                </div>
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
    </div>
  </div>
</template>

<style scoped>
/* ===== Memory Config Grid ===== */
.memory-config-grid {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: var(--space-4) var(--space-5);
  align-items: center;
}

.memory-field {
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
}

.memory-field.toggle-field {
  flex-direction: row;
  align-items: center;
  justify-content: space-between;
  padding: var(--space-2) 0;
}

.memory-label {
  font-size: var(--text-sm);
  color: var(--text);
  font-weight: 600;
}

.toggle-field .memory-label {
  flex: 1;
}

/* ===== Config Editor ===== */
.config-editor-wrap {
  margin-top: var(--space-4);
  border-top: 1px solid var(--border);
  padding-top: var(--space-4);
}

.config-editor-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: var(--space-2);
}

.config-editor-title {
  font-size: var(--text-sm);
  font-weight: 600;
  color: var(--text);
}

.config-editor-hint {
  font-size: var(--text-xs);
  color: var(--text-muted);
  margin-bottom: var(--space-2);
}

.config-editor {
  min-height: 200px;
  font-family: var(--font-mono);
  font-size: var(--text-xs);
}

/* ===== Tier Chips ===== */
.tier-chips {
  display: flex;
  flex-wrap: wrap;
  gap: var(--space-2);
}

.tier-chip {
  padding: var(--space-2) var(--space-3);
  border: 1px solid var(--border);
  background: var(--surface);
  color: var(--text-secondary);
  font-size: var(--text-sm);
  font-weight: 500;
  border-radius: var(--radius-md);
  cursor: pointer;
  transition: all var(--duration-fast);
  font-family: var(--font-sans);
}

.tier-chip:hover:not(.disabled) {
  border-color: var(--text-muted);
  background: var(--surface-hover);
  color: var(--text);
}

.tier-chip.active {
  border-color: var(--accent);
  background: var(--accent-muted);
  color: var(--accent);
  box-shadow: 0 0 0 1px rgba(232, 112, 90, 0.15);
}

.tier-chip.disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

/* ===== Nice Slider ===== */
.slider-row {
  display: flex;
  align-items: center;
  gap: var(--space-3);
}

.nice-slider {
  -webkit-appearance: none;
  appearance: none;
  flex: 1;
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

.nice-slider:disabled {
  opacity: 0.5;
  cursor: not-allowed;
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

/* ===== Nice Toggle ===== */
.nice-toggle {
  display: inline-flex;
  align-items: center;
  gap: var(--space-3);
  cursor: pointer;
  user-select: none;
  transition: opacity var(--duration-fast);
}

.nice-toggle.disabled {
  opacity: 0.4;
  cursor: not-allowed;
  pointer-events: none;
}

.nice-toggle .toggle-track {
  position: relative;
  width: 44px;
  height: 24px;
  background: var(--border);
  border-radius: var(--radius-full);
  transition: background var(--duration-fast);
  flex-shrink: 0;
}

.nice-toggle.active .toggle-track {
  background: var(--accent);
}

.nice-toggle .toggle-thumb {
  position: absolute;
  top: 2px;
  left: 2px;
  width: 20px;
  height: 20px;
  background: white;
  border-radius: 50%;
  transition: transform var(--duration-fast);
  box-shadow: var(--shadow-xs);
}

.nice-toggle.active .toggle-thumb {
  transform: translateX(20px);
}

.nice-toggle .toggle-text {
  font-size: var(--text-sm);
  color: var(--text-secondary);
  font-weight: 500;
}

/* ===== Responsive ===== */
@media (max-width: 768px) {
  .memory-config-grid {
    grid-template-columns: 1fr;
  }
}
</style>
