<template>
  <div :class="embedded ? 'persona-shop-embed' : 'page-persona-shop'">
    <div v-if="!embedded" class="page-header"><h2>人格超市</h2></div>
    <div :class="embedded ? '' : 'page-body'">
      <!-- Source tabs -->
      <div class="tabs">
        <button
          v-for="src in sources"
          :key="src"
          class="tab"
          :class="{ active: selectedSource === src }"
          @click="selectedSource = src; browse()"
        >{{ src }}</button>
      </div>

      <!-- Search bar -->
      <div class="shop-search-bar">
        <input
          class="form-input"
          v-model="searchQuery"
          placeholder="搜索人格名称或关键词..."
          @keyup.enter="search"
        >
        <button class="btn btn-primary" @click="search">搜索</button>
      </div>

      <!-- Division filter pills -->
      <div class="filter-pills">
        <span
          class="filter-pill"
          :class="{ active: !selectedDivision }"
          @click="setDivision('')"
        >全部</span>
        <span
          v-for="d in divisions"
          :key="d"
          class="filter-pill"
          :class="{ active: selectedDivision === d }"
          @click="setDivision(d)"
        >{{ d }}</span>
      </div>

      <!-- Loading -->
      <div v-if="loading" style="text-align: center; padding: var(--space-8);">
        <div class="spinner spinner-lg" style="margin: 0 auto;"></div>
      </div>

      <!-- Results count -->
      <div v-if="!loading && items.length > 0" class="shop-count">
        共 {{ items.length }} 个人格
      </div>

      <!-- Card grid -->
      <div v-if="!loading && items.length > 0" class="shop-grid">
        <div
          v-for="p in items"
          :key="p.id"
          class="card shop-card"
          @click="showPreview(p)"
        >
          <div class="card-header">
            <span class="shop-card-emoji">{{ p.emoji }}</span>
            <h3>{{ p.name }}</h3>
          </div>
          <div class="card-body">
            <span class="badge badge-info" style="margin-bottom: var(--space-2);">{{ p.division }}</span>
            <p class="shop-card-desc">{{ p.description }}</p>
          </div>
          <div class="card-footer">
            <span v-if="p.installed" class="badge badge-neutral">已安装</span>
            <button
              v-else
              class="btn btn-sm btn-primary"
              @click.stop="downloadPersona(p.id)"
              :disabled="downloading === p.id"
            >
              {{ downloading === p.id ? '下载中...' : '下载' }}
            </button>
          </div>
        </div>
      </div>

      <!-- Empty -->
      <div v-if="!loading && items.length === 0" style="text-align: center; padding: var(--space-8); color: var(--text-secondary);">
        没有找到匹配的人格
      </div>

      <!-- Preview modal -->
      <div v-if="previewPersona" class="modal-backdrop" @click.self="closePreview">
        <div class="modal preview-modal">
          <div class="modal-header">
            <h3>{{ previewPersona.emoji }} {{ previewPersona.name }}</h3>
            <button class="modal-close" @click="closePreview">&times;</button>
          </div>

          <!-- Preview tabs -->
          <div class="preview-tabs">
            <button
              class="preview-tab"
              :class="{ active: previewTab === 'original' }"
              @click="previewTab = 'original'"
            >原始内容</button>
            <button
              class="preview-tab"
              :class="{ active: previewTab === 'converted' }"
              @click="previewTab = 'converted'"
            >转换预览</button>
          </div>

          <!-- Fixed-height body -->
          <div class="preview-body">
            <!-- Loading -->
            <div v-if="previewLoading" style="text-align: center; padding: var(--space-8);">
              <div class="spinner" style="margin: 0 auto;"></div>
            </div>

            <!-- Tab 1: Original — full markdown render -->
            <template v-else-if="previewTab === 'original'">
              <div v-if="previewPersona.raw" class="markdown-body" v-html="renderMd(previewPersona.raw)"></div>
              <div v-else style="text-align: center; padding: var(--space-4); color: var(--text-secondary);">
                暂无内容
              </div>
            </template>

            <!-- Tab 2: Converted files -->
            <template v-else-if="previewTab === 'converted'">
              <div v-if="convertedFileList.length > 0" class="converted-layout">
                <div class="converted-sidebar">
                  <div
                    v-for="(f, idx) in convertedFileList"
                    :key="idx"
                    class="converted-file-item"
                    :class="{ active: selectedConverted === idx }"
                    @click="selectedConverted = idx"
                  >{{ f.name }}</div>
                </div>
                <div class="converted-content">
                  <div v-if="convertedFileList[selectedConverted]?.name === 'PERSONA.json'" class="json-block" v-html="escapeHtml(convertedFileList[selectedConverted].content)"></div>
                  <div v-else class="markdown-body" v-html="renderMd(convertedFileList[selectedConverted].content)"></div>
                </div>
              </div>
              <div v-else style="text-align: center; padding: var(--space-4); color: var(--text-secondary);">
                暂无转换数据
              </div>
            </template>
          </div>

          <div class="modal-footer">
            <button class="btn" @click="closePreview">关闭</button>
            <button
              v-if="!previewPersona.installed"
              class="btn btn-primary"
              @click="downloadPersona(previewPersona.id)"
              :disabled="downloading === previewPersona.id"
            >
              {{ downloading === previewPersona.id ? '下载中...' : '下载' }}
            </button>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { marked } from 'marked'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'

defineProps<{ embedded?: boolean }>()

const { request } = useWSAPI()
const toast = useToast()

interface ShopPersona {
  id: string
  name: string
  emoji: string
  division: string
  description: string
  installed: boolean
  raw?: string
  converted?: Record<string, string>
}

const sources = ref(['Agency Agents'])
const selectedSource = ref('Agency Agents')
const searchQuery = ref('')
const selectedDivision = ref('')
const divisions = ref(['开发', '营销', '安全', '创意', '数据', '产品', '通用'])
const items = ref<ShopPersona[]>([])
const loading = ref(false)
const downloading = ref('')
const previewPersona = ref<ShopPersona | null>(null)
const previewLoading = ref(false)
const selectedConverted = ref(0)
const previewTab = ref('original')

const convertedFileList = computed(() => {
  if (!previewPersona.value?.converted) return []
  return Object.entries(previewPersona.value.converted).map(([name, content]) => ({ name, content }))
})

function renderMd(text: string): string {
  if (!text) return ''
  return marked.parse(text, { async: false }) as string
}

function escapeHtml(s: string): string {
  if (!s) return ''
  return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
}

async function browse() {
  loading.value = true
  try {
    const data = await request('persona', 'shop.browse', {
      division: selectedDivision.value || undefined,
    })
    items.value = data.items || []
  } catch (e: any) {
    toast.error('加载失败: ' + (e.message || e))
  } finally {
    loading.value = false
  }
}

async function search() {
  if (!searchQuery.value.trim()) {
    return browse()
  }
  loading.value = true
  try {
    const data = await request('persona', 'shop.search', {
      query: searchQuery.value,
    })
    items.value = data.items || []
  } catch (e: any) {
    toast.error('搜索失败: ' + (e.message || e))
  } finally {
    loading.value = false
  }
}

async function downloadPersona(id: string) {
  downloading.value = id
  try {
    await request('persona', 'shop.download', { id })
    toast.success('下载成功！若要使用新人格，需要去人格标签页里面启用')
    if (previewPersona.value && previewPersona.value.id === id) {
      previewPersona.value.installed = true
    }
    await browse()
  } catch (e: any) {
    toast.error('下载失败: ' + (e.message || e))
  } finally {
    downloading.value = ''
  }
}

async function showPreview(p: ShopPersona) {
  previewPersona.value = p
  previewLoading.value = true
  previewTab.value = 'original'
  selectedConverted.value = 0
  try {
    const data = await request('persona', 'shop.preview', { id: p.id })
    previewPersona.value = {
      ...p,
      name: data.name || p.name,
      emoji: data.emoji || p.emoji,
      installed: data.installed ?? p.installed,
      raw: data.raw || undefined,
      converted: data.converted || undefined,
    }
  } catch (e: any) {
    toast.error('加载预览失败: ' + (e.message || e))
  } finally {
    previewLoading.value = false
  }
}

function closePreview() {
  previewPersona.value = null
}

function setDivision(d: string) {
  selectedDivision.value = d
  browse()
}

onMounted(async () => {
  try { await request('persona', 'shop.refresh', {}) } catch {}
  browse()
})
</script>

<style scoped>
.shop-search-bar {
  display: flex;
  gap: var(--space-3);
  margin-bottom: var(--space-3);
}

.shop-search-bar .form-input {
  max-width: 400px;
}

.filter-pills {
  display: flex;
  gap: 6px;
  flex-wrap: wrap;
  margin-bottom: var(--space-3);
}

.filter-pill {
  padding: 4px 12px;
  border-radius: 9999px;
  font-size: var(--text-sm);
  cursor: pointer;
  border: 1px solid var(--border);
  background: var(--surface);
  color: var(--text-secondary);
  transition: all 0.15s;
}

.filter-pill:hover {
  border-color: var(--primary);
  color: var(--primary);
}

.filter-pill.active {
  background: var(--primary);
  border-color: var(--primary);
  color: white;
}

.shop-count {
  font-size: var(--text-sm);
  color: var(--text-secondary);
  margin-bottom: var(--space-3);
}

.shop-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(280px, 1fr));
  gap: var(--space-4);
}

.shop-card {
  cursor: pointer;
  transition: transform 0.15s, box-shadow 0.15s;
}

.shop-card:hover {
  transform: translateY(-2px);
  box-shadow: var(--shadow-md);
}

.shop-card-emoji {
  font-size: 1.5rem;
  margin-right: var(--space-2);
}

.shop-card .card-header {
  display: flex;
  align-items: center;
}

.shop-card .card-header h3 {
  margin: 0;
  font-size: var(--text-base);
}

.shop-card-desc {
  margin: 0;
  color: var(--text-secondary);
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
  overflow: hidden;
  font-size: var(--text-sm);
}

.shop-card .card-footer {
  display: flex;
  justify-content: flex-end;
  padding: var(--space-2) var(--space-3);
  border-top: 1px solid var(--border);
}

/* --- Preview modal (fixed size) --- */
.preview-modal {
  max-width: 960px;
  width: 90vw;
  display: flex;
  flex-direction: column;
}

.preview-tabs {
  display: flex;
  border-bottom: 1px solid var(--border);
  padding: 0 var(--space-3);
  flex-shrink: 0;
}

.preview-tab {
  padding: var(--space-2) var(--space-4);
  font-size: var(--text-sm);
  color: var(--text-secondary);
  border: none;
  background: none;
  cursor: pointer;
  border-bottom: 2px solid transparent;
  transition: all 0.15s;
}

.preview-tab:hover {
  color: var(--text-primary);
}

.preview-tab.active {
  color: var(--primary);
  border-bottom-color: var(--primary);
  font-weight: 500;
}

/* Fixed-height scrollable body */
.preview-body {
  height: 65vh;
  overflow-y: auto;
  padding: var(--space-4);
}

/* --- Converted layout (sidebar + content) --- */
.converted-layout {
  display: flex;
  gap: var(--space-3);
  height: 100%;
}

.converted-sidebar {
  flex-shrink: 0;
  width: 140px;
  border-right: 1px solid var(--border);
  overflow-y: auto;
}

.converted-file-item {
  padding: var(--space-2) var(--space-3);
  cursor: pointer;
  font-size: var(--text-sm);
  color: var(--text-secondary);
  border-left: 2px solid transparent;
  transition: all 0.15s;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  font-family: var(--font-mono);
}

.converted-file-item:hover {
  color: var(--text-primary);
  background: var(--surface);
}

.converted-file-item.active {
  color: var(--primary);
  border-left-color: var(--primary);
  background: var(--surface);
  font-weight: 500;
}

.converted-content {
  flex: 1;
  overflow-y: auto;
  min-width: 0;
}

/* --- Markdown rendering --- */
.preview-body :deep(.markdown-body) {
  font-size: var(--text-sm);
  line-height: 1.6;
}

.preview-body :deep(.markdown-body h1),
.preview-body :deep(.markdown-body h2),
.preview-body :deep(.markdown-body h3) {
  margin-top: var(--space-3);
  margin-bottom: var(--space-2);
}

.preview-body :deep(.markdown-body h1) { font-size: 1.25rem; }
.preview-body :deep(.markdown-body h2) { font-size: 1.1rem; }
.preview-body :deep(.markdown-body h3) { font-size: 1rem; }

.preview-body :deep(.markdown-body code) {
  background: var(--surface);
  padding: 1px 4px;
  border-radius: 3px;
  font-size: 0.9em;
}

.preview-body :deep(.markdown-body pre) {
  background: var(--surface);
  padding: var(--space-3);
  border-radius: var(--radius-md);
  overflow-x: auto;
}

.preview-body :deep(.markdown-body pre code) {
  background: none;
  padding: 0;
}

.preview-body :deep(.markdown-body ul),
.preview-body :deep(.markdown-body ol) {
  padding-left: var(--space-4);
}

.preview-body :deep(.markdown-body blockquote) {
  border-left: 3px solid var(--primary);
  margin: var(--space-2) 0;
  padding: var(--space-2) var(--space-3);
  color: var(--text-secondary);
}

.preview-body :deep(.markdown-body hr) {
  border: none;
  border-top: 1px solid var(--border);
  margin: var(--space-3) 0;
}

.preview-body :deep(.markdown-body table) {
  border-collapse: collapse;
  width: 100%;
  margin: var(--space-2) 0;
}

.preview-body :deep(.markdown-body th),
.preview-body :deep(.markdown-body td) {
  border: 1px solid var(--border);
  padding: var(--space-1) var(--space-2);
  font-size: 0.9em;
}

.preview-body :deep(.markdown-body th) {
  background: var(--surface);
  font-weight: 500;
}

/* JSON code block */
.preview-body :deep(.json-block) {
  white-space: pre-wrap;
  word-break: break-word;
  font-size: var(--text-sm);
  line-height: 1.6;
  font-family: var(--font-mono);
  background: var(--surface);
  padding: var(--space-4);
  border-radius: var(--radius-md);
  margin: 0;
}

.modal-footer {
  display: flex;
  justify-content: flex-end;
  gap: var(--space-2);
  padding: var(--space-3);
  border-top: 1px solid var(--border);
  flex-shrink: 0;
}
</style>
