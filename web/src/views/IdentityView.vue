<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'

const { request } = useWSAPI()
const toast = useToast()

interface DocInfo { name: string; exists: boolean }

const docs = ref<DocInfo[]>([])
const activeDoc = ref('')
const docContent = ref('')
const editing = ref(false)
const editContent = ref('')
const loading = ref(true)

async function loadDocs() {
  try {
    const data = await request('identity', 'list')
    docs.value = data?.documents || []
  } catch (e: any) {
    toast.error('加载失败: ' + e)
  }
  loading.value = false
}

async function loadDoc(name: string) {
  try {
    const data = await request('identity', 'get', { name })
    docContent.value = data?.content || ''
    activeDoc.value = name
    editing.value = false
  } catch (e: any) {
    toast.error('读取失败: ' + e)
  }
}

function startEdit() {
  editContent.value = docContent.value
  editing.value = true
}

async function saveDoc() {
  try {
    await request('identity', 'save', { name: activeDoc.value, content: editContent.value })
    toast.success('已保存')
    docContent.value = editContent.value
    editing.value = false
  } catch (e: any) {
    toast.error('保存失败: ' + e)
  }
}

const docLabels: Record<string, string> = {
  AGENT: 'AGENT.md — 行为指南',
  IDENTITY: 'IDENTITY.md — 身份定义',
  SOUL: 'SOUL.md — 核心原则',
  USER: 'USER.md — 用户偏好',
}

onMounted(async () => {
  await loadDocs()
  if (docs.value.length > 0) {
    await loadDoc(docs.value[0].name)
  }
})
</script>

<template>
  <div class="page-identity">
    <div class="page-header"><h2>身份管理</h2></div>
    <div class="page-body">
      <div v-if="loading" style="text-align: center; padding: var(--space-8);">
        <div class="spinner spinner-lg" style="margin: 0 auto;"></div>
      </div>

      <div v-if="!loading">
        <div class="tabs">
          <button v-for="d in docs" :key="d.name" class="tab" :class="{ active: activeDoc === d.name }" @click="loadDoc(d.name)">
            {{ d.name }}
          </button>
        </div>

        <div class="card">
          <div class="card-header">
            <h3>{{ docLabels[activeDoc] || activeDoc }}</h3>
            <div style="display: flex; gap: var(--space-2);">
              <template v-if="!editing">
                <button class="btn btn-sm" @click="startEdit">编辑</button>
              </template>
              <template v-else>
                <button class="btn btn-sm" @click="editing = false">取消</button>
                <button class="btn btn-sm btn-primary" @click="saveDoc">保存</button>
              </template>
            </div>
          </div>
          <div class="card-body">
            <div v-if="editing">
              <textarea class="form-textarea" style="min-height: 500px; font-family: var(--font-mono); font-size: var(--text-sm);" v-model="editContent"></textarea>
            </div>
            <div v-else class="markdown-body" style="max-height: 600px; overflow-y: auto;">
              <pre style="white-space: pre-wrap; word-break: break-word;">{{ docContent || '（空文件）' }}</pre>
            </div>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>
