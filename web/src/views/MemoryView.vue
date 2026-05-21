<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'

const { request } = useWSAPI()
const toast = useToast()

interface DocEntry { path: string; size?: number; modified?: string }

const activeTab = ref('documents')
const documents = ref<DocEntry[]>([])
const docContent = ref('')
const docPath = ref('')
const editing = ref(false)
const editContent = ref('')
const vectorEnabled = ref(false)
const loading = ref(true)

async function loadStatus() {
  try {
    const data = await request('memory', 'status')
    vectorEnabled.value = data?.vector_memory?.enabled || false
  } catch { /* ignore */ }
}

async function loadDocuments() {
  try {
    const data = await request('memory', 'documents')
    documents.value = data?.documents || []
  } catch (e: any) {
    toast.error('加载失败: ' + e)
  }
  loading.value = false
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

function formatSize(bytes?: number): string {
  if (!bytes) return '--'
  if (bytes < 1024) return bytes + ' B'
  return (bytes / 1024).toFixed(1) + ' KB'
}

onMounted(async () => {
  await Promise.all([loadStatus(), loadDocuments()])
})
</script>

<template>
  <div class="page-memory">
    <div class="page-header"><h2>记忆管理</h2></div>
    <div class="page-body">
      <div class="tabs">
        <button class="tab" :class="{ active: activeTab === 'documents' }" @click="activeTab = 'documents'">文档记忆</button>
        <button class="tab" :class="{ active: activeTab === 'vector' }" @click="activeTab = 'vector'">强化记忆</button>
      </div>

      <!-- Documents tab -->
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

      <!-- Vector tab -->
      <div v-if="activeTab === 'vector'">
        <div class="card">
          <div class="card-header"><h3>强化记忆</h3></div>
          <div class="card-body">
            <div class="stat-card" style="margin-bottom: var(--space-4);">
              <div class="stat-label">状态</div>
              <div class="stat-value">
                <span class="badge" :class="vectorEnabled ? 'badge-success' : 'badge-neutral'">
                  {{ vectorEnabled ? '已启用' : '未启用' }}
                </span>
              </div>
            </div>
            <p v-if="!vectorEnabled" style="color: var(--text-muted); font-size: var(--text-sm);">
              强化记忆（向量搜索）当前未启用。请在 config.enhanced_memory.json 中设置 enabled: true 以启用此功能。
            </p>
            <div v-if="vectorEnabled" style="color: var(--text-secondary); font-size: var(--text-sm);">
              <p>向量存储已启用。系统将自动对记忆文档进行向量化索引，支持语义搜索。</p>
            </div>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>
