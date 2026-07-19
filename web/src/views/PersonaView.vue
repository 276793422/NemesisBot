<template>
  <div class="page-persona">
    <div class="page-header"><h2>人格</h2></div>
    <div class="page-body">
      <!-- Status bar -->
      <div v-if="currentPersona" class="persona-status-bar">
        <span class="persona-status-emoji">{{ currentPersona.emoji }}</span>
        <span>当前：{{ currentPersona.name }}</span>
      </div>

      <!-- Tabs: 超市 merged in (was separate sidebar item) -->
      <div class="tabs">
        <button class="tab" :class="{ active: activeTab === 'current' }" @click="setTab('current')">当前</button>
        <button class="tab" :class="{ active: activeTab === 'local' }" @click="setTab('local')">本地</button>
        <button class="tab" :class="{ active: activeTab === 'shop' }" @click="setTab('shop')">超市</button>
      </div>

      <!-- Current tab: file editor -->
      <div v-if="activeTab === 'current'">
        <div v-if="loading" style="text-align: center; padding: var(--space-8);">
          <div class="spinner spinner-lg" style="margin: 0 auto;"></div>
        </div>
        <div v-else-if="currentPersona" class="persona-files">
          <div v-for="file in currentPersona.files" :key="file" class="card persona-file-card">
            <div class="card-header">
              <h3>{{ file }}</h3>
              <div class="card-actions">
                <template v-if="editingFile === file">
                  <button class="btn btn-sm" @click="cancelEdit">取消</button>
                  <button class="btn btn-sm btn-primary" @click="saveFile">保存</button>
                </template>
                <button v-else class="btn btn-sm" @click="startEdit(file)">编辑</button>
              </div>
            </div>
            <div class="card-body">
              <pre class="persona-preview">{{ fileContents[file] || '(空文件)' }}</pre>
            </div>
          </div>
        </div>
      </div>

      <!-- Shop tab: former 人格超市 page -->
      <div v-if="activeTab === 'shop'" class="persona-shop-embed">
        <PersonaShopView embedded />
      </div>

      <!-- Local tab: persona cards -->
      <div v-if="activeTab === 'local'">
        <div v-if="loading" style="text-align: center; padding: var(--space-8);">
          <div class="spinner spinner-lg" style="margin: 0 auto;"></div>
        </div>
        <div v-else class="persona-grid">
          <div
            v-for="p in personas"
            :key="p.dir"
            class="card persona-card"
            :class="{ 'persona-card-active': p.is_active }"
            @click="showLocalPreview(p)"
          >
            <div class="card-header">
              <span class="persona-emoji">{{ p.emoji }}</span>
              <div class="persona-card-title">
                <h3>{{ p.name }}</h3>
                <span v-if="p.is_active" class="badge badge-success">使用中</span>
              </div>
            </div>
            <div class="card-body">
              <p class="persona-description">{{ p.description }}</p>
            </div>
            <div class="card-footer" @click.stop>
              <template v-if="p.is_default">
                <button
                  class="btn btn-sm"
                  @click="restoreDefault"
                  :disabled="p.is_active"
                >{{ p.is_active ? '当前人格' : '还原' }}</button>
              </template>
              <template v-else>
                <button
                  class="btn btn-sm btn-primary"
                  @click="activatePersona(p.dir)"
                  :disabled="p.is_active"
                >{{ p.is_active ? '使用中' : '启用' }}</button>
                <button class="btn btn-sm btn-danger" @click="removePersona(p.dir)">删除</button>
              </template>
            </div>
          </div>
        </div>
      </div>
    </div>

    <!-- Edit modal -->
    <div v-if="editingFile" class="modal-backdrop" @click.self="cancelEdit">
      <div class="modal" style="max-width: 900px; max-height: 90vh;">
        <div class="modal-header">
          <h3>编辑 {{ editingFile }}</h3>
          <button class="modal-close" @click="cancelEdit">&times;</button>
        </div>
        <div class="modal-body" style="padding: 0;">
          <textarea
            class="form-textarea persona-editor-modal"
            v-model="editContent"
          ></textarea>
        </div>
        <div class="modal-footer">
          <button class="btn" @click="cancelEdit">取消</button>
          <button class="btn btn-primary" @click="saveFile">保存</button>
        </div>
      </div>
    </div>

    <!-- Local preview modal -->
    <div v-if="previewingPersona" class="modal-backdrop" @click.self="closeLocalPreview">
      <div class="modal" style="max-width: 800px;">
        <div class="modal-header">
          <h3>{{ previewingPersona.emoji }} {{ previewingPersona.name }}</h3>
          <button class="modal-close" @click="closeLocalPreview">&times;</button>
        </div>
        <div class="preview-tabs">
          <button
            v-for="file in previewingPersona.files"
            :key="file"
            class="preview-tab"
            :class="{ active: previewSelectedFile === file }"
            @click="previewSelectedFile = file"
          >{{ file }}</button>
        </div>
        <div class="preview-body">
          <div v-if="previewLoading" style="text-align: center; padding: var(--space-8);">
            <div class="spinner" style="margin: 0 auto;"></div>
          </div>
          <div v-else class="markdown-body" v-html="renderMd(previewFileContents[previewSelectedFile] || '')"></div>
        </div>
        <div class="modal-footer">
          <button class="btn" @click="closeLocalPreview">关闭</button>
          <button
            class="btn btn-primary"
            @click="activatePersona(previewingPersona!.dir); closeLocalPreview()"
          >启用</button>
        </div>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { marked } from 'marked'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'
import { useChatStore } from '../stores/chat'
import { usePageTab } from '../lib/pageTab'
import PersonaShopView from './PersonaShopView.vue'

const { request } = useWSAPI()
const toast = useToast()
const chatStore = useChatStore()

function resetChatHistory() {
  chatStore.messages.splice(0, chatStore.messages.length)
  chatStore.historyLoaded = false
  chatStore.hasMoreHistory = true
  chatStore.oldestIndex = null
  chatStore.historyLoading = false
}

interface PersonaInfo {
  dir: string
  name: string
  emoji: string
  description: string
  is_default: boolean
  is_active: boolean
  files: string[]
}

const activeTab = ref('current')
const { setTab } = usePageTab(activeTab, ['current', 'local', 'shop'] as const, 'current')
const loading = ref(true)
const currentPersona = ref<any>(null)
const personas = ref<PersonaInfo[]>([])

// File editing state
const editingFile = ref('')
const editContent = ref('')
const fileContents = ref<Record<string, string>>({})

// Local preview state
const previewingPersona = ref<PersonaInfo | null>(null)
const previewFileContents = ref<Record<string, string>>({})
const previewSelectedFile = ref('IDENTITY.md')
const previewLoading = ref(false)

function renderMd(text: string): string {
  if (!text) return ''
  return marked.parse(text, { async: false }) as string
}

async function showLocalPreview(p: PersonaInfo) {
  if (p.is_active) {
    activeTab.value = 'current'
    return
  }
  previewingPersona.value = p
  previewLoading.value = true
  previewSelectedFile.value = 'IDENTITY.md'
  try {
    const files: Record<string, string> = {}
    for (const file of p.files) {
      const data = await request('persona', 'file.get', { name: p.dir, file })
      files[file] = data.content || ''
    }
    previewFileContents.value = files
  } catch (e: any) {
    toast.error('加载失败: ' + (e.message || e))
  } finally {
    previewLoading.value = false
  }
}

function closeLocalPreview() {
  previewingPersona.value = null
}

async function loadCurrent() {
  try {
    const data = await request('persona', 'current')
    currentPersona.value = data
    // Load all file contents
    for (const file of data.files || []) {
      try {
        const result = await request('persona', 'file.get', { name: data.active_dir, file })
        fileContents.value[file] = result.content || ''
      } catch {
        fileContents.value[file] = ''
      }
    }
  } catch (e: any) {
    toast.error('加载当前人格失败: ' + (e.message || e))
  }
}

async function loadPersonas() {
  try {
    const data = await request('persona', 'list')
    personas.value = data.personas || []
  } catch (e: any) {
    toast.error('加载人格列表失败: ' + (e.message || e))
  }
}

function startEdit(file: string) {
  editingFile.value = file
  editContent.value = fileContents.value[file] || ''
}

function cancelEdit() {
  editingFile.value = ''
  editContent.value = ''
}

async function saveFile() {
  if (!editingFile.value || !currentPersona.value) return
  try {
    await request('persona', 'file.save', {
      name: currentPersona.value.active_dir,
      file: editingFile.value,
      content: editContent.value,
    })
    fileContents.value[editingFile.value] = editContent.value
    toast.success('保存成功')
    editingFile.value = ''
  } catch (e: any) {
    toast.error('保存失败: ' + (e.message || e))
  }
}

async function activatePersona(dir: string) {
  try {
    await request('persona', 'activate', { name: dir })
    toast.success('人格已切换')
    resetChatHistory()
    await loadCurrent()
    await loadPersonas()
  } catch (e: any) {
    toast.error('切换失败: ' + (e.message || e))
  }
}

async function restoreDefault() {
  try {
    await request('persona', 'restore')
    toast.success('已恢复默认人格')
    resetChatHistory()
    await loadCurrent()
    await loadPersonas()
  } catch (e: any) {
    toast.error('恢复失败: ' + (e.message || e))
  }
}

async function removePersona(dir: string) {
  try {
    await request('persona', 'remove', { name: dir })
    toast.success('已删除')
    await loadCurrent()
    await loadPersonas()
  } catch (e: any) {
    toast.error('删除失败: ' + (e.message || e))
  }
}

onMounted(async () => {
  loading.value = true
  await Promise.all([loadCurrent(), loadPersonas()])
  loading.value = false
})
</script>

<style scoped>
.persona-status-bar {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  padding: var(--space-2) var(--space-3);
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
  margin-bottom: var(--space-4);
  font-weight: 600;
}

.persona-status-emoji {
  font-size: 1.5rem;
}

.persona-files {
  display: flex;
  flex-direction: column;
  gap: var(--space-4);
}

.persona-file-card .card-body {
  padding: 0;
}

.persona-editor-modal {
  min-height: 70vh;
  max-height: 70vh;
  width: 100%;
  border: none;
  border-radius: 0;
  font-family: var(--font-mono);
  font-size: var(--text-sm);
  line-height: 1.6;
  resize: none;
  padding: var(--space-3);
}

.modal-footer {
  display: flex;
  justify-content: flex-end;
  gap: var(--space-2);
  padding: var(--space-3);
  border-top: 1px solid var(--border);
}

.persona-preview {
  max-height: 25vh;
  overflow-y: auto;
  padding: var(--space-3);
  margin: 0;
  font-family: var(--font-mono);
  font-size: var(--text-sm);
  line-height: 1.6;
  white-space: pre-wrap;
  word-break: break-word;
  color: var(--text-secondary);
}

.persona-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(280px, 1fr));
  gap: var(--space-4);
}

.persona-card {
  cursor: pointer;
  transition: border-color 0.2s, box-shadow 0.2s;
}

.persona-card-active {
  border-color: var(--success) !important;
  box-shadow: 0 0 0 1px var(--success);
}

.persona-card .card-header {
  display: flex;
  align-items: center;
  gap: var(--space-2);
}

.persona-emoji {
  font-size: 2rem;
  line-height: 1;
}

.persona-card-title {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  flex: 1;
}

.persona-card-title h3 {
  margin: 0;
  font-size: var(--text-base);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  min-width: 0;
}

.persona-card-title .badge {
  flex-shrink: 0;
  white-space: nowrap;
}

.persona-description {
  margin: 0;
  color: var(--text-secondary);
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
  overflow: hidden;
}

.card-footer {
  display: flex;
  gap: var(--space-2);
  padding: var(--space-2) var(--space-3);
  border-top: 1px solid var(--border);
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

.preview-body {
  height: 65vh;
  overflow-y: auto;
  padding: var(--space-4);
}

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
</style>
