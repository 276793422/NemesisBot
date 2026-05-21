<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'

const { request } = useWSAPI()
const toast = useToast()

interface Model {
  name: string
  provider?: string
  api_key?: string
  base_url?: string
  proxy?: string
  is_default?: boolean
}

const models = ref<Model[]>([])
const loading = ref(true)
const showAdd = ref(false)
const addForm = ref({ name: '', provider: '', api_key: '', base_url: '', proxy: '' })
const testing = ref<string | null>(null)

async function loadModels() {
  try {
    const data = await request('models', 'list')
    models.value = data?.models || []
  } catch (e: any) {
    toast.error('加载模型失败: ' + e)
  }
  loading.value = false
}

async function addModel() {
  if (!addForm.value.name) { toast.warn('请输入模型名称'); return }
  try {
    const payload: any = { name: addForm.value.name }
    if (addForm.value.provider) payload.provider = addForm.value.provider
    if (addForm.value.api_key) payload.api_key = addForm.value.api_key
    if (addForm.value.base_url) payload.base_url = addForm.value.base_url
    if (addForm.value.proxy) payload.proxy = addForm.value.proxy
    await request('models', 'add', payload)
    toast.success('模型已添加')
    showAdd.value = false
    addForm.value = { name: '', provider: '', api_key: '', base_url: '', proxy: '' }
    await loadModels()
  } catch (e: any) {
    toast.error('添加失败: ' + e)
  }
}

async function deleteModel(name: string) {
  if (!confirm(`确定删除模型 "${name}" 吗？`)) return
  try {
    await request('models', 'delete', { name })
    toast.success('已删除')
    await loadModels()
  } catch (e: any) {
    toast.error('删除失败: ' + e)
  }
}

async function setDefault(name: string) {
  try {
    await request('models', 'set_default', { name })
    toast.success('已设为默认')
    await loadModels()
  } catch (e: any) {
    toast.error('设置失败: ' + e)
  }
}

async function testModel(name: string) {
  testing.value = name
  try {
    const data = await request('models', 'test', { name })
    toast.success(data?.message || '测试完成')
  } catch (e: any) {
    toast.error('测试失败: ' + e)
  }
  testing.value = null
}

onMounted(loadModels)
</script>

<template>
  <div class="page-models">
    <div class="page-header">
      <h2>模型管理</h2>
      <div class="page-header-actions">
        <button class="btn btn-primary" @click="showAdd = !showAdd">{{ showAdd ? '取消' : '+ 添加模型' }}</button>
      </div>
    </div>
    <div class="page-body">
      <!-- Add form -->
      <div v-if="showAdd" class="card" style="margin-bottom: var(--space-4);">
        <div class="card-header"><h3>添加模型</h3></div>
        <div class="card-body">
          <div style="display: grid; grid-template-columns: 1fr 1fr; gap: var(--space-3);">
            <div class="form-group">
              <label class="form-label">模型名称 *</label>
              <input class="form-input" v-model="addForm.name" placeholder="例如: gpt-4o / zhipu/glm-4">
            </div>
            <div class="form-group">
              <label class="form-label">Provider</label>
              <input class="form-input" v-model="addForm.provider" placeholder="例如: openai / zhipu">
            </div>
            <div class="form-group">
              <label class="form-label">API Key</label>
              <input class="form-input" type="password" v-model="addForm.api_key" placeholder="sk-...">
            </div>
            <div class="form-group">
              <label class="form-label">Base URL</label>
              <input class="form-input" v-model="addForm.base_url" placeholder="https://api.openai.com/v1">
            </div>
          </div>
          <div style="margin-top: var(--space-3); display: flex; justify-content: flex-end; gap: var(--space-2);">
            <button class="btn" @click="showAdd = false">取消</button>
            <button class="btn btn-primary" @click="addModel">添加</button>
          </div>
        </div>
      </div>

      <!-- Loading -->
      <div v-if="loading" style="text-align: center; padding: var(--space-8);">
        <div class="spinner spinner-lg" style="margin: 0 auto;"></div>
      </div>

      <!-- Empty -->
      <div v-if="!loading && models.length === 0" class="empty-state">
        <h3>暂无模型</h3>
        <p>点击上方"添加模型"按钮配置第一个 AI 模型</p>
      </div>

      <!-- Model list -->
      <div v-if="!loading && models.length > 0" style="display: grid; grid-template-columns: repeat(auto-fill, minmax(340px, 1fr)); gap: var(--space-4);">
        <div v-for="m in models" :key="m.name" class="card">
          <div class="card-header">
            <h3>{{ m.name }}</h3>
            <div style="display: flex; gap: var(--space-2); align-items: center;">
              <span v-if="m.is_default" class="badge badge-success">默认</span>
              <span v-if="m.provider" class="badge badge-info">{{ m.provider }}</span>
            </div>
          </div>
          <div class="card-body">
            <div class="settings-grid" style="font-size: var(--text-sm);">
              <span class="settings-key">API Key</span>
              <span class="settings-value">{{ m.api_key || '--' }}</span>
              <span class="settings-key">Base URL</span>
              <span class="settings-value">{{ m.base_url || '--' }}</span>
            </div>
          </div>
          <div class="card-footer">
            <button class="btn btn-sm btn-ghost" @click="testModel(m.name)" :disabled="testing === m.name">
              <span v-if="testing === m.name" class="spinner" style="width:14px;height:14px;"></span>
              {{ testing === m.name ? '测试中...' : '测试' }}
            </button>
            <button v-if="!m.is_default" class="btn btn-sm" @click="setDefault(m.name)">设为默认</button>
            <button class="btn btn-sm btn-danger" @click="deleteModel(m.name)">删除</button>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>
